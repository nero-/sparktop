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
                "UPTIME_S" => {
                    s.uptime_s = buf.trim().split('.').next().unwrap_or("0").parse().unwrap_or(0);
                    if let Some(l) = buf.lines().nth(1) {
                        // " 09:08:31 up 2:26, 3 users, load average: 0.15, 6.56, 9.25"
                        if let Some(after) = l.split_once("load average:") {
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
                        }
                    }
                }
                // CPU replaces the whole struct — it runs after UPTIME_S,
                // so re-apply the load averages parsed there (they live in
                // the Cpu struct)
                "CPU" => {
                    let load = (s.cpu.load1, s.cpu.load5, s.cpu.load15);
                    s.cpu = parse_proc_stat(buf.trim())?;
                    s.cpu.load1 = load.0;
                    s.cpu.load5 = load.1;
                    s.cpu.load15 = load.2;
                }
                "CPUFREQ" => {
                    s.cpu.mhz = buf
                        .lines()
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
        if line.starts_with("procs_running ") {
            cpu.procs_running = line["procs_running ".len()..].trim().parse().unwrap_or(0);
            continue;
        }
        if line.starts_with("procs_total ") {
            cpu.procs_total = line["procs_total ".len()..].trim().parse().unwrap_or(0);
            continue;
        }
        if line.starts_with("procs_blocked ") {
            // some kernels omit procs_total; blocked+running is a fine proxy
            if cpu.procs_total == 0 {
                cpu.procs_total = cpu.procs_running
                    + line["procs_blocked ".len()..].trim().parse::<u32>().unwrap_or(0);
            }
            continue;
        }
        if line.starts_with("processes ")
            || line.starts_with("softirq ")
            || line.starts_with("intr ")
            || line.starts_with("ctxt ")
            || line.starts_with("btime ")
        {
            continue;
        }
        if let Some(rest) = line.strip_prefix("cpu") {
            let ticks = if rest.starts_with(' ') { rest } else {
                rest.split_once(char::is_whitespace).map(|(_,v)|v).unwrap_or("")
            };
            let fields: Vec<u64> = ticks.split_whitespace().filter_map(|f| f.parse().ok()).collect();
            if fields.len() < 5 {
                continue;
            }
            // user nice system idle iowait irq softirq steal ...
            let idle = fields[3] + fields.get(4).copied().unwrap_or(0);
            let total: u64 = fields.iter().take(8).sum();
            if rest.starts_with(' ') {
                cpu.all = (idle, total);
            } else {
                let idx: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(n) = idx.parse::<usize>() {
                    // store per-index, resize on demand: tolerant of
                    // non-contiguous / offline cores
                    if cpu.cores.len() <= n {
                        cpu.cores.resize(n + 1, (0, 0));
                    }
                    cpu.cores[n] = (idle, total);
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
            mem_used_mb: if f.len() >= 12 { num(mem_used) } else { 0.0 },
            mem_total_mb: if f.len() >= 12 { num(mem_total) } else { 0.0 },
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

/// Split labels outside quoted/escaped values and canonicalize series identity.
fn label_pairs(labels: &str) -> Vec<&str> {
    let (mut quoted,mut escape,mut start)=(false,false,0);
    let mut parts=Vec::new();
    for (i,c) in labels.char_indices() {
        if escape {escape=false;continue;}
        if c=='\\' && quoted {escape=true;continue;}
        if c=='"' {quoted=!quoted;}
        if c==',' && !quoted {parts.push(labels[start..i].trim());start=i+1;}
    }
    if start<labels.len() {parts.push(labels[start..].trim());}
    parts.sort_unstable();parts
}

/// Parse counters by series; scalar fields summarize independent series.
pub fn parse_prometheus(text: &str) -> VllmMetrics {
    let mut m=VllmMetrics::default();
    let mut legacy_tpot=Vec::new();
    let mut modern_tpot=false;
    let mut legacy_cache:f64=0.0;
    let mut modern_cache=false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('#') || line.is_empty() {continue;}
        let (mut quoted,mut escape,mut braces)=(false,false,0i32);
        let split=line.char_indices().find_map(|(i,c)| {
            if escape {escape=false;return None;}
            if c=='\\' && quoted {escape=true;return None;}
            if c=='"' {quoted=!quoted;}
            if !quoted {if c=='{' {braces+=1;} if c=='}' {braces-=1;}}
            if c.is_whitespace() && !quoted && braces==0 {Some(i)} else {None}
        });
        let Some(i)=split else {continue};
        let metric=&line[..i];
        let Some(v)=line[i..].split_whitespace().next().and_then(|v|v.parse::<f64>().ok()).filter(|v|v.is_finite()) else {continue};
        let (name,labels)=metric.split_once('{').map(|(n,l)|(n,l.trim_end_matches('}'))).unwrap_or((metric,""));
        let pairs=label_pairs(labels);
        let le=|| pairs.iter().find_map(|p|p.split_once('=').filter(|(k,_)|k.trim()=="le").map(|(_,v)|v.trim_matches('"')))
            .and_then(|v|if v=="+Inf" {Some(f64::INFINITY)} else {v.parse().ok()});
        let key=format!("{name}[{}]",pairs.join(","));
        if m.raw.contains_key(&key) {continue;}
        m.raw.insert(key,v);
        match name {
            "vllm:prompt_tokens_total"=>m.prompt_tps+=v,
            "vllm:generation_tokens_total"=>m.generation_tps+=v,
            "vllm:num_requests_running"=>m.running+=v.max(0.0) as u64,
            "vllm:num_requests_waiting"=>m.waiting+=v.max(0.0) as u64,
            "vllm:kv_cache_usage_perc"=>{m.gpu_cache_usage=m.gpu_cache_usage.max(v);modern_cache=true;},
            "vllm:gpu_cache_usage_perc"=>legacy_cache=legacy_cache.max(v),
            "vllm:cpu_cache_usage_perc"=>m.cpu_cache_usage=m.cpu_cache_usage.max(v),
            "vllm:time_to_first_token_seconds_bucket"=>if let Some(le)=le(){bucket_push(&mut m.ttft_buckets,le,v);},
            "vllm:inter_token_latency_seconds_bucket"=>{modern_tpot=true;if let Some(le)=le(){bucket_push(&mut m.tpot_buckets,le,v);}},
            "vllm:time_per_output_token_seconds_bucket"=>if let Some(le)=le(){bucket_push(&mut legacy_tpot,le,v);},
            _=>{}
        }
    }
    if !modern_tpot {m.tpot_buckets=legacy_tpot;}
    if !modern_cache {m.gpu_cache_usage=legacy_cache;}
    m.ttft_buckets.sort_by(|a,b|a.0.total_cmp(&b.0));
    m.tpot_buckets.sort_by(|a,b|a.0.total_cmp(&b.0));
    m
}
fn bucket_push(buckets:&mut Vec<(f64,f64)>,le:f64,v:f64) {
    if let Some(slot)=buckets.iter_mut().find(|(l,_)|*l==le) {slot.1+=v;}
    else {buckets.push((le,v));}
}
