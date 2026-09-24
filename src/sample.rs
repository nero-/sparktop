/// One polled sample; wall time for display and monotonic receipt time for rates.
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct Gpu {
    pub index: u32,
    pub name: String,
    pub util_pct: f64,
    pub mem_used_mb: f64,
    pub mem_total_mb: f64,
    pub power_w: f64,
    pub power_limit_w: f64,
    pub temp_c: f64,
    pub sm_clock_mhz: f64,
    #[allow(dead_code)] // collected; graphed in node detail later
    pub mem_clock_mhz: f64,
    #[allow(dead_code)] // collected; graphed in node detail later
    pub pcie_rx_mbs: f64,
    #[allow(dead_code)] // collected; graphed in node detail later
    pub pcie_tx_mbs: f64,
    #[allow(dead_code)] // collected now; surfaces in node-detail graphs later
    pub pids: Vec<GpuProc>,
}

#[derive(Debug, Clone, Default)]
pub struct GpuProc {
    pub pid: u32,
    pub name: String,
    pub mem_mb: f64,
}

#[derive(Debug, Clone, Default)]
pub struct Mem {
    pub total_kb: u64,
    pub avail_kb: u64,
    pub buffers_kb: u64,
    pub cached_kb: u64,
}

impl Mem {
    pub fn used_kb(&self) -> u64 {
        self.total_kb.saturating_sub(self.avail_kb)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Cpu {
    /// per-core ticks: (idle, total)
    pub cores: Vec<(u64, u64)>,
    /// aggregate (idle, total)
    pub all: (u64, u64),
    pub mhz: Vec<f64>,
    pub load1: f64,
    pub load5: f64,
    pub load15: f64,
    pub procs_running: u32,
    pub procs_total: u32,
}

#[derive(Debug, Clone, Default)]
pub struct NetDev {
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    #[allow(dead_code)]
    pub rx_pkts: u64,
    #[allow(dead_code)]
    pub tx_pkts: u64,
}

#[derive(Debug, Clone, Default)]
pub struct DiskDev {
    pub read_bytes: u64,
    pub write_bytes: u64,
    #[allow(dead_code)]
    pub io_ticks_ms: u64,
}

#[derive(Debug, Clone, Default)]
pub struct Sample {
    #[allow(dead_code)]
    pub t_unix_us: u64,
    /// Monotonic receipt timestamp used for rates; zero in legacy fixtures.
    pub t_mono_us: u64,
    pub hostname: String,
    pub gpus: Vec<Gpu>,
    pub mem: Mem,
    pub cpu: Cpu,
    pub net: HashMap<String, NetDev>,
    pub disks: HashMap<String, DiskDev>,
    pub uptime_s: u64,
    pub vllm: Option<VllmMetrics>,
}

impl Sample {
    pub fn rate_time_us(&self) -> u64 {
        if self.t_mono_us > 0 { self.t_mono_us } else { self.t_unix_us }
    }
}

#[derive(Debug, Clone, Default)]
pub struct VllmMetrics {
    /// Cumulative prompt token counter; rates live in Derived.
    pub prompt_tps: f64,
    /// Cumulative generation token counter; rates live in Derived.
    pub generation_tps: f64,
    pub running: u64,
    pub waiting: u64,
    pub gpu_cache_usage: f64,
    pub cpu_cache_usage: f64,
    /// TTFT buckets: (upper bound, cumulative count).
    pub ttft_buckets: Vec<(f64, f64)>,
    /// histogram bucket sums for ITL/TPOT
    pub tpot_buckets: Vec<(f64, f64)>,
    /// raw counters we diff for derived rates
    pub raw: HashMap<String, f64>,
}
