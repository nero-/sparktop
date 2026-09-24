//! Timestamp-based rates and rolling, time-weighted display statistics.
use crate::sample::{Sample, VllmMetrics};
use std::collections::HashMap;

pub const HISTORY_CAP: usize = 1801; // at least 15 minutes at the fastest 0.5s poll

#[derive(Debug, Clone, Default)]
pub struct Derived {
    pub cpu_pct: f64,
    pub mem_used_pct: f64,
    pub load1: f64,
    pub per_core_pct: Vec<f64>,
    pub net_rx_bps: f64,
    pub net_tx_bps: f64,
    pub disk_read_bps: f64,
    pub disk_write_bps: f64,
    pub prompt_tps: f64,
    pub generation_tps: f64,
    pub prompt_smooth: f64,
    pub generation_smooth: f64,
    pub prompt_avg30: f64,
    pub generation_avg30: f64,
    pub token_interval_s: f64,
    pub ttft_p50: Option<f64>,
    pub ttft_p95: Option<f64>,
    pub tpot_p50: Option<f64>,
    pub tpot_p95: Option<f64>,
}

pub struct NodeHistory {
    pub samples: Vec<Sample>,
    pub derived: Vec<Derived>,
    prev_cpu: (u64, u64),
    prev_cores: Vec<(u64, u64)>,
    prev_net: HashMap<String, (u64, u64)>,
    prev_disk: HashMap<String, (u64, u64)>,
    prev_time: Option<u64>,
    prev_vllm: Option<(u64, VllmMetrics)>,
}

impl NodeHistory {
    pub fn new() -> Self {
        Self { samples: Vec::new(), derived: Vec::new(), prev_cpu: (0,0),
            prev_cores: Vec::new(), prev_net: HashMap::new(), prev_disk: HashMap::new(),
            prev_time: None, prev_vllm: None }
    }

    pub fn push(&mut self, s: Sample) {
        let d = self.derive(&s);
        self.samples.push(s);
        self.derived.push(d);
        if self.samples.len() > HISTORY_CAP {
            self.samples.remove(0); self.derived.remove(0);
        }
        let p5 = self.token_average(5.0, |d| d.prompt_tps);
        let g5 = self.token_average(5.0, |d| d.generation_tps);
        let p30 = self.token_average(30.0, |d| d.prompt_tps);
        let g30 = self.token_average(30.0, |d| d.generation_tps);
        let d = self.derived.last_mut().unwrap();
        d.prompt_smooth = if d.prompt_tps.is_finite() { p5 } else { f64::NAN };
        d.generation_smooth = if d.generation_tps.is_finite() { g5 } else { f64::NAN };
        d.prompt_avg30 = if d.prompt_tps.is_finite() { p30 } else { f64::NAN };
        d.generation_avg30 = if d.generation_tps.is_finite() { g30 } else { f64::NAN };
    }

    /// Weight only the portion of each valid measurement inside the window.
    pub fn token_average(&self, seconds: f64, value: impl Fn(&Derived)->f64) -> f64 {
        let Some(last) = self.samples.last() else { return f64::NAN };
        let end = last.rate_time_us() as f64 / 1e6;
        let start = end - seconds;
        let (mut tokens, mut duration) = (0.0, 0.0);
        for (s,d) in self.samples.iter().zip(&self.derived).rev() {
            let t = s.rate_time_us() as f64 / 1e6;
            if t <= start { break; }
            let rate = value(d);
            if !rate.is_finite() && d.token_interval_s > 0.0 { break; }
            let overlap = (t - (t-d.token_interval_s).max(start)).max(0.0);
            if rate.is_finite() && overlap > 0.0 {
                tokens += rate*overlap; duration += overlap;
            }
        }
        if duration > 0.0 { tokens/duration } else { f64::NAN }
    }

    fn derive(&mut self, s: &Sample) -> Derived {
        let mut d = Derived { prompt_tps: f64::NAN, generation_tps: f64::NAN,
            net_rx_bps: f64::NAN, net_tx_bps: f64::NAN,
            disk_read_bps: f64::NAN, disk_write_bps: f64::NAN, ..Default::default() };
        let now = s.rate_time_us();
        let dt = self.prev_time.and_then(|p| now.checked_sub(p)).filter(|t| *t > 0).map(|t| t as f64 / 1e6);
        d.mem_used_pct = if s.mem.total_kb > 0 { 100.0*s.mem.used_kb() as f64/s.mem.total_kb as f64 } else { 0.0 };
        d.load1 = s.cpu.load1;
        d.cpu_pct = cpu_percent(self.prev_cpu, s.cpu.all);
        d.per_core_pct = s.cpu.cores.iter().enumerate().map(|(i,c)|
            self.prev_cores.get(i).map(|p| cpu_percent(*p,*c)).unwrap_or(0.0)).collect();
        self.prev_cpu=s.cpu.all; self.prev_cores=s.cpu.cores.clone();
        let net: HashMap<_,_> = s.net.iter().filter(|(n,_)| *n!="lo" && !n.starts_with("docker") && !n.starts_with("veth"))
            .map(|(n,v)| (n.clone(),(v.rx_bytes,v.tx_bytes))).collect();
        let disk: HashMap<_,_> = s.disks.iter().filter(|(n,_)| whole_disk(n))
            .map(|(n,v)| (n.clone(),(v.read_bytes,v.write_bytes))).collect();
        if let Some(dt)=dt {
            d.net_rx_bps=device_rate(&self.prev_net,&net,0,dt);
            d.net_tx_bps=device_rate(&self.prev_net,&net,1,dt);
            d.disk_read_bps=device_rate(&self.prev_disk,&disk,0,dt);
            d.disk_write_bps=device_rate(&self.prev_disk,&disk,1,dt);
        }
        self.prev_net=net; self.prev_disk=disk; self.prev_time=Some(now);
        if let Some(v)=&s.vllm {
            if let Some((t,prev))=&self.prev_vllm {
                if let Some(delta)=now.checked_sub(*t).filter(|x| *x>0) {
                    let dt=delta as f64/1e6; d.token_interval_s=dt;
                    d.prompt_tps=counter_delta(prev,v,"vllm:prompt_tokens_total").map(|x| x/dt).unwrap_or(f64::NAN);
                    d.generation_tps=counter_delta(prev,v,"vllm:generation_tokens_total").map(|x| x/dt).unwrap_or(f64::NAN);
                    if counter_delta(prev,v,"vllm:time_to_first_token_seconds_bucket").is_some() {
                        if let Some((p50,p95))=histogram_pcts(&prev.ttft_buckets,&v.ttft_buckets) { d.ttft_p50=p50;d.ttft_p95=p95; }
                    }
                    let metric=if v.raw.keys().any(|k| k.starts_with("vllm:inter_token_latency_seconds_bucket[")) {
                        "vllm:inter_token_latency_seconds_bucket"
                    } else { "vllm:time_per_output_token_seconds_bucket" };
                    if counter_delta(prev,v,metric).is_some() {
                        if let Some((p50,p95))=histogram_pcts(&prev.tpot_buckets,&v.tpot_buckets) { d.tpot_p50=p50;d.tpot_p95=p95; }
                    }
                }
            }
            self.prev_vllm=Some((now,v.clone()));
        }
        d
    }
    pub fn last(&self) -> Option<(&Sample,&Derived)> {
        self.samples.last().zip(self.derived.last())
    }
}

fn cpu_percent(prev:(u64,u64),cur:(u64,u64))->f64 {
    match (cur.0.checked_sub(prev.0),cur.1.checked_sub(prev.1)) {
        (Some(idle),Some(total)) if prev.1>0 && total>0 => 100.0*(1.0-idle.min(total) as f64/total as f64),
        _=>0.0
    }
}
fn whole_disk(n:&str)->bool {
    if ["loop","ram","zram"].iter().any(|p| n.starts_with(p)) { return false; }
    if let Some((_,suffix))=n.rsplit_once('p') {
        if !suffix.is_empty() && suffix.bytes().all(|b|b.is_ascii_digit()) { return false; }
    }
    !(["sd","vd","hd"].iter().any(|p|n.starts_with(p)) && n.ends_with(|c:char|c.is_ascii_digit()))
}
fn device_rate(prev:&HashMap<String,(u64,u64)>,cur:&HashMap<String,(u64,u64)>,direction:usize,dt:f64)->f64 {
    if prev.len()!=cur.len() || cur.is_empty() { return f64::NAN; }
    let mut sum=0.0;
    for (name,c) in cur {
        let Some(p)=prev.get(name) else { return f64::NAN };
        let (p,c)=if direction==0 {(p.0,c.0)} else {(p.1,c.1)};
        let Some(delta)=c.checked_sub(p) else {return f64::NAN}; sum+=delta as f64;
    }
    sum/dt
}
fn counter_delta(prev:&VllmMetrics,cur:&VllmMetrics,name:&str)->Option<f64> {
    let prefix=format!("{name}[");
    let p:HashMap<_,_>=prev.raw.iter().filter(|(k,_)|k.starts_with(&prefix)).collect();
    let c:HashMap<_,_>=cur.raw.iter().filter(|(k,_)|k.starts_with(&prefix)).collect();
    if c.is_empty() || p.len()!=c.len() {return None;}
    let mut delta=0.0;
    for (key,value) in c {
        let old=**p.get(key)?;
        if !value.is_finite() || *value<old {return None;}
        delta+=*value-old;
    }
    Some(delta)
}

fn histogram_pcts(prev:&[(f64,f64)],cur:&[(f64,f64)])->Option<(Option<f64>,Option<f64>)> {
    if prev.is_empty() || prev.len()!=cur.len() {return None;}
    let mut buckets=Vec::new();
    for (le,count) in cur {
        let old=prev.iter().find(|(p,_)|p==le)?.1;
        if *count<old {return None;}
        buckets.push((*le,count-old));
    }
    buckets.sort_by(|a,b|a.0.total_cmp(&b.0));
    let total=buckets.iter().find(|(le,_)|*le==f64::INFINITY)?.1;
    if total<=0.0 || buckets.windows(2).any(|w|w[1].1<w[0].1) {return None;}
    let quantile=|q:f64| {
        let target=total*q; let(mut last_le,mut last_count)=(0.0,0.0);
        for (le,count) in &buckets {
            if *count>=target {
                if le.is_infinite() {return if last_le>0.0 {Some(last_le)} else {None};}
                if *count<=last_count {return Some(*le);}
                return Some(last_le+(le-last_le)*(target-last_count)/(count-last_count));
            }
            last_le=*le;last_count=*count;
        }
        None
    };
    Some((quantile(0.5),quantile(0.95)))
}
pub type Cluster=HashMap<String,NodeHistory>;
