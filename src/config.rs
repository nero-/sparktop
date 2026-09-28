use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct NodeConfig {
    pub name: String,
    pub host: String,
    pub user: Option<String>,
    /// Optional vLLM/OpenAI-compatible server base URL for metrics scraping,
    /// e.g. "http://localhost:8000"
    #[serde(default)]
    pub vllm_url: Option<String>,
    /// Optional cluster/group tag — `g` filters the view to one group.
    /// Nodes without a tag belong to the "default" group.
    #[serde(default)]
    pub cluster: Option<String>,
}

impl NodeConfig {
    pub fn group(&self) -> &str {
        self.cluster.as_deref().unwrap_or("default")
    }
}

/// Thresholds that turn values and panel borders warn/bad.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Alerts {
    pub temp_warn_c: f64,
    pub temp_crit_c: f64,
    /// unified-memory fraction (0–1)
    pub mem_warn: f64,
    pub mem_crit: f64,
}

impl Default for Alerts {
    fn default() -> Self {
        Self { temp_warn_c: 75.0, temp_crit_c: 85.0, mem_warn: 0.90, mem_crit: 0.97 }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub nodes: Vec<NodeConfig>,
    /// Poll interval seconds (clamped 0.5..=10)
    #[serde(default = "default_interval")]
    pub interval: f64,
    /// Starting theme name (a theme picked with `t` is remembered instead)
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub alerts: Alerts,
}

fn default_interval() -> f64 {
    1.0
}

impl Config {
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let cfg: Config = toml::from_str(text)?;
        if cfg.nodes.is_empty() {
            anyhow::bail!("config has no nodes");
        }
        if !(0.5..=10.0).contains(&cfg.interval) {
            anyhow::bail!("interval must be 0.5..=10 seconds");
        }
        let mut seen = std::collections::HashSet::new();
        for n in &cfg.nodes {
            if !seen.insert(n.name.as_str()) {
                anyhow::bail!("duplicate node name {:?}", n.name);
            }
        }
        Ok(cfg)
    }

    /// Distinct group names in config order.
    pub fn groups(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for n in &self.nodes {
            if !out.iter().any(|g| g == n.group()) {
                out.push(n.group().to_string());
            }
        }
        out
    }
}
