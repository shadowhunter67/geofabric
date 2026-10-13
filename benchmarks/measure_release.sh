#!/usr/bin/env bash
# Release build of the benchmark binary; records wall time + free RAM floor (ADR-006).
LOG=${1:-benchmarks/results/release_build.log}
: > "$LOG"; : > "$LOG.mem"
( while true; do powershell -NoProfile -c "[int]((Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory/1024)" >> "$LOG.mem" 2>/dev/null; sleep 10; done ) &
S=$!
T0=$(date +%s)
CARGO_BUILD_JOBS=8 cargo build --release -p geo-bench >> "$LOG" 2>&1
RC=$?
T1=$(date +%s)
kill $S 2>/dev/null
echo "exit=$RC seconds=$((T1-T0))" >> "$LOG"
echo "min_free_mb=$(sort -n "$LOG.mem" | head -1) start_free_mb=$(head -1 "$LOG.mem")" >> "$LOG"
ls -l target/release/geo-bench.exe >> "$LOG" 2>&1
