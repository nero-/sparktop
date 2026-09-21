//! Ring-buffer history per node plus derived rates (CPU%, net B/s, tok/s,
//! disk IOPS). Delta math lives here, not in the UI.
use crate::sample::Sample;
use std::collections::HashMap;

pub const HISTORY_CAP: usize = 900; // ~15 min at 1s

#[derive(Debug, Clone, Default)]
pub struct Derived {
    pub cpu_pct: f64,
    pub per_core_pct: Vec<f64>,
    pub net_rx_bps: f64,
    pub net_tx_bps: f64,
    pub disk_read_bps: f64,
    pub disk_write_bps: f64,
    pub prompt_tps: f64,
    pub generation_tps: f64,
    pub ttft_p50: Option<f64>,
    pub ttft_p95: Option<f64>,
    pub tpot_p50: Option<f64>,
    pub tpot_p95: Option<f64>,
}

pub struct NodeHistory {
    pub samples: Vec<Sample>,  // ring buffer as vec, front trimmed
    pub derived: Vec<Derived>, // parallel to samples
    /// last cumulative counters for delta computation
    prev_cpu: (u64, u64),
    prev_cores: Vec<(u64, u64)>,
    prev_net: HashMap<String, (u64, u64)>,
    prev_disk: HashMap<String, (u64, u64)>,
    prev_tokens: (f64, f64),
    prev_ttft: Vec<(f64, f64)>,
    prev_tpot: Vec<(f64, f64)>,
}

impl NodeHistory {
    pub fn new() -> Self {
        Self {
            samples: Vec::with_capacity(HISTORY_CAP),
            derived: Vec::with_capacity(HISTORY_CAP),
            prev_cpu: (0, 0),
            prev_cores: Vec::new(),
            prev_net: HashMap::new(),
            prev_disk: HashMap::new(),
            prev_tokens: (0.0, 0.0),
            prev_ttft: Vec::new(),
            prev_tpot: Vec::new(),
        }
    }

    pub fn push(&mut self, s: Sample) {
        let d = self.derive(&s);
        self.samples.push(s);
        self.derived.push(d);
        if self.samples.len() > HISTORY_CAP {
            self.samples.remove(0);
            self.derived.remove(0);
        }
    }

    fn derive(&mut self, s: &Sample) -> Derived {
        let mut d = Derived::default();

        // CPU
        if self.prev_cpu.1 > 0 && s.cpu.all.1 > self.prev_cpu.1 {
            let dt = (s.cpu.all.1 - self.prev_cpu.1) as f64;
            d.cpu_pct = 100.0 * (dt - (s.cpu.all.0 - self.prev_cpu.0) as f64) / dt;
        }
        for (i, (idle, total)) in s.cpu.cores.iter().enumerate() {
            if let Some((pidle, ptotal)) = self.prev_cores.get(i) {
                if *ptotal > 0 && total > ptotal {
                    let dt = (total - ptotal) as f64;
                    d.per_core_pct
                        .push(100.0 * (dt - (idle - pidle) as f64) / dt);
                    continue;
                }
            }
            d.per_core_pct.push(0.0);
        }
        self.prev_cpu = s.cpu.all;
        self.prev_cores = s.cpu.cores.clone();

        // Network: sum physical + RDMA-capable links; skip lo
        let mut rx = 0u64;
        let mut tx = 0u64;
        for (iface, dev) in &s.net {
            if iface == "lo" || iface.starts_with("docker") || iface.starts_with("veth") {
                continue;
            }
            rx += dev.rx_bytes;
            tx += dev.tx_bytes;
        }
        let prev_net_rx = self.prev_net.values().map(|v| v.0).sum::<u64>();
        let prev_net_tx = self.prev_net.values().map(|v| v.1).sum::<u64>();
        if prev_net_rx > 0 && rx > prev_net_rx {
            d.net_rx_bps = (rx - prev_net_rx) as f64;
            d.net_tx_bps = (tx - prev_net_tx) as f64;
        }
        self.prev_net = s
            .net
            .iter()
            .filter(|(i, _)| *i != "lo" && !i.starts_with("docker") && !i.starts_with("veth"))
            .map(|(i, dev)| (i.clone(), (dev.rx_bytes, dev.tx_bytes)))
            .collect();

        // Disk: sum whole disks, skip partitions and loop/ram devices.
        // Whole disks: nvme\d+n\d+ (nvme0n1), mmcblk\d+; partitions end in
        // p<digits> (nvme0n1p1) or are sd/vd/hd + digit.
        let is_partition = |name: &str| -> bool {
            if let Some(pos) = name.rfind('p') {
                if name[pos + 1..].bytes().all(|c| c.is_ascii_digit())
                    && !name[pos + 1..].is_empty()
                {
                    // nvme0n1p1 / mmcblk0p2 — but not a whole disk named e.g. "p1"
                    return pos > 0;
                }
            }
            (name.starts_with("sd") || name.starts_with("vd") || name.starts_with("hd"))
                && name[2..].bytes().all(|c| c.is_ascii_digit())
        };
        let mut dr = 0u64;
        let mut dw = 0u64;
        for (name, dev) in &s.disks {
            if name.starts_with("loop") || name.starts_with("ram") || name.starts_with("zram") {
                continue;
            }
            if is_partition(name) {
                continue;
            }
            dr += dev.read_bytes;
            dw += dev.write_bytes;
        }
        // keep the same filter on both sides of the delta, or the difference
        // undercounts by cumulative partition bytes
        let prev_dr: u64 = self
            .prev_disk
            .iter()
            .filter(|(n, _)| !is_partition(n) && !n.starts_with("loop") && !n.starts_with("ram") && !n.starts_with("zram"))
            .map(|(_, v)| v.0)
            .sum();
        let prev_dw: u64 = self
            .prev_disk
            .iter()
            .filter(|(n, _)| !is_partition(n) && !n.starts_with("loop") && !n.starts_with("ram") && !n.starts_with("zram"))
            .map(|(_, v)| v.1)
            .sum();
        if prev_dr > 0 && dr > prev_dr {
            d.disk_read_bps = (dr - prev_dr) as f64;
            d.disk_write_bps = (dw - prev_dw) as f64;
        }
        self.prev_disk = s
            .disks
            .iter()
            .filter(|(n, _)| !is_partition(n) && !n.starts_with("loop") && !n.starts_with("ram") && !n.starts_with("zram"))
            .map(|(n, v)| (n.clone(), (v.read_bytes, v.write_bytes)))
            .collect();

        // vLLM token rates from cumulative counters
        if let Some(v) = &s.vllm {
            if self.prev_tokens.0 > 0.0 && v.prompt_tps > self.prev_tokens.0 {
                d.prompt_tps = v.prompt_tps - self.prev_tokens.0;
            }
            if self.prev_tokens.1 > 0.0 && v.generation_tps > self.prev_tokens.1 {
                d.generation_tps = v.generation_tps - self.prev_tokens.1;
            }
            if let Some((p50, p95)) = histogram_pcts(&self.prev_ttft, &v.ttft_buckets) {
                d.ttft_p50 = p50;
                d.ttft_p95 = p95;
            }
            if let Some((p50, p95)) = histogram_pcts(&self.prev_tpot, &v.tpot_buckets) {
                d.tpot_p50 = p50;
                d.tpot_p95 = p95;
            }
            self.prev_tokens = (v.prompt_tps, v.generation_tps);
            self.prev_ttft = v.ttft_buckets.clone();
            self.prev_tpot = v.tpot_buckets.clone();
        }

        d
    }

    pub fn last(&self) -> Option<(&Sample, &Derived)> {
        let i = self.samples.len().checked_sub(1)?;
        Some((&self.samples[i], &self.derived[i]))
    }
}

/// Compute p50/p95 from cumulative histogram bucket deltas between two
/// snapshots of the same histogram. Returns None if no observations happened.
fn histogram_pcts(
    prev: &[(f64, f64)],
    cur: &[(f64, f64)],
) -> Option<(Option<f64>, Option<f64>)> {
    if prev.is_empty() || prev.len() != cur.len() {
        return None;
    }
    let mut deltas: Vec<(f64, f64)> = Vec::new(); // (le, delta count)
    let mut total_delta = 0.0;
    for ((ple, pc), (cle, cc)) in prev.iter().zip(cur.iter()) {
        if ple != cle {
            return None;
        }
        let d = cc - pc;
        if d < 0.0 {
            return None; // histogram reset
        }
        total_delta += d;
        deltas.push((*cle, d));
    }
    if total_delta <= 0.0 {
        return None;
    }
    let pct = |q: f64| -> Option<f64> {
        let target = total_delta * q;
        let mut cum = 0.0;
        for (le, d) in &deltas {
            cum += d;
            if cum >= target {
                return Some(*le);
            }
        }
        None
    };
    Some((pct(0.50), pct(0.95)))
}

pub type Cluster = HashMap<String, NodeHistory>;
