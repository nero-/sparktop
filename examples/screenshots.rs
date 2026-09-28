//! Render README screenshots from a simulated cluster: the real UI code
//! drawn into a test backend, dumped as JSON cell grids that
//! `scripts/screenshots.py` rasterizes to PNG/GIF.
//!
//!   cargo run --release --example screenshots          # -> target/screenshots
//!   python3 scripts/screenshots.py target/screenshots docs/img
//!
//! The workload is synthetic (a vLLM concurrency sweep 1→16 on a two-node
//! tensor-parallel pair plus a second serving node) and deterministic, so
//! screenshots only change when the UI does.
use ratatui::{backend::TestBackend, style::Color, Terminal};
use serde_json::json;
use sparktop::app::{ChartKind, ClusterView, NodeInfo, Overlay, Page, UiState};
use sparktop::config::Alerts;
use sparktop::history::{Cluster, NodeHistory};
use sparktop::parse::parse_collection;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

const W: u16 = 150;
const H: u16 = 44;
const DT: f64 = 0.5;

/// Deterministic noise in [-1, 1].
fn noise(seed: u64, t: f64) -> f64 {
    let a = (t * 1.7 + seed as f64 * 12.9898).sin() * 0.5;
    let b = (t * 4.3 + seed as f64 * 78.233).sin() * 0.3;
    let c = ((t * 13.1 + seed as f64 * 3.1).sin() * 43758.5453).fract() * 0.4;
    (a + b + c).clamp(-1.0, 1.0)
}

/// Benchmark phase at time t: (concurrency, active) — 55s per level with
/// a 7s gap between levels, then idle.
fn phase(t: f64) -> (f64, bool) {
    let levels = [1.0, 2.0, 4.0, 8.0, 16.0];
    let i = (t / 62.0) as usize;
    if i >= levels.len() {
        return (0.0, false);
    }
    (levels[i], t % 62.0 > 7.0)
}

struct Sim {
    name: &'static str,
    host: &'static str,
    group: &'static str,
    vllm: bool,
    /// share of the sweep this node participates in (0 = idle box)
    role: f64,
    mem_frac: f64,
    // integrated counters
    cpu_busy: Vec<f64>,
    cpu_idle: Vec<f64>,
    rx: f64,
    tx: f64,
    rd: f64,
    wr: f64,
    prompt: f64,
    generated: f64,
    reqs: f64,
    pq: f64,
    ph: f64,
    ttft: [f64; 7],
    itl: [f64; 6],
    temp: f64,
    seed: u64,
}

impl Sim {
    fn new(name: &'static str, host: &'static str, group: &'static str, vllm: bool, role: f64, mem: f64, seed: u64) -> Self {
        Self {
            name, host, group, vllm, role, mem_frac: mem,
            cpu_busy: vec![0.0; 20], cpu_idle: vec![0.0; 20],
            rx: 8.1e11 * (seed + 1) as f64, tx: 6.4e11 * (seed + 1) as f64,
            rd: 3.2e11, wr: 9.0e10, prompt: 0.0, generated: 0.0, reqs: 0.0, pq: 0.0, ph: 0.0,
            ttft: [0.0; 7], itl: [0.0; 6], temp: 44.0 + seed as f64, seed,
        }
    }

    fn collect(&mut self, t: f64) -> String {
        let (conc, active) = phase(t);
        let load = if active { self.role * (conc / 16.0).powf(0.45) } else { 0.0 };
        let busy = active && self.role > 0.0;
        let skew = self.seed as f64 * 2.5;
        let gpu = if busy { (60.0 + 33.0 * load - skew + 4.0 * noise(self.seed, t)).clamp(0.0, 100.0) } else { (1.5 + noise(self.seed + 9, t)).max(0.0) };
        let power = if busy { 38.0 + 55.0 * load - 2.0 * skew + 3.0 * noise(self.seed + 3, t) } else { 14.0 + 1.5 * noise(self.seed + 4, t) };
        self.temp += (38.0 + power * 0.36 - self.temp) * 0.02;
        // gpt-oss-120b on the gx10 decodes faster than the 235B on the spark pair
        let speed = if self.group == "gx10" { 1.45 } else { 1.0 };
        let decode = if busy && self.vllm { (48.0 * speed * conc.powf(0.78)) * (1.0 + 0.05 * noise(self.seed + 5, t)) } else { 0.0 };
        let prefill = if busy && self.vllm {
            let spike = if t % 62.0 < 12.0 { 3.5 } else { 1.0 };
            decode * 5.5 * spike * (1.0 + 0.25 * noise(self.seed + 6, t)).max(0.2)
        } else {
            0.0
        };
        // tensor-parallel pair: activations cross the ConnectX link every step
        let tp = if busy && self.group == "spark" { (1.6e8 + 9.0e8 * load) * (1.0 - 0.06 * self.seed as f64) } else { 0.0 };
        self.rx += (tp + 2.0e4 * (1.0 + noise(self.seed + 7, t))) * DT;
        self.tx += (tp * 0.97 + 1.5e4 * (1.0 + noise(self.seed + 8, t))) * DT;
        self.rd += (if t < 4.0 { 1.8e9 } else { 2.0e4 }) * DT;
        self.wr += (6.0e4 + 4.0e4 * noise(self.seed + 2, t).abs()) * DT;
        for c in 0..20 {
            let base = if busy { 6.0 + 10.0 * load } else { 1.5 };
            let hot = if busy && c == (self.seed as usize * 3 + 1) % 20 { 88.0 } else { 0.0 };
            let pct = (base + hot + 5.0 * noise(self.seed * 31 + c as u64, t)).clamp(0.5, 100.0);
            self.cpu_busy[c] += pct * DT;
            self.cpu_idle[c] += (100.0 - pct) * DT;
        }
        let (busy_all, idle_all): (f64, f64) = (self.cpu_busy.iter().sum(), self.cpu_idle.iter().sum());
        let mut s = format!(
            "===HOSTNAME===\n{}\n===UPTIME_S===\n{}.42\n 1 users, load average: {:.2}, {:.2}, {:.2}\n===CPU===\ncpu {:.0} 0 0 {:.0} 0 0 0 0\n",
            self.name, 412_000 + self.seed * 86_400 + t as u64,
            if busy { 1.8 + load * 2.0 } else { 0.2 }, if busy { 1.5 } else { 0.3 }, 0.9,
            busy_all, idle_all
        );
        for c in 0..20 {
            s += &format!("cpu{c} {:.0} 0 0 {:.0} 0 0 0 0\n", self.cpu_busy[c], self.cpu_idle[c]);
        }
        s += &format!("procs_running {}\nprocs_blocked 0\n===CPUFREQ===\n", if busy { 4 } else { 1 });
        for c in 0..20 {
            s += &format!("{}\n", if c < 10 { if busy { 3900 } else { 2100 } } else if busy { 2800 } else { 1400 });
        }
        let total = 124_590_000u64;
        let avail = (total as f64 * (1.0 - self.mem_frac - 0.004 * noise(self.seed + 11, t))) as u64;
        s += &format!("===MEM===\nMemTotal: {total} kB\nMemAvailable: {avail} kB\nBuffers: 212000 kB\nCached: 6300000 kB\n");
        s += &format!("===GPU===\n0, NVIDIA GB10, {gpu:.0}, {power:.2}, {:.0}, {}\n", self.temp, if busy { 2418 } else { 870 });
        s += "===GPUPROC===\n";
        if self.role > 0.0 {
            s += &format!("{}, VLLM::EngineCore, {}\n", 3101 + self.seed * 17, (self.mem_frac * 118_000.0) as u64 - 4000);
            s += &format!("{}, /usr/bin/python3, 1180\n", 2987 + self.seed * 17);
        }
        s += "===NET===\nInter-|\n face |\n";
        s += "    lo: 1 1 0 0 0 0 0 0 1 1 0 0 0 0 0 0\n";
        s += &format!("  enp1s0f0np0: {:.0} 9 0 0 0 0 0 0 {:.0} 9 0 0 0 0 0 0\n", self.rx, self.tx);
        s += &format!("  enP7s7: {:.0} 9 0 0 0 0 0 0 {:.0} 9 0 0 0 0 0 0\n", 4.1e9 + t * 3.0e4, 1.2e9 + t * 2.2e4);
        s += "  wlP9s9: 77120 1 0 0 0 0 0 0 5120 1 0 0 0 0 0 0\n";
        s += &format!("===DISK===\n 259 0 nvme0n1 1 0 {:.0} 0 1 0 {:.0} 0 0 0 0\n", self.rd / 512.0, self.wr / 512.0);
        if self.vllm {
            self.generated += decode * DT;
            self.prompt += prefill * DT;
            if busy {
                let reqs = decode * DT / 900.0;
                self.reqs += reqs;
                let q = prefill * DT;
                self.pq += q;
                self.ph += q * (0.42 + 0.06 * noise(self.seed + 12, t));
                // TTFT shifts right with concurrency; ITL grows slowly
                let shift = (conc.log2() / 4.0).clamp(0.0, 1.0);
                let ttft_w = [0.25 - 0.2 * shift, 0.35 - 0.2 * shift, 0.22, 0.1 + 0.2 * shift, 0.05 + 0.15 * shift, 0.03 + 0.05 * shift, 0.0];
                let n = 40.0 * reqs.max(0.02);
                let mut acc = 0.0;
                for (i, w) in ttft_w.iter().enumerate() {
                    acc += w * n;
                    self.ttft[i] += acc;
                }
                let itl_w = [0.1 - 0.08 * shift, 0.55 - 0.3 * shift, 0.25 + 0.25 * shift, 0.07 + 0.08 * shift, 0.03 + 0.05 * shift];
                let n = decode * DT;
                let mut acc = 0.0;
                for (i, w) in itl_w.iter().enumerate() {
                    acc += w * n;
                    self.itl[i] += acc;
                }
                self.itl[5] += n;
            }
            let model = if self.group == "spark" { "nvidia/Qwen3-235B-A22B-NVFP4" } else { "openai/gpt-oss-120b" };
            let kv = if busy { 0.08 + 0.55 * (conc / 16.0) + 0.03 * noise(self.seed + 13, t) } else { 0.01 };
            let running = if busy { conc } else { 0.0 };
            let waiting = if busy && conc >= 16.0 { 2.0 + 2.0 * noise(self.seed + 14, t).abs() } else { 0.0 };
            s += "===VLLM===\n";
            s += &format!("vllm:prompt_tokens_total{{model_name=\"{model}\"}} {:.0}\n", self.prompt);
            s += &format!("vllm:generation_tokens_total{{model_name=\"{model}\"}} {:.0}\n", self.generated);
            s += &format!("vllm:num_requests_running{{model_name=\"{model}\"}} {running:.0}\n");
            s += &format!("vllm:num_requests_waiting{{model_name=\"{model}\"}} {waiting:.0}\n");
            s += &format!("vllm:kv_cache_usage_perc{{model_name=\"{model}\"}} {kv:.3}\n");
            s += &format!("vllm:prefix_cache_queries_total{{model_name=\"{model}\"}} {:.0}\n", self.pq);
            s += &format!("vllm:prefix_cache_hits_total{{model_name=\"{model}\"}} {:.0}\n", self.ph);
            s += &format!("vllm:request_success_total{{finished_reason=\"length\",model_name=\"{model}\"}} {:.0}\n", self.reqs.floor());
            s += &format!("vllm:num_preemptions_total{{model_name=\"{model}\"}} {}\n", if t > 270.0 { 3 } else { 0 });
            for (le, v) in ["0.05", "0.1", "0.25", "0.5", "1.0", "2.5", "+Inf"].iter().zip(self.ttft) {
                s += &format!("vllm:time_to_first_token_seconds_bucket{{le=\"{le}\",model_name=\"{model}\"}} {v:.0}\n");
            }
            for (le, v) in ["0.01", "0.025", "0.05", "0.075", "0.1", "+Inf"].iter().zip(self.itl) {
                s += &format!("vllm:inter_token_latency_seconds_bucket{{le=\"{le}\",model_name=\"{model}\"}} {v:.0}\n");
            }
        }
        s + "===END===\n"
    }
}

struct World {
    sims: Vec<Sim>,
    cluster: Cluster,
    nodes: Vec<NodeInfo>,
    t: f64,
}

impl World {
    fn new() -> Self {
        let sims = vec![
            Sim::new("spark-r0", "192.168.50.219", "spark", true, 1.0, 0.89, 0),
            Sim::new("spark-r1", "192.168.50.129", "spark", false, 1.0, 0.86, 1),
            Sim::new("gx10-r0", "gx10-r0", "gx10", true, 0.55, 0.81, 2),
            Sim::new("gx10-r3", "192.168.50.5", "gx10", false, 0.0, 0.17, 3),
        ];
        let nodes = sims
            .iter()
            .map(|s| NodeInfo { name: s.name.into(), host: s.host.into(), group: s.group.into(), has_vllm: s.vllm })
            .collect();
        let cluster = sims.iter().map(|s| (s.name.to_string(), NodeHistory::new())).collect();
        Self { sims, cluster, nodes, t: 0.0 }
    }

    fn step(&mut self) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        for s in self.sims.iter_mut() {
            let mut sample = parse_collection(&s.collect(self.t), 1).unwrap();
            sample.t_mono_us = ((self.t + 1.0) * 1e6) as u64;
            let h = self.cluster.get_mut(s.name).unwrap();
            h.push(sample);
            h.mark_ok(now);
        }
        self.t += DT;
    }

    fn run_until(&mut self, t: f64) {
        while self.t < t {
            self.step();
        }
    }
}

fn dump(world: &World, st: &mut UiState, w: u16, h: u16, path: &Path) {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| sparktop::ui::draw(f, &world.cluster, &world.nodes, st)).unwrap();
    let buf = term.backend().buffer();
    let theme = sparktop::ui::theme();
    let hex = |c: Color, fallback: Color| {
        let c = if c == Color::Reset { fallback } else { c };
        match c {
            Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
            _ => "#ffffff".into(),
        }
    };
    let mut rows = Vec::new();
    for y in 0..h {
        let mut row = Vec::new();
        for x in 0..w {
            let c = &buf[(x, y)];
            let bold = c.modifier.contains(ratatui::style::Modifier::BOLD);
            row.push(json!([c.symbol(), hex(c.fg, theme.fg), hex(c.bg, theme.bg), bold]));
        }
        rows.push(row);
    }
    let doc = json!({ "w": w, "h": h, "bg": hex(theme.bg, theme.bg), "rows": rows });
    std::fs::write(path, doc.to_string()).unwrap();
}

fn state(page: Page, view: ClusterView, window_idx: usize) -> UiState {
    let mut st = UiState::new(0.5, Alerts::default());
    st.page = page;
    st.view = view;
    st.window_idx = window_idx;
    st
}

fn main() {
    let out = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "target/screenshots".into()));
    std::fs::create_dir_all(&out).unwrap();
    std::fs::create_dir_all(out.join("gif")).unwrap();
    sparktop::ui::THEME_IDX.store(0, Ordering::Relaxed);
    let mut world = World::new();

    // stills: mid-sweep at concurrency 16, 5m window shows the staircase
    world.run_until(300.0);
    let shots: Vec<(&str, UiState)> = vec![
        ("cluster-panels", {
            let mut st = state(Page::Cluster, ClusterView::Panels, 1);
            st.cluster_charts = vec![ChartKind::Gpu, ChartKind::Decode, ChartKind::Net];
            st
        }),
        ("cluster-table", state(Page::Cluster, ClusterView::Table, 1)),
        ("cluster-compare", state(Page::Cluster, ClusterView::Compare, 1)),
        ("node", state(Page::Node, ClusterView::Panels, 0)),
        ("vllm", state(Page::Vllm, ClusterView::Panels, 1)),
    ];
    for (name, mut st) in shots {
        dump(&world, &mut st, W, H, &out.join(format!("{name}.json")));
    }
    let mut st = state(Page::Cluster, ClusterView::Table, 1);
    st.overlay = Overlay::Nodes { cursor: 1 };
    dump(&world, &mut st, W, H, &out.join("picker.json"));

    // theme gallery: the node page in every palette, smaller terminal
    for (i, t) in sparktop::theme::THEMES.iter().enumerate() {
        sparktop::ui::THEME_IDX.store(i, Ordering::Relaxed);
        let mut st = state(Page::Node, ClusterView::Panels, 0);
        dump(&world, &mut st, 110, 34, &out.join(format!("theme-{}.json", t.name)));
    }
    sparktop::ui::THEME_IDX.store(0, Ordering::Relaxed);

    // animated tour: live frames while the sweep runs, cycling views
    let mut world = World::new();
    world.run_until(230.0);
    let tour: Vec<(UiState, usize)> = vec![
        ({
            let mut st = state(Page::Cluster, ClusterView::Panels, 0);
            st.cluster_charts = vec![ChartKind::Gpu, ChartKind::Decode, ChartKind::Net];
            st
        }, 14),
        (state(Page::Cluster, ClusterView::Compare, 0), 12),
        (state(Page::Cluster, ClusterView::Table, 0), 10),
        (state(Page::Node, ClusterView::Panels, 0), 12),
        (state(Page::Vllm, ClusterView::Panels, 0), 12),
    ];
    let mut n = 0;
    for (mut st, frames) in tour {
        for _ in 0..frames {
            world.step();
            dump(&world, &mut st, W, H, &out.join("gif").join(format!("{n:03}.json")));
            n += 1;
        }
    }
    eprintln!("wrote stills + {n} gif frames to {}", out.display());
}
