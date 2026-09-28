//! Shared building blocks: formatting (vllm-top conventions), kv columns,
//! hero cells, meters, health dots, and the boxed graph panel.
use super::theme;
use crate::app::ChartKind;
use crate::config::Alerts;
use crate::graph::{block_titled, grid_for_scale, mini_line_graph, PlotDress};
use crate::history::{Derived, Health, NodeHistory};
use crate::theme::{lerp, Theme};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

// ── formatting ───────────────────────────────────────────────────────
pub fn fmt_tokens(v: f64) -> String {
    if !v.is_finite() {
        return "—".into();
    }
    if v >= 1.0e6 {
        format!("{:.1}M", v / 1.0e6)
    } else if v >= 1.0e4 {
        format!("{:.0}k", v / 1.0e3)
    } else if v >= 1.0e3 {
        format!("{:.1}k", v / 1.0e3)
    } else {
        format!("{v:.0}")
    }
}

pub fn fmt_bps(bps: f64) -> String {
    if !bps.is_finite() {
        return "—".into();
    }
    if bps.abs() >= 1e9 {
        format!("{:.2}GB/s", bps / 1e9)
    } else if bps.abs() >= 1e6 {
        format!("{:.1}MB/s", bps / 1e6)
    } else if bps.abs() >= 1e3 {
        format!("{:.1}KB/s", bps / 1e3)
    } else {
        format!("{:.0}B/s", bps)
    }
}

pub fn fmt_mb(mb: f64) -> String {
    if mb >= 1024.0 * 1024.0 {
        format!("{:.1}TB", mb / 1048576.0)
    } else if mb >= 1024.0 {
        format!("{:.1}GB", mb / 1024.0)
    } else {
        format!("{:.0}MB", mb)
    }
}

pub fn fmt_dur(s: u64) -> String {
    if s >= 86400 {
        format!("{}d{:02}h", s / 86400, (s % 86400) / 3600)
    } else if s >= 3600 {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m{}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

pub fn fmt_secs(v: Option<f64>) -> String {
    match v {
        Some(v) if v >= 100.0 => format!("{v:.0}s"),
        Some(v) if v >= 10.0 => format!("{v:.1}s"),
        Some(v) if v >= 1.0 => format!("{v:.2}s"),
        Some(v) => format!("{:.0}ms", v * 1000.0),
        None => "—".into(),
    }
}

/// Small per-second rates keep a decimal (0.3 req/s, not 0).
pub fn fmt_rate(v: f64) -> String {
    if !v.is_finite() {
        "—".into()
    } else if v < 10.0 {
        format!("{v:.1}")
    } else {
        fmt_tokens(v)
    }
}

pub fn fmt_pct(v: f64) -> String {
    if v.is_finite() { format!("{v:.0}%") } else { "—".into() }
}

/// Value text for a chart kind in its natural unit.
pub fn fmt_kind(kind: ChartKind, v: f64) -> String {
    if !v.is_finite() {
        return "—".into();
    }
    match kind {
        ChartKind::Gpu | ChartKind::Mem | ChartKind::Cpu | ChartKind::Kv => format!("{v:.0}%"),
        ChartKind::Net | ChartKind::Disk => fmt_bps(v),
        ChartKind::Load => format!("{v:.2}"),
        ChartKind::Power => format!("{v:.1}W"),
        ChartKind::Temp => format!("{v:.0}°C"),
        ChartKind::Decode => format!("{} tok/s", fmt_tokens(v)),
    }
}

pub fn bold(c: Color) -> Style {
    Style::new().fg(c).add_modifier(Modifier::BOLD)
}

pub fn dim() -> Style {
    Style::new().fg(theme().dim)
}

pub fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
    }
}

pub fn pad(s: &str, n: usize) -> String {
    let t = truncate(s, n);
    let w = t.chars().count();
    format!("{t}{}", " ".repeat(n.saturating_sub(w)))
}

pub fn pad_left(s: &str, n: usize) -> String {
    let t = truncate(s, n);
    let w = t.chars().count();
    format!("{}{t}", " ".repeat(n.saturating_sub(w)))
}

pub fn truncate_line(line: Line<'static>, width: usize) -> Line<'static> {
    if line.width() <= width {
        return line;
    }
    if width == 0 {
        return Line::from("");
    }
    let style = line.style;
    let mut budget = width - 1; // reserve one column for the ellipsis
    let mut out: Vec<Span> = Vec::with_capacity(line.spans.len());
    for s in line.spans {
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
    Line::from(out).style(style)
}

// ── status / alert helpers ───────────────────────────────────────────
pub fn health_dot(h: Health, t: &Theme) -> Span<'static> {
    match h {
        Health::Ok => Span::styled("●", bold(t.good)),
        Health::Stale(_) => Span::styled("◐", bold(t.warn)),
        Health::Down => Span::styled("✕", bold(t.bad)),
        Health::Waiting => Span::styled("○", Style::new().fg(t.dim)),
    }
}

pub fn health_text(h: Health) -> String {
    match h {
        Health::Ok => "live".into(),
        Health::Stale(a) => format!("{a}s old"),
        Health::Down => "down".into(),
        Health::Waiting => "connecting".into(),
    }
}

pub fn temp_color(t: &Theme, a: &Alerts, c: f64) -> Color {
    if c >= a.temp_crit_c {
        t.bad
    } else if c >= a.temp_warn_c {
        t.warn
    } else {
        t.s1
    }
}

pub fn mem_color(t: &Theme, a: &Alerts, frac: f64) -> Color {
    if frac >= a.mem_crit {
        t.bad
    } else if frac >= a.mem_warn {
        t.warn
    } else {
        t.s1
    }
}

/// Does this node's latest sample breach an alert threshold?
pub fn alerting(d: &Derived, a: &Alerts) -> bool {
    d.temp_c >= a.temp_warn_c || d.mem_used_pct / 100.0 >= a.mem_warn
}

// ── meters ───────────────────────────────────────────────────────────
/// btop-style ■ meter: filled cells blend from `color` toward `hot` as the
/// bar grows; empty cells are a faint track.
pub fn meter(frac: f64, width: usize, color: Color, hot: Color, t: &Theme) -> Vec<Span<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let frac = if frac.is_finite() { frac.clamp(0.0, 1.0) } else { 0.0 };
    let filled = (frac * width as f64).round() as usize;
    let track = lerp(t.bg, t.dim, 0.35);
    (0..width)
        .map(|i| {
            if i < filled {
                let c = lerp(color, hot, i as f64 / (width.max(2) - 1) as f64);
                Span::styled("■", Style::new().fg(c))
            } else {
                Span::styled("■", Style::new().fg(track))
            }
        })
        .collect()
}

const SPARK: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Block sparkline from resampled columns; each glyph is colored by level.
pub fn sparkline(cols: &[Option<f64>], scale: f64, color: Color, hot: Option<Color>, t: &Theme) -> Vec<Span<'static>> {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let track = lerp(t.bg, t.dim, 0.25);
    cols.iter()
        .map(|v| match v {
            None => Span::styled("·", Style::new().fg(track)),
            Some(v) => {
                let f = (v / scale).clamp(0.0, 1.0);
                let i = if *v > 0.0 { ((f * 8.0).round() as usize).clamp(1, 8) } else { 1 };
                let c = hot.map(|h| lerp(color, h, f)).unwrap_or(color);
                Span::styled(SPARK[i].to_string(), Style::new().fg(if *v > 0.0 { c } else { track }))
            }
        })
        .collect()
}

// ── kv columns (vllm-top Kv/kv_lines) ────────────────────────────────
pub struct Kv {
    pub k: String,
    pub v: String,
    pub gloss: String,
    pub style: Option<Style>,
}

impl Kv {
    pub fn plain(k: &str, v: String, gloss: &str) -> Self {
        Self { k: k.into(), v, gloss: gloss.into(), style: None }
    }
    pub fn colored(k: &str, v: String, gloss: &str, c: Color) -> Self {
        Self { k: k.into(), v, gloss: gloss.into(), style: Some(Style::new().fg(c)) }
    }
    pub fn gloss_only(gloss: String) -> Self {
        Self { k: String::new(), v: String::new(), gloss, style: None }
    }
}

pub fn kv_lines(t: &Theme, entries: Vec<Kv>, width: usize) -> Vec<Line<'static>> {
    let kpad = entries.iter().map(|e| e.k.chars().count()).max().unwrap_or(0).max(6);
    let vpad = entries.iter().map(|e| e.v.chars().count()).max().unwrap_or(1);
    entries
        .into_iter()
        .map(|e| {
            if e.k.is_empty() && e.v.is_empty() {
                return truncate_line(Line::from(Span::styled(format!(" {}", e.gloss), Style::new().fg(t.dim))), width);
            }
            let value_style = e.style.unwrap_or(Style::new().fg(t.fg)).add_modifier(Modifier::BOLD);
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

pub fn panel_title(text: &str) -> Line<'static> {
    Line::from(Span::styled(format!(" {text} "), bold(theme().fg)))
}

pub fn panel_sub(text: String) -> Option<Line<'static>> {
    Some(Line::from(Span::styled(format!(" {text} "), dim())))
}

// ── hero strip ───────────────────────────────────────────────────────
/// Label/value cell like vllm-top's engine status: dim label, bright value
/// (optionally followed by a ■ meter sized to the cell), then dim glosses.
pub struct HeroCell {
    pub label: String,
    /// value text, its style, and an optional meter fraction (0–1)
    pub value: Option<(String, Style, Option<f64>)>,
    pub lines: Vec<Line<'static>>,
}

impl HeroCell {
    pub fn new(label: &str, value: String, gloss: String, vstyle: Style) -> Self {
        Self { label: label.into(), value: Some((value, vstyle, None)), lines: vec![Line::from(Span::styled(gloss, dim()))] }
    }

    pub fn metered(label: &str, value: String, frac: f64, color: Color, gloss: String) -> Self {
        Self {
            label: label.into(),
            value: Some((value, bold(color), Some(frac))),
            lines: vec![Line::from(Span::styled(gloss, dim()))],
        }
    }

    pub fn custom(label: &str, lines: Vec<Line<'static>>) -> Self {
        Self { label: label.into(), value: None, lines }
    }
}

/// Render hero cells side by side, sharing the width evenly (each at least
/// `min_w`); cells that don't fit are dropped from the right.
pub fn render_hero(f: &mut Frame, t: &Theme, area: Rect, cells: &[HeroCell], min_w: u16) {
    if cells.is_empty() || area.width == 0 {
        return;
    }
    let fit = ((area.width / min_w.max(1)) as usize).clamp(1, cells.len());
    let w = area.width / fit as u16;
    for (ci, cell) in cells.iter().take(fit).enumerate() {
        let x = area.x + ci as u16 * w;
        let cw = if ci + 1 == fit { area.x + area.width - x } else { w };
        let cell_area = Rect { x, width: cw, ..area };
        let inner_w = cw.saturating_sub(1) as usize;
        let mut ls = vec![Line::from(Span::styled(format!(" {}", cell.label.to_lowercase()), Style::new().fg(t.dim)))];
        if let Some((v, style, frac)) = &cell.value {
            let mut spans = vec![Span::styled(format!(" {v} "), *style)];
            if let Some(frac) = frac {
                let mw = inner_w.saturating_sub(v.chars().count() + 2).min(12);
                if mw >= 3 {
                    spans.extend(meter(*frac, mw, style.fg.unwrap_or(t.fg), t.bad, t));
                }
            }
            ls.push(truncate_line(Line::from(spans), inner_w));
        }
        for l in &cell.lines {
            let mut l = l.clone();
            l.spans.insert(0, Span::raw(" "));
            ls.push(truncate_line(l, inner_w));
        }
        ls.truncate(area.height as usize);
        f.render_widget(Paragraph::new(ls), cell_area);
    }
}

// ── series and graph panels ──────────────────────────────────────────
pub fn dt_of(h: &NodeHistory) -> Vec<f64> {
    h.samples
        .windows(2)
        .map(|w| w[1].rate_time_us().saturating_sub(w[0].rate_time_us()) as f64 / 1e6)
        .collect()
}

pub fn series(h: &NodeHistory, sel: impl Fn(&Derived) -> f64) -> Vec<Option<f64>> {
    h.derived.iter().map(|d| Some(sel(d)).filter(|v| v.is_finite())).collect()
}

/// The value series for a chart kind, with gaps where a value is missing.
pub fn chart_series(h: &NodeHistory, kind: ChartKind) -> Vec<Option<f64>> {
    match kind {
        ChartKind::Gpu => series(h, |d| d.gpu_pct),
        ChartKind::Mem => series(h, |d| d.mem_used_pct),
        ChartKind::Cpu => series(h, |d| d.cpu_pct),
        ChartKind::Net => series(h, |d| d.net_rx_bps),
        ChartKind::Disk => series(h, |d| d.disk_read_bps),
        ChartKind::Load => series(h, |d| d.load1),
        ChartKind::Power => series(h, |d| d.power_w),
        ChartKind::Temp => series(h, |d| d.temp_c),
        ChartKind::Decode => series(h, |d| d.generation_smooth),
        ChartKind::Kv => series(h, |d| d.kv_pct),
    }
}

/// Secondary series drawn as a line over the fill (tx over rx, etc.).
pub fn chart_overlay(h: &NodeHistory, kind: ChartKind) -> Option<(Vec<Option<f64>>, &'static str)> {
    match kind {
        ChartKind::Net => Some((series(h, |d| d.net_tx_bps), "↑ tx")),
        ChartKind::Disk => Some((series(h, |d| d.disk_write_bps), "write")),
        _ => None,
    }
}

pub fn is_pct(kind: ChartKind) -> bool {
    matches!(kind, ChartKind::Gpu | ChartKind::Mem | ChartKind::Cpu | ChartKind::Kv)
}

pub fn chart_title(kind: ChartKind) -> &'static str {
    match kind {
        ChartKind::Gpu => "GPU",
        ChartKind::Mem => "memory",
        ChartKind::Cpu => "CPU",
        ChartKind::Net => "network",
        ChartKind::Disk => "disk",
        ChartKind::Load => "load avg",
        ChartKind::Power => "power",
        ChartKind::Temp => "temp",
        ChartKind::Decode => "decode",
        ChartKind::Kv => "KV cache",
    }
}

pub fn chart_gloss(kind: ChartKind) -> &'static str {
    match kind {
        ChartKind::Gpu => "compute",
        ChartKind::Mem => "unified pool",
        ChartKind::Cpu => "aggregate",
        ChartKind::Net => "↓ rx fill",
        ChartKind::Disk => "read fill",
        ChartKind::Load => "1m average",
        ChartKind::Power => "GPU board",
        ChartKind::Temp => "GPU package",
        ChartKind::Decode => "tok/s · 5s mean",
        ChartKind::Kv => "vLLM GPU pool",
    }
}

pub fn chart_color(kind: ChartKind, t: &Theme) -> Color {
    match kind {
        ChartKind::Gpu => t.s3,
        ChartKind::Mem => t.warn,
        ChartKind::Cpu => t.s1,
        // net must not share the CPU green: they end up adjacent
        ChartKind::Net => t.accent,
        ChartKind::Disk => t.s2,
        ChartKind::Load => t.bad,
        ChartKind::Power => t.warn,
        ChartKind::Temp => t.bad,
        ChartKind::Decode => t.s1,
        ChartKind::Kv => t.s2,
    }
}

/// Color the gradient heads toward: something hotter than the base.
pub fn hot_for(color: Color, t: &Theme) -> Color {
    if color == t.bad || color == t.warn { t.bad } else { lerp(color, t.warn, 0.85) }
}

/// Index of the first sample worth drawing for `window_secs`: one sample
/// before the window edge so the leftmost segment still connects.
pub fn window_start(dt: &[f64], window_secs: f64) -> usize {
    let mut acc = 0.0;
    for (i, d) in dt.iter().enumerate().rev() {
        acc += d;
        if acc >= window_secs {
            return i;
        }
    }
    0
}

pub struct GraphSpec<'a> {
    pub title: &'a str,
    pub gloss: &'a str,
    pub vals: &'a [Option<f64>],
    pub dt: &'a [f64],
    pub color: Color,
    pub unit: &'a str,
    pub fmt: &'a dyn Fn(f64) -> String,
    pub sub: Option<Line<'static>>,
    pub overlay: Option<(&'a [Option<f64>], Color, &'a str)>,
    pub gradient: bool,
    pub pct: bool,
    pub border: Option<Color>,
}

/// Boxed braille graph: title carries now/peak over the visible window.
pub fn draw_graph(f: &mut Frame, t: &Theme, area: Rect, window_secs: f64, g: GraphSpec<'_>) {
    let start = window_start(g.dt, window_secs);
    let vals = &g.vals[start.min(g.vals.len())..];
    let dt = &g.dt[start.min(g.dt.len())..];
    let ov = g.overlay.map(|(o, c, l)| (&o[start.min(o.len())..], c, l));
    let peak = vals.iter().filter_map(|v| *v).fold(0.0_f64, f64::max);
    let ov_peak = ov.map(|(o, _, _)| o.iter().filter_map(|v| *v).fold(0.0_f64, f64::max)).unwrap_or(0.0);
    // percent graphs are pinned to the 0–100 scale so a 45% reading fills
    // 45% of the panel — auto-scaling would make idle look pegged
    let scale = if g.pct { 100.0 } else { peak.max(ov_peak) };
    let (mut ref_lines, _) = grid_for_scale(scale);
    // short panels: keep roughly one ruling per three rows, always the top
    let max_lines = (area.height.saturating_sub(2) as usize / 3).max(1);
    if ref_lines.len() > max_lines {
        let k = ref_lines.len().div_ceil(max_lines);
        let top = ref_lines.len() - 1;
        ref_lines = ref_lines.iter().enumerate().filter(|(i, _)| (top - i) % k == 0).map(|(_, v)| *v).collect();
    }
    let now = vals.last().copied().flatten().unwrap_or(f64::NAN);
    let mut title = vec![
        Span::styled(format!(" {} ", g.title), bold(t.fg)),
        Span::styled(format!("{} ", g.gloss), Style::new().fg(t.dim)),
        Span::styled(format!("{} ", (g.fmt)(now)), bold(g.color)),
        Span::styled(format!("peak {} ", (g.fmt)(peak)), Style::new().fg(t.dim)),
    ];
    if let Some((o, c, label)) = ov {
        let onow = o.last().copied().flatten().unwrap_or(f64::NAN);
        title.push(Span::styled(format!("│ {label} "), Style::new().fg(t.dim)));
        title.push(Span::styled(format!("{} ", (g.fmt)(onow)), bold(c)));
    }
    let title = truncate_line(Line::from(title), area.width.saturating_sub(2) as usize);
    let mut block = block_titled(title, g.sub, t);
    if let Some(b) = g.border {
        block = block.border_style(Style::new().fg(b));
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    let hot = g.gradient.then(|| hot_for(g.color, t));
    mini_line_graph(
        f,
        inner,
        vals,
        dt,
        window_secs,
        g.color,
        PlotDress { unit: g.unit, ref_lines: &ref_lines, overlay: ov.map(|(o, c, _)| (o, c)), dim: t.dim, hot },
    );
}

/// Chart panel for a kind from a node's history, with the usual dressing.
pub fn draw_kind(
    f: &mut Frame,
    t: &Theme,
    area: Rect,
    h: &NodeHistory,
    kind: ChartKind,
    window_secs: f64,
    gradient: bool,
    sub: Option<Line<'static>>,
) {
    if matches!(kind, ChartKind::Decode | ChartKind::Kv) && h.last().is_some_and(|(s, _)| s.vllm.is_none()) {
        let block = block_titled(
            Line::from(vec![Span::styled(format!(" {} ", chart_title(kind)), bold(t.fg))]),
            None,
            t,
        );
        let inner = block.inner(area);
        f.render_widget(block, area);
        super::centered_msg(f, inner, "no vLLM endpoint");
        return;
    }
    let vals = chart_series(h, kind);
    let dt = dt_of(h);
    let overlay = chart_overlay(h, kind);
    let ocolor = match kind {
        ChartKind::Net => t.s2,
        ChartKind::Disk => t.warn,
        _ => t.s2,
    };
    let unit = match kind {
        ChartKind::Gpu | ChartKind::Mem | ChartKind::Cpu | ChartKind::Kv => "%",
        ChartKind::Net | ChartKind::Disk => "B/s",
        ChartKind::Power => "W",
        ChartKind::Temp => "°",
        ChartKind::Decode => "",
        ChartKind::Load => "",
    };
    let fmt = move |v: f64| fmt_kind(kind, v);
    draw_graph(
        f,
        t,
        area,
        window_secs,
        GraphSpec {
            title: chart_title(kind),
            gloss: chart_gloss(kind),
            vals: &vals,
            dt: &dt,
            color: chart_color(kind, t),
            unit,
            fmt: &fmt,
            sub,
            overlay: overlay.as_ref().map(|(o, l)| (o.as_slice(), ocolor, *l)),
            gradient,
            pct: is_pct(kind),
            border: None,
        },
    );
}
