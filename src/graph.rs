//! Graph renderer adapted from vllm-top (GPLv3) — mratsim/vllm-top src/ui.rs
//! `mini_line_graph` / `grid_for_scale`. Braille canvas, interpolated area
//! fill with gaps at missing samples, dashed gridlines calibrated to the
//! observed peak, unit on the lowest gridline label, newest sample pinned
//! to the right edge.
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{
        canvas::{Canvas, Line as CLine},
        Block, BorderType, Borders,
    },
    symbols::Marker,
    Frame,
};
use std::sync::Mutex;


/// Style lock so Canvas closures can borrow theme colors across threads.
pub static THEME_LOCK: Mutex<()> = Mutex::new(());

/// Calibration gridlines for an observed peak, at any magnitude: one line
/// per round step (widened to keep ≤5 lines), the last step above the
/// peak, scale topped 10% higher.
/// The lowest line carries the axis unit.
pub fn grid_for_scale(scale: f64) -> (Vec<f64>, f64) {
    if scale > 0.0 {
        let mut step = 10.0_f64.powf(scale.log10().floor());
        // at most five lines: 84°C gets 20/40/60/80, not nine rulings
        for widen in [2.0, 2.5] {
            if (scale / step).ceil() > 5.0 {
                step *= widen;
            }
        }
        let top_line = (scale / step).ceil() * step;
        let lines: Vec<f64> = (1..=(top_line / step) as i64)
            .map(|m| m as f64 * step)
            .collect();
        (lines, top_line * 1.1)
    } else {
        (Vec::new(), 1.0)
    }
}

pub struct PlotDress<'a> {
    pub unit: &'a str,
    pub ref_lines: &'a [f64],
    /// optional overlay series drawn as a connected line over the fill
    pub overlay: Option<(&'a [Option<f64>], Color)>,
    /// dim color for gridline labels (faded, must not compete with data)
    pub dim: Color,
    /// btop-style vertical gradient: the fill blends from the series color
    /// at the baseline to this color at the top of the scale
    pub hot: Option<Color>,
}

/// Right-aligned x position (in dots) of every sample: the newest sample
/// sits on the right edge and older ones step left by their real interval.
fn x_positions(n: usize, dt: &[f64], window_secs: f64, w_dots: f64) -> Vec<f64> {
    let span: f64 = dt.iter().take(n.saturating_sub(1)).sum();
    let mut out = Vec::with_capacity(n);
    let mut x = 0.0;
    for i in 0..n {
        out.push((w_dots - 1.0) - (span - x) / window_secs * (w_dots - 1.0));
        x += dt.get(i).copied().unwrap_or(1.0);
    }
    out
}

/// Bucket a series into `width` columns covering the last `window_secs`
/// (mean per column, newest on the right). Columns with no samples are None.
pub fn resample(vals: &[Option<f64>], dt: &[f64], window_secs: f64, width: usize) -> Vec<Option<f64>> {
    if width == 0 || vals.is_empty() || window_secs <= 0.0 {
        return vec![None; width];
    }
    let xs = x_positions(vals.len(), dt, window_secs, width as f64 + 1.0);
    let mut sum = vec![0.0; width];
    let mut cnt = vec![0u32; width];
    for (v, x) in vals.iter().zip(xs) {
        if let Some(v) = v {
            if x >= 0.0 {
                let c = (x as usize).min(width - 1);
                sum[c] += v;
                cnt[c] += 1;
            }
        }
    }
    // carry the last value across empty columns between samples so slow
    // polls on wide sparklines read as a continuous line, not dashes
    let mut out: Vec<Option<f64>> = sum.iter().zip(&cnt).map(|(s, c)| (*c > 0).then(|| s / *c as f64)).collect();
    let first = out.iter().position(|v| v.is_some());
    if let Some(first) = first {
        let mut last = out[first];
        for v in out.iter_mut().skip(first) {
            if v.is_some() { last = *v; } else { *v = last; }
        }
    }
    out
}

fn trim_float(v: f64) -> String {
    if (v - v.round()).abs() < 0.05 { format!("{v:.0}") } else { format!("{v:.1}") }
}

/// One series in a comparison graph.
pub struct Series<'a> {
    pub vals: &'a [Option<f64>],
    pub dt: &'a [f64],
    pub color: Color,
}

/// Overlay several series as lines on a shared scale (compare view).
pub fn multi_line_graph(
    f: &mut Frame,
    area: Rect,
    series: &[Series<'_>],
    window_secs: f64,
    ref_lines: &[f64],
    unit: &str,
    dim: Color,
) {
    if window_secs <= 0.0 || area.width == 0 || area.height == 0 {
        return;
    }
    let w_dots = area.width as f64 * 2.0;
    let h_dots = area.height as f64 * 4.0;
    let ymax = ymax_of(ref_lines, window_secs);
    let y_of = |v: f64| (v / ymax * h_dots * 0.96).clamp(0.0, h_dots * 0.96);
    let mut segs: Vec<(f64, f64, f64, f64, Color)> = Vec::new();
    for s in series {
        let xs = x_positions(s.vals.len(), s.dt, window_secs, w_dots);
        let mut prev: Option<(f64, f64)> = None;
        for (v, x) in s.vals.iter().zip(&xs) {
            match v {
                Some(v) => {
                    let p = (*x, y_of(*v));
                    if let Some(q) = prev {
                        if p.0 >= 0.0 {
                            segs.push((q.0.max(0.0), q.1, p.0, p.1, s.color));
                        }
                    }
                    prev = Some(p);
                }
                None => prev = None,
            }
        }
    }
    let grid: Vec<f64> = ref_lines.iter().map(|v| y_of(*v).clamp(1.0, h_dots - 2.0)).collect();
    let labels: Vec<(f64, String)> = ref_lines
        .iter()
        .zip(&grid)
        .enumerate()
        .filter(|(i, _)| *i == 0 || *i + 1 == ref_lines.len())
        .map(|(i, (v, y))| (y + 2.0, if i == 0 { format!("{}{unit}", short_num(*v)) } else { short_num(*v) }))
        .collect();
    let canvas = Canvas::default()
        .marker(Marker::Braille)
        .x_bounds([0.0, w_dots])
        .y_bounds([0.0, h_dots])
        .paint(move |ctx| {
            for ry in &grid {
                let mut x = 0.0;
                while x < w_dots {
                    ctx.draw(&CLine { x1: x, y1: *ry, x2: (x + 1.0).min(w_dots - 1.0), y2: *ry, color: dim });
                    x += 6.0;
                }
            }
            ctx.layer();
            for (x1, y1, x2, y2, color) in &segs {
                ctx.draw(&CLine { x1: *x1, y1: *y1, x2: *x2, y2: *y2, color: *color });
            }
            if area.width >= 16 {
                for (y, text) in &labels {
                    ctx.print(1.0, *y, Span::styled(text.clone(), Style::new().fg(dim)));
                }
            }
        });
    f.render_widget(canvas, area);
}

fn short_num(v: f64) -> String {
    if v >= 1.0e9 {
        format!("{}G", trim_float(v / 1.0e9))
    } else if v >= 1.0e6 {
        format!("{:.0}M", v / 1.0e6)
    } else if v >= 1.0e3 {
        format!("{:.0}k", v / 1.0e3)
    } else if v < 0.1 {
        format!("{v:.2}")
    } else if v < 1.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.0}")
    }
}

/// Draw the fill graph into `area` (already inside the panel borders).
/// `dt` holds each sample's interval length (wall-clock x axis);
/// `vals` may contain None gaps where a poll failed.
pub fn mini_line_graph(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    vals: &[Option<f64>],
    dt: &[f64],
    window_secs: f64,
    color: Color,
    dress: PlotDress<'_>,
) {
    let unit = dress.unit;
    let ref_lines = dress.ref_lines;
    let overlay = dress.overlay;
    let dim = dress.dim;
    let hot = dress.hot;
    if window_secs <= 0.0 || area.width == 0 || area.height == 0 {
        return;
    }
    // all-None (no successful poll yet) or all-zero: render an idle gloss
    // INSTEAD of a fill, but keep the gridlines so % panels keep their
    // 0–100 scale context alongside their neighbors
    let has_data = vals.iter().any(|v| v.map(|v| v > 0.0).unwrap_or(false));
    let is_idle = vals.iter().all(|v| v.is_none()) || (!has_data && vals.iter().all(|v| v.is_some()));
    // single sample with no dt yet: seed a nominal interval so the newest
    // value renders instead of a blank panel for the first second
    let dt: Vec<f64> = if dt.is_empty() && !vals.is_empty() {
        vec![window_secs.min(1.0)]
    } else {
        dt.to_vec()
    };
    let w_dots = (area.width as f64) * 2.0;
    let h_dots = (area.height as f64) * 4.0;
    let span: f64 = dt.iter().sum::<f64>();
    // young session on a long window: say so, or the mostly-empty plot
    // reads as broken
    let collecting = span < window_secs / 4.0 && !is_idle;
    let axis_x = |cum: f64| {
        ((w_dots - 1.0) - (span - cum) / window_secs * (w_dots - 1.0)).clamp(0.0, w_dots - 1.0)
    };
    // solid histogram: interpolate between interval ends; None gaps break the fill
    let mut tops: Vec<(usize, f64, f64)> = Vec::new();
    let mut x = 0.0;
    for (i, v) in vals.iter().enumerate() {
        let cx = axis_x(x);
        if let Some(v) = v {
            let h = ((v / ymax_of(ref_lines, window_secs)) * h_dots * 0.96).min(h_dots * 0.96);
            if !is_idle {
                tops.push((i, cx, h));
            }
        }
        x += dt.get(i).copied().unwrap_or(1.0);
    }
    let mut cols: Vec<(f64, f64)> = Vec::new();
    for w in tops.windows(2) {
        let (i0, x0, h0) = w[0];
        let (i1, x1, h1) = w[1];
        if i1 != i0 + 1 {
            continue;
        }
        let mut cx = x0;
        while cx < x1 {
            let frac = (cx - x0) / (x1 - x0).max(1.0);
            let h = h0 + (h1 - h0) * frac;
            cols.push((cx, h.max(1.0)));
            cx += 1.0;
        }
    }
    // hold the newest (still in progress) value out to the right edge
    if let Some(&(i, start, h)) = tops.last() {
        if i + 1 == vals.len() {
            let h = h.max(1.0);
            for cx in start as i64..=(w_dots as i64 - 1) {
                cols.push((cx as f64, h));
            }
        }
    }
    let mut overlay_tops: Vec<(f64, f64)> = Vec::new();
    let mut overlay_color = color;
    if let Some((series, ocolor)) = overlay {
        overlay_color = ocolor;
        let mut x = 0.0;
        for (i, v) in series.iter().enumerate() {
            if let Some(v) = v {
                let h = ((v / ymax_of(ref_lines, window_secs)) * h_dots * 0.96).min(h_dots * 0.96);
                overlay_tops.push((axis_x(x), h));
            }
            x += dt.get(i).copied().unwrap_or(1.0);
        }
    }
    // dashed gridlines with faded labels; keep lowest label per slot
    let grid: Vec<(f64, f64)> = ref_lines
        .iter()
        .map(|v| (*v, (*v / ymax_of(ref_lines, window_secs) * h_dots * 0.96).clamp(1.0, h_dots - 2.0)))
        .collect();
    let canvas_rows = (h_dots / 4.0) as usize;
    let slot_of = |y: f64| (((h_dots - y) * (canvas_rows.max(2) - 1) as f64) / h_dots) as usize;
    let lowest = grid.first().map(|(v, _)| *v);
    let mut labels: Vec<(f64, String)> = Vec::new();
    let mut used_slot: Option<usize> = None;
    for (v, ry) in &grid {
        let y = ry + 2.0;
        let slot = slot_of(y);
        if used_slot == Some(slot) {
            continue;
        }
        used_slot = Some(slot);
        let n = short_num(*v);
        let text = if Some(*v) == lowest {
            format!("{n}{unit}")
        } else {
            n
        };
        labels.push((y, text));
    }
    // rulings are faint: a tint of the series, never competing with data
    let grid_color = crate::theme::lerp(dim, color, 0.35);
    let gradient: Option<Vec<Color>> = hot.map(|hot| {
        let rows = canvas_rows.max(1);
        (0..rows).map(|k| crate::theme::lerp(color, hot, k as f64 / rows.saturating_sub(1).max(1) as f64)).collect()
    });
    let canvas = Canvas::default()
        .marker(Marker::Braille)
        .x_bounds([0.0, w_dots])
        .y_bounds([0.0, h_dots])
        .paint(move |ctx| {
            for (_, ry) in &grid {
                let mut x = 0.0;
                while x < w_dots {
                    ctx.draw(&CLine {
                        x1: x,
                        y1: *ry,
                        x2: (x + 1.0).min(w_dots - 1.0),
                        y2: *ry,
                        color: grid_color,
                    });
                    x += 6.0;
                }
            }
            // labels overlap the fill on narrow panels — skip below 16 cols
            if area.width >= 16 {
                for (y, text) in &labels {
                    ctx.print(2.0, *y, Span::styled(text.clone(), Style::new().fg(dim)));
                }
            }
            match &gradient {
                None => {
                    for (cx, h) in &cols {
                        ctx.draw(&CLine { x1: *cx, y1: 0.0, x2: *cx, y2: *h, color });
                    }
                }
                // one color per braille cell row (the terminal's color
                // resolution), so segments break on 4-dot boundaries
                Some(g) => {
                    for (cx, h) in &cols {
                        let mut y0 = 0.0;
                        let mut k = 0;
                        while y0 < *h {
                            let y1 = (y0 + 3.0).min(*h);
                            ctx.draw(&CLine { x1: *cx, y1: y0, x2: *cx, y2: y1, color: g[k.min(g.len() - 1)] });
                            y0 += 4.0;
                            k += 1;
                        }
                    }
                }
            }
            // own layer: the line replaces the fill's dots in its cells, so
            // it reads as a distinct trace instead of vanishing into the fill
            ctx.layer();
            for w in overlay_tops.windows(2) {
                let (x0, y0) = w[0];
                let (x1, y1) = w[1];
                ctx.draw(&CLine { x1: x0, y1: y0, x2: x1, y2: y1, color: overlay_color });
            }
        });
    f.render_widget(canvas, area);
    if is_idle {
        let row = Rect { y: area.y + area.height / 2, height: 1, ..area };
        f.render_widget(
            ratatui::widgets::Paragraph::new(ratatui::text::Line::from(
                ratatui::text::Span::styled(" idle ", Style::new().fg(dim)),
            )),
            row,
        );
    } else if collecting {
        let secs = span as u64;
        let label = if secs >= 60 {
            format!(" collecting — {}m of {}", secs / 60, crate::ui::window_label(window_secs))
        } else {
            format!(" collecting — {secs}s of {}", crate::ui::window_label(window_secs))
        };
        let row = Rect { y: area.y + area.height / 2, height: 1, ..area };
        f.render_widget(
            ratatui::widgets::Paragraph::new(ratatui::text::Line::from(
                ratatui::text::Span::styled(label, Style::new().fg(dim)),
            )),
            row,
        );
    }
}

fn ymax_of(ref_lines: &[f64], _window_secs: f64) -> f64 {
    // ymax is passed via ref_lines' companion; recomputed by caller using
    // grid_for_scale — here we recover it as 1.1x the top line
    ref_lines.last().map(|v| v * 1.1).unwrap_or(1.0)
}

/// Rounded panel with an optional dim bottom-subtitle (vllm-top block_titled).
pub fn block_titled<'a>(
    title: Line<'static>,
    subtitle: Option<Line<'static>>,
    t: &crate::theme::Theme,
) -> Block<'a> {
    let mut b = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(t.dim))
        .style(Style::new().bg(t.bg))
        .title(title);
    if let Some(sub) = subtitle {
        b = b.title_bottom(sub);
    }
    b
}
