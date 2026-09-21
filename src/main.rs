use clap::Parser;
use parking_lot::Mutex;
use sparktop::config::Config;
use sparktop::history::{Cluster, NodeHistory};
use sparktop::poller::Poller;
use sparktop::ui::Page;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "sparktop", about = "Terminal resource monitor for DGX Spark clusters")]
struct Args {
    /// Config file path (default: ./sparktop.toml or ~/.config/sparktop/sparktop.toml)
    #[arg(short, long)]
    config: Option<PathBuf>,
    /// Poll interval seconds, overrides config
    #[arg(short, long)]
    interval: Option<f64>,
    /// One text snapshot then exit (for scripts/cron)
    #[arg(long)]
    once: bool,
}

static PAUSED: AtomicBool = AtomicBool::new(false);

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

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let cfg = Config::load(&find_config(&args.config)?)?;
    let interval = args.interval.unwrap_or(cfg.interval).clamp(0.5, 10.0);

    let node_names: Vec<String> = cfg.nodes.iter().map(|n| n.name.clone()).collect();
    let pollers: Vec<Arc<Mutex<Poller>>> =
        cfg.nodes.into_iter().map(|n| Arc::new(Mutex::new(Poller::new(n)))).collect();
    let cluster: Arc<Mutex<Cluster>> =
        Arc::new(Mutex::new(node_names.iter().map(|n| (n.clone(), NodeHistory::new())).collect()));
    let last_err: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    // per-node unix seconds of last successful poll — data-age display
    let ages: Arc<Mutex<Vec<Option<u64>>>> =
        Arc::new(Mutex::new(vec![None; node_names.len()]));

    if args.once {
        for p in &pollers {
            let mut p = p.lock();
            match p.poll() {
                Ok(s) => {
                    println!("=== {} ({}) ===", s.hostname, p.status());
                    for g in &s.gpus {
                        println!("  GPU{} {} util {:.0}% mem {}/{} MB {}W {:.0}°C", g.index, g.name, g.util_pct, g.mem_used_mb, g.mem_total_mb, g.power_w, g.temp_c);
                    }
                    println!("  MEM {}/{} MB  load {:.1}/{:.1}/{:.1}", s.mem.used_kb() / 1024, s.mem.total_kb / 1024, s.cpu.load1, s.cpu.load5, s.cpu.load15);
                }
                Err(e) => println!("=== {} DOWN: {e}", p.node.name),
            }
        }
        return Ok(());
    }

    // Background poll threads: one per node.
    for (i, p) in pollers.clone().into_iter().enumerate() {
        let cluster = cluster.clone();
        let last_err = last_err.clone();
        let ages = ages.clone();
        std::thread::spawn(move || loop {
            let paused = PAUSED.load(Ordering::Relaxed);
            let start = std::time::Instant::now();
            if paused {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            let sample = {
                let mut p = p.lock();
                p.poll()
            };
            match sample {
                Ok(s) => {
                    let name = p.lock().node.name.clone();
                    {
                        let mut c = cluster.lock();
                        if let Some(h) = c.get_mut(&name) {
                            h.push(s);
                        }
                    }
                    ages.lock()[i] = p.lock().last_ok_s;
                    *last_err.lock() = None;
                }
                Err(e) => {
                    *last_err.lock() = Some(format!("{}: {e}", p.lock().node.name));
                }
            }
            // poll cadence = interval from poll start, so the effective
            // period matches the advertised interval even when ssh is slow
            let elapsed = start.elapsed();
            if elapsed < Duration::from_secs_f64(interval) {
                std::thread::sleep(Duration::from_secs_f64(interval) - elapsed);
            }
        });
    }

    // TUI loop
    let mut stdout = io::stdout();
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    let mut page = Page::Cluster;
    let mut focused: usize = 0;
    let mut running = true;
    let mut window_idx: usize = 0;
    // per-page chart selections (empty = defaults)
    let mut cluster_charts: Vec<sparktop::ui::ChartKind> = vec![
        sparktop::ui::ChartKind::Gpu,
        sparktop::ui::ChartKind::Mem,
        sparktop::ui::ChartKind::Net,
    ];
    let mut node_charts: Vec<sparktop::ui::ChartKind> = Vec::new(); // defaults: cpu/mem/gpu
    let mut show_help = false;
    // transient status for no-op keys (e/r/c feedback): (message, set-at)
    let status: Arc<Mutex<Option<(String, std::time::Instant)>>> = Arc::new(Mutex::new(None));
    let (tx, rx) = std::sync::mpsc::channel::<crossterm::event::Event>();
    std::thread::spawn(move || {
        loop {
            if let Ok(ev) = crossterm::event::read() {
                if tx.send(ev).is_err() {
                    break;
                }
            }
        }
    });

    while running {
        terminal.draw(|f| {
            sparktop::ui::draw(
                f,
                &cluster.lock(),
                &node_names,
                page,
                focused,
                PAUSED.load(Ordering::Relaxed),
                interval,
                &last_err.lock(),
                &ages.lock(),
                sparktop::ui::WINDOWS[window_idx],
                &cluster_charts,
                &node_charts,
                show_help,
                &status.lock(),
            );
        })?;
        while let Ok(ev) = rx.recv_timeout(Duration::from_millis(100)) {
            if let crossterm::event::Event::Key(k) = ev {
                if k.kind == crossterm::event::KeyEventKind::Press {
                    use crossterm::event::KeyCode::*;
                    match k.code {
                        Char('q') => running = false,
                        Esc if show_help => show_help = false,
                        Esc => running = false,
                        Char(' ') => {
                            PAUSED.store(!PAUSED.load(Ordering::Relaxed), Ordering::Relaxed);
                        }
                        Char('1') => page = Page::Cluster,
                        Char('2') => page = Page::Node,
                        Char('3') => page = Page::Vllm,
                        Char('t') => {
                            sparktop::ui::THEME_IDX.fetch_add(1, Ordering::Relaxed);
                        }
                        Char('w') => {
                            window_idx = (window_idx + 1) % sparktop::ui::WINDOWS.len();
                        }
                        Char('e') => {
                            // add the next chart kind not already shown;
                            // no-op when all six kinds are already present
                            // (the ring would otherwise loop forever)
                            let (charts, skip) = match page {
                                Page::Cluster => (&mut cluster_charts, vec![]),
                                // node page always renders net+disk rows below,
                                // so skip those kinds when choosing what to add
                                Page::Node => (&mut node_charts, vec![sparktop::ui::ChartKind::Net, sparktop::ui::ChartKind::Disk]),
                                Page::Vllm => {
                                    *status.lock() = Some(("chart keys apply to pages 1–2".to_string(), std::time::Instant::now()));
                                    continue;
                                }
                            };
                            if charts.len() >= 6 {
                                *status.lock() = Some(("page full — 6/6 charts (r to remove)".to_string(), std::time::Instant::now()));
                                continue;
                            }
                            // seed from the page's first default when empty, so
                            // re-adding rebuilds toward the defaults (Gpu on
                            // cluster, Cpu on node) instead of a lone Disk
                            let seed = charts
                                .last()
                                .copied()
                                .or_else(|| sparktop::ui::defaults_for(page).first().copied())
                                .unwrap_or(sparktop::ui::ChartKind::Net);
                            let mut next = seed.next();
                            while charts.contains(&next) || skip.contains(&next) {
                                next = next.next();
                            }
                            charts.push(next);
                            status.lock().take();
                        }
                        Char('r') => {
                            let charts = match page {
                                Page::Cluster => &mut cluster_charts,
                                Page::Node => &mut node_charts,
                                Page::Vllm => {
                                    *status.lock() = Some(("chart keys apply to pages 1–2".to_string(), std::time::Instant::now()));
                                    continue;
                                }
                            };
                            match charts.pop() {
                                Some(_) => {
                                    status.lock().take();
                                }
                                None => *status.lock() = Some(("nothing to remove — page shows defaults".to_string(), std::time::Instant::now())),
                            }
                        }
                        Char('c') => {
                            match page {
                                Page::Cluster => cluster_charts = sparktop::ui::DEFAULTS_CLUSTER.to_vec(),
                                Page::Node => node_charts = Vec::new(), // defaults
                                Page::Vllm => {}
                            }
                            status.lock().take();
                        }
                        Char('?') => show_help = !show_help,
                        Tab => focused = (focused + 1) % node_names.len().max(1),
                        BackTab => focused = focused.checked_sub(1).unwrap_or(node_names.len().saturating_sub(1)),
                        _ => {}
                    }
                }
            }
        }
    }

    crossterm::execute!(io::stdout(), crossterm::terminal::LeaveAlternateScreen)?;
    crossterm::terminal::disable_raw_mode()?;
    Ok(())
}
