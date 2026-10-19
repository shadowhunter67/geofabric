# Pre-PostGIS gate (2026-10-19)

Báo cáo đầy đủ: `REPORT-GEOFABRIC-PRE-POSTGIS-GATE.md`. ADR: 010 (addendum round III), 011 (metric), 012 (render sampling); RFC-001 (SpatialSemiJoin, Transform).

## Quy trình đo đã dùng (làm lại được)
1. Calibration: `benchmarks/make_browsercal_cases.py` (seed 20261020) → `run_heldout.py` với `HO_CDP=1 HO_PREFIX=bcal` → `browsercal_table.py`/`replan.py` → `fit_browser_model.py` (NNLS từng thành phần) → hằng số vào `browser_model.rs`.
2. Held-out MỘT lần: `HO_SEED=20261019 HO_SUFFIXES=_s5 python make_browsercal_cases.py > results/browserheld-cases.json`, data `gen_synthetic.py --seed 20261019 --suffix _s5`, `HO_CDP=1 HO_PREFIX=bheld HO_CASES_FILE=browserheld-cases.json python run_heldout.py LOCAL FAST_LAN GOOD_BROADBAND MOBILE HIGH_LATENCY`, rồi `replan.py bheld ...` + `analyze_heldout.py --prefix bheld`.
3. Ngoài khoảng calibration (đo SAU held-out, chỉ để quyết R5c): `buildings-cases.json`, `gb-cases.json` → `bbuild-LOCAL.json`, `bgb-LOCAL.json`.
4. Thiết bị: `benchmarks/run_devicecheck.mjs chrome msedge webkit` (chạy từ `web/`, copy vào `web/` vì import `@playwright/test`).
5. Flake: `benchmarks/flake_run_all.sh` (14 Chrome + 6 Edge, release gateway).

## Kết quả cần nhớ
- Held-out: median 1.19×, P90 2.28×, P95 2.54×, max 10.2×; planner 96.3% (always-server 82.3%), regret P90 1.00, max 4.63.
- Thiên lệch LOCAL/FAST_LAN ~1.7× (chưa giải thích; plan_ms chậm 57% so với lúc calibrate → có thể do trạng thái máy).
- Oracle Demo B: 27,624 = 27,624, hash id khớp; 3/1,955 cặp gần ngưỡng khác geodesic.
- 8 digest lệch ở held-out = tie-break `count_by` (đã sửa, 8/8 khớp khi chạy lại).
- Perf release z14: 2.24 → 1.78 s tới khi vẽ (mẫu đại diện 12k feature thay vì 20k đầu file).
