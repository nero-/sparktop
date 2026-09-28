//! Node page: btop-style full detail for the focused node.
use super::cluster::node_hero;
use super::widgets::*;
use super::{health_of, theme};
use crate::app::{ChartKind, NodeInfo, UiState};
use crate::graph::block_titled;
use crate::history::{Cluster, Health, NodeHistory};
use crate::theme::Theme;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

pub fn draw(f: &mut Frame, area: Rect, cluster: &Cluster, nodes: &[NodeInfo], st: &mut UiState) {
    let t = theme();
    let Some(n) = st.focused_node(nodes) else { return };
    let h = match cluster.get(&n.name) {
        Some(h) if !h.samples.is_empty() => h,
        other => {
            let msg = match other.and_then(|h| h.status.last_err.as_ref()) {
                Some(e) => format!("{} unreachable — {}", n.name, e.lines().last().unwrap_or("")),
                None => format!("connecting to {} ({})…", n.name, n.host),
            };
            super::centered_msg(f, area, &msg);
            return;
        }
    };
    let (s, d) = h.last().unwrap();

    // shrink gracefully: detail rows go first on short terminals
    let show_mid = area.height >= 22;
    let show_bottom = area.height >= 30;
    let mut cons = vec![Constraint::Length(3), Constraint::Min(8)];
    if show_mid {
        cons.push(Constraint::Length(9));
    }
    if show_bottom {
        cons.push(Constraint::Length(8));
    }
    let rows = Layout::vertical(cons).split(area);

    // hero strip (+ stale banner in place of the last row when frozen)
    let health = health_of(cluster, &n.name, st.health_interval());
    render_hero(f, t, rows[0], &node_hero(t, s, d, &st.alerts, s.vllm.is_some()), 14);
    let notice = match health {
        Health::Stale(a) => Some((format!(" ◐ data {a}s old — retrying "), t.warn)),
        Health::Down => Some((" ✕ node down — showing last data ".to_string(), t.bad)),
        _ => None,
    };
    if let Some((msg, color)) = notice {
        let w = (msg.chars().count() as u16).min(rows[0].width);
        let r = Rect { x: rows[0].x + rows[0].width - w, width: w, height: 1, ..rows[0] };
        f.render_widget(ratatui::widgets::Clear, r);
        f.render_widget(Paragraph::new(Span::styled(msg, bold(color))), r);
    }

    // chart row: the user's selection (defaults CPU / memory / GPU)
    let charts = st.page_charts();
    let big = Layout::horizontal(std::iter::repeat_n(Constraint::Ratio(1, charts.len() as u32), charts.len())).split(rows[1]);
    for (ci, kind) in charts.iter().enumerate() {
        let sub = match kind {
            ChartKind::Cpu => panel_sub(format!(
                "load {:.2}/{:.2}/{:.2} · {} running / {} procs",
                s.cpu.load1, s.cpu.load5, s.cpu.load15, s.cpu.procs_running, s.cpu.procs_total
            )),
            ChartKind::Gpu => match s.gpus.first() {
                Some(g) => panel_sub(format!("{:.1}W · {:.0}°C · SM {:.0}MHz", g.power_w, g.temp_c, g.sm_clock_mhz)),
                None => panel_sub("no GPU detected".into()),
            },
            ChartKind::Mem => panel_sub(format!(
                "used {} of {} · cached {}",
                fmt_mb(s.mem.used_kb() as f64 / 1024.0),
                fmt_mb(s.mem.total_kb as f64 / 1024.0),
                fmt_mb(s.mem.cached_kb as f64 / 1024.0)
            )),
            _ => None,
        };
        draw_kind(f, t, big[ci], h, *kind, st.window_secs(), st.gradient, sub);
    }

    if show_mid {
        let mid = Layout::horizontal([Constraint::Percentage(34), Constraint::Percentage(40), Constraint::Percentage(26)]).split(rows[2]);
        draw_cores(f, t, mid[0], h);
        draw_kind(f, t, mid[1], h, ChartKind::Net, st.window_secs(), st.gradient, None);
        draw_ifaces(f, t, mid[2], h);
    }
    if show_bottom {
        let bot = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[3]);
        draw_kind(f, t, bot[0], h, ChartKind::Disk, st.window_secs(), st.gradient, None);
        draw_procs(f, t, bot[1], h);
    }
}

/// btop-style per-core rows: `C00 ■■■■■□□□  45%`, flowing into columns.
fn draw_cores(f: &mut Frame, t: &Theme, area: Rect, h: &NodeHistory) {
    let (s, d) = h.last().unwrap();
    let mhz: Vec<f64> = s.cpu.mhz.iter().copied().filter(|m| *m > 0.0).collect();
    let freq = if mhz.is_empty() {
        String::new()
    } else {
        let avg = mhz.iter().sum::<f64>() / mhz.len() as f64;
        let max = mhz.iter().copied().fold(0.0, f64::max);
        format!("avg {:.2}GHz · max {:.2}GHz", avg / 1000.0, max / 1000.0)
    };
    let block = block_titled(
        Line::from(vec![
            Span::styled(" cores ", bold(t.fg)),
            Span::styled(format!("{} ", d.per_core_pct.len()), dim()),
            Span::styled(format!("{} ", fmt_pct(d.cpu_pct)), bold(t.s1)),
        ]),
        if freq.is_empty() { None } else { panel_sub(freq) },
        t,
    );
    let inner = block.inner(area);
    f.render_widget(block, area);
    let n = d.per_core_pct.len();
    if n == 0 || inner.height == 0 {
        return;
    }
    let rows = inner.height as usize;
    let ncols = n.div_ceil(rows);
    let col_w = (inner.width as usize / ncols).max(10);
    let label_w = if n > 100 { 4 } else { 3 };
    let bar_w = col_w.saturating_sub(label_w + 6).clamp(0, 16);
    let mut lines: Vec<Line> = Vec::new();
    for r in 0..rows {
        let mut spans = Vec::new();
        for c in 0..ncols {
            let i = c * rows + r;
            let Some(pct) = d.per_core_pct.get(i) else { break };
            let color = crate::theme::lerp(t.s1, t.bad, (pct / 100.0).clamp(0.0, 1.0));
            spans.push(Span::styled(format!(" {:<label_w$}", i), dim()));
            if bar_w >= 3 {
                spans.extend(meter(pct / 100.0, bar_w, t.s1, t.bad, t));
            }
            spans.push(Span::styled(format!("{:>4.0}%", pct), Style::new().fg(color)));
            let used = 1 + label_w + if bar_w >= 3 { bar_w } else { 0 } + 5;
            if c + 1 < ncols && col_w > used {
                spans.push(Span::raw(" ".repeat(col_w - used)));
            }
        }
        lines.push(truncate_line(Line::from(spans), inner.width as usize));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// Busiest interfaces with live rates, then totals since boot.
fn draw_ifaces(f: &mut Frame, t: &Theme, area: Rect, h: &NodeHistory) {
    let (s, d) = h.last().unwrap();
    let block = block_titled(panel_title("interfaces"), panel_sub("↓ rx · ↑ tx".into()), t);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let w = inner.width as usize;
    let name_w = d.ifaces.iter().map(|(n, _, _)| n.chars().count()).max().unwrap_or(4).clamp(4, 12);
    let mut lines: Vec<Line> = Vec::new();
    let budget = (inner.height as usize).saturating_sub(2);
    for (name, rx, tx) in d.ifaces.iter().take(budget) {
        let idle = *rx + *tx < 1.0;
        lines.push(truncate_line(
            Line::from(vec![
                Span::styled(format!(" {} ", pad(name, name_w)), if idle { dim() } else { Style::new().fg(t.fg) }),
                Span::styled(format!("↓{:<10}", fmt_bps(*rx)), Style::new().fg(if idle { t.dim } else { t.accent })),
                Span::styled(format!("↑{}", fmt_bps(*tx)), Style::new().fg(if idle { t.dim } else { t.s2 })),
            ]),
            w,
        ));
    }
    if d.ifaces.is_empty() {
        lines.push(Line::from(Span::styled(" measuring…", dim())));
    }
    let tot_rx: u64 = s.net.iter().filter(|(n, _)| *n != "lo").map(|(_, v)| v.rx_bytes).sum();
    let tot_tx: u64 = s.net.iter().filter(|(n, _)| *n != "lo").map(|(_, v)| v.tx_bytes).sum();
    while lines.len() < inner.height.saturating_sub(1) as usize {
        lines.push(Line::from(""));
    }
    lines.push(truncate_line(
        Line::from(Span::styled(
            format!(" since boot ↓{} ↑{}", fmt_mb(tot_rx as f64 / 1048576.0), fmt_mb(tot_tx as f64 / 1048576.0)),
            dim(),
        )),
        w,
    ));
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_procs(f: &mut Frame, t: &Theme, area: Rect, h: &NodeHistory) {
    let (s, _) = h.last().unwrap();
    let mut procs: Vec<_> = s.gpus.iter().flat_map(|g| g.pids.iter()).collect();
    procs.sort_by(|a, b| b.mem_mb.total_cmp(&a.mem_mb));
    let total: f64 = procs.iter().map(|p| p.mem_mb).sum();
    let block = block_titled(
        Line::from(vec![
            Span::styled(" gpu processes ", bold(t.fg)),
            Span::styled(format!("{} · {} ", procs.len(), fmt_mb(total)), dim()),
        ]),
        None,
        t,
    );
    let inner = block.inner(area);
    f.render_widget(block, area);
    let mem_total_mb = s.mem.total_kb as f64 / 1024.0;
    let mut lines: Vec<Line> = Vec::new();
    if procs.is_empty() {
        lines.push(Line::from(Span::styled(" none — no compute jobs", dim())));
    }
    let w = inner.width as usize;
    for p in procs.iter().take(inner.height as usize) {
        let frac = if mem_total_mb > 0.0 { p.mem_mb / mem_total_mb } else { 0.0 };
        let mut spans = vec![
            Span::styled(format!(" {:>7} ", p.pid), dim()),
            Span::styled(format!("{:>8} ", fmt_mb(p.mem_mb)), bold(t.warn)),
        ];
        spans.extend(meter(frac, 8, t.warn, t.bad, t));
        spans.push(Span::styled(format!(" {}", p.name.rsplit('/').next().unwrap_or(&p.name)), Style::new().fg(t.fg)));
        lines.push(truncate_line(Line::from(spans), w));
    }
    f.render_widget(Paragraph::new(lines), inner);
}
