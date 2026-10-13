#!/usr/bin/env bash
# Measures clean build time + min free RAM of the duckdb adapter crate (ADR-006).
# Usage: benchmarks/measure_build.sh <log-file>
LOG=${1:-benchmarks/results/duckdb_build.log}
: > "$LOG"
( while true; do
    powershell -NoProfile -c "[int]((Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory/1024)" >> "$LOG.mem" 2>/dev/null
    sleep 10
  done ) &
SAMP=$!
START=$(date +%s)
CARGO_BUILD_JOBS=8 cargo build -p geo-engine-duckdb >> "$LOG" 2>&1
RC=$?
END=$(date +%s)
kill $SAMP 2>/dev/null
echo "exit=$RC seconds=$((END-START))" >> "$LOG"
echo "min_free_mb=$(sort -n "$LOG.mem" | head -1) start_free_mb=$(head -1 "$LOG.mem")" >> "$LOG"
