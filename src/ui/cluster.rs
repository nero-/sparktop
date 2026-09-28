//! Cluster page: aggregate summary strip, then one of three views —
//! panels (band per node), table (row per node + sparklines), compare
//! (chart per metric, line per node).
use super::widgets::*;
use super::{health_of, now_unix, theme};
use crate::app::{ChartKind, ClusterView, Hit, NodeInfo, UiState};
use crate::graph::{block_titled, grid_for_scale, multi_line_graph, resample, Series};
use crate::history::{Cluster, Health, NodeHistory};
use crate::sample::Sample;
use crate::theme::{lerp, Theme};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Borders, Paragraph},
    Frame,
};

/// Rows a panel band needs to be legible (border 2 + hero 2 + graph 4).
const MIN_BAND: u16 = 8;

pub fn draw(f: &mut Frame, area: Rect, cluster: &Cluster, nodes: &[NodeInfo], st: &mut UiState) {
    let vis: Vec<NodeInfo> = st.visible(nodes).into_iter().cloned().collect();
    let summary_h = if area.height >= 20 { 4 } else { 0 };
    if summary_h > 0 {
        draw_summary(f, Rect { height: summary_h, ..area }, cluster, &vis, st);
    }
    let body = Rect { y: area.y + summary_h, height: area.height - summary_h, ..area };
    match st.view {
        ClusterView::Panels => {
            let cols = if body.width >= 180 && vis.len() >= 3 { 2 } else { 1 };
            let rows = vis.len().div_ceil(cols) as u16;
            if rows * MIN_BAND > body.height {
                // too many nodes for bands: fall back rather than draw slivers
                draw_table_with_detail(f, body, cluster, &vis, st, true);
            } else {
                draw_panels(f, body, cluster, &vis, st, cols);
            }
        }
        ClusterView::Table => draw_table_with_detail(f, body, cluster, &vis, st, false),
        ClusterView::Compare => draw_compare(f, body, cluster, &vis, st),
    }
}

/// Table sized to its rows; leftover height shows the focused node's band
/// (hero + page charts) so the table view is overview and detail at once.
fn draw_table_with_detail(f: &mut Frame, area: Rect, cluster: &Cluster, vis: &[NodeInfo], st: &mut UiState, auto: bool) {
    let table_h = (vis.len() as u16 + 3).min(area.height);
    let spare = area.height - table_h;
    let focused = st.focused_node(vis).cloned();
    match focused {
        Some(n) if spare >= MIN_BAND + 1 => {
            draw_table(f, Rect { height: table_h, ..area }, cluster, vis, st, auto);
            let band = Rect { y: area.y + table_h, height: spare, ..area };
            let charts = st.page_charts();
            let decode = vis.iter().any(|n| n.has_vllm);
            st.hits.push((band, Hit::Node(n.name.clone())));
            draw_band(f, theme(), band, cluster.get(&n.name), &n, true, &charts, decode, st);
        }
        _ => draw_table(f, area, cluster, vis, st, auto),
    }
}

fn mem_frac(s: &Sample) -> f64 {
    if s.mem.total_kb > 0 { s.mem.used_kb() as f64 / s.mem.total_kb as f64 } else { f64::NAN }
}

// ── summary strip ────────────────────────────────────────────────────
fn draw_summary(f: &mut Frame, area: Rect, cluster: &Cluster, vis: &[NodeInfo], st: &UiState) {
    let t = theme();
    let now = now_unix();
    let mut up = 0;
    let mut stale = 0;
    let mut down = 0;
    let (mut gpu_sum, mut gpu_n, mut gpu_max, mut gpu_max_node) = (0.0, 0, f64::NEG_INFINITY, String::new());
    let (mut mem_used, mut mem_total) = (0u64, 0u64);
    let (mut power, mut power_n) = (0.0, 0);
    let (mut decode, mut prefill, mut vllm_n, mut running, mut waiting) = (0.0, 0.0, 0, 0u64, 0u64);
    let (mut rx, mut tx) = (0.0, 0.0);
    let mut hottest = (f64::NEG_INFINITY, String::new());
    for n in vis {
        let Some(h) = cluster.get(&n.name) else { continue };
        match h.status.health(now, st.health_interval()) {
            Health::Ok => up += 1,
            Health::Stale(_) => stale += 1,
            Health::Down => down += 1,
            Health::Waiting => {}
        }
        // down nodes carry frozen numbers — keep them out of the totals
        if h.status.health(now, st.health_interval()) == Health::Down {
            continue;
        }
        let Some((s, d)) = h.last() else { continue };
        if d.gpu_pct.is_finite() {
            gpu_sum += d.gpu_pct;
            gpu_n += 1;
            if d.gpu_pct > gpu_max {
                gpu_max = d.gpu_pct;
                gpu_max_node = n.name.clone();
            }
        }
        mem_used += s.mem.used_kb();
        mem_total += s.mem.total_kb;
        if d.power_w.is_finite() {
            power += d.power_w;
            power_n += 1;
        }
        if d.temp_c.is_finite() && d.temp_c > hottest.0 {
            hottest = (d.temp_c, n.name.clone());
        }
        if let Some(v) = &s.vllm {
            vllm_n += 1;
            running += v.running;
            waiting += v.waiting;
            if d.generation_smooth.is_finite() {
                decode += d.generation_smooth;
            }
            if d.prompt_smooth.is_finite() {
                prefill += d.prompt_smooth;
            }
        }
        if d.net_rx_bps.is_finite() {
            rx += d.net_rx_bps;
        }
        if d.net_tx_bps.is_finite() {
            tx += d.net_tx_bps;
        }
    }
    let group = st.group.as_deref().unwrap_or("all nodes");
    let block = ratatui::widgets::Block::default()
        .borders(Borders::TOP)
        .border_style(Style::new().fg(t.dim))
        .title(Line::from(vec![
            Span::styled(format!(" cluster · {group} "), bold(t.fg)),
            Span::styled(format!("{} node{} ", vis.len(), if vis.len() == 1 { "" } else { "s" }), dim()),
        ]));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let total = vis.len();
    let nodes_color = if down > 0 { t.bad } else if stale > 0 { t.warn } else { t.good };
    let mut gloss = Vec::new();
    if stale > 0 {
        gloss.push(format!("{stale} stale"));
    }
    if down > 0 {
        gloss.push(format!("{down} down"));
    }
    let gpu_avg = if gpu_n > 0 { gpu_sum / gpu_n as f64 } else { f64::NAN };
    let mem_f = if mem_total > 0 { mem_used as f64 / mem_total as f64 } else { f64::NAN };
    let mut cells = vec![
        HeroCell::new(
            "NODES",
            format!("{}/{total} up", up + stale),
            if gloss.is_empty() { "all healthy".into() } else { gloss.join(" · ") },
            bold(nodes_color),
        ),
        HeroCell::metered(
            "GPU avg",
            fmt_pct(gpu_avg),
            gpu_avg / 100.0,
            t.s3,
            if gpu_n > 1 { format!("max {} {}", fmt_pct(gpu_max), gpu_max_node) } else { "compute".into() },
        ),
        HeroCell::metered(
            "MEMORY",
            fmt_pct(mem_f * 100.0),
            mem_f,
            mem_color(t, &st.alerts, mem_f),
            format!("{} / {}", fmt_mb(mem_used as f64 / 1024.0), fmt_mb(mem_total as f64 / 1024.0)),
        ),
        HeroCell::new(
            "POWER",
            if power_n > 0 { format!("{power:.0}W") } else { "—".into() },
            if hottest.0.is_finite() { format!("hottest {:.0}°C {}", hottest.0, hottest.1) } else { "GPU boards".into() },
            bold(if hottest.0 >= st.alerts.temp_warn_c { t.bad } else { t.warn }),
        ),
    ];
    if vllm_n > 0 {
        cells.push(HeroCell::new(
            "DECODE",
            format!("{} tok/s", fmt_tokens(decode)),
            format!("prefill {} · {running} run · {waiting} queued", fmt_tokens(prefill)),
            bold(t.s1),
        ));
    }
    cells.push(HeroCell::new("NET ↓", fmt_bps(rx), format!("↑ {}", fmt_bps(tx)), bold(t.accent)));
    render_hero(f, t, inner, &cells, 18);
}

// ── panels view ──────────────────────────────────────────────────────
fn border_for(t: &Theme, health: Health, focused: bool, alert: bool) -> ratatui::style::Color {
    match health {
        Health::Down => t.bad,
        Health::Stale(_) => t.warn,
        _ if focused => t.accent,
        _ if alert => t.warn,
        _ => t.dim,
    }
}

fn draw_panels(f: &mut Frame, area: Rect, cluster: &Cluster, vis: &[NodeInfo], st: &mut UiState, cols: usize) {
    let t = theme();
    let rows = vis.len().div_ceil(cols);
    let row_rects = Layout::vertical(std::iter::repeat_n(Constraint::Ratio(1, rows as u32), rows)).split(area);
    let focused = st.focused_node(vis).map(|n| n.name.clone());
    let charts = st.page_charts();
    let decode = vis.iter().any(|n| n.has_vllm);
    for (i, n) in vis.iter().enumerate() {
        let row = row_rects[i / cols];
        let band = if cols == 1 {
            row
        } else {
            Layout::horizontal(std::iter::repeat_n(Constraint::Ratio(1, cols as u32), cols)).split(row)[i % cols]
        };
        st.hits.push((band, Hit::Node(n.name.clone())));
        draw_band(f, t, band, cluster.get(&n.name), n, focused.as_deref() == Some(n.name.as_str()), &charts, decode, st);
    }
}

fn node_title(t: &Theme, n: &NodeInfo, h: Option<&NodeHistory>, health: Health, focused: bool) -> Line<'static> {
    let mut spans = vec![Span::raw(" "), health_dot(health, t)];
    spans.push(Span::styled(
        format!(" {} ", n.name),
        if focused { bold(t.accent) } else { bold(t.fg) },
    ));
    if n.group != "default" {
        spans.push(Span::styled(format!("[{}] ", n.group), Style::new().fg(t.accent)));
    }
    if let Some((s, _)) = h.and_then(|h| h.last()) {
        spans.push(Span::styled(format!("up {} ", fmt_dur(s.uptime_s)), dim()));
        if let Some(g) = s.gpus.first() {
            spans.push(Span::styled(format!("· {} ", g.name.trim_start_matches("NVIDIA ")), dim()));
        }
    }
    Line::from(spans)
}

fn band_subtitle(t: &Theme, h: Option<&NodeHistory>, health: Health) -> Option<Line<'static>> {
    match health {
        Health::Stale(age) => Some(Line::from(Span::styled(format!(" ◐ data {age}s old — retrying "), bold(t.warn)))),
        Health::Down => {
            let err = h.and_then(|h| h.status.last_err.clone()).unwrap_or_default();
            Some(Line::from(Span::styled(
                format!(" ✕ down: {} ", truncate(err.lines().last().unwrap_or(""), 60)),
                bold(t.bad),
            )))
        }
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_band(
    f: &mut Frame,
    t: &Theme,
    band: Rect,
    h: Option<&NodeHistory>,
    n: &NodeInfo,
    focused: bool,
    charts: &[ChartKind],
    decode: bool,
    st: &UiState,
) {
    let health = h.map(|h| h.status.health(now_unix(), st.health_interval())).unwrap_or(Health::Waiting);
    let alert = h.and_then(|h| h.last()).is_some_and(|(_, d)| alerting(d, &st.alerts));
    let block = block_titled(node_title(t, n, h, health, focused), band_subtitle(t, h, health), t)
        .border_style(Style::new().fg(border_for(t, health, focused, alert)));
    let inner = block.inner(band);
    f.render_widget(block, band);
    let Some(h) = h.filter(|h| !h.samples.is_empty()) else {
        let msg = match (&health, h.and_then(|h| h.status.last_err.as_ref())) {
            (Health::Down, Some(e)) => format!("unreachable — {}", e.lines().last().unwrap_or("")),
            _ => format!("connecting to {}…", n.host),
        };
        super::centered_msg(f, inner, &msg);
        return;
    };
    let (s, d) = h.last().unwrap();
    let rows = Layout::vertical([Constraint::Length(2), Constraint::Min(3)]).split(inner);
    render_hero(f, t, rows[0], &node_hero(t, s, d, &st.alerts, decode), 13);
    let cols = Layout::horizontal(std::iter::repeat_n(Constraint::Ratio(1, charts.len() as u32), charts.len())).split(rows[1]);
    for (ci, kind) in charts.iter().enumerate() {
        draw_kind(f, t, cols[ci], h, *kind, st.window_secs(), st.gradient, None);
    }
}

/// Compact hero for a node: shared by the cluster bands and the node page.
/// `decode` adds the vLLM cell (shown as — on nodes without an endpoint so
/// stacked cluster bands keep their columns aligned).
pub fn node_hero(t: &Theme, s: &Sample, d: &crate::history::Derived, a: &crate::config::Alerts, decode: bool) -> Vec<HeroCell> {
    let mf = mem_frac(s);
    let mut cells = vec![
        HeroCell::metered("GPU", fmt_pct(d.gpu_pct), d.gpu_pct / 100.0, t.s3, "compute".into()),
        HeroCell::metered(
            "MEMORY",
            fmt_pct(mf * 100.0),
            mf,
            mem_color(t, a, mf),
            format!("{} / {}", fmt_mb(s.mem.used_kb() as f64 / 1024.0), fmt_mb(s.mem.total_kb as f64 / 1024.0)),
        ),
        HeroCell::metered(
            "CPU",
            fmt_pct(d.cpu_pct),
            d.cpu_pct / 100.0,
            t.s1,
            format!("load {:.2} · {} cores", s.cpu.load1, s.cpu.cores.len()),
        ),
        HeroCell::new(
            "POWER",
            if d.power_w.is_finite() { format!("{:.1}W", d.power_w) } else { "—".into() },
            s.gpus.first().map(|g| format!("SM {:.0}MHz", g.sm_clock_mhz)).unwrap_or_else(|| "no GPU".into()),
            bold(t.warn),
        ),
        HeroCell::new(
            "TEMP",
            if d.temp_c.is_finite() { format!("{:.0}°C", d.temp_c) } else { "—".into() },
            "GPU package".into(),
            bold(temp_color(t, a, d.temp_c)),
        ),
        HeroCell::new("NET ↓", fmt_bps(d.net_rx_bps), format!("↑ {}", fmt_bps(d.net_tx_bps)), bold(t.accent)),
    ];
    if decode {
        cells.push(match s.vllm {
            Some(_) => HeroCell::new(
                "DECODE",
                format!("{} tok/s", fmt_tokens(d.generation_smooth)),
                format!("prefill {}", fmt_tokens(d.prompt_smooth)),
                bold(t.s1),
            ),
            None => HeroCell::new("DECODE", "—".into(), "no vLLM".into(), dim()),
        });
    }
    cells
}

// ── table view ───────────────────────────────────────────────────────
struct Col {
    head: &'static str,
    w: usize,
    /// lower drops first when the terminal is narrow
    prio: u8,
}

fn draw_table(f: &mut Frame, area: Rect, cluster: &Cluster, vis: &[NodeInfo], st: &mut UiState, auto: bool) {
    let t = theme();
    let title = if auto { "nodes · table (auto: too many for panels)" } else { "nodes" };
    let block = block_titled(
        panel_title(title),
        panel_sub(format!("GPU trend over {} · click a row, enter opens", crate::app::window_label(st.window_secs()))),
        t,
    );
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height < 2 {
        return;
    }
    let any_vllm = vis.iter().any(|n| n.has_vllm);
    let name_w = vis.iter().map(|n| n.name.chars().count()).max().unwrap_or(4).clamp(6, 18);
    let mut cols = vec![
        Col { head: "", w: 3, prio: 9 },
        Col { head: "node", w: name_w + 1, prio: 9 },
        Col { head: "GPU", w: 5, prio: 9 },
        Col { head: "trend", w: 0, prio: 8 },
        Col { head: "MEM", w: 12, prio: 7 },
        Col { head: "CPU", w: 5, prio: 4 },
        Col { head: "PWR", w: 7, prio: 6 },
        Col { head: "TEMP", w: 6, prio: 5 },
        Col { head: "LOAD", w: 6, prio: 2 },
        Col { head: "NET ↓", w: 10, prio: 5 },
        Col { head: "NET ↑", w: 10, prio: 3 },
    ];
    if any_vllm {
        cols.push(Col { head: "DECODE", w: 8, prio: 6 });
        cols.push(Col { head: "KV", w: 5, prio: 3 });
    }
    cols.push(Col { head: "UP", w: 8, prio: 1 });
    // drop low-priority columns until the sparkline gets at least 12 cells
    let width = inner.width as usize;
    loop {
        let fixed: usize = cols.iter().map(|c| c.w).sum();
        if fixed + 12 <= width {
            break;
        }
        let Some(i) = cols
            .iter()
            .enumerate()
            .filter(|(_, c)| c.prio < 8)
            .min_by_key(|(_, c)| c.prio)
            .map(|(i, _)| i)
        else {
            break;
        };
        cols.remove(i);
    }
    let fixed: usize = cols.iter().map(|c| c.w).sum();
    let spark_w = width.saturating_sub(fixed + 1).max(4);
    for c in cols.iter_mut() {
        if c.head == "trend" {
            c.w = spark_w + 1;
        }
    }

    // header row
    let head: Vec<Span> = cols
        .iter()
        .map(|c| Span::styled(pad(c.head, c.w), Style::new().fg(t.dim).add_modifier(Modifier::BOLD)))
        .collect();
    f.render_widget(Paragraph::new(Line::from(head)), Rect { height: 1, ..inner });

    let rows_avail = (inner.height - 1) as usize;
    let focused = st.focused_node(vis).map(|n| n.name.clone());
    let fidx = vis.iter().position(|n| Some(&n.name) == focused.as_ref()).unwrap_or(0);
    if fidx < st.table_scroll {
        st.table_scroll = fidx;
    } else if fidx >= st.table_scroll + rows_avail {
        st.table_scroll = fidx + 1 - rows_avail;
    }
    st.table_scroll = st.table_scroll.min(vis.len().saturating_sub(rows_avail));
    let now = now_unix();
    for (ri, n) in vis.iter().enumerate().skip(st.table_scroll).take(rows_avail) {
        let y = inner.y + 1 + (ri - st.table_scroll) as u16;
        let row_rect = Rect { y, height: 1, ..inner };
        st.hits.push((row_rect, Hit::Node(n.name.clone())));
        let is_f = Some(&n.name) == focused.as_ref();
        let base = if is_f { Style::new().bg(t.highlight()) } else { Style::new() };
        let h = cluster.get(&n.name);
        let health = h.map(|h| h.status.health(now, st.health_interval())).unwrap_or(Health::Waiting);
        let last = h.and_then(|h| h.last());
        let mut spans: Vec<Span> = Vec::new();
        for c in &cols {
            let cell: Vec<Span> = match (c.head, last) {
                ("", _) => {
                    let mut d = health_dot(health, t);
                    d.style = d.style.patch(base);
                    vec![Span::styled(if is_f { "▸" } else { " " }, base.fg(t.accent)), d, Span::styled(" ", base)]
                }
                ("node", _) => vec![Span::styled(pad(&n.name, c.w), base.fg(if is_f { t.accent } else { t.fg }).add_modifier(Modifier::BOLD))],
                (_, None) => vec![Span::styled(
                    pad(if c.head == "GPU" { "—" } else { "" }, c.w),
                    base.fg(t.dim),
                )],
                ("GPU", Some((_, d))) => vec![Span::styled(pad_left(&fmt_pct(d.gpu_pct), 4) + " ", base.fg(t.s3).add_modifier(Modifier::BOLD))],
                ("trend", Some(_)) => {
                    let h = h.unwrap();
                    let vals = chart_series(h, ChartKind::Gpu);
                    let cols_v = resample(&vals, &dt_of(h), st.window_secs(), spark_w);
                    let mut sp = sparkline(&cols_v, 100.0, t.s3, st.gradient.then(|| hot_for(t.s3, t)), t);
                    for s in sp.iter_mut() {
                        s.style = s.style.patch(base);
                    }
                    sp.push(Span::styled(" ", base));
                    sp
                }
                ("MEM", Some((s, _))) => {
                    let mf = mem_frac(s);
                    let color = mem_color(t, &st.alerts, mf);
                    let mut v = vec![Span::styled(pad_left(&fmt_pct(mf * 100.0), 4) + " ", base.fg(color).add_modifier(Modifier::BOLD))];
                    let mut m = meter(mf, c.w - 6, color, t.bad, t);
                    for s in m.iter_mut() {
                        s.style = s.style.patch(base);
                    }
                    v.extend(m);
                    v.push(Span::styled(" ", base));
                    v
                }
                ("CPU", Some((_, d))) => vec![Span::styled(pad(&fmt_pct(d.cpu_pct), c.w), base.fg(t.s1))],
                ("PWR", Some((_, d))) => vec![Span::styled(
                    pad(&if d.power_w.is_finite() { format!("{:.0}W", d.power_w) } else { "—".into() }, c.w),
                    base.fg(t.warn),
                )],
                ("TEMP", Some((_, d))) => vec![Span::styled(
                    pad(&if d.temp_c.is_finite() { format!("{:.0}°C", d.temp_c) } else { "—".into() }, c.w),
                    base.fg(temp_color(t, &st.alerts, d.temp_c)),
                )],
                ("LOAD", Some((s, _))) => vec![Span::styled(pad(&format!("{:.2}", s.cpu.load1), c.w), base.fg(t.fg))],
                ("NET ↓", Some((_, d))) => vec![Span::styled(pad(&fmt_bps(d.net_rx_bps), c.w), base.fg(t.accent))],
                ("NET ↑", Some((_, d))) => vec![Span::styled(pad(&fmt_bps(d.net_tx_bps), c.w), base.fg(t.s2))],
                ("DECODE", Some((s, d))) => vec![Span::styled(
                    pad(&if s.vllm.is_some() { fmt_tokens(d.generation_smooth) } else { "·".into() }, c.w),
                    base.fg(t.s1).add_modifier(Modifier::BOLD),
                )],
                ("KV", Some((_, d))) => vec![Span::styled(pad(&fmt_pct(d.kv_pct), c.w), base.fg(t.s2))],
                ("UP", Some((s, _))) => vec![Span::styled(pad(&fmt_dur(s.uptime_s), c.w), base.fg(t.dim))],
                _ => vec![Span::styled(" ".repeat(c.w), base)],
            };
            spans.extend(cell);
        }
        let mut line = truncate_line(Line::from(spans), width);
        let w = line.width();
        if w < width {
            line.spans.push(Span::styled(" ".repeat(width - w), base));
        }
        // stale/down rows fade so frozen numbers don't read as live
        if matches!(health, Health::Down | Health::Stale(_)) {
            for s in line.spans.iter_mut() {
                if let Some(fg) = s.style.fg {
                    s.style = s.style.fg(lerp(fg, t.bg, 0.45));
                }
            }
        }
        f.render_widget(Paragraph::new(line), row_rect);
    }
    if vis.len() > rows_avail {
        let more = format!(" {}–{} of {} ", st.table_scroll + 1, (st.table_scroll + rows_avail).min(vis.len()), vis.len());
        let r = Rect { x: area.x + area.width.saturating_sub(more.len() as u16 + 2), y: area.y, width: more.len() as u16, height: 1 };
        f.render_widget(Paragraph::new(Span::styled(more, dim())), r);
    }
}

// ── compare view ─────────────────────────────────────────────────────
fn draw_compare(f: &mut Frame, area: Rect, cluster: &Cluster, vis: &[NodeInfo], st: &mut UiState) {
    let t = theme();
    let any_vllm = vis.iter().any(|n| n.has_vllm);
    let mut kinds = vec![ChartKind::Gpu, ChartKind::Mem, ChartKind::Power, ChartKind::Temp, ChartKind::Net];
    kinds.push(if any_vllm { ChartKind::Decode } else { ChartKind::Cpu });

    // legend: node → color
    let mut legend = vec![Span::styled(" ", dim())];
    let mut x = area.x + 1;
    for (i, n) in vis.iter().enumerate() {
        let text = format!("━━ {}  ", n.name);
        let w = text.chars().count() as u16;
        st.hits.push((Rect { x, y: area.y, width: w, height: 1 }, Hit::Node(n.name.clone())));
        x += w;
        let health = health_of(cluster, &n.name, st.health_interval());
        let mut c = t.series(i);
        if matches!(health, Health::Down | Health::Waiting) {
            c = lerp(c, t.bg, 0.5);
        }
        legend.push(Span::styled(text, bold(c)));
    }
    f.render_widget(Paragraph::new(truncate_line(Line::from(legend), area.width as usize)), Rect { height: 1, ..area });

    let grid = Rect { y: area.y + 1, height: area.height.saturating_sub(1), ..area };
    let (gc, gr) = if grid.width >= 150 { (3, 2) } else if grid.height >= 30 { (2, 3) } else { (3, 2) };
    let rows = Layout::vertical(std::iter::repeat_n(Constraint::Ratio(1, gr), gr as usize)).split(grid);
    for (ki, kind) in kinds.iter().enumerate() {
        let r = ki / gc as usize;
        let c = ki % gc as usize;
        if r >= rows.len() {
            break;
        }
        let cell = Layout::horizontal(std::iter::repeat_n(Constraint::Ratio(1, gc), gc as usize)).split(rows[r])[c];
        draw_compare_chart(f, t, cell, cluster, vis, *kind, st);
    }
}

fn draw_compare_chart(f: &mut Frame, t: &Theme, area: Rect, cluster: &Cluster, vis: &[NodeInfo], kind: ChartKind, st: &UiState) {
    let window = st.window_secs();
    let data: Vec<(usize, &NodeInfo, Vec<Option<f64>>, Vec<f64>)> = vis
        .iter()
        .enumerate()
        .filter_map(|(i, n)| {
            let h = cluster.get(&n.name)?;
            let dt = dt_of(h);
            let vals = chart_series(h, kind);
            let start = window_start(&dt, window);
            Some((i, n, vals[start.min(vals.len())..].to_vec(), dt[start.min(dt.len())..].to_vec()))
        })
        .collect();
    let peak = data.iter().flat_map(|(_, _, v, _)| v.iter().filter_map(|x| *x)).fold(0.0_f64, f64::max);
    let scale = if is_pct(kind) { 100.0 } else { peak };
    let (ref_lines, _) = grid_for_scale(scale);
    // subtitle: each node's current value in its series color
    let mut sub: Vec<Span> = vec![Span::raw(" ")];
    for (i, n, v, _) in &data {
        let now = v.last().copied().flatten().unwrap_or(f64::NAN);
        sub.push(Span::styled(format!("{} ", truncate(&n.name, 10)), dim()));
        sub.push(Span::styled(format!("{}  ", fmt_kind(kind, now)), bold(t.series(*i))));
    }
    let sub = truncate_line(Line::from(sub), area.width.saturating_sub(2) as usize);
    let title = Line::from(vec![
        Span::styled(format!(" {} ", chart_title(kind)), bold(t.fg)),
        Span::styled(format!("peak {} ", fmt_kind(kind, peak)), dim()),
    ]);
    let block = block_titled(title, Some(sub), t);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let series: Vec<Series> = data.iter().map(|(i, _, v, dt)| Series { vals: v, dt, color: t.series(*i) }).collect();
    let unit = match kind {
        ChartKind::Net | ChartKind::Disk => "B/s",
        ChartKind::Power => "W",
        ChartKind::Temp => "°",
        k if is_pct(k) => "%",
        _ => "",
    };
    multi_line_graph(f, inner, &series, window, &ref_lines, unit, t.dim);
}
