#!/bin/bash
# Gate 11: DuckDB vs PostGIS, three windows. PostgreSQL is restarted before each window (cold database cache; the OS cache stays warm).
cd "$(dirname "$0")/.."
source /tmp/gf_xe_slice.sh
export PYTHONIOENCODING=utf-8
export GF_BENCH=1 GF_XE_IN="house,apartments"
run_window() { # name bbox skip
  python tools/postgis_portable.py stop >/dev/null 2>&1; python tools/postgis_portable.py start >/dev/null 2>&1
  export GF_XE_BBOX="$2" GF_XE_SKIP="$3" GF_BENCH_OUT="$(cygpath -w "$PWD/benchmarks/results/postgis/bench-$1.json")"
  echo "=== window $1 ($2) skip=[$3]"; xe | grep -E "first d=|panicked|test result"
}
run_window small  "106.690,10.770,106.700,10.780" ""
run_window medium "106.68,10.76,106.72,10.80"     "semi_join_intersects_admin"
run_window wide   "106.60,10.72,106.76,10.86"     "semi_join_intersects_admin"
echo BENCHDONE
