//! TUI rendering — layout and panel conventions adapted from vllm-top
//! (GPLv3, mratsim/vllm-top) and btop (Apache-2.0, aristocratos/btop):
//! boxed rounded panels, hero label/value strip, braille area graphs with
//! dashed calibrated gridlines, aligned kv columns, banner for pause/error.
use crate::graph::{block_titled, grid_for_scale, mini_line_graph, PlotDress};
use crate::history::{Cluster, Derived, NodeHistory};
use crate::sample::Sample;
use crate::theme::{usage_color, Theme, THEMES};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Clear},
    Frame,
};
use std::sync::atomic::{AtomicUsize, Ordering};

pub static THEME_IDX: AtomicUsize = AtomicUsize::new(0);

pub fn theme() -> &'static Theme {
    &THEMES[THEME_IDX.load(Ordering::Relaxed) % THEMES.len()]
}

// ── timescale windows ────────────────────────────────────────────────
pub const WINDOWS: [f64; 3] = [60.0, 300.0, 900.0];
pub const WINDOW_LABELS: [&str; 3] = ["60s", "5m", "15m"];

pub fn window_label(secs: f64) -> &'static str {
    for (i, w) in WINDOWS.iter().enumerate() {
        if *w == secs {
            return WINDOW_LABELS[i];
        }
    }
    "?"
}

// ── chart selection (page 1 + 2 add/remove) ─────────────────────────
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartKind {
    Gpu,
    Mem,
    Cpu,
    Net,
    Disk,
    Load,
}

/// Default chart sets per page — single source of truth for draw fallbacks,
/// `c` reset, and the `e` seed when the selection is empty.
pub const DEFAULTS_CLUSTER: [ChartKind; 3] = [ChartKind::Gpu, ChartKind::Mem, ChartKind::Net];
pub const DEFAULTS_NODE: [ChartKind; 3] = [ChartKind::Cpu, ChartKind::Mem, ChartKind::Gpu];

pub fn defaults_for(page: Page) -> &'static [ChartKind] {
    match page {
        Page::Cluster => &DEFAULTS_CLUSTER,
        Page::Node => &DEFAULTS_NODE,
        Page::Vllm => &[],
    }
}

impl ChartKind {
    pub fn title(self) -> &'static str {
        match self {
            ChartKind::Gpu => "GPU",
            ChartKind::Mem => "memory",
            ChartKind::Cpu => "CPU",
            ChartKind::Net => "network ↓",
            ChartKind::Disk => "disk read",
            ChartKind::Load => "load avg",
        }
    }
    pub fn gloss(self) -> &'static str {
        match self {
            ChartKind::Gpu => "compute",
            ChartKind::Mem => "unified pool",
            ChartKind::Cpu => "aggregate",
            ChartKind::Net => "receive",
            ChartKind::Disk => "nvme",
            ChartKind::Load => "1m average",
        }
    }
    pub fn next(self) -> Self {
        use ChartKind::*;
        match self {
            Gpu => Mem,
            Mem => Cpu,
            Cpu => Net,
            Net => Disk,
            Disk => Load,
            Load => Gpu,
        }
    }
}

/// The value series for a chart kind, as (vals, dt) with gaps at bad polls.
fn chart_series(h: &NodeHistory, kind: ChartKind) -> (Vec<Option<f64>>, Vec<f64>) {
    match kind {
        ChartKind::Gpu => (gpu_vals(h), dt_of(h)),
        ChartKind::Mem => gaps(h, |d| d.mem_used_pct),
        ChartKind::Cpu => gaps(h, |d| d.cpu_pct),
        ChartKind::Net => gaps(h, |d| d.net_rx_bps),
        ChartKind::Disk => gaps(h, |d| d.disk_read_bps),
        ChartKind::Load => gaps(h, |d| d.load1),
    }
}

fn chart_unit(kind: ChartKind) -> &'static str {
    match kind {
        ChartKind::Gpu | ChartKind::Mem | ChartKind::Cpu => "%",
        ChartKind::Net | ChartKind::Disk => "B/s",
        ChartKind::Load => "",
    }
}

fn chart_color(kind: ChartKind, t: &Theme) -> ratatui::style::Color {
    match kind {
        ChartKind::Gpu => t.s3,
        ChartKind::Mem => t.warn,
        ChartKind::Cpu => t.s1,
        // net must not share the CPU green: they end up adjacent when the
        // user builds a full row
        ChartKind::Net => t.accent,
        ChartKind::Disk => t.s2,
        ChartKind::Load => t.bad,
    }
}

// ── formatting (vllm-top conventions) ────────────────────────────────
fn fmt_tokens(v: f64) -> String {
    if !v.is_finite() { return "—".into(); }
    if v >= 1.0e6 {
        format!("{:.1}M", v / 1.0e6)
    } else if v >= 1.0e3 {
        format!("{:.0}k", v / 1.0e3)
    } else {
        format!("{v:.0}")
    }
}

fn fmt_bps(bps: f64) -> String {
    if !bps.is_finite() { return "—".into(); }
    if bps.abs() >= 1e6 {
        format!("{:.1}MB/s", bps / 1e6)
    } else if bps.abs() >= 1e3 {
        format!("{:.1}KB/s", bps / 1e3)
    } else {
        format!("{:.0}B/s", bps)
    }
}

fn fmt_mb(mb: f64) -> String {
    if mb >= 1024.0 {
        format!("{:.1}GB", mb / 1024.0)
    } else {
        format!("{:.0}MB", mb)
    }
}

fn fmt_dur(s: u64) -> String {
    if s >= 3600 {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m{}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

fn fmt_secs(v: Option<f64>) -> String {
    match v {
        Some(v) if v >= 100.0 => format!("{v:.0}s"),
        Some(v) if v >= 10.0 => format!("{v:.1}s"),
        Some(v) => format!("{:.0}ms", v * 1000.0),
        None => "—".into(),
    }
}

fn triple(v: &[Option<f64>; 3]) -> String {
    v.iter()
        .map(|x| x.map(|x| format!("{x:.0}")).unwrap_or_else(|| "—".into()))
        .collect::<Vec<_>>()
        .join("/")
}

fn truncate_line(line: Line<'static>, width: usize) -> Line<'static> {
    let w: usize = line.width();
    if w <= width {
        return line;
    }
    if width == 0 {
        return Line::from("");
    }
    let mut spans = line.spans;
    let mut budget = width - 1; // reserve one column for the ellipsis
    let mut out: Vec<Span> = Vec::with_capacity(spans.len());
    for s in spans.drain(..) {
        let sw = s.content.chars().count();
        if sw >= budget {
            let mut text: String = s.content.chars().take(budget).collect();
            text.push('…');
            out.push(Span::styled(text, s.style));
            break;
        }
        budget -= sw;
        out.push(s);
    }
    Line::from(out)
}

fn banner(f: &mut Frame, area: Rect, msg: String, color: ratatui::style::Color) {
    let a = Rect {
        y: area.y + area.height.saturating_sub(1),
        ..area
    };
    let line = Line::from(Span::styled(msg, Style::new().fg(color).add_modifier(Modifier::BOLD)));
    f.render_widget(Paragraph::new(line), a);
}

// ── kv helpers (vllm-top Kv/kv_lines) ────────────────────────────────
struct Kv {
    k: String,
    v: String,
    gloss: String,
    style: Option<Style>,
}

impl Kv {
    fn plain(k: &str, v: String, gloss: &str) -> Self {
        Self { k: k.into(), v, gloss: gloss.into(), style: None }
    }
    fn colored(k: &str, v: String, gloss: &str, style: ratatui::style::Color) -> Self {
        Self { k: k.into(), v, gloss: gloss.into(), style: Some(Style::new().fg(style)) }
    }
    fn gloss_only(gloss: String) -> Self {
        Self { k: String::new(), v: String::new(), gloss, style: None }
    }
}

fn kv_lines(t: &Theme, entries: Vec<Kv>, width: usize) -> Vec<Line<'static>> {
    let kpad = entries.iter().map(|e| e.k.chars().count()).max().unwrap_or(0).max(8);
    let vpad = entries.iter().map(|e| e.v.chars().count()).max().unwrap_or(1);
    entries
        .into_iter()
        .map(|e| {
            if e.k.is_empty() && e.v.is_empty() {
                return truncate_line(
                    Line::from(Span::styled(format!(" {}", e.gloss), Style::new().fg(t.dim))),
                    width,
                );
            }
            let value_style =
                e.style.unwrap_or(Style::new().fg(t.fg)).add_modifier(Modifier::BOLD);
            truncate_line(
                Line::from(vec![
                    Span::styled(format!(" {:<kpad$}  ", e.k), Style::new().fg(t.fg)),
                    Span::styled(format!("{:<vpad$}  ", e.v), value_style),
                    Span::styled(e.gloss, Style::new().fg(t.dim)),
                ]),
                width,
            )
        })
        .collect()
}

// ── series prep ──────────────────────────────────────────────────────
fn dt_of(h: &NodeHistory) -> Vec<f64> {
    if h.samples.len() > 1 {
        h.samples
            .windows(2)
            .map(|w| {
                w[1].rate_time_us().saturating_sub(w[0].rate_time_us()) as f64 / 1e6
            })
            .collect()
    } else {
        Vec::new()
    }
}

fn gaps(h: &NodeHistory, sel: impl Fn(&Derived) -> f64) -> (Vec<Option<f64>>, Vec<f64>) {
    let vals: Vec<Option<f64>> = h
        .derived
        .iter()
        .map(|d| {
            let v = sel(d);
            if v.is_finite() {
                Some(v)
            } else {
                None
            }
        })
        .collect();
    (vals, dt_of(h))
}

fn gpu_vals(h: &NodeHistory) -> Vec<Option<f64>> {
    h.samples.iter().map(|s| s.gpus.first().map(|g| g.util_pct)).collect()
}

fn draw_graph(
    f: &mut Frame,
    t: &Theme,
    area: Rect,
    title: &str,
    gloss: &str,
    vals: &[Option<f64>],
    dt: &[f64],
    window_secs: f64,
    color: ratatui::style::Color,
    unit: &str,
    peak: f64,
    sub: Option<Line<'static>>,
) {
    let vmax = vals.iter().filter_map(|v| *v).fold(0.0_f64, f64::max);
    // percent graphs are pinned to the 0–100 scale so a 45% reading fills
    // 45% of the panel — auto-scaling would make idle look pegged
    let scale = if unit.contains('%') { 100.0 } else { vmax.max(peak) };
    let (ref_lines, _ymax) = grid_for_scale(scale);
    let now = vals.last().copied().flatten().unwrap_or(f64::NAN);
    let fmtv = |v: f64| -> String {
        if !v.is_finite() { return "—".into(); }
        if unit.contains("tok/s") {
            format!("{} tok/s", fmt_tokens(v))
        } else if unit.contains("B/s") {
            fmt_bps(v)
        } else if unit.contains('%') {
            format!("{v:.0}%")
        } else if unit.contains("ms") {
            format!("{v:.0}ms")
        } else {
            format!("{v:.2}")
        }
    };
    let mut title_spans = vec![
        Span::styled(format!(" {title} "), Style::new().fg(t.fg).add_modifier(Modifier::BOLD)),
        Span::styled(format!("({gloss}) "), Style::new().fg(t.dim)),
        Span::styled(
            format!("— {} {} · peak {} ", "now", fmtv(now), fmtv(peak)),
            Style::new().fg(t.fg).add_modifier(Modifier::BOLD),
        ),
        Span::styled("● ", Style::new().fg(color).add_modifier(Modifier::BOLD)),
    ];
    title_spans.truncate(6);
    let block = block_titled(Line::from(title_spans), sub, t);
    let inner = block.inner(area);
    f.render_widget(block, area);
    mini_line_graph(
        f,
        inner,
        vals,
        dt,
        window_secs,
        color,
        PlotDress { unit, ref_lines: &ref_lines, overlay: None, dim: t.dim },
    );
}

/// Hero label/value cell like vllm-top Engine status: dim label, bright
/// value, dim gloss.
fn hero_cell(label: &str, value: &str, gloss: &str, vstyle: Style) -> (String, Vec<Line<'static>>) {
    (
        label.to_string(),
        vec![
            Line::from(Span::styled(format!(" {value}"), vstyle)),
            Line::from(Span::styled(format!(" {gloss}"), Style::new().fg(theme().dim))),
        ],
    )
}

fn hero_cells_for_node(s: &Sample, d: &Derived, t: &Theme) -> Vec<(String, Vec<Line<'static>>)> {
    let gpu = s.gpus.first();
    let mem_frac = if s.mem.total_kb > 0 {
        s.mem.used_kb() as f64 / s.mem.total_kb as f64
    } else {
        0.0
    };
    let bold = |c: ratatui::style::Color| Style::new().fg(c).add_modifier(Modifier::BOLD);
    let mut cells = Vec::new();
    let run = gpu.map(|g| g.util_pct).unwrap_or(0.0);
    cells.push(hero_cell(
        "GPU",
        &format!("{run:.0}%"),
        "of compute capacity",
        bold(if run >= 90.0 { t.bad } else { t.good }),
    ));
    cells.push(hero_cell(
        "MEMORY",
        &format!("{:.0}%", mem_frac * 100.0),
        "unified — CPU + GPU share",
        bold(usage_color(t, mem_frac)),
    ));
    cells.push((
        "CPU".into(),
        vec![
            Line::from(vec![
                Span::styled(" load ", Style::new().fg(t.dim)),
                Span::styled(format!("{:.2}", d.load1.max(s.cpu.load1)), bold(t.s3)),
                Span::styled(
                    format!("  ·  {}", triple(&[Some(s.cpu.load5), Some(s.cpu.load15), None])),
                    Style::new().fg(t.dim),
                ),
            ]),
            Line::from(vec![
                Span::styled(" cores ", Style::new().fg(t.dim)),
                Span::styled(format!("{}", s.cpu.cores.len()), bold(t.s1)),
                Span::styled(format!("  ·  {} running", s.cpu.procs_running), Style::new().fg(t.dim)),
            ]),
        ],
    ));
    cells.push(hero_cell(
        "POWER",
        &format!("{:.1}W", gpu.map(|g| g.power_w).unwrap_or(0.0)),
        "board draw",
        bold(t.warn),
    ));
    cells.push(hero_cell(
        "TEMP",
        &format!("{:.0}°C", gpu.map(|g| g.temp_c).unwrap_or(0.0)),
        "GPU package",
        bold(if gpu.map(|g| g.temp_c).unwrap_or(0.0) >= 80.0 { t.bad } else { t.s1 }),
    ));
    cells.push(hero_cell(
        "NET ↓",
        &fmt_bps(d.net_rx_bps),
        &format!("↑ {}", fmt_bps(d.net_tx_bps)),
        bold(t.accent),
    ));
    cells
}

/// Render hero cells across `area` at fixed column widths.
fn render_hero(f: &mut Frame, t: &Theme, area: Rect, cells: &[(String, Vec<Line<'static>>)], col_ws: &[u16]) {
    let mut x = area.x;
    for (ci, (label, lines)) in cells.iter().enumerate() {
        if ci >= col_ws.len() {
            break;
        }
        let w = col_ws[ci].min(area.width.saturating_sub(x - area.x));
        let cell_area = Rect { x, width: w, ..area };
        let mut ls = vec![Line::from(Span::styled(label.to_lowercase(), Style::new().fg(t.dim)))];
        for l in lines {
            ls.push(truncate_line(l.clone(), w as usize));
        }
        f.render_widget(Paragraph::new(ls), cell_area);
        x += w;
    }
}

/// Help overlay (vllm-top `?` style): centered panel over the page.
fn draw_help(f: &mut Frame, area: Rect) {
    let t = theme();
    let w = 62.min(area.width.saturating_sub(4));
    let h = 24.min(area.height.saturating_sub(4));
    if w < 30 || h < 10 {
        return;
    }
    let x = area.x + (area.width - w) / 2;
    let y = area.y + (area.height - h) / 2;
    let popup = Rect { x, y, width: w, height: h };
    f.render_widget(Clear, popup);
    let block = block_titled(
        Line::from(Span::styled(
            " keys ",
            Style::new().fg(t.fg).add_modifier(Modifier::BOLD),
        )),
        Some(Line::from(Span::styled(" esc closes help — esc again quits ", Style::new().fg(t.dim)))),
        t,
    );
    f.render_widget(block, popup);
    let inner = Rect { x: x + 1, y: y + 1, width: w - 2, height: h - 2 };
    let lines = vec![
        Line::from(vec![
            Span::styled(" 1 2 3      ", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
            Span::styled("cluster / node / vllm pages", Style::new().fg(t.fg)),
        ]),
        Line::from(vec![
            Span::styled(" tab       ", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
            Span::styled("switch focused node", Style::new().fg(t.fg)),
        ]),
        Line::from(vec![
            Span::styled(" w         ", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
            Span::styled("graph timescale: 60s → 5m → 15m", Style::new().fg(t.fg)),
        ]),
        Line::from(vec![
            Span::styled(" e         ", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
            Span::styled("add next chart (max 6; pages 1+2)", Style::new().fg(t.fg)),
        ]),
        Line::from(vec![
            Span::styled(" r         ", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
            Span::styled("remove last chart (pages 1+2)", Style::new().fg(t.fg)),
        ]),
        Line::from(vec![
            Span::styled(" c         ", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
            Span::styled("reset page charts to defaults", Style::new().fg(t.fg)),
        ]),
        Line::from(vec![
            Span::styled(" space     ", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
            Span::styled("pause / resume polling", Style::new().fg(t.fg)),
        ]),
        Line::from(vec![
            Span::styled(" t         ", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
            Span::styled("theme: gruvbox → catppuccin → tokyonight", Style::new().fg(t.fg)),
        ]),
        Line::from(vec![
            Span::styled(" tab/⇧tab  ", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
            Span::styled("next / previous node", Style::new().fg(t.fg)),
        ]),
        Line::from(vec![
            Span::styled(" q / esc   ", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
            Span::styled("quit", Style::new().fg(t.fg)),
        ]),
        Line::from(""),
        Line::from(Span::styled(" chart kinds", Style::new().fg(t.dim).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled(
            "  gpu · memory · cpu · network · disk · load",
            Style::new().fg(t.dim),
        )),
        Line::from(""),
        Line::from(Span::styled(" data notes", Style::new().fg(t.dim).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled(
            "  % graphs are pinned 0–100; others autoscale to peak",
            Style::new().fg(t.dim),
        )),
        Line::from(Span::styled(
            "  vLLM page needs vllm_url in sparktop.toml",
            Style::new().fg(t.dim),
        )),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

// ── top-level draw ───────────────────────────────────────────────────
pub fn draw(
    f: &mut Frame,
    cluster: &Cluster,
    order: &[String],
    page: Page,
    focused: usize,
    paused: bool,
    interval: f64,
    err: &Option<String>,
    ages: &[Option<u64>],
    window_secs: f64,
    cluster_charts: &[ChartKind],
    node_charts: &[ChartKind],
    show_help: bool,
    status: &Option<(String, std::time::Instant)>,
) {
    let t = theme();
    f.render_widget(Block::new().style(Style::new().bg(t.bg)), f.area());

    // header (vllm-top style)
    let now_s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut spans = vec![Span::styled(
        " sparktop",
        Style::new().fg(t.accent).add_modifier(Modifier::BOLD),
    )];
    spans.push(Span::styled(
        format!(" · {} node{}", order.len(), if order.len() == 1 { "" } else { "s" }),
        Style::new().fg(t.fg),
    ));
    if page != Page::Cluster {
        if let Some(name) = order.get(focused) {
            spans.push(Span::styled(format!(" · {name}"), Style::new().fg(t.accent)));
        }
        // stale-data age: frozen data must never look live
        if let Some(Some(ok)) = ages.get(focused) {
            let age = now_s.saturating_sub(*ok);
            if age > 3 {
                spans.push(Span::styled(
                    format!(" · data {}s old", age),
                    Style::new().fg(if age > 10 { t.bad } else { t.warn }).add_modifier(Modifier::BOLD),
                ));
            }
        }
    }
    spans.push(Span::styled(format!(" · poll {interval:.1}s"), Style::new().fg(t.dim)));
    spans.push(Span::styled(
        format!(" · window {}", window_label(window_secs)),
        Style::new().fg(t.dim),
    ));
    if paused {
        spans.push(Span::styled(" · ⏸ paused", Style::new().fg(t.warn).add_modifier(Modifier::BOLD)));
    }
    if let Some(e) = err {
        spans.push(Span::styled(format!(" · ⚠ {e}"), Style::new().fg(t.bad)));
    }
    let hints = "[1/2/3 · tab node · space pause · w window · e add · r rm chart · c reset · t theme · ? help · q quit]";
    spans.push(Span::styled(format!("  {hints}"), Style::new().fg(t.dim)));
    // keep the full hint readable: drop earlier spans before cutting mid-word
    let width = f.area().width as usize;
    let mut kept: Vec<Span> = Vec::new();
    let mut used = 0usize;
    for s in spans.iter().rev() {
        let l = s.content.chars().count();
        if used + l > width {
            break;
        }
        used += l;
        kept.push(s.clone());
    }
    kept.reverse();
    f.render_widget(Paragraph::new(Line::from(kept)), f.area());

    // reserve the last row for the pause/error banner instead of
    // overdrawing the outer panel border
    let body_h = f.area().height.saturating_sub(2);
    let body = Rect { y: f.area().y + 1, height: body_h, ..f.area() };
    match page {
        Page::Cluster => draw_cluster(f, body, cluster, order, focused, err, window_secs, cluster_charts),
        Page::Node => draw_node(f, body, cluster, order.get(focused), window_secs, node_charts),
        Page::Vllm => draw_vllm(f, body, cluster, order.get(focused), window_secs),
    }

    if paused {
        banner(f, f.area(), "⏸ paused — press space to resume".into(), t.warn);
    } else if err.is_some() {
        banner(f, f.area(), format!("⚠ {} — retrying…", err.clone().unwrap()), t.bad);
    } else if let Some((msg, at)) = status {
        // transient no-op feedback: show for 2s then drop
        if at.elapsed() < std::time::Duration::from_secs(2) {
            banner(f, f.area(), msg.clone(), t.dim);
        }
    }

    if show_help {
        draw_help(f, f.area());
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Page {
    Cluster,
    Node,
    Vllm,
}

// ── cluster page: hero strip + user-selected graphs per node ────────
fn draw_cluster(
    f: &mut Frame,
    area: Rect,
    cluster: &Cluster,
    order: &[String],
    focused: usize,
    err: &Option<String>,
    window_secs: f64,
    charts: &[ChartKind],
) {
    let t = theme();
    let n = order.len().max(1);
    let bands = Layout::vertical(
        std::iter::repeat(Constraint::Ratio(1, n as u32)).take(n),
    )
    .split(area);
    let charts: &[ChartKind] = if charts.is_empty() { &DEFAULTS_CLUSTER } else { charts };
    for (i, name) in order.iter().enumerate() {
        let band = bands[i];
        let h = cluster.get(name);
        let alive = h.map(|h| !h.samples.is_empty()).unwrap_or(false);
        let (title, subtitle) = if alive {
            let (s, _d) = h.unwrap().last().unwrap();
            (
                Line::from(vec![
                    Span::styled(format!(" {name} "), Style::new().fg(t.fg).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("up {} ", fmt_dur(s.uptime_s)), Style::new().fg(t.dim)),
                ]),
                Some(Line::from(Span::styled(
                    format!(" gpu {:.0}% · mem {:.0}% · load {:.1} ", s.gpus.first().map(|g| g.util_pct).unwrap_or(0.0),
                        if s.mem.total_kb > 0 { 100.0 * s.mem.used_kb() as f64 / s.mem.total_kb as f64 } else { 0.0 },
                        s.cpu.load1),
                    Style::new().fg(t.dim),
                ))),
            )
        } else {
            (Line::from(Span::styled(format!(" {name} "), Style::new().fg(t.fg).add_modifier(Modifier::BOLD))), None)
        };
        let block = block_titled(title, subtitle, t)
            .border_style(Style::new().fg(if i == focused { t.accent } else { t.dim }));
        let inner = block.inner(band);
        f.render_widget(block, band);

        let (Some(h), true) = (h, alive) else {
            let msg = match err {
                Some(e) if e.contains(name.as_str()) => e.clone(),
                _ => "waiting for first poll…".into(),
            };
            let row = Rect { y: inner.y + inner.height / 2, height: 1, ..inner };
            f.render_widget(Paragraph::new(Line::from(Span::styled(msg, Style::new().fg(t.dim)))), row);
            continue;
        };
        let (s, d) = h.last().unwrap();

        let rows = Layout::vertical([Constraint::Length(2), Constraint::Min(3)]).split(inner);
        // hero cells
        let cells = hero_cells_for_node(s, d, t);
        let col_ws = [10u16, 10, 12, 9, 9, 12];
        render_hero(f, t, rows[0], &cells, &col_ws);
        // user-selected graphs, evenly split
        let n_charts = charts.len().max(1) as u32;
        let cols = Layout::horizontal(
            std::iter::repeat(Constraint::Ratio(1, n_charts)).take(charts.len().max(1)),
        )
        .split(rows[1]);
        for (ci, kind) in charts.iter().enumerate() {
            let (vals, dt) = chart_series(h, *kind);
            let peak = vals.iter().filter_map(|v| *v).fold(0.0, f64::max);
            draw_graph(
                f,
                t,
                cols[ci],
                kind.title(),
                kind.gloss(),
                &vals,
                &dt,
                window_secs,
                chart_color(*kind, t),
                chart_unit(*kind),
                peak,
                None,
            );
        }
    }
}

// ── node page: btop-style full detail ────────────────────────────────
fn draw_node(f: &mut Frame, area: Rect, cluster: &Cluster, name: Option<&String>, window_secs: f64, charts: &[ChartKind]) {
    let t = theme();
    let h = match name.and_then(|n| cluster.get(n)) {
        Some(h) if !h.samples.is_empty() => h,
        _ => return,
    };
    let (s, d) = h.last().unwrap();

    let rows = Layout::vertical([
        Constraint::Length(2), // hero strip
        Constraint::Min(10),   // big graphs row (cpu/mem/gpu)
        Constraint::Length(9), // net + totals
        Constraint::Length(7), // disk + procs
    ])
    .split(area);

    // hero strip
    let cells = hero_cells_for_node(s, d, t);
    let col_ws = [9u16, 9, 14, 8, 8, 13];
    render_hero(f, t, rows[0], &cells, &col_ws);

    // big graphs row: CPU / memory / GPU equal thirds (btop-style balance),
    // or the user's chart selection when customized
    let charts: &[ChartKind] = if charts.is_empty() { &DEFAULTS_NODE } else { charts };
    let n = charts.len().max(1) as u32;
    let big = Layout::horizontal(
        std::iter::repeat(Constraint::Ratio(1, n)).take(charts.len().max(1)),
    )
    .split(rows[1]);
    for (ci, kind) in charts.iter().enumerate() {
        let (vals, dt) = chart_series(h, *kind);
        let peak = vals.iter().filter_map(|v| *v).fold(0.0, f64::max);
        let sub = match kind {
            ChartKind::Cpu => Some(Line::from(Span::styled(
                format!(
                    " load {:.1}/{:.1}/{:.1} · {} running/{} total ",
                    s.cpu.load1, s.cpu.load5, s.cpu.load15, s.cpu.procs_running, s.cpu.procs_total
                ),
                Style::new().fg(t.dim),
            ))),
            ChartKind::Gpu => match s.gpus.first() {
                Some(g) => Some(Line::from(Span::styled(
                    format!(" power {:.1}W · temp {:.0}°C · SM {:.0}MHz ", g.power_w, g.temp_c, g.sm_clock_mhz),
                    Style::new().fg(t.dim),
                ))),
                None => Some(Line::from(Span::styled(" no GPU detected ", Style::new().fg(t.dim)))),
            },
            ChartKind::Mem => Some(Line::from(Span::styled(
                format!(
                    " used {:.1}GB of {:.1}GB · cached {:.1}GB ",
                    s.mem.used_kb() as f64 / 1048576.0,
                    s.mem.total_kb as f64 / 1048576.0,
                    s.mem.cached_kb as f64 / 1048576.0
                ),
                Style::new().fg(t.dim),
            ))),
            _ => None,
        };
        draw_graph(
            f,
            t,
            big[ci],
            kind.title(),
            kind.gloss(),
            &vals,
            &dt,
            window_secs,
            chart_color(*kind, t),
            chart_unit(*kind),
            peak,
            sub,
        );
    }

    // net row
    let net_cols = Layout::horizontal([Constraint::Percentage(65), Constraint::Percentage(35)]).split(rows[2]);
    let (nv, ndt) = gaps(h, |d| d.net_rx_bps);
    let net_sub = Line::from(Span::styled(
        format!(" ↑ now {} ", fmt_bps(d.net_tx_bps)),
        Style::new().fg(t.dim),
    ));
    let net_peak = nv.iter().filter_map(|v| *v).fold(0.0, f64::max);
    draw_graph(f, t, net_cols[0], "network ↓", "receive", &nv, &ndt, window_secs, t.accent, "B/s", net_peak, Some(net_sub));

    let tot_rx: u64 = s.net.values().map(|n| n.rx_bytes).sum();
    let tot_tx: u64 = s.net.values().map(|n| n.tx_bytes).sum();
    let mut ifaces: Vec<_> = s.net.iter().filter(|(n, _)| *n != "lo").collect();
    ifaces.sort_by_key(|(n, _)| (*n).clone());
    let mut net_kvs = vec![
        Kv::colored("↓ now", fmt_bps(d.net_rx_bps), "receive", t.s1),
        Kv::colored("↑ now", fmt_bps(d.net_tx_bps), "transmit", t.s2),
        Kv::plain("↓ total", fmt_mb(tot_rx as f64 / 1048576.0), "since boot"),
        Kv::plain("↑ total", fmt_mb(tot_tx as f64 / 1048576.0), "since boot"),
    ];
    for (n, _) in ifaces.iter().take(3) {
        net_kvs.push(Kv::gloss_only(format!("iface {n}")));
    }
    let net_block = block_titled(
        Line::from(Span::styled(" totals ", Style::new().fg(t.fg).add_modifier(Modifier::BOLD))),
        None,
        t,
    );
    let net_inner = net_block.inner(net_cols[1]);
    f.render_widget(net_block, net_cols[1]);
    f.render_widget(Paragraph::new(kv_lines(t, net_kvs, net_inner.width as usize)), net_inner);

    // disk + gpu processes
    let dp_cols = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[3]);
    let (dv, ddt) = gaps(h, |d| d.disk_read_bps);
    let disk_peak = dv.iter().filter_map(|v| *v).fold(0.0, f64::max);
    let disk_sub = Line::from(Span::styled(
        format!(" write {} ", fmt_bps(d.disk_write_bps)),
        Style::new().fg(t.dim),
    ));
    draw_graph(f, t, dp_cols[0], "disk read", "nvme", &dv, &ddt, window_secs, t.s2, "B/s", disk_peak, Some(disk_sub));

    let proc_block = block_titled(
        Line::from(Span::styled(" gpu processes ", Style::new().fg(t.fg).add_modifier(Modifier::BOLD))),
        None,
        t,
    );
    let proc_inner = proc_block.inner(dp_cols[1]);
    f.render_widget(proc_block, dp_cols[1]);
    let procs = s.gpus.first().map(|g| g.pids.as_slice()).unwrap_or(&[]);
    let mut proc_kvs = Vec::new();
    if procs.is_empty() {
        proc_kvs.push(Kv::gloss_only("none — no compute jobs".into()));
    }
    for p in procs.iter().take(proc_inner.height.saturating_sub(1) as usize) {
        proc_kvs.push(Kv::colored(
            &format!("{}", p.pid),
            fmt_mb(p.mem_mb),
            &truncate(&p.name, 28),
            t.fg,
        ));
    }
    f.render_widget(Paragraph::new(kv_lines(t, proc_kvs, proc_inner.width as usize)), proc_inner);
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
    }
}

// ── vllm page: vllm-top engine status + rate graphs ──────────────────
fn draw_vllm(f: &mut Frame, area: Rect, cluster: &Cluster, name: Option<&String>, window_secs: f64) {
    let t = theme();
    let h = match name.and_then(|n| cluster.get(n)) {
        Some(h) if !h.samples.is_empty() => h,
        _ => return,
    };
    let (s, d) = h.last().unwrap();
    let v = match &s.vllm {
        Some(v) => v,
        None => {
            let msg = format!("no vLLM metrics for {} — set vllm_url in sparktop.toml (server must expose /metrics)", name.unwrap_or(&String::new()));
            let row = Rect { y: area.y + area.height / 2, ..area };
            f.render_widget(Paragraph::new(Line::from(Span::styled(msg, Style::new().fg(t.dim)))), row);
            return;
        }
    };

    let rows = Layout::vertical([
        Constraint::Length(7),  // engine status hero
        Constraint::Min(9),    // prefill / decode graphs
        Constraint::Length(7), // latency table + plots
        Constraint::Min(4),    // cache / peaks
    ])
    .split(area);

    // engine status hero (vllm-top style cells)
    let bold = |c: ratatui::style::Color| Style::new().fg(c).add_modifier(Modifier::BOLD);
    let cells: Vec<(String, Vec<Line>)> = vec![
        hero_cell("RUNNING", &fmt_tokens(v.running as f64), "being answered now", bold(t.fg)),
        hero_cell("QUEUE", &fmt_tokens(v.waiting as f64), "waiting to start", bold(if v.waiting > 20 { t.bad } else if v.waiting > 5 { t.warn } else { t.fg })),
        (
            "TOK/S · 5s / 30s".into(),
            vec![
                Line::from(Span::styled(format!(" prefill {} / {}", fmt_tokens(d.prompt_smooth), fmt_tokens(d.prompt_avg30)), bold(t.s2))),
                Line::from(Span::styled(format!(" decode  {} / {}", fmt_tokens(d.generation_smooth), fmt_tokens(d.generation_avg30)), bold(t.s1))),
                Line::from(Span::styled(format!(" raw PP {} · TG {}", fmt_tokens(d.prompt_tps), fmt_tokens(d.generation_tps)), Style::new().fg(t.dim))),
            ],
        ),
        hero_cell("TTFT", &fmt_secs(d.ttft_p95), "p95 · first token", bold(if d.ttft_p95.unwrap_or(0.0) > 1.0 { t.bad } else { t.good })),
        hero_cell("TPOT", &fmt_secs(d.tpot_p95), "p95 · per output token", bold(t.s1)),
        (
            "MEMORY POOLS".into(),
            vec![
                Line::from(Span::styled(format!(" max gpu KV {:.0}%", v.gpu_cache_usage * 100.0), bold(usage_color(t, v.gpu_cache_usage)))),
                Line::from(Span::styled(format!(" cpu KV {:.0}%", v.cpu_cache_usage * 100.0), bold(t.accent))),
                Line::from(Span::styled(" full = requests queue", Style::new().fg(t.dim))),
            ],
        ),
    ];
    let col_ws = [12u16, 12, 28, 14, 14, 18];
    render_hero(f, t, rows[0], &cells, &col_ws);

    // prefill / decode graphs (the centerpiece)
    let chart_cols = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[1]);
    let (pv, pdt) = gaps(h, |d| d.prompt_smooth);
    let (dv, ddt) = gaps(h, |d| d.generation_smooth);
    let p_peak = pv.iter().filter_map(|v| *v).fold(0.0, f64::max);
    let d_peak = dv.iter().filter_map(|v| *v).fold(0.0, f64::max);
    draw_graph(f, t, chart_cols[0], "Prefill", "5s mean", &pv, &pdt, window_secs, t.s2, " tok/s", p_peak, Some(Line::from(format!(" raw {} · 30s avg {} tok/s ", fmt_tokens(d.prompt_tps), fmt_tokens(d.prompt_avg30)))));
    draw_graph(f, t, chart_cols[1], "Decode", "5s mean · aggregate", &dv, &ddt, window_secs, t.s1, " tok/s", d_peak, Some(Line::from(format!(" raw {} · 30s avg {} tok/s ", fmt_tokens(d.generation_tps), fmt_tokens(d.generation_avg30)))));

    // latency table + ttft/tpot plots
    let lat_cols = Layout::horizontal([Constraint::Percentage(34), Constraint::Percentage(33), Constraint::Percentage(33)]).split(rows[2]);
    let lat_block = block_titled(
        Line::from(Span::styled(
            " latency ",
            Style::new().fg(t.fg).add_modifier(Modifier::BOLD),
        )),
        Some(Line::from(Span::styled(" p50 typical · p95 1-in-20 ", Style::new().fg(t.dim)))),
        t,
    );
    let lat_inner = lat_block.inner(lat_cols[0]);
    f.render_widget(lat_block, lat_cols[0]);
    let lat_kvs = vec![
        Kv::colored("TTFT p50", fmt_secs(d.ttft_p50), "first token", t.s3),
        Kv::colored("TTFT p95", fmt_secs(d.ttft_p95), "1-in-20", t.warn),
        Kv::colored("TPOT p50", fmt_secs(d.tpot_p50), "per token", t.s1),
        Kv::colored("TPOT p95", fmt_secs(d.tpot_p95), "1-in-20", t.accent),
    ];
    f.render_widget(Paragraph::new(kv_lines(t, lat_kvs, lat_inner.width as usize)), lat_inner);

    let ttft_data: Vec<Option<f64>> = h.derived.iter().map(|dd| dd.ttft_p50.map(|v| v * 1000.0)).collect();
    let tdt = dt_of(h);
    let ttft_peak = ttft_data.iter().filter_map(|v| *v).fold(0.0, f64::max);
    draw_graph(f, t, lat_cols[1], "TTFT p50", "time to first token", &ttft_data, &tdt, window_secs, t.s3, "ms", ttft_peak, None);
    let tpot_data: Vec<Option<f64>> = h.derived.iter().map(|dd| dd.tpot_p50.map(|v| v * 1000.0)).collect();
    let tpot_peak = tpot_data.iter().filter_map(|v| *v).fold(0.0, f64::max);
    draw_graph(f, t, lat_cols[2], "TPOT p50", "per output token", &tpot_data, &tdt, window_secs, t.accent, "ms", tpot_peak, None);

    // cache + peaks
    let bot_cols = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[3]);
    let cache_block = block_titled(
        Line::from(Span::styled(" cache ", Style::new().fg(t.fg).add_modifier(Modifier::BOLD))),
        Some(Line::from(Span::styled(" prefix cache / KV pools ", Style::new().fg(t.dim)))),
        t,
    );
    let cache_inner = cache_block.inner(bot_cols[0]);
    f.render_widget(cache_block, bot_cols[0]);
    let cache_kvs = vec![
        Kv::colored("max gpu KV", format!("{:.0}%", v.gpu_cache_usage * 100.0), "busiest engine", usage_color(t, v.gpu_cache_usage)),
        Kv::colored("cpu KV", format!("{:.0}%", v.cpu_cache_usage * 100.0), "host offload tier", t.accent),
    ];
    f.render_widget(Paragraph::new(kv_lines(t, cache_kvs, cache_inner.width as usize)), cache_inner);

    let peaks_block = block_titled(
        Line::from(Span::styled(" peaks ", Style::new().fg(t.fg).add_modifier(Modifier::BOLD))),
        Some(Line::from(Span::styled(" raw · retained history ", Style::new().fg(t.dim)))),
        t,
    );
    let peaks_inner = peaks_block.inner(bot_cols[1]);
    f.render_widget(peaks_block, bot_cols[1]);
    let peaks_kvs = vec![
        Kv::colored("decode", format!("{} tok/s", fmt_tokens(h.derived.iter().map(|d| d.generation_tps).filter(|v| v.is_finite()).fold(0.0, f64::max))), &format!("raw {}", fmt_tokens(d.generation_tps)), t.s1),
        Kv::colored("prefill", format!("{} tok/s", fmt_tokens(h.derived.iter().map(|d| d.prompt_tps).filter(|v| v.is_finite()).fold(0.0, f64::max))), &format!("raw {}", fmt_tokens(d.prompt_tps)), t.s2),
    ];
    f.render_widget(Paragraph::new(kv_lines(t, peaks_kvs, peaks_inner.width as usize)), peaks_inner);
}
