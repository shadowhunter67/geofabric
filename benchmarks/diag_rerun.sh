#!/bin/sh
# Diagnostic re-runs (NOT held-out): same cases, same harness, now, to separate model error from environment drift.
export PATH="/c/Users/<user>/AppData/Local/nvm/v24.16.0:$PATH"
PY=benchmarks/.venv/Scripts/python.exe
HO_CDP=1 HO_PREFIX=diagcal HO_CASES_FILE=browsercal-cases.json HO_TIMEOUT=120 $PY benchmarks/run_heldout.py LOCAL FAST_LAN
HO_CDP=1 HO_PREFIX=diagheld HO_CASES_FILE=browserheld-cases.json HO_TIMEOUT=120 $PY benchmarks/run_heldout.py LOCAL FAST_LAN
echo DIAGDONE
