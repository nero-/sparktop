//! TUI rendering — layout and panel conventions adapted from vllm-top
//! (GPLv3, mratsim/vllm-top) and btop (Apache-2.0, aristocratos/btop):
//! boxed rounded panels, hero label/value strip, braille area graphs with
//! dashed calibrated gridlines, aligned kv columns, status footer.
mod cluster;
mod node;
mod vllm;
pub mod widgets;

pub use crate::app::{
    defaults_for, window_label, ChartKind, ClusterView, NodeInfo, Page, UiState, DEFAULTS_CLUSTER, DEFAULTS_NODE,
    WINDOWS, WINDOW_LABELS,
};

use crate::app::{Hit, Overlay};
use crate::graph::block_titled;
use crate::history::{Cluster, Health};
use crate::theme::{Theme, THEMES};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
    Frame,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use widgets::*;

pub static THEME_IDX: AtomicUsize = AtomicUsize::new(0);

pub fn theme() -> &'static Theme {
    &THEMES[THEME_IDX.load(Ordering::Relaxed) % THEMES.len()]
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Health of a node by name (Waiting when it has no history entry yet).
pub fn health_of(cluster: &Cluster, name: &str, interval: f64) -> Health {
    cluster.get(name).map(|h| h.status.health(now_unix(), interval)).unwrap_or(Health::Waiting)
}

// ── top-level draw ───────────────────────────────────────────────────
pub fn draw(f: &mut Frame, cluster: &Cluster, nodes: &[NodeInfo], st: &mut UiState) {
    let t = theme();
    let area = f.area();
    st.hits.clear();
    f.render_widget(Block::new().style(Style::new().bg(t.bg).fg(t.fg)), area);
    if area.height < 6 || area.width < 30 {
        f.render_widget(Paragraph::new("sparktop: terminal too small"), area);
        return;
    }

    draw_header(f, Rect { height: 1, ..area }, cluster, nodes, st);
    let mut body = Rect { y: area.y + 1, height: area.height - 2, ..area };
    if st.page != Page::Cluster {
        draw_node_tabs(f, Rect { height: 1, ..body }, cluster, nodes, st);
        body.y += 1;
        body.height -= 1;
    }

    let vis = st.visible(nodes);
    if vis.is_empty() {
        let msg = if nodes.is_empty() {
            "no nodes configured"
        } else {
            "every node in this view is hidden — press n to pick nodes, g to change cluster"
        };
        centered_msg(f, body, msg);
    } else {
        match st.page {
            Page::Cluster => cluster::draw(f, body, cluster, nodes, st),
            Page::Node => node::draw(f, body, cluster, nodes, st),
            Page::Vllm => vllm::draw(f, body, cluster, nodes, st),
        }
    }

    draw_footer(f, Rect { y: area.y + area.height - 1, height: 1, ..area }, cluster, nodes, st);

    match st.overlay.clone() {
        Overlay::None => {}
        Overlay::Help => draw_help(f, area),
        Overlay::Nodes { cursor } => draw_node_picker(f, area, cluster, nodes, st, cursor),
        Overlay::Charts { cursor } => draw_chart_picker(f, area, st, cursor),
    }
}

pub(crate) fn centered_msg(f: &mut Frame, area: Rect, msg: &str) {
    if area.height == 0 {
        return;
    }
    let row = Rect { y: area.y + area.height / 2, height: 1, ..area };
    let line = truncate_line(Line::from(Span::styled(msg.to_string(), dim())), area.width as usize);
    f.render_widget(Paragraph::new(line).alignment(ratatui::layout::Alignment::Center), row);
}

fn draw_header(f: &mut Frame, area: Rect, cluster: &Cluster, nodes: &[NodeInfo], st: &mut UiState) {
    let t = theme();
    let mut left: Vec<Span> = vec![Span::styled(" sparktop ", bold(t.accent))];
    let mut x = area.x + 10;
    for (key, label, page) in [("1", "cluster", Page::Cluster), ("2", "node", Page::Node), ("3", "vllm", Page::Vllm)] {
        let text = format!(" {key} {label} ");
        let w = text.chars().count() as u16;
        let style = if st.page == page {
            Style::new().fg(t.bg).bg(t.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(t.dim)
        };
        st.hits.push((Rect { x, y: area.y, width: w, height: 1 }, Hit::Page(page)));
        x += w + 1;
        left.push(Span::styled(text, style));
        left.push(Span::raw(" "));
    }
    let vis = st.visible(nodes);
    let up = vis.iter().filter(|n| matches!(health_of(cluster, &n.name, st.health_interval()), Health::Ok | Health::Stale(_))).count();
    let group = st.group.as_deref().unwrap_or("all");
    left.push(Span::styled(format!(" cluster {group} "), bold(t.fg)));
    let hidden = nodes.iter().filter(|n| st.in_group(n) && st.hidden.contains(&n.name)).count();
    left.push(Span::styled(
        format!("{up}/{} up{}", vis.len(), if hidden > 0 { format!(" · {hidden} hidden") } else { String::new() }),
        Style::new().fg(if up < vis.len() { t.warn } else { t.dim }),
    ));

    let mut right: Vec<Span> = Vec::new();
    if st.paused {
        right.push(Span::styled("⏸ paused ", bold(t.warn)));
    }
    if st.page == Page::Cluster {
        right.push(Span::styled(format!("view {} · ", st.view.label()), Style::new().fg(t.dim)));
    }
    right.push(Span::styled(
        format!("poll {}s · window {} · {} ", st.interval, window_label(st.window_secs()), theme().name),
        Style::new().fg(t.dim),
    ));
    let lw: usize = left.iter().map(|s| s.content.chars().count()).sum();
    let rw: usize = right.iter().map(|s| s.content.chars().count()).sum();
    let width = area.width as usize;
    let mut spans = left;
    if lw + rw < width {
        spans.push(Span::raw(" ".repeat(width - lw - rw)));
        spans.extend(right);
    }
    f.render_widget(Paragraph::new(truncate_line(Line::from(spans), width)), area);
}

/// Node tabs for pages 2/3: health dot + name, focused one highlighted.
fn draw_node_tabs(f: &mut Frame, area: Rect, cluster: &Cluster, nodes: &[NodeInfo], st: &mut UiState) {
    let t = theme();
    let focused = st.focused_node(nodes).map(|n| n.name.clone());
    let mut spans = vec![Span::raw(" ")];
    let mut x = area.x + 1;
    for n in st.visible(nodes) {
        let is_f = focused.as_deref() == Some(n.name.as_str());
        let dimmed = st.page == Page::Vllm && !n.has_vllm;
        let text = format!(" {} ", n.name);
        let w = text.chars().count() as u16 + 2;
        if x + w > area.x + area.width {
            spans.push(Span::styled("…", dim()));
            break;
        }
        st.hits.push((Rect { x, y: area.y, width: w, height: 1 }, Hit::Node(n.name.clone())));
        x += w + 1;
        let style = if is_f {
            Style::new().fg(t.fg).bg(t.highlight()).add_modifier(Modifier::BOLD)
        } else if dimmed {
            Style::new().fg(crate::theme::lerp(t.bg, t.dim, 0.6))
        } else {
            Style::new().fg(t.dim)
        };
        let mut dot = health_dot(health_of(cluster, &n.name, st.health_interval()), t);
        if is_f {
            dot.style = dot.style.bg(t.highlight());
        }
        spans.push(Span::styled(" ", if is_f { Style::new().bg(t.highlight()) } else { Style::new() }));
        spans.push(dot);
        spans.push(Span::styled(text, style));
        spans.push(Span::raw(" "));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_footer(f: &mut Frame, area: Rect, cluster: &Cluster, nodes: &[NodeInfo], st: &UiState) {
    let t = theme();
    let line = if st.paused {
        Line::from(Span::styled(" ⏸ paused — graphs frozen, press space to resume polling", bold(t.warn)))
    } else if let Some(msg) = st.live_status() {
        Line::from(Span::styled(format!(" {msg}"), bold(t.accent)))
    } else if let Some(line) = down_notice(cluster, nodes, st) {
        line
    } else {
        let hints: &[(&str, &str)] = match st.page {
            Page::Cluster => &[
                ("↑↓", "node"),
                ("enter", "open"),
                ("v", "view"),
                ("g", "cluster"),
                ("n", "nodes"),
                ("m", "charts"),
                ("w", "window"),
                ("space", "pause"),
                ("?", "help"),
                ("q", "quit"),
            ],
            Page::Node => &[
                ("tab", "node"),
                ("m", "charts"),
                ("e/r", "add/rm"),
                ("w", "window"),
                ("3", "vllm"),
                ("esc", "back"),
                ("?", "help"),
                ("q", "quit"),
            ],
            Page::Vllm => &[("tab", "vLLM node"), ("w", "window"), ("2", "node"), ("esc", "back"), ("?", "help"), ("q", "quit")],
        };
        let mut spans = vec![Span::raw(" ")];
        for (k, v) in hints {
            spans.push(Span::styled(*k, bold(t.accent)));
            spans.push(Span::styled(format!(" {v}  "), Style::new().fg(t.dim)));
        }
        Line::from(spans)
    };
    f.render_widget(Paragraph::new(truncate_line(line, area.width as usize)), area);
}

/// Footer warning for visible nodes that are down (with the ssh error).
fn down_notice(cluster: &Cluster, nodes: &[NodeInfo], st: &UiState) -> Option<Line<'static>> {
    let t = theme();
    let now = now_unix();
    let down: Vec<(&str, Option<&String>)> = st
        .visible(nodes)
        .into_iter()
        .filter_map(|n| {
            let h = cluster.get(&n.name)?;
            (h.status.health(now, st.health_interval()) == Health::Down).then_some((n.name.as_str(), h.status.last_err.as_ref()))
        })
        .collect();
    if down.is_empty() {
        return None;
    }
    let (name, err) = down[0];
    let more = if down.len() > 1 { format!(" (+{} more)", down.len() - 1) } else { String::new() };
    let err = err.map(|e| e.lines().last().unwrap_or("").to_string()).unwrap_or_default();
    Some(Line::from(vec![
        Span::styled(format!(" ✕ {name} down{more}"), bold(t.bad)),
        Span::styled(format!(" — {err} · retrying"), Style::new().fg(t.bad)),
    ]))
}

// ── overlays ─────────────────────────────────────────────────────────
fn popup(f: &mut Frame, area: Rect, w: u16, h: u16, title: &str, sub: &str) -> Option<Rect> {
    let t = theme();
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    if w < 24 || h < 5 {
        return None;
    }
    let r = Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h };
    f.render_widget(Clear, r);
    let block = block_titled(panel_title(title), panel_sub(sub.to_string()), t).border_style(Style::new().fg(t.accent));
    let inner = block.inner(r);
    f.render_widget(block, r);
    Some(inner)
}

pub const KEYS: &[(&str, &str)] = &[
    ("1 2 3", "cluster / node / vLLM pages"),
    ("enter", "open focused node (cluster page)"),
    ("tab ↑↓ jk", "next / previous node"),
    ("esc", "back to cluster page · quit from it"),
    ("v", "cluster view: panels → table → compare"),
    ("g", "cycle cluster filter (cluster = \"…\")"),
    ("n", "node picker: show / hide / jump"),
    ("m", "chart picker for this page"),
    ("e r c", "add / remove / reset charts"),
    ("w W", "graph window: 60s · 5m · 15m"),
    ("+ -", "poll interval faster / slower"),
    ("space", "pause / resume polling"),
    ("t T", "next / previous theme"),
    ("G", "gradient graph fill on/off"),
    ("mouse", "click node/tab · click again opens · wheel"),
    ("q ctrl-c", "quit"),
];

fn draw_help(f: &mut Frame, area: Rect) {
    let t = theme();
    let Some(inner) = popup(f, area, 64, KEYS.len() as u16 + 9, "keys", "esc / ? closes") else { return };
    let mut lines: Vec<Line> = KEYS
        .iter()
        .map(|(k, v)| {
            Line::from(vec![Span::styled(format!(" {k:<11}"), bold(t.accent)), Span::styled(v.to_string(), Style::new().fg(t.fg))])
        })
        .collect();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(" health  ● live  ◐ stale  ✕ down  ○ connecting", dim())));
    lines.push(Line::from(Span::styled(" % graphs are pinned 0–100; others autoscale to window peak", dim())));
    lines.push(Line::from(Span::styled(" config edits (nodes, clusters) apply live — no restart", dim())));
    lines.push(Line::from(Span::styled(" vLLM page needs vllm_url in sparktop.toml", dim())));
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_node_picker(f: &mut Frame, area: Rect, cluster: &Cluster, nodes: &[NodeInfo], st: &UiState, cursor: usize) {
    let t = theme();
    let h = (nodes.len() as u16 + 4).max(8);
    let Some(inner) = popup(f, area, 72, h, "nodes", "space show/hide · a all · enter focus · esc close") else {
        return;
    };
    let name_w = nodes.iter().map(|n| n.name.chars().count()).max().unwrap_or(4).clamp(4, 20);
    let group_w = nodes.iter().map(|n| n.group.chars().count()).max().unwrap_or(4).clamp(4, 14);
    let rows = inner.height as usize;
    let first = cursor.saturating_sub(rows.saturating_sub(1));
    let mut lines = Vec::new();
    for (i, n) in nodes.iter().enumerate().skip(first).take(rows) {
        let sel = i == cursor;
        let shown = !st.hidden.contains(&n.name);
        let in_view = st.in_group(n);
        let health = health_of(cluster, &n.name, st.health_interval());
        let base = if sel { Style::new().bg(t.highlight()) } else { Style::new() };
        let fg = if !in_view { t.dim } else { t.fg };
        let mut dot = health_dot(health, t);
        dot.style = dot.style.patch(base);
        let spans = vec![
            Span::styled(if sel { " ▸ " } else { "   " }, base.fg(t.accent)),
            Span::styled(if shown { "[x] " } else { "[ ] " }, base.fg(if shown { t.good } else { t.dim })),
            dot,
            Span::styled(format!(" {} ", pad(&n.name, name_w)), base.fg(fg).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{} ", pad(&n.group, group_w)), base.fg(t.accent)),
            Span::styled(format!("{:<10}", health_text(health)), base.fg(t.dim)),
            Span::styled(if n.has_vllm { "vllm " } else { "     " }, base.fg(t.s1)),
            Span::styled(truncate(&n.host, 18), base.fg(t.dim)),
        ];
        let mut line = truncate_line(Line::from(spans), inner.width as usize);
        if sel {
            let w = line.width();
            line.spans.push(Span::styled(" ".repeat((inner.width as usize).saturating_sub(w)), base));
        }
        lines.push(line);
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_chart_picker(f: &mut Frame, area: Rect, st: &UiState, cursor: usize) {
    let t = theme();
    let choices = st.chart_choices();
    let current = st.page_charts();
    let page = if st.page == Page::Cluster { "cluster page" } else { "node page" };
    let Some(inner) = popup(
        f,
        area,
        56,
        choices.len() as u16 + 5,
        &format!("charts · {page}"),
        "space toggle · c reset · enter done",
    ) else {
        return;
    };
    let mut lines = vec![Line::from(Span::styled(
        format!(" {}/{} shown, in order of selection", current.len(), crate::app::MAX_CHARTS),
        dim(),
    ))];
    for (i, k) in choices.iter().enumerate() {
        let sel = i == cursor;
        let pos = current.iter().position(|c| c == k);
        let base = if sel { Style::new().bg(t.highlight()) } else { Style::new() };
        let mut line = Line::from(vec![
            Span::styled(if sel { " ▸ " } else { "   " }, base.fg(t.accent)),
            Span::styled(
                match pos {
                    Some(p) => format!("[{}] ", p + 1),
                    None => "[ ] ".into(),
                },
                base.fg(if pos.is_some() { t.good } else { t.dim }),
            ),
            Span::styled("● ", base.fg(chart_color(*k, t))),
            Span::styled(format!("{:<10}", chart_title(*k)), base.fg(t.fg).add_modifier(Modifier::BOLD)),
            Span::styled(chart_gloss(*k).to_string(), base.fg(t.dim)),
        ]);
        if sel {
            let w = line.width();
            line.spans.push(Span::styled(" ".repeat((inner.width as usize).saturating_sub(w)), base));
        }
        lines.push(line);
    }
    f.render_widget(Paragraph::new(lines), inner);
}
