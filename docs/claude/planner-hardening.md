# Planner hardening (2026-10-14) — ghi chú làm việc

Báo cáo: `REPORT-GEOFABRIC-PLANNER-HARDENING.md`. ADR-009 (probe), ADR-010 (confidence/health/network), RFC-004 (lowering chung, chỉ đề xuất).

## Thành phần mới
- `crates/geo-planner/src/{failure,probe,network,estimator,confidence,state}.rs`; `Planner::plan_with_inputs(.., &PlanInputs)`.
- Gateway: `/api/probe`, `/api/report`, `/api/state`, `/api/state/reset` (chỉ dev/test), `/api/telemetry`, `/api/stats`, `/api/network`; cờ `--network`, `--simulate-network`, `--telemetry`, `--no-learn`; template `kind: "count"`; `network`, `client:{warm}`, `browser_limit_mb` theo request.
- Web: `engine/probe.ts` (+`probe-model.ts` thuần), `engine/failure.ts`; panel "Execution Plan" có Confidence/Health/Probe/Network/Risk/Fallback.

## Bài học (đừng lặp lại)
- DuckDB-Wasm mặc định TẢI CẢ FILE bằng 1 GET (không Range). Phải `db.open({filesystem:{reliableHeadRequests:true, allowFullHTTPReads:false}})`. Mọi số đo trình duyệt trước 2026-10-14 là số đo tải cả file.
- Range request của DuckDB-Wasm tuần tự, ~14 ms/request trên loopback → file nhỏ (≤32 MB) tải cả file nhanh hơn (planner chọn `full`/`range`).
- Cột `GEOMETRY('OGC:CRS84')` không stream được từ DuckDB-Wasm (`ActiveTransaction…`) → `ST_AsWKB` ngay ở scan (sqlgen.ts).
- DuckDB cache metadata Parquet theo TÊN file: tái dùng `gf_src_0.parquet` cho file khác → "No magic bytes". Mỗi lần execute dùng tên mới.
- Mô phỏng mạng phải tính bytes từ `Content-Length` (body stream của ServeFile không có `size_hint`). Một bản mô phỏng sai đã làm hỏng cả một lượt đo (đã bỏ, `results/invalid-sim/` local).
- Đo benchmark: KHÔNG chạy việc khác song song (curl/cargo làm thời gian server nhân đôi); kiểm cổng 8810 trống trước khi chạy `run_heldout.py` (gateway cũ chiếm cổng → LOCAL đo nhầm).
- Held-out: calibration = `calibration-set.jsonl` (lưới crossover run5) + cal2; held-out = `heldout-cases.json` (seed 20261015). Không chỉnh model theo held-out. Bản held-out thăm dò đầu (seed 20261014) nằm ở `exploratory-ho1/`, không dùng để báo cáo.
- Chạy: `python benchmarks/run_heldout.py LOCAL FAST_LAN GOOD_BROADBAND MOBILE HIGH_LATENCY` (env `HO_SAFE_MB`, `HO_PREFIX`, `HO_CASES_FILE`), rồi `analyze_heldout.py --uncalibrated <gateway không telemetry>` và `make_report_tables.py`.
