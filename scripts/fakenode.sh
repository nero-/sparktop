#!/bin/bash
# Fake node for smoke-testing sparktop without a real Spark.
# Usage: sparktop-fakenode.sh <port>
# Serves an SSH command-execution channel? No — simplest: serve the COLLECT
# output over HTTP for the parser test, and also support `nc`-style exec.
set -euo pipefail
PORT="${1:-2222}"

while true; do
  # minimal inetd-style server: responds to any TCP connection with
  # the same output the collect script would produce on a DGX Spark
  cat <<EOF | nc -l "$PORT" >/dev/null
===HOSTNAME===
spark-fake
===UPTIME_S===
123456.78
 1 users, load average: 0.42, 0.38, 0.31
===CPU===
cpu  1000 0 500 8000 100 0 0 0 0 0
cpu0 100 0 50 800 10 0 0 0 0 0
cpu1 100 0 50 800 10 0 0 0 0 0
cpu2 100 0 50 800 10 0 0 0 0 0
===CPUFREQ===
cpu0: 3200 MHz
===MEM===
MemTotal:       132000000 kB
MemAvailable:   99000000 kB
Buffers:         2000000 kB
Cached:         18000000 kB
===GPU===
0, NVIDIA GB10, 45, 64000, 127000, 90, 120, 55, 1200, 2730, 100, 80
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
===END===
EOF
done
