//! SSH transport: one compound collection script per poll, multiplexed over
//! a ControlMaster connection for low overhead.
use crate::config::NodeConfig;
use crate::parse::parse_collection;
use crate::sample::Sample;
use anyhow::Context;
use std::time::{SystemTime, UNIX_EPOCH};

/// The single compound command run on each node every poll. Cheap; pure text.
pub const COLLECT_SCRIPT: &str = r#"
echo ===HOSTNAME===
hostname
echo ===UPTIME_S===
awk '{print $1}' /proc/uptime
uptime
echo ===CPU===
cat /proc/stat
echo ===CPUFREQ===
cat /sys/devices/system/cpu/cpu*/cpufreq/scaling_cur_freq 2>/dev/null | awk -F/ '{print "cpu"substr($5,4)": "($1/1000)" MHz"}'
echo ===MEM===
grep -E 'MemTotal|MemAvailable|Buffers|^Cached:' /proc/meminfo
echo ===GPU===
nvidia-smi --query-gpu=index,name,utilization.gpu,power.draw,temperature.gpu,clocks.sm --format=csv,noheader,nounits 2>/dev/null || true
echo ===GPUPROC===
nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv,noheader,nounits 2>/dev/null || true
echo ===NET===
cat /proc/net/dev
echo ===DISK===
cat /proc/diskstats
if [ -n "$SPARKTOP_VLLM_URL" ]; then
  echo ===VLLM===
  curl -sf --max-time 2 "$SPARKTOP_VLLM_URL/metrics" 2>/dev/null | grep -E '^(vllm:|HELP|TYPE)' | grep -vE '^#' || true
fi
echo ===END===
"#;

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub struct Poller {
    pub node: NodeConfig,
    last_err: Option<String>,
    consecutive_errs: u32,
    /// unix seconds of the last successful poll — drives the data-age indicator
    pub last_ok_s: Option<u64>,
}

impl Poller {
    pub fn new(node: NodeConfig) -> Self {
        Self { node, last_err: None, consecutive_errs: 0, last_ok_s: None }
    }

    /// Collect one sample. Uses a warm ssh ControlMaster when available,
    /// falling back to a direct ssh invocation. Blocking; run on its own thread.
    pub fn poll(&mut self) -> anyhow::Result<Sample> {
        let t = SystemTime::now().duration_since(UNIX_EPOCH)?.as_micros() as u64;
        let target = format!(
            "{}{}",
            self.node.user.as_deref().map(|u| format!("{u}@")).unwrap_or_default(),
            self.node.host
        );
        // -o ControlMaster=auto: first call creates master socket, later calls reuse it.
        // ServerAlive keeps masters alive across long TUI sessions.
        let mut cmd = std::process::Command::new("ssh");
        cmd.args([
            "-o", "BatchMode=yes",
            "-o", "ConnectTimeout=5",
            "-o", "ControlMaster=auto",
            "-o", "ControlPath=/tmp/sparktop-%u-ssh-%r@%h:%p",
            "-o", "ControlPersist=120",
            "-o", "ServerAliveInterval=15",
            target.as_str(),
        ]);
        // vLLM metrics are scraped from the node itself (localhost), so the
        // vllm_url in config only needs to be reachable from the node.
        // ssh does not forward arbitrary env vars — embed as a POSIX
        // env-assignment prefix on the remote command.
        let remote = if let Some(url) = &self.node.vllm_url {
            format!("SPARKTOP_VLLM_URL={} {}", shell_quote(url), COLLECT_SCRIPT)
        } else {
            COLLECT_SCRIPT.to_string()
        };
        cmd.arg(remote);
        let out = cmd.output().context("ssh spawn failed")?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            self.consecutive_errs += 1;
            self.last_err = Some(err.clone());
            anyhow::bail!("ssh {target}: {err}");
        }
        self.consecutive_errs = 0;
        self.last_err = None;
        self.last_ok_s = Some(t / 1_000_000);
        parse_collection(&String::from_utf8_lossy(&out.stdout), t)
    }

    pub fn status(&self) -> String {
        match &self.last_err {
            Some(e) if self.consecutive_errs > 2 => format!("DOWN ({e})"),
            Some(_) => "retrying".into(),
            None => "ok".into(),
        }
    }
}
