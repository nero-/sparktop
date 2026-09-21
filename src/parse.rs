//! Parsers for raw text collected from remote nodes.
use crate::sample::{Cpu, DiskDev, Gpu, GpuProc, Mem, NetDev, Sample, VllmMetrics};
use std::collections::HashMap;

/// Parse the full output of our compound collection script.
/// Sections are delimited by lines like `===SECTION===`.
pub fn parse_collection(raw: &str, t_unix_us: u64) -> anyhow::Result<Sample> {
    let mut s = Sample { t_unix_us, ..Default::default() };
    let mut section = "";
    let mut buf = String::new();
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("===").and_then(|l| l.strip_suffix("===")) {
            match section {
                "HOSTNAME" => s.hostname = buf.trim().to_string(),
                "UPTIME" => {
                    // "up 2 days, 3:14"  or load averages line
                    if let Some(after) = buf.trim().split_once("load average:") {
                        let la: Vec<f64> = after
                            .1
                            .split(',')
                            .filter_map(|x| x.trim().parse().ok())
                            .collect();
                        if la.len() == 3 {
                            s.cpu.load1 = la[0];
                            s.cpu.load5 = la[1];
                            s.cpu.load15 = la[2];
                        }
                        // also grab proc counts from "x users, load average"
                    }
                    // uptime seconds from `awk '{print $1}' /proc/uptime` variant
                }
                "UPTIME_S" => {
                    s.uptime_s = buf.trim().split('.').next().unwrap_or("0").parse().unwrap_or(0);
                    if let Some(l) = buf.lines().nth(1) {
                        let f: Vec<f64> = l.split_whitespace().filter_map(|x| x.parse().ok()).collect();
                        if f.len() >= 2 {
                            s.cpu.procs_running = f[0] as u32;
                            s.cpu.procs_total = f[1] as u32;
                        }
                    }
                }
                "CPU" => s.cpu = parse_proc_stat(buf.trim())?,
                "CPUFREQ" => {
                    s.cpu.mhz = buf
                        .lines()
                        .filter_map(|l| l.split(':').nth(1))
                        .filter_map(|v| v.trim().parse::<f64>().ok())
                        .collect();
                }
                "MEM" => s.mem = parse_meminfo(buf.trim())?,
                "GPU" => s.gpus = parse_nvidia_smi(buf.trim())?,
                "NET" => s.net = parse_proc_net_dev(buf.trim())?,
                "DISK" => s.disks = parse_diskstats(buf.trim())?,
                "VLLM" if !buf.trim().is_empty() => s.vllm = Some(parse_prometheus(buf.trim())),
                "GPUPROC" => parse_gpu_procs(buf.trim(), &mut s.gpus),
                _ => {}
            }
            section = rest;
            buf.clear();
        } else {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    if s.hostname.is_empty() {
        anyhow::bail!("no HOSTNAME section — node unreachable or script failed");
    }
    Ok(s)
}

fn parse_proc_stat(text: &str) -> anyhow::Result<Cpu> {
    let mut cpu = Cpu::default();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("cpu") {
            let fields: Vec<u64> =
                rest.split_whitespace().filter_map(|f| f.parse().ok()).collect();
            if fields.len() < 5 {
                continue;
            }
            // user nice system idle iowait irq softirq steal ...
            let idle = fields[3] + fields.get(4).copied().unwrap_or(0);
            let total: u64 = fields.iter().sum();
            if rest.starts_with(' ') {
                cpu.all = (idle, total);
            } else {
                let idx: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(n) = idx.parse::<usize>() {
                    if n == cpu.cores.len() {
                        cpu.cores.push((idle, total));
                    }
                }
            }
        }
    }
    Ok(cpu)
}

fn parse_meminfo(text: &str) -> anyhow::Result<Mem> {
    let mut m = Mem::default();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let key = it.next().unwrap_or("");
        let val: u64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        match key {
            "MemTotal:" => m.total_kb = val,
            "MemAvailable:" => m.avail_kb = val,
            "Buffers:" => m.buffers_kb = val,
            "Cached:" => m.cached_kb = val,
            _ => {}
        }
    }
    Ok(m)
}

pub fn parse_nvidia_smi(text: &str) -> anyhow::Result<Vec<Gpu>> {
    // CSV: index, name, util, [mem_used, mem_total,] power[, power_limit,] temp, sm_clk[, mem_clk, pcie_rx, pcie_tx]
    // Column count varies by GPU type (GB10 reports no VRAM/PCIe fields).
    let mut gpus = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split(',').map(|x| x.trim()).collect();
        if f.len() < 6 {
            continue;
        }
        let num = |i: usize| -> f64 {
            f.get(i)
                .and_then(|v| v.split(' ').next())
                .and_then(|v| v.replace("N/A", "0").parse().ok())
                .unwrap_or(0.0)
        };
        // Layout depends on column count:
        // 6 cols (GB10):  idx,name,util,power,temp,sm_clk
        // 12 cols:        idx,name,util,mem_used,mem_total,power,power_limit,temp,sm_clk,mem_clk,pcie_rx,pcie_tx
        let (mem_used, mem_total, power_i, limit_i, temp_i, clk_i) = if f.len() >= 12 {
            (3, 4, 5, Some(6), 7, 8)
        } else {
            (0, 0, 3, None, 4, 5)
        };
        gpus.push(Gpu {
            index: f[0].parse().unwrap_or(0),
            name: f[1].to_string(),
            util_pct: num(2),
            mem_used_mb: num(mem_used),
            mem_total_mb: num(mem_total),
            power_w: num(power_i),
            power_limit_w: limit_i.map(num).unwrap_or(0.0),
            temp_c: num(temp_i),
            sm_clock_mhz: num(clk_i),
            mem_clock_mhz: if f.len() >= 12 { num(9) } else { 0.0 },
            pcie_rx_mbs: if f.len() >= 12 { num(10) } else { 0.0 },
            pcie_tx_mbs: if f.len() >= 12 { num(11) } else { 0.0 },
            pids: Vec::new(),
        });
    }
    Ok(gpus)
}

fn parse_gpu_procs(text: &str, gpus: &mut [Gpu]) {
    // "pid, name, mem" lines
    for line in text.lines() {
        let f: Vec<&str> = line.split(',').map(|x| x.trim()).collect();
        if f.len() != 3 {
            continue;
        }
        let pid: u32 = f[0].parse().unwrap_or(0);
        let mem = f[2].split_whitespace().next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
        let gp = GpuProc { pid, name: f[1].to_string(), mem_mb: mem };
        // DGX Spark unified memory: attach to gpu 0
        if let Some(g) = gpus.first_mut() {
            g.pids.push(gp);
        }
    }
}

fn parse_proc_net_dev(text: &str) -> anyhow::Result<HashMap<String, NetDev>> {
    let mut map = HashMap::new();
    for line in text.lines().skip(2) {
        let (iface, rest) = match line.split_once(':') {
            Some(x) => x,
            None => continue,
        };
        let f: Vec<u64> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
        if f.len() < 9 {
            continue;
        }
        map.insert(
            iface.trim().to_string(),
            NetDev { rx_bytes: f[0], tx_bytes: f[8], rx_pkts: f[1], tx_pkts: f[9].max(0) as u64 },
        );
    }
    Ok(map)
}

pub fn parse_diskstats(text: &str) -> anyhow::Result<HashMap<String, DiskDev>> {
    // fields: major minor name reads_completed reads_merged sectors_read ms_reading
    //         writes_completed writes_merged sectors_written ms_writing ios_in_progress ms_doing_io weighted_ms
    let mut map = HashMap::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 7 {
            continue;
        }
        let name = f[2].to_string();
        if name.starts_with("loop") || name.starts_with("ram") || name.starts_with("zram") {
            continue;
        }
        let u64f = |i: usize| f.get(i).and_then(|x| x.parse().ok()).unwrap_or(0);
        map.insert(
            name,
            DiskDev {
                read_bytes: u64f(5) * 512,   // sectors read
                write_bytes: u64f(9) * 512,  // sectors written
                io_ticks_ms: u64f(12),
            },
        );
    }
    Ok(map)
}

/// Parse a Prometheus text exposition payload, extracting vLLM metrics.
pub fn parse_prometheus(text: &str) -> VllmMetrics {
    let mut m = VllmMetrics::default();
    let mut raw = HashMap::new();
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let (metric, val) = match line.split_once(' ') {
            Some(x) => x,
            None => continue,
        };
        let v: f64 = val.trim().parse().unwrap_or(0.0);
        // metric{labels}
        let (name, labels) = match metric.split_once('{') {
            Some((n, l)) => (n, l.trim_end_matches('}')),
            None => (metric, ""),
        };
        let label = |want: &str| -> Option<String> {
            for pair in labels.split(',').filter(|p| !p.is_empty()) {
                if let Some((k, v)) = pair.split_once('=') {
                    if k.trim() == want {
                        return Some(v.trim_matches('"').to_string());
                    }
                }
            }
            None
        };
        raw.insert(format!("{name}[{labels}]"), v);
        match name {
            "vllm:prompt_tokens_total" => m.prompt_tps = v, // raw counter; rate computed later
            "vllm:generation_tokens_total" => m.generation_tps = v,
            "vllm:num_requests_running" => m.running = v as u64,
            "vllm:num_requests_waiting" => m.waiting = v as u64,
            "vllm:gpu_cache_usage_perc" => m.gpu_cache_usage = v,
            "vllm:cpu_cache_usage_perc" => m.cpu_cache_usage = v,
            "vllm:time_to_first_token_seconds_bucket" => {
                if let Some(le) = label("le") {
                    let le: f64 = if le == "+Inf" { f64::INFINITY } else { le.parse().unwrap_or(0.0) };
                    bucket_push(&mut m.ttft_buckets, le, v);
                }
            }
            "vllm:time_per_output_token_seconds_bucket" => {
                if let Some(le) = label("le") {
                    let le: f64 = if le == "+Inf" { f64::INFINITY } else { le.parse().unwrap_or(0.0) };
                    bucket_push(&mut m.tpot_buckets, le, v);
                }
            }
            _ => {}
        }
    }
    m.raw = raw;
    m
}

fn bucket_push(buckets: &mut Vec<(f64, f64)>, le: f64, v: f64) {
    if let Some(slot) = buckets.iter_mut().find(|(l, _)| *l == le) {
        slot.1 = v;
    } else {
        buckets.push((le, v));
    }
}
