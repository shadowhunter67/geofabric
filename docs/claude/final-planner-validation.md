# Final planner validation (2026-10-19/20)

Báo cáo: `REPORT-GEOFABRIC-FINAL-PLANNER-VALIDATION.md`. ADR-008 (Accepted, scoped), ADR-010 (Proposed + addendum).

## Quy trình đã dùng (làm lại được)
1. Đổi hợp đồng thời gian (trong trang) → `web/e2e/heldout.spec.ts` ghi `ms` (trong trang), `outer_ms` (cũ), `reference` (probe chuẩn mỗi lô dataset).
2. Calibrate lại: `benchmarks/recalibrate.sh` (prefix `ccal`, cases `browsercal-cases.json`) → `replan.py ccal …` → `fit_browser_model.py results/ccal-table.json > browser-model-fit-v2.json` → hằng số vào `browser_model.rs`. Server: fit LOCAL features (`ccal-table.json`: `server_ms`, `sel_geom_bytes`) → `server_ms_per_geom_mb` / `server_ms_per_attr_mb`.
3. Đóng băng bằng commit (`e909764`) TRƯỚC khi sinh dữ liệu mới: `benchmarks/gen_final_data.sh` (seed 20261026, `_s6`, gồm 300mb/500mb/1gb), cases `make_final_cases.py` → `results/final-cases.json`.
4. Chạy MỘT lần: `benchmarks/run_final.sh` (prefix `final`, HO_CDP=1) → `python benchmarks/analyze_final.py --prefix final` (model-fastest vs policy-selected vs actual-fastest, R5c, penalty tuyệt đối, probe tham chiếu).
5. Chẩn đoán lệch LOCAL/FAST_LAN: `benchmarks/diag_rerun.sh` (chạy lại ca cũ, KHÔNG phải held-out).
6. Thiết bị: `benchmarks/run_devicecheck.mjs chrome msedge firefox webkit` (từ `web/`; WebKit cần `WK_EXE`).
7. Flake: `benchmarks/flake_run_all.sh` (14 Chrome + 6 Edge, release gateway, đủ spec kể cả data-pack).

## Số cần nhớ
- Browser model: calibration (Rust) median 1.07× / P90 1.67×; held-out cuối 1.23× / 2.66× (≤128 MiB: 1.16× / 2.09×). LOCAL 1.78×, FAST_LAN 1.41× (common-mode: server cũng lệch ~1.6×).
- Server model held-out: 1.40× / 2.02×.
- Planner: 95.0%, regret P90 1.00, max 1.59 (75 ms). R5c: 8/45 ca lớn kích hoạt, 0 lần chọn browser.
- Chưa làm: thiết bị vật lý thứ hai; số hạng decode theo MB cho range mode (cần calibrate + held-out mới).
