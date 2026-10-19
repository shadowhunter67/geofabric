#!/bin/sh
export PATH="/c/Users/<user>/AppData/Local/nvm/v24.16.0:$PATH"
PY=benchmarks/.venv/Scripts/python.exe
HO_CDP=1 HO_PREFIX=ccal HO_CASES_FILE=browsercal-cases.json HO_TIMEOUT=120 $PY benchmarks/run_heldout.py LOCAL FAST_LAN GOOD_BROADBAND MOBILE HIGH_LATENCY
echo RECALDONE
