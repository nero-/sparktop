# Changelog

## 0.2.0: clusters, views, polish

### Added
- `cluster = "…"` node tags, a `g` group filter, and `--cluster` on the command line
- Cluster summary strip: nodes up, average and max GPU, pooled memory, total power and the hottest node, decode, network
- Cluster views: **panels**, **table** (sparklines, plus the focused node's charts), **compare** (a line per node)
- Node picker (`n`) and chart picker (`m`); new chart kinds: power, temp, decode, KV cache
- Node page: per-core CPU grid with clocks, live per-interface rates, GPU processes sorted by memory
- vLLM page: model name, prefix-cache hit rate, finished req/s (30s), preemptions, KV-cache graph
- Per-node health (`● ◐ ✕ ○`) with colored borders, faded stale rows, and footer errors
- Live config reload: nodes are added, removed, or re-pointed without a restart
- Remembered UI preferences; `--fresh` ignores them
- Mouse support, `+`/`-` poll interval, `W`/`T` reverse cycling, `G` gradient toggle
- Gradient graph fills, overlay lines (network tx, disk write), ■ meters, Nord and Dracula themes
- `--once` polls all nodes in parallel; `--json` for scripts
- Render tests for every page and view, and a screenshot pipeline (`examples/screenshots.rs`)

### Fixed
- One node's successful poll cleared another node's error, so down nodes looked healthy
- `ctrl-c` reset the charts instead of quitting
- The terminal stayed in raw mode after a panic
- Uptime parsed as 0 when the node reported whole seconds
- Decode charts were squashed by the prefill scale; graphs are now limited to about five gridlines

## 0.1.0

- First release: cluster, node, and vLLM pages over agentless SSH
