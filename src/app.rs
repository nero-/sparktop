//! UI state and input handling, independent of the terminal so it can be
//! driven from tests. `main` owns the runtime (pollers, config reload) and
//! mirrors `paused` / `interval` from here into the poll threads.
use crate::config::{Alerts, NodeConfig};
use crate::theme::THEMES;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Page {
    Cluster,
    Node,
    Vllm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClusterView {
    /// one boxed band per node: hero strip + charts
    Panels,
    /// one row per node with sparklines — scales to many nodes
    Table,
    /// one chart per metric with a line per node
    Compare,
}

impl ClusterView {
    pub fn next(self) -> Self {
        match self {
            ClusterView::Panels => ClusterView::Table,
            ClusterView::Table => ClusterView::Compare,
            ClusterView::Compare => ClusterView::Panels,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            ClusterView::Panels => "panels",
            ClusterView::Table => "table",
            ClusterView::Compare => "compare",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChartKind {
    Gpu,
    Mem,
    Cpu,
    Net,
    Disk,
    Load,
    Power,
    Temp,
    Decode,
    Kv,
}

impl ChartKind {
    pub const ALL: [ChartKind; 10] = [
        ChartKind::Gpu,
        ChartKind::Mem,
        ChartKind::Cpu,
        ChartKind::Net,
        ChartKind::Disk,
        ChartKind::Load,
        ChartKind::Power,
        ChartKind::Temp,
        ChartKind::Decode,
        ChartKind::Kv,
    ];

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|k| *k == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

/// Default chart sets per page — single source of truth for draw fallbacks,
/// `c` reset, and the `e` seed when the selection is empty.
pub const DEFAULTS_CLUSTER: [ChartKind; 3] = [ChartKind::Gpu, ChartKind::Mem, ChartKind::Net];
pub const DEFAULTS_NODE: [ChartKind; 3] = [ChartKind::Cpu, ChartKind::Mem, ChartKind::Gpu];
pub const MAX_CHARTS: usize = 6;

pub fn defaults_for(page: Page) -> &'static [ChartKind] {
    match page {
        Page::Cluster => &DEFAULTS_CLUSTER,
        Page::Node => &DEFAULTS_NODE,
        Page::Vllm => &[],
    }
}

/// Kinds the node page never offers in its chart row: it always renders
/// dedicated network and disk panels below.
fn skipped_for(page: Page) -> &'static [ChartKind] {
    match page {
        Page::Node => &[ChartKind::Net, ChartKind::Disk],
        _ => &[],
    }
}

// ── timescale windows ────────────────────────────────────────────────
pub const WINDOWS: [f64; 3] = [60.0, 300.0, 900.0];
pub const WINDOW_LABELS: [&str; 3] = ["60s", "5m", "15m"];
pub const INTERVALS: [f64; 7] = [0.5, 1.0, 2.0, 3.0, 5.0, 7.5, 10.0];

pub fn window_label(secs: f64) -> &'static str {
    WINDOWS.iter().position(|w| *w == secs).map(|i| WINDOW_LABELS[i]).unwrap_or("?")
}

/// What the static parts of a node look like to the UI.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeInfo {
    pub name: String,
    pub host: String,
    pub group: String,
    pub has_vllm: bool,
}

impl From<&NodeConfig> for NodeInfo {
    fn from(n: &NodeConfig) -> Self {
        Self { name: n.name.clone(), host: n.host.clone(), group: n.group().to_string(), has_vllm: n.vllm_url.is_some() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Overlay {
    None,
    Help,
    Nodes { cursor: usize },
    Charts { cursor: usize },
}

/// Clickable regions recorded during the last draw.
#[derive(Debug, Clone, PartialEq)]
pub enum Hit {
    Node(String),
    Page(Page),
}

pub struct UiState {
    pub page: Page,
    /// focused node by name, so focus survives filters and config reloads
    pub focused: Option<String>,
    pub view: ClusterView,
    pub window_idx: usize,
    pub cluster_charts: Vec<ChartKind>,
    /// empty = defaults
    pub node_charts: Vec<ChartKind>,
    pub overlay: Overlay,
    pub status: Option<(String, Instant)>,
    /// cluster/group filter; None shows every group
    pub group: Option<String>,
    pub hidden: BTreeSet<String>,
    pub paused: bool,
    pub interval: f64,
    pub gradient: bool,
    pub alerts: Alerts,
    pub hits: Vec<(Rect, Hit)>,
    pub quit: bool,
    /// first row of the cluster table when it has more nodes than rows
    pub table_scroll: usize,
}

impl UiState {
    pub fn new(interval: f64, alerts: Alerts) -> Self {
        Self {
            page: Page::Cluster,
            focused: None,
            view: ClusterView::Panels,
            window_idx: 0,
            cluster_charts: DEFAULTS_CLUSTER.to_vec(),
            node_charts: Vec::new(),
            overlay: Overlay::None,
            status: None,
            group: None,
            hidden: BTreeSet::new(),
            paused: false,
            interval,
            gradient: true,
            alerts,
            hits: Vec::new(),
            quit: false,
            table_scroll: 0,
        }
    }

    /// Interval used for staleness: infinite while paused, so frozen data
    /// isn't flagged stale just because polling was stopped on purpose.
    pub fn health_interval(&self) -> f64 {
        if self.paused { f64::INFINITY } else { self.interval }
    }

    pub fn window_secs(&self) -> f64 {
        WINDOWS[self.window_idx % WINDOWS.len()]
    }

    pub fn flash(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
    }

    /// Transient message still worth showing (2.5s lifetime).
    pub fn live_status(&self) -> Option<&str> {
        self.status
            .as_ref()
            .filter(|(_, at)| at.elapsed() < Duration::from_millis(2500))
            .map(|(m, _)| m.as_str())
    }

    pub fn in_group(&self, n: &NodeInfo) -> bool {
        self.group.as_ref().is_none_or(|g| &n.group == g)
    }

    /// Nodes on screen: in the selected group and not hidden, config order.
    pub fn visible<'a>(&self, nodes: &'a [NodeInfo]) -> Vec<&'a NodeInfo> {
        nodes.iter().filter(|n| self.in_group(n) && !self.hidden.contains(&n.name)).collect()
    }

    /// The focused node, falling back to the first visible one.
    pub fn focused_node<'a>(&self, nodes: &'a [NodeInfo]) -> Option<&'a NodeInfo> {
        let vis = self.visible(nodes);
        self.focused
            .as_ref()
            .and_then(|f| vis.iter().find(|n| &n.name == f).copied())
            .or_else(|| {
                // the vLLM page prefers a node that actually serves metrics
                if self.page == Page::Vllm {
                    vis.iter().find(|n| n.has_vllm).copied().or(vis.first().copied())
                } else {
                    vis.first().copied()
                }
            })
    }

    /// Move focus through visible nodes; the vLLM page skips nodes without
    /// an endpoint when at least one has one.
    pub fn step(&mut self, nodes: &[NodeInfo], dir: isize) {
        let vis = self.visible(nodes);
        let pool: Vec<&NodeInfo> = if self.page == Page::Vllm && vis.iter().any(|n| n.has_vllm) {
            vis.into_iter().filter(|n| n.has_vllm).collect()
        } else {
            vis
        };
        if pool.is_empty() {
            return;
        }
        let cur = self
            .focused_node(nodes)
            .and_then(|f| pool.iter().position(|n| n.name == f.name));
        let len = pool.len() as isize;
        let next = match cur {
            Some(i) => (i as isize + dir).rem_euclid(len) as usize,
            None => 0,
        };
        self.focused = Some(pool[next].name.clone());
    }

    fn charts_mut(&mut self) -> Option<&mut Vec<ChartKind>> {
        match self.page {
            Page::Cluster => Some(&mut self.cluster_charts),
            Page::Node => Some(&mut self.node_charts),
            Page::Vllm => None,
        }
    }

    /// Charts the current page actually shows (defaults when unset).
    pub fn page_charts(&self) -> Vec<ChartKind> {
        let c = match self.page {
            Page::Cluster => &self.cluster_charts,
            Page::Node => &self.node_charts,
            Page::Vllm => return Vec::new(),
        };
        if c.is_empty() { defaults_for(self.page).to_vec() } else { c.clone() }
    }

    /// Kinds the chart picker offers on this page.
    pub fn chart_choices(&self) -> Vec<ChartKind> {
        let skip = skipped_for(self.page);
        ChartKind::ALL.iter().copied().filter(|k| !skip.contains(k)).collect()
    }

    fn toggle_chart(&mut self, kind: ChartKind) {
        let current = self.page_charts();
        let Some(charts) = self.charts_mut() else { return };
        if charts.is_empty() {
            *charts = current;
        }
        if let Some(i) = charts.iter().position(|k| *k == kind) {
            if charts.len() == 1 {
                self.flash("keep at least one chart");
                return;
            }
            charts.remove(i);
        } else if charts.len() >= MAX_CHARTS {
            self.flash(format!("page full — {MAX_CHARTS}/{MAX_CHARTS} charts"));
        } else {
            charts.push(kind);
        }
    }

    fn add_next_chart(&mut self) {
        let page = self.page;
        let skip = skipped_for(page);
        let Some(charts) = self.charts_mut() else {
            self.flash("chart keys apply to pages 1–2");
            return;
        };
        let avail = ChartKind::ALL.len() - skip.len();
        if charts.len() >= MAX_CHARTS.min(avail) {
            let n = charts.len();
            self.flash(format!("page full — {n}/{n} charts (r removes, m picks)"));
            return;
        }
        // seed from the page's first default when empty, so re-adding
        // rebuilds toward the defaults instead of an arbitrary kind
        let seed = charts.last().copied().or_else(|| defaults_for(page).first().copied()).unwrap_or(ChartKind::Gpu);
        let mut next = seed.next();
        while charts.contains(&next) || skip.contains(&next) {
            next = next.next();
        }
        charts.push(next);
        self.status = None;
    }

    fn remove_last_chart(&mut self) {
        let Some(charts) = self.charts_mut() else {
            self.flash("chart keys apply to pages 1–2");
            return;
        };
        if charts.pop().is_none() {
            self.flash("nothing to remove — page shows defaults");
        } else {
            self.status = None;
        }
    }

    fn reset_charts(&mut self) {
        match self.page {
            Page::Cluster => self.cluster_charts = DEFAULTS_CLUSTER.to_vec(),
            Page::Node => self.node_charts = Vec::new(),
            Page::Vllm => {}
        }
        self.flash("charts reset to defaults");
    }

    fn cycle_group(&mut self, groups: &[String]) {
        if groups.len() < 2 {
            self.flash("one cluster — tag nodes with cluster = \"name\" to group them");
            return;
        }
        self.group = match &self.group {
            None => Some(groups[0].clone()),
            Some(g) => groups.iter().position(|x| x == g).and_then(|i| groups.get(i + 1)).cloned(),
        };
        self.table_scroll = 0;
        self.flash(format!("cluster: {}", self.group.as_deref().unwrap_or("all")));
    }

    fn step_interval(&mut self, dir: isize) {
        let i = INTERVALS.iter().position(|v| *v >= self.interval - 1e-9).unwrap_or(0) as isize;
        let j = (i + dir).clamp(0, INTERVALS.len() as isize - 1) as usize;
        self.interval = INTERVALS[j];
        self.flash(format!("poll interval {}s", self.interval));
    }

    fn cycle_theme(&mut self, dir: isize) {
        let n = THEMES.len();
        let cur = crate::ui::THEME_IDX.load(Ordering::Relaxed) % n;
        let next = (cur as isize + dir).rem_euclid(n as isize) as usize;
        crate::ui::THEME_IDX.store(next, Ordering::Relaxed);
        self.flash(format!("theme: {}", THEMES[next].name));
    }

    pub fn on_key(&mut self, k: KeyEvent, nodes: &[NodeInfo], groups: &[String]) {
        use KeyCode::*;
        if k.modifiers.contains(KeyModifiers::CONTROL) && matches!(k.code, Char('c') | Char('q')) {
            self.quit = true;
            return;
        }
        match self.overlay.clone() {
            Overlay::None => {}
            Overlay::Help => {
                match k.code {
                    Esc | Char('?') | Enter => self.overlay = Overlay::None,
                    Char('q') => self.quit = true,
                    _ => {}
                }
                return;
            }
            Overlay::Nodes { cursor } => {
                let len = nodes.len();
                match k.code {
                    Esc | Char('n') => self.overlay = Overlay::None,
                    Char('q') => self.quit = true,
                    Up | Char('k') if len > 0 => {
                        self.overlay = Overlay::Nodes { cursor: (cursor + len - 1) % len }
                    }
                    Down | Char('j') if len > 0 => self.overlay = Overlay::Nodes { cursor: (cursor + 1) % len },
                    Char(' ') | Char('x') => {
                        if let Some(n) = nodes.get(cursor) {
                            if !self.hidden.remove(&n.name) {
                                self.hidden.insert(n.name.clone());
                            }
                        }
                    }
                    Char('a') => self.hidden.clear(),
                    Enter => {
                        if let Some(n) = nodes.get(cursor) {
                            self.hidden.remove(&n.name);
                            if !self.in_group(n) {
                                self.group = None;
                            }
                            self.focused = Some(n.name.clone());
                        }
                        self.overlay = Overlay::None;
                    }
                    _ => {}
                }
                return;
            }
            Overlay::Charts { cursor } => {
                let choices = self.chart_choices();
                let len = choices.len();
                match k.code {
                    Esc | Enter | Char('m') => self.overlay = Overlay::None,
                    Char('q') => self.quit = true,
                    Up | Char('k') => self.overlay = Overlay::Charts { cursor: (cursor + len - 1) % len },
                    Down | Char('j') => self.overlay = Overlay::Charts { cursor: (cursor + 1) % len },
                    Char(' ') | Char('x') => {
                        if let Some(kind) = choices.get(cursor) {
                            self.toggle_chart(*kind);
                        }
                    }
                    Char('c') => self.reset_charts(),
                    _ => {}
                }
                return;
            }
        }
        match k.code {
            Char('q') => self.quit = true,
            Esc if self.page != Page::Cluster => self.page = Page::Cluster,
            Esc => self.quit = true,
            Char(' ') => self.paused = !self.paused,
            Char('1') => self.page = Page::Cluster,
            Char('2') => self.page = Page::Node,
            Char('3') => self.page = Page::Vllm,
            Enter if self.page == Page::Cluster => {
                if let Some(n) = self.focused_node(nodes) {
                    self.focused = Some(n.name.clone());
                    self.page = Page::Node;
                }
            }
            Tab | Right | Down | Char('j') | Char('l') => self.step(nodes, 1),
            BackTab | Left | Up | Char('k') | Char('h') => self.step(nodes, -1),
            Char('t') => self.cycle_theme(1),
            Char('T') => self.cycle_theme(-1),
            Char('w') => {
                self.window_idx = (self.window_idx + 1) % WINDOWS.len();
                self.flash(format!("window {}", window_label(self.window_secs())));
            }
            Char('W') => {
                self.window_idx = (self.window_idx + WINDOWS.len() - 1) % WINDOWS.len();
                self.flash(format!("window {}", window_label(self.window_secs())));
            }
            Char('v') => {
                self.view = self.view.next();
                self.page = Page::Cluster;
                self.flash(format!("view: {}", self.view.label()));
            }
            Char('g') => self.cycle_group(groups),
            Char('n') => {
                let cursor = self
                    .focused_node(nodes)
                    .and_then(|f| nodes.iter().position(|n| n.name == f.name))
                    .unwrap_or(0);
                self.overlay = Overlay::Nodes { cursor };
            }
            Char('m') => {
                if self.page == Page::Vllm {
                    self.flash("chart picker applies to pages 1–2");
                } else {
                    self.overlay = Overlay::Charts { cursor: 0 };
                }
            }
            Char('e') => self.add_next_chart(),
            Char('r') => self.remove_last_chart(),
            Char('c') => self.reset_charts(),
            Char('+') | Char('=') => self.step_interval(1),
            Char('-') | Char('_') => self.step_interval(-1),
            Char('G') => {
                self.gradient = !self.gradient;
                self.flash(if self.gradient { "gradient fill on" } else { "gradient fill off" });
            }
            Char('?') => self.overlay = Overlay::Help,
            _ => {}
        }
    }

    pub fn on_mouse(&mut self, m: MouseEvent, nodes: &[NodeInfo]) {
        if self.overlay != Overlay::None {
            if let MouseEventKind::Down(_) = m.kind {
                self.overlay = Overlay::None;
            }
            return;
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let hit = self
                    .hits
                    .iter()
                    .find(|(r, _)| m.column >= r.x && m.column < r.x + r.width && m.row >= r.y && m.row < r.y + r.height)
                    .map(|(_, h)| h.clone());
                match hit {
                    Some(Hit::Page(p)) => self.page = p,
                    Some(Hit::Node(name)) => {
                        let already = self.focused_node(nodes).is_some_and(|n| n.name == name);
                        self.focused = Some(name);
                        // click the focused node again to drill into it
                        if already && self.page == Page::Cluster {
                            self.page = Page::Node;
                        }
                    }
                    None => {}
                }
            }
            MouseEventKind::ScrollDown => self.step(nodes, 1),
            MouseEventKind::ScrollUp => self.step(nodes, -1),
            _ => {}
        }
    }
}

// ── remembered preferences ───────────────────────────────────────────
/// UI choices restored on the next launch (`--fresh` ignores them).
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub theme: Option<String>,
    pub window_idx: Option<usize>,
    pub view: Option<ClusterView>,
    pub page: Option<Page>,
    pub focused: Option<String>,
    pub cluster_charts: Option<Vec<ChartKind>>,
    pub node_charts: Option<Vec<ChartKind>>,
    pub group: Option<String>,
    pub hidden: Vec<String>,
    pub gradient: Option<bool>,
}

pub fn prefs_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("sparktop").join("state.json"))
}

impl Prefs {
    pub fn load() -> Option<Self> {
        let text = std::fs::read_to_string(prefs_path()?).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let Some(p) = prefs_path() else { return Ok(()) };
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&p, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn capture(st: &UiState) -> Self {
        let theme = THEMES[crate::ui::THEME_IDX.load(Ordering::Relaxed) % THEMES.len()].name;
        Self {
            theme: Some(theme.to_string()),
            window_idx: Some(st.window_idx),
            view: Some(st.view),
            page: Some(st.page),
            focused: st.focused.clone(),
            cluster_charts: Some(st.cluster_charts.clone()),
            node_charts: Some(st.node_charts.clone()),
            group: st.group.clone(),
            hidden: st.hidden.iter().cloned().collect(),
            gradient: Some(st.gradient),
        }
    }

    /// Apply onto a fresh state; stale names (removed nodes/groups) are
    /// harmless — focus falls back and filters simply match nothing extra.
    pub fn apply(self, st: &mut UiState, groups: &[String]) {
        if let Some(i) = self.theme.as_deref().and_then(crate::theme::theme_index) {
            crate::ui::THEME_IDX.store(i, Ordering::Relaxed);
        }
        if let Some(w) = self.window_idx.filter(|w| *w < WINDOWS.len()) {
            st.window_idx = w;
        }
        if let Some(v) = self.view {
            st.view = v;
        }
        if let Some(p) = self.page {
            st.page = p;
        }
        st.focused = self.focused;
        if let Some(mut c) = self.cluster_charts.filter(|c| !c.is_empty()) {
            c.truncate(MAX_CHARTS);
            st.cluster_charts = c;
        }
        if let Some(mut c) = self.node_charts {
            c.retain(|k| !skipped_for(Page::Node).contains(k));
            c.truncate(MAX_CHARTS);
            st.node_charts = c;
        }
        st.group = self.group.filter(|g| groups.contains(g));
        st.hidden = self.hidden.into_iter().collect();
        if let Some(g) = self.gradient {
            st.gradient = g;
        }
    }
}
