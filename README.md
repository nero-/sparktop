# sparktop

Terminal resource monitor for DGX Spark clusters (btop/vllm-top style).

## Build & run
```sh
cargo build --release
./target/release/sparktop                 # uses ./sparktop.toml
./target/release/sparktop --config path/to/sparktop.toml
./target/release/sparktop --interval 0.5  # 0.5-10s
./target/release/sparktop --once          # one text snapshot
```

## Config
```toml
interval = 1.0

[[nodes]]
name = "hostname"
host = "host_ip"
user = "user"
# vllm_url = "http://localhost:8000"   # scrape vLLM /metrics from the node
```
Add nodes by appending `[[nodes]]` blocks. Polling is agentless SSH — nothing
is installed on the monitored machines (the SSH key must be authorized).

## Keys
`1/2/3` cluster · node · vLLM pages — `tab` switch node — `space` pause —
`t` cycle theme (gruvbox/catppuccin/tokyonight) — `q` quit.

## What it shows
- Cluster overview: per-node hero strip (GPU%, unified-mem%, load, net) with
  live GPU and network graphs
- Node detail: CPU graph + btop-style per-core rows, unified memory (LPDDR5X
  pool), GPU util/power/temp/clocks, network graph + per-iface totals, disk
  I/O, GPU compute processes
- vLLM: prefill/decode tok/s graphs, TTFT/TPOT p50+p95, running/queue,
  GPU/CPU KV-cache pools, peaks (set `vllm_url` per node)

## Acknowledgments
Graph renderer, theme system, and panel conventions adapted from
[vllm-top](https://github.com/mratsim/vllm-top) (GPLv3, © mratsim); layout
inspiration from [btop](https://github.com/aristocratos/btop).
This project is GPLv3 as a result.

## Rate display

The vLLM page shows a time-weighted **5-second mean** for smooth live graphs,
a **30-second rolling average**, and the unsmoothed **raw** rate from the latest
counter interval. Rates divide token/byte counter changes by actual monotonic
elapsed time, including delayed SSH polls. Idle intervals count toward averages;
startup uses the available history. Missing metrics show unavailable, and counter
resets or engine-series changes begin a new averaging segment.

Decode is aggregate output across requests at the selected API endpoint. Engine
series are summed; tensor-parallel GPU ranks must not be counted again. This is
not the same window as a benchmark's per-request generation average. Prompt
counter throughput includes idle time and is not a direct cold-prefill benchmark.
Peaks refer to raw samples retained in history, not process lifetime. Latency
percentiles are interpolated from cumulative histogram changes between scrapes;
KV occupancy is the busiest engine's value.

Run regression checks with `cargo test --offline`.
