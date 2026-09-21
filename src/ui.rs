//! TUI rendering. Three pages: cluster overview, per-node detail, vLLM.
use crate::history::Cluster;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line, Span},
    widgets::{Axis, Block, Borders, Chart, Dataset, Gauge, Paragraph, Sparkline},
    Frame,
};

fn pct_color(v: f64) -> Color {
    if v >= 90.0 {
        Color::Red
    } else if v >= 70.0 {
        Color::Yellow
    } else {
        Color::Green
    }
}

fn mem_color(used_frac: f64) -> Color {
    if used_frac >= 0.9 {
        Color::Red
    } else if used_frac >= 0.75 {
        Color::Yellow
    } else {
        Color::Green
    }
}

fn fmt_bps(bps: f64) -> String {
    const U: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if bps <= 0.0 {
        return "0 B/s".into();
    }
    let mut v = bps;
    let mut u = 0;
    while v >= 1024.0 && u < U.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    format!("{v:.1} {}/s", U[u])
}

fn fmt_mb(mb: f64) -> String {
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else {
        format!("{mb:.0} MB")
    }
}

pub fn draw(f: &mut Frame, cluster: &Cluster, order: &[String], page: Page, focused: usize, paused: bool, interval: f64, err: &Option<String>) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(f.area());
    // status bar
    let status = Span::styled(
        format!(
            " sparktop │ {} nodes │ {:.1}s poll{}{}",
            order.len(),
            interval,
            if paused { " │ PAUSED" } else { "" },
            err.as_ref().map(|e| format!(" │ ERR: {e}")).unwrap_or_default()
        ),
        Style::default().fg(if err.is_some() { Color::Red } else { Color::DarkGray }),
    );
    f.render_widget(Paragraph::new(Line::from(vec![status, Span::raw("  │  1:cluster 2:node 3:vllm  │  tab:switch node  q:quit")])), chunks[0]);

    match page {
        Page::Cluster => draw_cluster(f, cluster, order, chunks[1]),
        Page::Node => draw_node(f, cluster, order.get(focused), chunks[1]),
        Page::Vllm => draw_vllm(f, cluster, order.get(focused), chunks[1]),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Page {
    Cluster,
    Node,
    Vllm,
}

fn draw_cluster(f: &mut Frame, cluster: &Cluster, order: &[String], area: Rect) {
    let rows = order.len() as u16;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(std::iter::repeat(Constraint::Ratio(1, rows.max(1) as u32)).take(rows as usize))
        .split(area);
    for (i, name) in order.iter().enumerate() {
        let h = match cluster.get(name) {
            Some(h) => h,
            None => continue,
        };
        let (s, d) = match h.last() {
            Some(x) => x,
            None => continue,
        };
        let gpu = s.gpus.first();
        let gpu_util = gpu.map(|g| g.util_pct).unwrap_or(0.0);
        let gpu_mem = gpu.map(|g| (g.mem_used_mb, g.mem_total_mb)).unwrap_or((0.0, 0.0));
        let gpu_txt = if gpu_mem.1 > 0.0 {
            format!("GPU {gpu_util:4.1}% {}/{}", fmt_mb(gpu_mem.0), fmt_mb(gpu_mem.1))
        } else {
            format!("GPU {gpu_util:4.1}%")
        };
        let mem_frac = if s.mem.total_kb > 0 { s.mem.used_kb() as f64 / s.mem.total_kb as f64 } else { 0.0 };

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(14),
                Constraint::Length(20),
                Constraint::Length(26),
                Constraint::Length(24),
                Constraint::Min(20),
                Constraint::Min(20),
                Constraint::Length(10),
            ])
            .split(chunks[i]);

        f.render_widget(
            Paragraph::new(Span::styled(format!(" {name} "), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
            cols[0],
        );
        // gpu gauge
        let g = Gauge::default().label("").gauge_style(Style::default().fg(pct_color(gpu_util))).ratio((gpu_util / 100.0).clamp(0.0, 1.0));
        f.render_widget(g, cols[1]);
        f.render_widget(
            Paragraph::new(gpu_txt),
            cols[2],
        );
        // mem gauge
        let mg = Gauge::default().label("").gauge_style(Style::default().fg(mem_color(mem_frac))).ratio(mem_frac.clamp(0.0, 1.0));
        f.render_widget(mg, cols[3]);
        f.render_widget(
            Paragraph::new(format!("MEM {:.0}%  load {:.1}", mem_frac * 100.0, s.cpu.load1)),
            cols[4],
        );
        // net sparkline (rx)
        let net_data: Vec<u64> = h
            .derived
            .iter()
            .rev()
            .take(cols[5].width as usize)
            .map(|d| d.net_rx_bps as u64)
            .collect();
        f.render_widget(
            Sparkline::default()
                .data(&net_data)
                .style(Style::default().fg(Color::Blue)),
            cols[5],
        );
        f.render_widget(Paragraph::new(format!("↓{} ↑{}", fmt_bps(d.net_rx_bps), fmt_bps(d.net_tx_bps))), cols[6]);
    }
}

fn draw_node(f: &mut Frame, cluster: &Cluster, name: Option<&String>, area: Rect) {
    let h = match name.and_then(|n| cluster.get(n)) {
        Some(h) => h,
        None => return,
    };
    let (s, d) = match h.last() {
        Some(x) => x,
        None => return,
    };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // gpu gauge row
            Constraint::Length(3),  // memory
            Constraint::Length(7),  // cpu cores
            Constraint::Length(5),  // net
            Constraint::Length(4),  // disk
            Constraint::Min(0),     // procs
        ])
        .split(area);

    // GPU row
    if let Some(g) = s.gpus.first() {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(rows[0]);
        let gg = Gauge::default()
            .label(format!(" {}  {:.0}%  {}W  {:.0}°C", g.name, g.util_pct, g.power_w, g.temp_c))
            .gauge_style(Style::default().fg(pct_color(g.util_pct)))
            .ratio((g.util_pct / 100.0).clamp(0.0, 1.0));
        f.render_widget(gg, cols[0]);
        if g.mem_total_mb > 0.0 {
            let mem_frac = g.mem_used_mb / g.mem_total_mb;
            let mg = Gauge::default()
                .label(format!("GPU MEM {}/{}  SM {}MHz", fmt_mb(g.mem_used_mb), fmt_mb(g.mem_total_mb), g.sm_clock_mhz))
                .gauge_style(Style::default().fg(mem_color(mem_frac)))
                .ratio(mem_frac.clamp(0.0, 1.0));
            f.render_widget(mg, cols[1]);
        } else {
            // Unified memory: GPU VRAM accounting is N/A on GB10; system-wide
            // memory is the same pool and is shown in the UNIFIED MEM gauge.
            let mg = Gauge::default()
                .label(format!("SM {}MHz  (unified memory — see system MEM)", g.sm_clock_mhz))
                .gauge_style(Style::default().fg(Color::DarkGray))
                .ratio(0.0);
            f.render_widget(mg, cols[1]);
        }
    }

    // System memory
    let mem_frac = if s.mem.total_kb > 0 { s.mem.used_kb() as f64 / s.mem.total_kb as f64 } else { 0.0 };
    let mm = Gauge::default()
        .label(format!(
            "UNIFIED MEM {}/{}  cached {}",
            fmt_mb(s.mem.used_kb() as f64 / 1024.0),
            fmt_mb(s.mem.total_kb as f64 / 1024.0),
            fmt_mb(s.mem.cached_kb as f64 / 1024.0)
        ))
        .gauge_style(Style::default().fg(mem_color(mem_frac)))
        .ratio(mem_frac.clamp(0.0, 1.0));
    f.render_widget(mm, rows[1]);

    // CPU cores as sparkline grid — one sparkline per row of cores
    let core_count = s.cpu.cores.len().max(1);
    let per_row = ((core_count + 5) / 6).max(1);
    let core_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(std::iter::repeat(Constraint::Ratio(1, per_row as u32)).take(per_row))
        .split(rows[2]);
    let block = Block::default().borders(Borders::TOP).title(Span::styled(
        format!(" CPU {} cores  {:.0}%  load {:.1}/{:.1}/{:.1}  {} running/{} total", core_count, d.cpu_pct, s.cpu.load1, s.cpu.load5, s.cpu.load15, s.cpu.procs_running, s.cpu.procs_total),
        Style::default().fg(Color::Cyan),
    ));
    f.render_widget(block, rows[2]);
    for r in 0..per_row.min(core_chunks.len()) {
        let start = r * per_row;
        let end = (start + per_row).min(core_count);
        let data: Vec<u64> = d.per_core_pct[start..end].iter().map(|p| *p as u64).collect();
        f.render_widget(Sparkline::default().data(&data).style(Style::default().fg(Color::Green)), core_chunks[r]);
    }

    // Net
    let nb = Block::default().borders(Borders::TOP).title(Span::styled(
        format!(" NET ↓{} ↑{}  (per iface below)", fmt_bps(d.net_rx_bps), fmt_bps(d.net_tx_bps)),
        Style::default().fg(Color::Blue),
    ));
    f.render_widget(nb, rows[3]);
    let mut ifaces: Vec<_> = s.net.iter().filter(|(n, _)| *n != "lo").collect();
    ifaces.sort_by_key(|(n, _)| (*n).clone());
    let net_txt: Vec<Line> = ifaces
        .iter()
        .take(2)
        .map(|(n, dev)| {
            Line::from(format!("  {n}: rx {} tx {}", fmt_mb(dev.rx_bytes as f64 / 1024.0 / 1024.0), fmt_mb(dev.tx_bytes as f64 / 1024.0 / 1024.0)))
        })
        .collect();
    let inner = Rect { x: rows[3].x + 1, y: rows[3].y + 1, width: rows[3].width.saturating_sub(1), height: rows[3].height.saturating_sub(1) };
    f.render_widget(Paragraph::new(net_txt), inner);

    // Disk
    let db = Block::default().borders(Borders::TOP).title(Span::styled(
        format!(" DISK read {} write {}", fmt_bps(d.disk_read_bps), fmt_bps(d.disk_write_bps)),
        Style::default().fg(Color::Magenta),
    ));
    f.render_widget(db, rows[4]);

    // GPU procs
    let proc_lines: Vec<Line> = s
        .gpus
        .first()
        .map(|g| {
            g.pids
                .iter()
                .map(|p| Line::from(format!("  pid {}  {}  {} MB", p.pid, p.name, p.mem_mb)))
                .collect()
        })
        .unwrap_or_default();
    f.render_widget(
        Paragraph::new(proc_lines).block(Block::default().borders(Borders::TOP).title(" GPU PROCESSES")),
        rows[5],
    );
}

fn series(h: &crate::history::NodeHistory, sel: impl Fn(&crate::history::Derived) -> f64, width: usize) -> Vec<(f64, f64)> {
    let n = h.derived.len();
    let start = n.saturating_sub(width);
    (start..n)
        .map(|i| ((i - start) as f64, sel(&h.derived[i])))
        .collect()
}

fn draw_vllm(f: &mut Frame, cluster: &Cluster, name: Option<&String>, area: Rect) {
    let h = match name.and_then(|n| cluster.get(n)) {
        Some(h) => h,
        None => {
            f.render_widget(Paragraph::new("no node focused"), area);
            return;
        }
    };
    let (s, d) = match h.last() {
        Some(x) => x,
        None => return,
    };
    let v = match &s.vllm {
        Some(v) => v,
        None => {
            f.render_widget(
                Paragraph::new(format!("No vLLM metrics for {}.\nSet vllm_url in sparktop.toml for this node; the server must expose /metrics (Prometheus).", name.unwrap_or(&String::new()))),
                area,
            );
            return;
        }
    };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // headline gauges
            Constraint::Min(6),     // tok/s chart
            Constraint::Length(4),  // TTFT chart
            Constraint::Length(4),  // TPOT chart
            Constraint::Min(0),     // kv cache / queue
        ])
        .split(area);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(1, 4); 4])
        .split(rows[0]);
    let gauge = |f: &mut Frame, a: Rect, label: String, frac: f64, color: Color| {
        let g = Gauge::default().label(label).gauge_style(Style::default().fg(color)).ratio(frac.clamp(0.0, 1.0));
        f.render_widget(g, a);
    };
    gauge(f, cols[0], format!("PROMPT {:>7.0} tok/s", d.prompt_tps), (d.prompt_tps / 5000.0).min(1.0), Color::Cyan);
    gauge(f, cols[1], format!("DECODE {:>7.0} tok/s", d.generation_tps), (d.generation_tps / 5000.0).min(1.0), Color::Green);
    gauge(f, cols[2], format!("RUN {:>3}  QUEUE {:>3}", v.running, v.waiting), (v.running as f64 / 64.0).min(1.0), Color::Yellow);
    gauge(f, cols[3], format!("KV {:.0}%  CPU-KV {:.0}%", v.gpu_cache_usage * 100.0, v.cpu_cache_usage * 100.0), v.gpu_cache_usage, mem_color(v.gpu_cache_usage));

    // tok/s chart
    let width = rows[1].width as usize;
    let pre = series(h, |dd| dd.prompt_tps, width);
    let dec = series(h, |dd| dd.generation_tps, width);
    let ymax = pre.iter().chain(dec.iter()).map(|(_, v)| *v).fold(1.0f64, f64::max);
    f.render_widget(
        Chart::new(vec![
            Dataset::default().name("prefill").marker(symbols::Marker::Braille).style(Style::default().fg(Color::Cyan)).data(&pre),
            Dataset::default().name("decode").marker(symbols::Marker::Braille).style(Style::default().fg(Color::Green)).data(&dec),
        ])
        .block(Block::default().borders(Borders::TOP).title(" TOKENS/S (60s window)"))
        .x_axis(Axis::default().bounds([0.0, width as f64]))
        .y_axis(Axis::default().bounds([0.0, ymax * 1.1])),
        rows[1],
    );

    // latency charts
    let lat_chart = |f: &mut Frame, a: Rect, title: String, sel: Box<dyn Fn(&crate::history::Derived) -> Option<f64>>| {
        let data: Vec<(f64, f64)> = {
            let w = a.width as usize;
            let n = h.derived.len();
            let start = n.saturating_sub(w);
            (start..n)
                .filter_map(|i| sel(&h.derived[i]).map(|v| ((i - start) as f64, v * 1000.0)))
                .collect()
        };
        let ymax = data.iter().map(|(_, v)| *v).fold(1.0f64, f64::max);
        f.render_widget(
            Chart::new(vec![Dataset::default().marker(symbols::Marker::Braille).style(Style::default().fg(Color::Yellow)).data(&data)])
                .block(Block::default().borders(Borders::TOP).title(title))
                .x_axis(Axis::default().bounds([0.0, a.width as f64]))
                .y_axis(Axis::default().bounds([0.0, ymax * 1.1])),
            a,
        );
    };
    lat_chart(f, rows[2], format!(" TTFT ms  p50 {:.1}  p95 {:.1}", d.ttft_p50.map(|v| v * 1000.0).unwrap_or(0.0), d.ttft_p95.map(|v| v * 1000.0).unwrap_or(0.0)), Box::new(|dd: &crate::history::Derived| dd.ttft_p50));
    lat_chart(f, rows[3], format!(" TPOT ms  p50 {:.1}  p95 {:.1}", d.tpot_p50.map(|v| v * 1000.0).unwrap_or(0.0), d.tpot_p95.map(|v| v * 1000.0).unwrap_or(0.0)), Box::new(|dd: &crate::history::Derived| dd.tpot_p50));

    f.render_widget(
        Paragraph::new(format!("  cache gpu {:.0}%  cpu {:.0}%  uptime {}", v.gpu_cache_usage * 100.0, v.cpu_cache_usage * 100.0, fmt_uptime(s.uptime_s))),
        rows[4],
    );
}

fn fmt_uptime(s: u64) -> String {
    let d = s / 86400;
    let h = (s % 86400) / 3600;
    let m = (s % 3600) / 60;
    if d > 0 { format!("{d}d{h}h{m}m") } else { format!("{h}h{m}m") }
}
