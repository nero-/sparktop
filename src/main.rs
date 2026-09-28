use clap::Parser;
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use parking_lot::Mutex;
use sparktop::app::{NodeInfo, Prefs, UiState};
use sparktop::config::{Config, NodeConfig};
use sparktop::history::{Cluster, NodeHistory};
use sparktop::poller::Poller;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

#[derive(Parser)]
#[command(name = "sparktop", version, about = "Terminal resource monitor for DGX Spark clusters")]
struct Args {
    /// Config file path (default: ./sparktop.toml or ~/.config/sparktop/sparktop.toml)
    #[arg(short, long)]
    config: Option<PathBuf>,
    /// Poll interval seconds, overrides config (0.5–10)
    #[arg(short, long)]
    interval: Option<f64>,
    /// Start filtered to one cluster group (the `cluster = "…"` node tag)
    #[arg(short = 'g', long)]
    cluster: Option<String>,
    /// One snapshot of every node then exit (for scripts/cron)
    #[arg(long)]
    once: bool,
    /// With --once: print JSON instead of text
    #[arg(long, requires = "once")]
    json: bool,
    /// Ignore remembered UI preferences (theme, view, charts, filters)
    #[arg(long)]
    fresh: bool,
    /// Don't capture the mouse (keeps terminal text selection)
    #[arg(long)]
    no_mouse: bool,
}

static PAUSED: AtomicBool = AtomicBool::new(false);
static INTERVAL_MS: AtomicU64 = AtomicU64::new(1000);

fn find_config(cli: &Option<PathBuf>) -> anyhow::Result<PathBuf> {
    if let Some(p) = cli {
        return Ok(p.clone());
    }
    let local = PathBuf::from("sparktop.toml");
    if local.exists() {
        return Ok(local);
    }
    if let Some(home) = std::env::var("HOME").ok().map(PathBuf::from) {
        let p = home.join(".config/sparktop/sparktop.toml");
        if p.exists() {
            return Ok(p);
        }
    }
    anyhow::bail!("no sparktop.toml found (looked ./ and ~/.config/sparktop/). Pass --config.")
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// ── poll threads ─────────────────────────────────────────────────────
struct PollHandle {
    cfg: NodeConfig,
    stop: Arc<AtomicBool>,
}

/// One thread per node. The stop flag is checked under the cluster lock
/// before writing, so a node removed (or re-pointed) by a config reload can
/// never write into its replacement's history.
fn spawn_poller(cfg: NodeConfig, cluster: Arc<Mutex<Cluster>>) -> PollHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let name = cfg.name.clone();
    let mut poller = Poller::new(cfg.clone());
    std::thread::spawn(move || {
        while !flag.load(Ordering::Relaxed) {
            if PAUSED.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            let start = Instant::now();
            let res = poller.poll();
            {
                let mut c = cluster.lock();
                if flag.load(Ordering::Relaxed) {
                    break;
                }
                if let Some(h) = c.get_mut(&name) {
                    match res {
                        Ok(s) => {
                            h.mark_ok(poller.last_ok_s.unwrap_or_else(unix_now));
                            h.push(s);
                        }
                        Err(e) => h.mark_err(format!("{e:#}")),
                    }
                }
            }
            // cadence = interval from poll start, so the effective period
            // matches the advertised one even when ssh is slow; sleep in
            // slices so interval changes and removal apply promptly
            loop {
                let interval = Duration::from_millis(INTERVAL_MS.load(Ordering::Relaxed));
                let left = interval.saturating_sub(start.elapsed());
                if left.is_zero() || flag.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(left.min(Duration::from_millis(100)));
            }
        }
    });
    PollHandle { cfg, stop }
}

/// Reconcile running pollers with a (re)loaded config. Returns (added,
/// removed) counts; unchanged nodes keep their thread and history.
fn apply_nodes(cfg: &Config, handles: &mut Vec<PollHandle>, cluster: &Arc<Mutex<Cluster>>) -> (usize, usize) {
    let mut c = cluster.lock();
    let mut removed = 0;
    handles.retain(|h| {
        let keep = cfg.nodes.iter().any(|n| *n == h.cfg);
        if !keep {
            h.stop.store(true, Ordering::Relaxed);
            c.remove(&h.cfg.name);
            removed += 1;
        }
        keep
    });
    let mut added = 0;
    let mut next = Vec::with_capacity(cfg.nodes.len());
    for n in &cfg.nodes {
        match handles.iter().position(|h| h.cfg == *n) {
            Some(i) => next.push(handles.swap_remove(i)),
            None => {
                c.insert(n.name.clone(), NodeHistory::new());
                next.push(spawn_poller(n.clone(), cluster.clone()));
                added += 1;
            }
        }
    }
    *handles = next;
    (added, removed)
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

// ── --once ───────────────────────────────────────────────────────────
fn run_once(cfg: &Config, json: bool) -> anyhow::Result<()> {
    // poll every node in parallel so one slow host doesn't serialize the rest
    let results: Vec<_> = cfg
        .nodes
        .iter()
        .cloned()
        .map(|n| std::thread::spawn(move || {
            let mut p = Poller::new(n.clone());
            (n, p.poll())
        }))
        .collect::<Vec<_>>()
        .into_iter()
        .map(|h| h.join().expect("poll thread panicked"))
        .collect();
    if json {
        let out: Vec<serde_json::Value> = results
            .iter()
            .map(|(n, r)| match r {
                Ok(s) => {
                    let g = s.gpus.first();
                    serde_json::json!({
                        "name": n.name, "cluster": n.group(), "host": n.host, "ok": true,
                        "hostname": s.hostname, "uptime_s": s.uptime_s,
                        "gpu": g.map(|g| serde_json::json!({
                            "name": g.name, "util_pct": g.util_pct, "power_w": g.power_w,
                            "temp_c": g.temp_c, "sm_clock_mhz": g.sm_clock_mhz,
                            "processes": g.pids.iter().map(|p| serde_json::json!({"pid": p.pid, "name": p.name, "mem_mb": p.mem_mb})).collect::<Vec<_>>(),
                        })),
                        "mem": {"used_mb": s.mem.used_kb() / 1024, "total_mb": s.mem.total_kb / 1024},
                        "cpu": {"cores": s.cpu.cores.len(), "load": [s.cpu.load1, s.cpu.load5, s.cpu.load15]},
                        "vllm": s.vllm.as_ref().map(|v| serde_json::json!({
                            "model": v.model, "running": v.running, "waiting": v.waiting,
                            "kv_cache_pct": v.gpu_cache_usage * 100.0,
                            "prompt_tokens_total": v.prompt_tps, "generation_tokens_total": v.generation_tps,
                        })),
                    })
                }
                Err(e) => serde_json::json!({"name": n.name, "cluster": n.group(), "host": n.host, "ok": false, "error": format!("{e:#}")}),
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    for (n, r) in &results {
        match r {
            Ok(s) => {
                println!("=== {} [{}] ({}) up {}s ===", n.name, n.group(), s.hostname, s.uptime_s);
                for g in &s.gpus {
                    println!("  GPU{} {} util {:.0}% {:.1}W {:.0}°C SM {:.0}MHz", g.index, g.name, g.util_pct, g.power_w, g.temp_c, g.sm_clock_mhz);
                }
                println!(
                    "  MEM {}/{} MB  load {:.2}/{:.2}/{:.2}  {} cores",
                    s.mem.used_kb() / 1024,
                    s.mem.total_kb / 1024,
                    s.cpu.load1,
                    s.cpu.load5,
                    s.cpu.load15,
                    s.cpu.cores.len()
                );
                if let Some(v) = &s.vllm {
                    println!(
                        "  vLLM {} running {} waiting {} KV {:.0}%",
                        v.model.as_deref().unwrap_or("?"),
                        v.running,
                        v.waiting,
                        v.gpu_cache_usage * 100.0
                    );
                }
            }
            Err(e) => println!("=== {} [{}] DOWN: {e:#}", n.name, n.group()),
        }
    }
    Ok(())
}

// ── terminal ─────────────────────────────────────────────────────────
fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = crossterm::execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen, crossterm::cursor::Show);
}

/// Restores the terminal on every exit path, including early `?` returns.
struct TermGuard;
impl Drop for TermGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let cfg_path = find_config(&args.config)?;
    let mut cfg = Config::load(&cfg_path)?;
    if args.once {
        return run_once(&cfg, args.json);
    }
    let mut interval = args.interval.unwrap_or(cfg.interval).clamp(0.5, 10.0);
    INTERVAL_MS.store((interval * 1000.0) as u64, Ordering::Relaxed);

    if let Some(i) = cfg.theme.as_deref().and_then(sparktop::theme::theme_index) {
        sparktop::ui::THEME_IDX.store(i, Ordering::Relaxed);
    }
    let cluster: Arc<Mutex<Cluster>> = Arc::new(Mutex::new(Cluster::new()));
    let mut handles: Vec<PollHandle> = Vec::new();
    apply_nodes(&cfg, &mut handles, &cluster);
    let mut nodes: Vec<NodeInfo> = cfg.nodes.iter().map(NodeInfo::from).collect();
    let mut groups = cfg.groups();

    let mut st = UiState::new(interval, cfg.alerts.clone());
    if !args.fresh {
        if let Some(p) = Prefs::load() {
            p.apply(&mut st, &groups);
        }
    }
    if let Some(g) = &args.cluster {
        if !groups.contains(g) {
            anyhow::bail!("no cluster named {g:?} (have: {})", groups.join(", "));
        }
        st.group = Some(g.clone());
    }

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));
    enable_raw_mode()?;
    let _guard = TermGuard;
    crossterm::execute!(io::stdout(), EnterAlternateScreen)?;
    if !args.no_mouse {
        crossterm::execute!(io::stdout(), EnableMouseCapture)?;
    }
    let mut terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(io::stdout()))?;

    let mut cfg_mtime = mtime(&cfg_path);
    let mut last_cfg_check = Instant::now();
    let frame = Duration::from_millis(200);
    while !st.quit {
        terminal.draw(|f| sparktop::ui::draw(f, &cluster.lock(), &nodes, &mut st))?;

        // wait for input up to one frame, then drain whatever queued up
        let deadline = Instant::now() + frame;
        let mut timeout = frame;
        while event::poll(timeout)? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => st.on_key(k, &nodes, &groups),
                Event::Mouse(m) => st.on_mouse(m, &nodes),
                Event::Resize(..) => {}
                _ => {}
            }
            if st.quit {
                break;
            }
            // redraw promptly after input instead of waiting out the frame
            timeout = Duration::ZERO;
            if Instant::now() >= deadline {
                break;
            }
        }
        PAUSED.store(st.paused, Ordering::Relaxed);
        if (st.interval - interval).abs() > 1e-9 {
            interval = st.interval;
            INTERVAL_MS.store((interval * 1000.0) as u64, Ordering::Relaxed);
        }

        // live config reload: add/remove/re-point nodes without a restart
        if last_cfg_check.elapsed() >= Duration::from_secs(1) {
            last_cfg_check = Instant::now();
            let m = mtime(&cfg_path);
            if m != cfg_mtime {
                cfg_mtime = m;
                match Config::load(&cfg_path) {
                    Ok(new) => {
                        let (added, removed) = apply_nodes(&new, &mut handles, &cluster);
                        nodes = new.nodes.iter().map(NodeInfo::from).collect();
                        groups = new.groups();
                        if st.group.as_ref().is_some_and(|g| !groups.contains(g)) {
                            st.group = None;
                        }
                        if args.interval.is_none() && (new.interval - cfg.interval).abs() > 1e-9 {
                            st.interval = new.interval;
                        }
                        st.alerts = new.alerts.clone();
                        cfg = new;
                        st.flash(format!("config reloaded — {} nodes (+{added} −{removed})", nodes.len()));
                    }
                    Err(e) => st.flash(format!("config error, keeping previous: {e:#}")),
                }
            }
        }
    }

    for h in &handles {
        h.stop.store(true, Ordering::Relaxed);
    }
    if !args.fresh {
        let _ = Prefs::capture(&st).save();
    }
    Ok(())
}
