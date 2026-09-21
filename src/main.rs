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
    for p in pollers.clone() {
        let cluster = cluster.clone();
        let last_err = last_err.clone();
        std::thread::spawn(move || loop {
            if PAUSED.load(Ordering::Relaxed) {
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
                    *last_err.lock() = None;
                }
                Err(e) => {
                    *last_err.lock() = Some(format!("{}: {e}", p.lock().node.name));
                }
            }
            std::thread::sleep(Duration::from_secs_f64(interval));
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
            sparktop::ui::draw(f, &cluster.lock(), &node_names, page, focused, PAUSED.load(Ordering::Relaxed), interval, &last_err.lock(), 60.0);
        })?;
        while let Ok(ev) = rx.recv_timeout(Duration::from_millis(100)) {
            if let crossterm::event::Event::Key(k) = ev {
                if k.kind == crossterm::event::KeyEventKind::Press {
                    use crossterm::event::KeyCode::*;
                    match k.code {
                        Char('q') | Esc => running = false,
                        Char(' ') => {
                            PAUSED.store(!PAUSED.load(Ordering::Relaxed), Ordering::Relaxed);
                        }
                        Char('1') => page = Page::Cluster,
                        Char('2') => page = Page::Node,
                        Char('3') => page = Page::Vllm,
                        Char('t') => {
                            sparktop::ui::THEME_IDX.fetch_add(1, Ordering::Relaxed);
                        }
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
