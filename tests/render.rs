//! Render every page/view against synthetic multi-node histories: no panics
//! at any size, and key content lands on screen. `SPARKTOP_DUMP=1 cargo test
//! --test render -- --nocapture` prints the screens for eyeballing.
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use sparktop::app::{ClusterView, NodeInfo, Overlay, Page, UiState};
use sparktop::config::Alerts;
use sparktop::history::{Cluster, NodeHistory};
use sparktop::parse::parse_collection;

fn raw(host: &str, t: f64, i: usize, vllm: bool) -> String {
    let gpu = 50.0 + 45.0 * ((t / 7.0 + i as f64).sin());
    let busy = (t * 100.0) as u64;
    let rx = (t * 3.0e6 * (i + 1) as f64) as u64;
    let tx = (t * 1.0e6) as u64;
    let mut s = format!(
        "===HOSTNAME===\n{host}\n===UPTIME_S===\n{}\n 1 users, load average: 1.{i}0, 0.80, 0.60\n===CPU===\n",
        123456 + i * 1000
    );
    s += &format!("cpu  {} 0 {} {} 0 0 0 0\n", busy * 20, busy * 4, busy * 30);
    for c in 0..20 {
        let b = busy * (c % 5 + 1) as u64;
        s += &format!("cpu{c} {b} 0 {} {} 0 0 0 0\n", b / 4, busy * 6);
    }
    s += "procs_running 3\nprocs_blocked 0\n===CPUFREQ===\n";
    for _ in 0..20 {
        s += "3900\n";
    }
    s += &format!(
        "===MEM===\nMemTotal: 128000000 kB\nMemAvailable: {} kB\nBuffers: 1000 kB\nCached: 9000000 kB\n",
        30000000 - i as u64 * 9_000_000
    );
    s += &format!("===GPU===\n0, NVIDIA GB10, {gpu:.0}, {:.1}, {}, 2400\n", 20.0 + gpu * 0.8, 48 + i * 12);
    s += "===GPUPROC===\n4242, /usr/bin/python3, 90000\n77, vllm-worker, 1200\n";
    s += "===NET===\nInter-|\n face |\n";
    s += &format!("    lo: 1 1 0 0 0 0 0 0 1 1 0 0 0 0 0 0\n  enP7s7: {rx} 10 0 0 0 0 0 0 {tx} 10 0 0 0 0 0 0\n  wlP9s9: 100 1 0 0 0 0 0 0 100 1 0 0 0 0 0 0\n");
    s += &format!("===DISK===\n 259 0 nvme0n1 1 0 {} 0 1 0 {} 0 0 0 0\n", busy * 50, busy * 20);
    if vllm {
        let g = (t * 900.0) as u64;
        let p = (t * 4000.0) as u64;
        s += &format!(
            "===VLLM===\nvllm:prompt_tokens_total{{model_name=\"nvidia/Qwen3-235B-FP4\"}} {p}\nvllm:generation_tokens_total{{model_name=\"nvidia/Qwen3-235B-FP4\"}} {g}\n\
             vllm:num_requests_running 4\nvllm:num_requests_waiting 1\nvllm:kv_cache_usage_perc 0.37\n\
             vllm:prefix_cache_queries_total {}\nvllm:prefix_cache_hits_total {}\nvllm:request_success_total {}\n\
             vllm:time_to_first_token_seconds_bucket{{le=\"0.1\"}} {}\nvllm:time_to_first_token_seconds_bucket{{le=\"0.5\"}} {}\nvllm:time_to_first_token_seconds_bucket{{le=\"+Inf\"}} {}\n\
             vllm:inter_token_latency_seconds_bucket{{le=\"0.02\"}} {}\nvllm:inter_token_latency_seconds_bucket{{le=\"0.05\"}} {}\nvllm:inter_token_latency_seconds_bucket{{le=\"+Inf\"}} {}\n",
            p, p * 3 / 5, (t / 3.0) as u64, busy, busy * 2, busy * 2, g / 2, g, g
        );
    }
    s + "===END===\n"
}

fn fixture(n: usize, secs: usize) -> (Cluster, Vec<NodeInfo>) {
    let mut c = Cluster::new();
    let mut nodes = Vec::new();
    for i in 0..n {
        let name = if i < 2 { format!("spark-r{i}") } else { format!("gx10-r{}", i - 2) };
        let vllm = i % 2 == 0;
        let mut h = NodeHistory::new();
        for k in 0..secs * 2 {
            let t = k as f64 * 0.5;
            let mut s = parse_collection(&raw(&name, t, i, vllm), 1).unwrap();
            s.t_mono_us = ((t + 1.0) * 1e6) as u64;
            h.push(s);
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        h.mark_ok(now);
        c.insert(name.clone(), h);
        nodes.push(NodeInfo {
            name,
            host: format!("192.168.50.{}", 10 + i),
            group: if i < 2 { "spark".into() } else { "gx10".into() },
            has_vllm: vllm,
        });
    }
    (c, nodes)
}

fn render(c: &Cluster, nodes: &[NodeInfo], st: &mut UiState, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| sparktop::ui::draw(f, c, nodes, st)).unwrap();
    let buf = term.backend().buffer();
    let mut out = String::new();
    for y in 0..h {
        for x in 0..w {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    if std::env::var("SPARKTOP_DUMP").is_ok() {
        println!("── {w}x{h} page={:?} view={:?} ──\n{out}", st.page, st.view);
    }
    out
}

fn key(st: &mut UiState, nodes: &[NodeInfo], groups: &[String], code: KeyCode) {
    st.on_key(KeyEvent::new(code, KeyModifiers::NONE), nodes, groups);
}

#[test]
fn every_page_renders_at_many_sizes() {
    let (c, nodes) = fixture(4, 90);
    for (w, h) in [(30, 6), (60, 20), (100, 30), (160, 45), (220, 60)] {
        for page in [Page::Cluster, Page::Node, Page::Vllm] {
            for view in [ClusterView::Panels, ClusterView::Table, ClusterView::Compare] {
                let mut st = UiState::new(0.5, Alerts::default());
                st.page = page;
                st.view = view;
                let out = render(&c, &nodes, &mut st, w, h);
                assert!(!out.contains("NaN"), "NaN on {page:?}/{view:?} {w}x{h}");
            }
        }
    }
}

#[test]
fn cluster_views_show_expected_content() {
    let (c, nodes) = fixture(4, 90);
    let mut st = UiState::new(0.5, Alerts::default());
    let out = render(&c, &nodes, &mut st, 160, 50);
    assert!(out.contains("spark-r0") && out.contains("gx10-r1"));
    assert!(out.contains("4/4 up"), "summary strip counts nodes");
    assert!(out.contains("decode"), "summary shows cluster decode");

    st.view = ClusterView::Table;
    let out = render(&c, &nodes, &mut st, 160, 50);
    assert!(out.contains("trend") && out.contains("NET ↓"));
    assert!(out.chars().any(|ch| "▁▂▃▄▅▆▇█".contains(ch)), "sparklines drawn");

    st.view = ClusterView::Compare;
    let out = render(&c, &nodes, &mut st, 160, 50);
    assert!(out.contains("━━ spark-r0") && out.contains("power"));
}

#[test]
fn panels_fall_back_to_table_when_crowded() {
    let (c, nodes) = fixture(4, 30);
    let mut st = UiState::new(0.5, Alerts::default());
    let out = render(&c, &nodes, &mut st, 120, 24);
    assert!(out.contains("table (auto"), "{out}");
}

#[test]
fn node_page_shows_cores_ifaces_procs() {
    let (c, nodes) = fixture(2, 60);
    let mut st = UiState::new(0.5, Alerts::default());
    st.page = Page::Node;
    let out = render(&c, &nodes, &mut st, 160, 48);
    assert!(out.contains("cores 20"), "{out}");
    assert!(out.contains("enP7s7"));
    assert!(out.contains("gpu processes 2"));
    assert!(out.contains("python3"));
}

#[test]
fn vllm_page_shows_model_and_cache() {
    let (c, nodes) = fixture(2, 60);
    let mut st = UiState::new(0.5, Alerts::default());
    st.page = Page::Vllm;
    let out = render(&c, &nodes, &mut st, 160, 45);
    assert!(out.contains("Qwen3-235B-FP4"), "{out}");
    assert!(out.contains("prefix hit 60%"), "{out}");
    // spark-r1 has no vllm_url: tab on the vLLM page skips it
    key(&mut st, &nodes, &[], KeyCode::Tab);
    assert_eq!(st.focused.as_deref(), Some("spark-r0"));
}

#[test]
fn group_filter_hide_and_picker() {
    let (c, nodes) = fixture(4, 20);
    let groups = vec!["spark".to_string(), "gx10".to_string()];
    let mut st = UiState::new(0.5, Alerts::default());
    key(&mut st, &nodes, &groups, KeyCode::Char('g'));
    assert_eq!(st.group.as_deref(), Some("spark"));
    let out = render(&c, &nodes, &mut st, 140, 40);
    assert!(out.contains("spark-r1") && !out.contains("gx10-r0"));
    key(&mut st, &nodes, &groups, KeyCode::Char('g'));
    key(&mut st, &nodes, &groups, KeyCode::Char('g'));
    assert_eq!(st.group, None, "cycles back to all");

    key(&mut st, &nodes, &groups, KeyCode::Char('n'));
    assert!(matches!(st.overlay, Overlay::Nodes { cursor: 0 }));
    key(&mut st, &nodes, &groups, KeyCode::Char(' '));
    assert!(st.hidden.contains("spark-r0"));
    let out = render(&c, &nodes, &mut st, 140, 40);
    assert!(out.contains("[ ]") && out.contains("[x]"));
    key(&mut st, &nodes, &groups, KeyCode::Esc);
    assert_eq!(st.visible(&nodes).len(), 3);
    assert_eq!(st.focused_node(&nodes).unwrap().name, "spark-r1");
}

#[test]
fn navigation_enter_esc_and_ctrl_c() {
    let (_, nodes) = fixture(3, 5);
    let mut st = UiState::new(0.5, Alerts::default());
    key(&mut st, &nodes, &[], KeyCode::Down);
    key(&mut st, &nodes, &[], KeyCode::Enter);
    assert_eq!((st.page, st.focused.as_deref()), (Page::Node, Some("spark-r1")));
    key(&mut st, &nodes, &[], KeyCode::Esc);
    assert_eq!(st.page, Page::Cluster);
    assert!(!st.quit);
    // ctrl-c must quit, not reset charts
    st.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL), &nodes, &[]);
    assert!(st.quit);
}

#[test]
fn chart_picker_toggles_and_caps() {
    let (_, nodes) = fixture(1, 5);
    let mut st = UiState::new(0.5, Alerts::default());
    key(&mut st, &nodes, &[], KeyCode::Char('m'));
    // cursor 0 = GPU, already shown: toggling removes it
    key(&mut st, &nodes, &[], KeyCode::Char(' '));
    assert!(!st.cluster_charts.contains(&sparktop::app::ChartKind::Gpu));
    for _ in 0..12 {
        key(&mut st, &nodes, &[], KeyCode::Down);
        key(&mut st, &nodes, &[], KeyCode::Char(' '));
    }
    assert!(st.cluster_charts.len() <= sparktop::app::MAX_CHARTS);
    // node page never offers net/disk (they have dedicated panels)
    st.overlay = Overlay::None;
    st.page = Page::Node;
    assert!(!st.chart_choices().contains(&sparktop::app::ChartKind::Net));
}

#[test]
fn down_node_is_flagged_not_cleared_by_others() {
    let (mut c, nodes) = fixture(2, 10);
    for _ in 0..3 {
        c.get_mut("spark-r1").unwrap().mark_err("ssh: connect to host 10.0.0.2 port 22: Operation timed out".into());
    }
    // another node's success must not clear spark-r1's error
    c.get_mut("spark-r0").unwrap().mark_ok(u64::MAX / 2);
    let mut st = UiState::new(0.5, Alerts::default());
    let out = render(&c, &nodes, &mut st, 140, 40);
    assert!(out.contains("✕"), "{out}");
    assert!(out.contains("timed out"));
}

#[test]
fn overlays_and_small_terminals() {
    let (mut c, nodes) = fixture(4, 30);
    for _ in 0..3 {
        c.get_mut("gx10-r1").unwrap().mark_err("ssh: connect to host gx10-r1 port 22: No route to host".into());
    }
    let mut st = UiState::new(0.5, Alerts::default());
    st.overlay = Overlay::Help;
    let out = render(&c, &nodes, &mut st, 100, 32);
    assert!(out.contains("node picker"));
    st.overlay = Overlay::Charts { cursor: 2 };
    let out = render(&c, &nodes, &mut st, 100, 32);
    assert!(out.contains("charts · cluster page") && out.contains("[1]"));
    st.overlay = Overlay::Nodes { cursor: 3 };
    let out = render(&c, &nodes, &mut st, 100, 32);
    assert!(out.contains("down"));
    st.overlay = Overlay::None;
    st.page = Page::Node;
    render(&c, &nodes, &mut st, 80, 24);
    st.page = Page::Cluster;
    let out = render(&c, &nodes, &mut st, 80, 24);
    assert!(out.contains("gx10-r1 down"), "footer names the down node");
}
