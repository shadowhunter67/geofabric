#!/bin/sh
# 20 combined iterations: 14 Chrome + 6 Edge (second Chromium-family browser on the same machine). Release gateway, started by Playwright.
export PATH="/c/Users/<user>/AppData/Local/nvm/v24.16.0:$PATH"
export GATEWAY_BIN='target\release\geo-gateway.exe'
export PYTHONIOENCODING=utf-8
SPECS="smoke explorer engines planner map sampling"
PY=benchmarks/.venv/Scripts/python.exe
$PY benchmarks/flake_loop.py --runs 14 --channel chrome --specs $SPECS --out benchmarks/results/flake-chrome.json
$PY benchmarks/flake_loop.py --runs 6 --channel msedge --specs $SPECS --out benchmarks/results/flake-msedge.json
echo ALLDONE
