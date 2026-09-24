#!/bin/bash
# Fake ssh for TUI testing: serves canned node output regardless of target.
# Usage: put an exec named `ssh` from this script first on PATH, run sparktop.
# Node name is derived from the first non-option ssh arg; canned output varies
# per fake host so multi-node pages are distinguishable.
host=""
prev=""
for a in "$@"; do
  case "$prev" in -o|-l|-p|-i|-F|-J) prev=""; continue ;; esac
  case "$a" in -*) prev="$a"; continue ;; esac
  host="${a##*@}"; break
done
[ -n "$host" ] || host=unknown
case "$host" in
  spark-r0|gx10-r0) VLLM=1; LOAD="0.42, 0.38, 0.31"; UPL=123456 ;;
  spark-r1)         VLLM=0; LOAD="0.10, 0.09, 0.08"; UPL=234567 ;;
  *)                VLLM=0; LOAD="0.05, 0.05, 0.05"; UPL=345678 ;;
esac
T=$(date +%s)
cat <<EOF
===HOSTNAME===
$host
===UPTIME_S===
$UPL
 1 users, load average: $LOAD
===CPU===
cpu  1000 0 500 8000 100 0 0 0 0 0
cpu0 100 0 50 800 10 0 0 0 0 0
cpu1 100 0 50 800 10 0 0 0 0 0
cpu2 100 0 50 800 10 0 0 0 0 0
===CPUFREQ===
3200
3200
3200
===MEM===
MemTotal:       132000000 kB
MemAvailable:   99000000 kB
Buffers:         2000000 kB
Cached:         18000000 kB
===GPU===
0, NVIDIA GB10, 45, 64, 55, 1200
===GPUPROC===
1234, python3, 61000
===NET===
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 1000000    8000    0    0    0     0          0         0  1000000    8000    0    0    0     0       0          0
  eth0: 2000000000 3000000    0    0    0     0          0         0 1000000000 2000000    0    0    0     0       0          0
===DISK===
   8       0 nvme0n1 1000 0 200000 500 800 0 160000 700 0 1200 0 0 0 0
   8       1 nvme0n1p1 900 0 180000 450 700 0 140000 600 0 1000 0 0 0 0
EOF
if [ "$VLLM" = 1 ]; then
  cat <<EOF
===VLLM===
vllm:prompt_tokens_total $((T * 7 % 100000))
vllm:generation_tokens_total $((T * 11 % 100000))
vllm:num_requests_running 2
vllm:num_requests_waiting 1
vllm:kv_cache_usage_perc 0.42
vllm:time_to_first_token_seconds_bucket{le="0.1",model_name="m"} 10
vllm:time_to_first_token_seconds_bucket{le="+Inf",model_name="m"} 12
vllm:inter_token_latency_seconds_bucket{le="0.05",model_name="m"} 500
vllm:inter_token_latency_seconds_bucket{le="+Inf",model_name="m"} 510
EOF
fi
echo "===END==="
