#!/bin/sh
# FINAL held-out: seed 20261026, data draw _s6, model/policy/estimator frozen at commit e909764. Run ONCE.
export PATH="/c/Users/<user>/AppData/Local/nvm/v24.16.0:$PATH"
PY=benchmarks/.venv/Scripts/python.exe
HO_CDP=1 HO_PREFIX=final HO_CASES_FILE=final-cases.json HO_TIMEOUT=120 $PY benchmarks/run_heldout.py LOCAL FAST_LAN GOOD_BROADBAND MOBILE HIGH_LATENCY
echo FINALDONE
