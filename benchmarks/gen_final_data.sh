#!/bin/sh
PY=.venv/Scripts/python.exe
for s in 10mb 30mb 100mb 300mb; do $PY gen_synthetic.py --size $s --out data --seed 20261026 --suffix _s6; done
$PY gen_synthetic.py --size 500mb --out data --seed 20261026 --suffix _s6 --only clustered_spatial uniform_shuffled
$PY gen_synthetic.py --size 1gb --out data --seed 20261026 --suffix _s6 --only clustered_spatial uniform_shuffled
echo GENDONE
