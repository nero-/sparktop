use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct NodeConfig {
    pub name: String,
    pub host: String,
    pub user: Option<String>,
    /// Optional vLLM/OpenAI-compatible server base URL for metrics scraping,
    /// e.g. "http://localhost:8000"
    #[serde(default)]
    pub vllm_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Config {
    pub nodes: Vec<NodeConfig>,
    /// Poll interval seconds (clamped 0.5..=10)
    #[serde(default = "default_interval")]
    pub interval: f64,
}

fn default_interval() -> f64 {
    1.0
}

impl Config {
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;
        let cfg: Config = toml::from_str(&text)?;
        if cfg.nodes.is_empty() {
            anyhow::bail!("config has no nodes");
        }
        if !(0.5..=10.0).contains(&cfg.interval) {
            anyhow::bail!("interval must be 0.5..=10 seconds");
        }
        Ok(cfg)
    }
}
