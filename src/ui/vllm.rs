//! vLLM page: vllm-top engine status + rate graphs for the focused node.
use super::theme;
use super::widgets::*;
use crate::app::{ChartKind, NodeInfo, UiState};
use crate::graph::block_titled;
use crate::history::Cluster;
use crate::theme::usage_color;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

pub fn draw(f: &mut Frame, area: Rect, cluster: &Cluster, nodes: &[NodeInfo], st: &mut UiState) {
    let t = theme();
    let Some(n) = st.focused_node(nodes) else { return };
    let h = match cluster.get(&n.name) {
        Some(h) if !h.samples.is_empty() => h,
        _ => {
            super::centered_msg(f, area, &format!("waiting for {}…", n.name));
            return;
        }
    };
    let (s, d) = h.last().unwrap();
    let Some(v) = &s.vllm else {
        let others: Vec<&str> = st.visible(nodes).iter().filter(|x| x.has_vllm).map(|x| x.name.as_str()).collect();
        let msg = if !n.has_vllm {
            if others.is_empty() {
                format!("no vLLM endpoint for {} — set vllm_url in sparktop.toml (server must expose /metrics)", n.name)
            } else {
                format!("{} has no vllm_url — tab to a vLLM node: {}", n.name, others.join(", "))
            }
        } else {
            format!("{}: vllm_url set but /metrics returned nothing — is the server up?", n.name)
        };
        super::centered_msg(f, area, &msg);
        return;
    };

    let rows = Layout::vertical([
        Constraint::Length(4), // engine status hero
        Constraint::Min(8),    // prefill / decode graphs
        Constraint::Length(7), // latency table + plots
        Constraint::Length(7), // KV / cache / peaks
    ])
    .split(area);

    // engine status hero
    let model = v.model.clone().unwrap_or_else(|| "model".into());
    let prefix = d.prefix_hit_pct;
    let cells = vec![
        HeroCell::custom(
            "MODEL",
            vec![
                Line::from(Span::styled(truncate(model.rsplit('/').next().unwrap_or(&model), 28), bold(t.accent))),
                Line::from(Span::styled(
                    format!("{} req/s · {:.0} preempt", fmt_rate(d.req_ps), v.preemptions),
                    dim(),
                )),
            ],
        ),
        HeroCell::new(
            "REQUESTS",
            format!("{} run · {} wait", v.running, v.waiting),
            "active · queued".into(),
            bold(if v.waiting > 20 { t.bad } else if v.waiting > 5 { t.warn } else { t.fg }),
        ),
        HeroCell::custom(
            "TOK/S · 5s / 30s",
            vec![
                Line::from(Span::styled(
                    format!("decode  {} / {}", fmt_tokens(d.generation_smooth), fmt_tokens(d.generation_avg30)),
                    bold(t.s1),
                )),
                Line::from(Span::styled(
                    format!("prefill {} / {}", fmt_tokens(d.prompt_smooth), fmt_tokens(d.prompt_avg30)),
                    bold(t.s2),
                )),
                Line::from(Span::styled(
                    format!("raw PP {} · TG {}", fmt_tokens(d.prompt_tps), fmt_tokens(d.generation_tps)),
                    dim(),
                )),
            ],
        ),
        HeroCell::new(
            "TTFT p95",
            fmt_secs(d.ttft_p95),
            format!("p50 {}", fmt_secs(d.ttft_p50)),
            bold(if d.ttft_p95.unwrap_or(0.0) > 1.0 { t.bad } else { t.good }),
        ),
        HeroCell::new("TPOT p95", fmt_secs(d.tpot_p95), format!("p50 {}", fmt_secs(d.tpot_p50)), bold(t.s1)),
        HeroCell::metered(
            "KV CACHE",
            format!("{:.0}%", v.gpu_cache_usage * 100.0),
            v.gpu_cache_usage,
            usage_color(t, v.gpu_cache_usage),
            match prefix {
                Some(p) => format!("prefix hit {p:.0}%"),
                None => "busiest engine".into(),
            },
        ),
    ];
    render_hero(f, t, rows[0], &cells, 18);

    // prefill / decode graphs (the centerpiece)
    let dt = dt_of(h);
    let chart_cols = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[1]);
    let pv = series(h, |d| d.prompt_smooth);
    let dv = series(h, |d| d.generation_smooth);
    let tok = |v: f64| format!("{} tok/s", fmt_tokens(v));
    draw_graph(
        f,
        t,
        chart_cols[0],
        st.window_secs(),
        GraphSpec {
            title: "prefill",
            gloss: "5s mean",
            vals: &pv,
            dt: &dt,
            color: t.s2,
            unit: "",
            fmt: &tok,
            sub: panel_sub(format!("raw {} · 30s avg {} tok/s", fmt_tokens(d.prompt_tps), fmt_tokens(d.prompt_avg30))),
            overlay: None,
            gradient: st.gradient,
            pct: false,
            border: None,
        },
    );
    draw_graph(
        f,
        t,
        chart_cols[1],
        st.window_secs(),
        GraphSpec {
            title: "decode",
            gloss: "5s mean · aggregate",
            vals: &dv,
            dt: &dt,
            color: t.s1,
            unit: "",
            fmt: &tok,
            sub: panel_sub(format!("raw {} · 30s avg {} tok/s", fmt_tokens(d.generation_tps), fmt_tokens(d.generation_avg30))),
            overlay: None,
            gradient: st.gradient,
            pct: false,
            border: None,
        },
    );

    // latency table + ttft/tpot plots
    let lat_cols = Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(35), Constraint::Percentage(35)]).split(rows[2]);
    let lat_block = block_titled(panel_title("latency"), panel_sub("p50 typical · p95 1-in-20".into()), t);
    let lat_inner = lat_block.inner(lat_cols[0]);
    f.render_widget(lat_block, lat_cols[0]);
    let lat_kvs = vec![
        Kv::colored("TTFT p50", fmt_secs(d.ttft_p50), "first token", t.s3),
        Kv::colored("TTFT p95", fmt_secs(d.ttft_p95), "1-in-20", t.warn),
        Kv::colored("TPOT p50", fmt_secs(d.tpot_p50), "per token", t.s1),
        Kv::colored("TPOT p95", fmt_secs(d.tpot_p95), "1-in-20", t.accent),
    ];
    f.render_widget(Paragraph::new(kv_lines(t, lat_kvs, lat_inner.width as usize)), lat_inner);

    let ms = |v: f64| if v.is_finite() { format!("{v:.0}ms") } else { "—".into() };
    let ttft: Vec<Option<f64>> = h.derived.iter().map(|x| x.ttft_p50.map(|v| v * 1000.0)).collect();
    let tpot: Vec<Option<f64>> = h.derived.iter().map(|x| x.tpot_p50.map(|v| v * 1000.0)).collect();
    for (col, title, gloss, vals, color) in [
        (lat_cols[1], "TTFT p50", "time to first token", &ttft, t.s3),
        (lat_cols[2], "TPOT p50", "per output token", &tpot, t.accent),
    ] {
        draw_graph(
            f,
            t,
            col,
            st.window_secs(),
            GraphSpec {
                title,
                gloss,
                vals,
                dt: &dt,
                color,
                unit: "ms",
                fmt: &ms,
                sub: None,
                overlay: None,
                gradient: st.gradient,
                pct: false,
                border: None,
            },
        );
    }

    // KV graph + cache/requests + peaks
    let bot = Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(30), Constraint::Percentage(30)]).split(rows[3]);
    draw_kind(f, t, bot[0], h, ChartKind::Kv, st.window_secs(), st.gradient, None);

    let cache_block = block_titled(panel_title("cache & requests"), None, t);
    let cache_inner = cache_block.inner(bot[1]);
    f.render_widget(cache_block, bot[1]);
    let cache_kvs = vec![
        Kv::colored("gpu KV", format!("{:.0}%", v.gpu_cache_usage * 100.0), "busiest engine", usage_color(t, v.gpu_cache_usage)),
        Kv::colored("cpu KV", format!("{:.0}%", v.cpu_cache_usage * 100.0), "host offload", t.accent),
        Kv::colored(
            "prefix hit",
            prefix.map(|p| format!("{p:.0}%")).unwrap_or_else(|| "—".into()),
            "last 30s",
            t.s1,
        ),
        Kv::plain("finished", fmt_tokens(v.requests_done), &format!("{} req/s · 30s", fmt_rate(d.req_ps))),
        Kv::colored("preempted", format!("{:.0}", v.preemptions), "KV evictions", if v.preemptions > 0.0 { t.warn } else { t.dim }),
    ];
    f.render_widget(Paragraph::new(kv_lines(t, cache_kvs, cache_inner.width as usize)), cache_inner);

    let peaks_block = block_titled(panel_title("peaks"), panel_sub("raw · retained history".into()), t);
    let peaks_inner = peaks_block.inner(bot[2]);
    f.render_widget(peaks_block, bot[2]);
    let peak = |sel: fn(&crate::history::Derived) -> f64| h.derived.iter().map(sel).filter(|v| v.is_finite()).fold(0.0, f64::max);
    let peaks_kvs = vec![
        Kv::colored("decode", format!("{} tok/s", fmt_tokens(peak(|d| d.generation_tps))), "raw interval", t.s1),
        Kv::colored("prefill", format!("{} tok/s", fmt_tokens(peak(|d| d.prompt_tps))), "raw interval", t.s2),
        Kv::colored("KV cache", format!("{:.0}%", peak(|d| d.kv_pct)), "gpu pool", t.s2),
        Kv::colored("TTFT p95", fmt_secs(h.derived.iter().filter_map(|d| d.ttft_p95).reduce(f64::max)), "worst window", t.warn),
    ];
    f.render_widget(Paragraph::new(kv_lines(t, peaks_kvs, peaks_inner.width as usize)), peaks_inner);
}
