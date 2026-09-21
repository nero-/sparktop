use sparktop::history::NodeHistory;
use sparktop::parse::parse_collection;
use std::io::Read;

fn main() -> anyhow::Result<()> {
    let mut raw = String::new();
    let arg: Option<String> = std::env::args().nth(1);
    match arg {
        Some(p) => std::fs::File::open(p)?.read_to_string(&mut raw)?,
        None => std::io::stdin().read_to_string(&mut raw)?,
    };

    let mut h = NodeHistory::new();
    let mut t: u64 = 1_000_000;
    let raw2 = raw
        .replace("cpu  1000 0 500 8000 100", "cpu  1200 0 600 8500 110")
        .replace("cpu0 100 0 50 800 10", "cpu0 130 0 60 850 12")
        .replace("  eth0: 2000000000", "  eth0: 2001000000")
        .replace("1000000000 2000000", "1000200000 2000000");
    let raw3 = raw2
        .replace("cpu  1200 0 600 8500 110", "cpu  1400 0 700 9000 120")
        .replace("cpu0 130 0 60 850 12", "cpu0 160 0 70 900 14")
        .replace("  eth0: 2001000000", "  eth0: 2002500000")
        .replace("1000200000 2000000", "1000500000 2000000");

    for r in [raw, raw2, raw3] {
        let s = parse_collection(&r, t)?;
        t += 1_000_000;
        h.push(s);
    }
    let (s, d) = h.last().unwrap();
    println!("hostname: {}", s.hostname);
    println!("gpus: {}", s.gpus.len());
    if let Some(g) = s.gpus.first() {
        println!("  {} util {:.0}% mem {}/{}MB {}W {}°C sm {}MHz", g.name, g.util_pct, g.mem_used_mb, g.mem_total_mb, g.power_w, g.temp_c, g.sm_clock_mhz);
        println!("  procs: {:?}", g.pids.iter().map(|p| (p.pid, p.name.clone(), p.mem_mb)).collect::<Vec<_>>());
    }
    println!("mem used {}/{} kB", s.mem.used_kb(), s.mem.total_kb);
    println!("cpu cores: {} load {:.1}", s.cpu.cores.len(), s.cpu.load1);
    println!("derived cpu%: {:.1} per-core: {:?}", d.cpu_pct, d.per_core_pct);
    println!("net rx {}/s tx {}/s", d.net_rx_bps, d.net_tx_bps);
    println!("disk read {}/s write {}/s", d.disk_read_bps, d.disk_write_bps);
    println!("uptime: {}", s.uptime_s);
    Ok(())
}
