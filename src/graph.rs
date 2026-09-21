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
/// per round step, the last step above the peak, scale topped 10% higher.
/// The lowest line carries the axis unit.
pub fn grid_for_scale(scale: f64) -> (Vec<f64>, f64) {
    if scale > 0.0 {
        let step = 10.0_f64.powf(scale.log10().floor());
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
        let n = if *v >= 1.0e6 {
            format!("{:.0}M", v / 1.0e6)
        } else if *v >= 1.0e3 {
            format!("{:.0}k", v / 1.0e3)
        } else if *v < 0.1 {
            format!("{v:.2}")
        } else if *v < 1.0 {
            format!("{v:.1}")
        } else {
            format!("{v:.0}")
        };
        let text = if Some(*v) == lowest {
            format!("{n}{unit}")
        } else {
            n
        };
        labels.push((y, text));
    }
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
                        x2: (x + 2.0).min(w_dots - 1.0),
                        y2: *ry,
                        color,
                    });
                    x += 5.0;
                }
            }
            // labels overlap the fill on narrow panels — skip below 16 cols
            if area.width >= 16 {
                for (y, text) in &labels {
                    ctx.print(2.0, *y, Span::styled(text.clone(), Style::new().fg(dim)));
                }
            }
            for (cx, h) in &cols {
                ctx.draw(&CLine { x1: *cx, y1: 0.0, x2: *cx, y2: *h, color });
            }
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
