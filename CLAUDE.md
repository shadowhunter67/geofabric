# GEOFABRIC

Dự án kết nối và sử dụng các thành phần liên quan đến GIS (nguồn dữ liệu, dịch vụ bản đồ, xử lý không gian). Phạm vi cụ thể do user bổ sung dần — **đang ở giai đoạn dựng khung (2026-10-13)**, stack đã chọn (xem dưới); core MVP + hardening + multi-engine validation xong 2026-10-13; planner hardening xong 2026-10-14 (REPORT-GEOFABRIC-PLANNER-HARDENING.md, chờ review, KHÔNG thêm PostGIS trước review) (REPORT-GEOFABRIC-CORE-MVP.md, REPORT-GEOFABRIC-HARDENING.md); Principal Architect đã Conditional Approve MVP, hardening đề xuất APPROVE — chờ review, KHÔNG mở rộng feature (real data, engine 2, WASM, AI) khi chưa được duyệt.

## Rules bắt buộc

- Không đọc/in file `.env` (xem rules ở CLAUDE.md gốc workspace).
- Luật kiến trúc: đọc `CONSTITUTION.md` và `AGENTS.md` trước khi sửa code. Đổi IR/CRS/unit/planner/API = cần RFC/ADR.
- Cài thư viện/build nặng (DuckDB bundled, dataset lớn, benchmark GB) phải hỏi user trước và ghi vào đây.
- Dữ liệu GIS lớn (shapefile, GeoTIFF, GPKG, tile) để trong `data/` — đã gitignore, không commit.

## Stack & môi trường

- **Dev mặc định: DuckDB prebuilt đã kiểm sha256** (`python tools/duckdb_prebuilt.py`, rồi `export DUCKDB_LIB_DIR=...; export PATH="$DUCKDB_LIB_DIR:$PATH"`, `cargo ... --no-default-features`). Bundled (feature mặc định) chỉ cho CI/release và CHƯA chạy lại vòng này. Test Playwright cần `GATEWAY_BIN` (bản release) khi đo thời gian; trong cmd.exe dùng dấu `\` trong đường dẫn.
- Rust 1.99 workspace (`crates/*`): geo-types → geo-ir → geo-executor/source-geoparquet → planner → engine-duckdb → runtime → gateway (axum). Web: TypeScript + Vite + MapLibre (không React) trong `web/`.
- **Đã cài thêm (phase multi-engine, 2026-10-13)**: `tools/.cache/duckdb-1.5.6/lib` (libduckdb prebuilt, sha256 đã kiểm); npm `@duckdb/duckdb-wasm`, `@playwright/test` (dùng Chrome cài sẵn, KHÔNG tải browser); venv: `shapely` (chỉ cho oracle Overture); dữ liệu Overture 1.45 GB `benchmarks/data/overture_buildings_vn_south.parquet` (gitignored, ODbL/attribution Overture).
- **Đã cài (2026-10-13)**: `benchmarks/.venv` (pyarrow, duckdb 1.5.6, numpy — chỉ để sinh dữ liệu/đối chiếu, KHÔNG nằm trên execution path); `web/node_modules` (maplibre-gl 6, vite 8, typescript 7, @types/geojson); DuckDB `spatial` extension tải về `~/.duckdb/extensions/v1.5.6/` (lần đầu cần mạng ~1 phút). Crate `duckdb` bundled build 3–5 phút, `target/` rất lớn (xem ADR-006) → dùng `CARGO_BUILD_JOBS=3` khi đang làm việc, build/bench nặng chạy mức ưu tiên BelowNormal.
- Dữ liệu synthetic: `benchmarks/data/` (gitignored) sinh bằng `benchmarks/gen_synthetic.py --size 10mb|100mb|1gb` (seed 42, tái lập được). KHÔNG tải 10 GB / Overture khi chưa báo user.

## Lệnh thường dùng

```
cargo test --workspace                       # cần benchmarks/data (thiếu thì test tự skip)
python tools/arch_check.py                   # luật phụ thuộc (đồ thị cargo metadata)
cargo run -p geo-gateway -- --data benchmarks/data --port 8787   # API + web/dist
cd web && npm run build | npm run typecheck | npm run dev | npx playwright test   # smoke dùng Chrome cài sẵn, tự chạy gateway
python benchmarks/oracle.py --size 10mb ...   # kết quả kỳ vọng độc lập trước khi chạy geo-bench
cargo build --release -p geo-bench && target/release/geo-bench.exe --data benchmarks/data --sizes 10mb,100mb --reps 3
```

## Kiến trúc (tóm tắt)

- `geo-ir` = IR logic, KHÔNG biết DuckDB/SQL; SQL chỉ sinh trong `geo-engine-duckdb/src/sql.rs`. Planner chỉ biết engine qua trait `ExecutionEngine`. UI chỉ gọi `geo-gateway`; GeoJSON chỉ tạo ở gateway.
- Gateway không nhận SQL, chỉ IR hoặc template. Mọi nguồn qua `SourceAccessGuard.authorize` TRƯỚC khi đọc bất kỳ metadata (production code cấm `GeoParquetSource::open`, arch_check chặn). Query có timeout thật (DuckDB `interrupt()`, mặc định 30 s trong `geo_executor::limits`), client ngắt kết nối thì cancel.
- Metrics có provenance (`Metric<T>{value,source}`): `bytes_read` luôn Unknown; `row_groups_skipped`/`rows_scanned` là SourceEstimated. UI phải hiện badge, không hiện estimate như số đo.
- Benchmark chỉ hợp lệ khi qua oracle độc lập (`benchmarks/oracle.py`, pyarrow+numpy): chạy oracle trước, `geo-bench` fail nếu hash lệch.
- `docs/claude/` — lịch sử & quyết định.

## Bẫy đã biết (gotchas)

- DuckDB profiler `total_bytes_read` ĐẾM THIẾU rất nhiều và `operator_rows_scanned` không đáng tin → số liệu quét lấy từ phân tích stats Parquet (`analyze_plan`), DuckDB chỉ để so sánh tương đối.
- Xuất cột GEOMETRY native qua `query_arrow` của binding Rust bị lỗi → adapter dùng `ST_AsWKB` + tự gắn metadata GeoArrow/CRS từ IR.
- Ổ đĩa: `target/` từng lên 59,6 GB làm đầy ổ E: (đã `cargo clean`); dev profile `debug = 0` (có debug info thì link test bị LNK1140). Cân nhắc `CARGO_TARGET_DIR` ngoài repo. DuckDB prebuilt: `--no-default-features` + `DUCKDB_DOWNLOAD_LIB=1` (ADR-006), build 54 s / 747 MB nhưng chưa chạy full test.
- Heredoc dài có dấu nháy đơn làm tool Bash lỗi parse → ghi file bằng công cụ Write. Hook chặn lệnh xóa đệ quy (kể cả khi chuỗi đó chỉ nằm trong nội dung lệnh).
- MapLibre 6 cần `setWorkerUrl(import ...-worker.mjs?url)` khi dùng Vite, nếu không bản đồ trống ("Worker failed to load").
- Codex (openai-bridge) làm sai spec ở task geo-types (đổi CRS/Unit thành String) và sandbox không có mạng để chạy cargo → đã loại bỏ, tự viết lại; chỉ giao Codex việc nhỏ có thể kiểm chứng.

## Tài liệu chi tiết — đọc khi cần

| File | Đọc khi |
|---|---|
| [`CONSTITUTION.md`](CONSTITUTION.md), [`AGENTS.md`](AGENTS.md) | Luôn đọc trước khi sửa code |
| [`docs/vision.md`](docs/vision.md), `rfcs/RFC-001..003`, `adr/ADR-001..005` | Cần hiểu thiết kế IR/engine/planner |
| [`docs/claude/lich-su.md`](docs/claude/lich-su.md) | Cần tra lịch sử sửa lỗi / quyết định thiết kế |
| [`docs/claude/planner-hardening.md`](docs/claude/planner-hardening.md) | Sửa planner/probe/health/network hoặc engine trình duyệt; chạy held-out |
| [`docs/claude/real-data.md`](docs/claude/real-data.md) | Gói dữ liệu Việt Nam (Overture, DEM), catalog, demo A–E, bootstrap, gateway nhiều `--data` |

**Quy tắc duy trì:** mục mới/lịch sử ghi vào `docs/claude/`, KHÔNG nối dài CLAUDE.md.


## Process safety (mandatory, added 2026-10-16)

NEVER terminate processes globally by executable name.

Forbidden: `taskkill /IM node.exe`, `taskkill /IM python.exe`, `pkill node`, `killall python` (and any other name-based kill).

Only terminate processes this project started, by a verified PID or an owned process handle (`taskkill /F /PID <pid>`; check with `netstat -ano` / `tasklist` first, and check that a port is free before starting a server on it: a stale server on the same port silently serves every test, see `docs/claude/planner-hardening.md`).

Do not change the global Node installation, the nvm default, or any shared toolchain. If a specific version is needed, put it on PATH for the command only (here Node 24 at `C:/Users/<user>/AppData/Local/nvm/v24.16.0`; never `nvm use`).

## Cài đặt thêm (ghi theo rule #7)

- 2026-10-16: `npm install geotiff@3.0.5 --save-exact` trong `web/` (đọc COG trong trình duyệt, range request; chỉ để hiển thị DEM). Dữ liệu tải về nằm ở `data/local/` (không commit), gói Overture buildings ở `benchmarks/data/`.
- 2026-10-19: `pyproj` + `shapely` vào `benchmarks/.venv` (oracle độc lập `benchmarks/oracle_near.py`); WebKit dùng bản có sẵn `ms-playwright/webkit-2336` qua `WK_EXE` (không tải thêm); Firefox CHƯA cài (cần ~90 MB, hỏi user). Dữ liệu synthetic `_s5` (seed 20261019) sinh vào `benchmarks/data/` cho held-out mới — KHÔNG dùng lại làm calibration.

## Pre-PostGIS gate (2026-10-19) — bẫy mới

Chi tiết: `docs/claude/pre-postgis-gate.md`, báo cáo `REPORT-GEOFABRIC-PRE-POSTGIS-GATE.md`.
- Model browser = tổng thành phần (`crates/geo-planner/src/browser_model.rs`), ĐÃ ĐÓNG BĂNG (fit chỉ trên `bcal-*`). Held-out seed 20261019: median 1.19×, P90 2.28×, nhưng LOCAL/FAST_LAN thiên lệch ~1.7× và chưa có máy vật lý thứ hai → ADR-010 vẫn Proposed. Đo lại = seed mới + data draw mới, đừng refit trên held-out.
- R5c: nguồn > 128 MiB không bao giờ tự chọn browser (đo 1 GB: browser chậm 1.5–3× server dù model dự đoán thắng).
- Histogram sidecar v3 có `row_index` (nguồn ≤ 100k hàng): cardinality + bytes geometry chính xác cho polygon lớn (admin). Đổi histogram thì bump `HIST_VERSION`.
- `count_by` luôn `ORDER BY count DESC, <group key> ASC`; thiếu tie-break thì 2 engine cắt top-50 khác nhau → digest lệch.
- Đo thời gian Playwright PHẢI có `GATEWAY_BIN=target\release\geo-gateway.exe` (mặc định là bản debug chậm 2–3×). Harness không trả cả result qua `page.evaluate` (làm phồng thời gian browser).
- Khoảng cách mét = UTM zone của vùng truy vấn (ADR-011), từ chối khi sai số > 0.5%; không bao giờ đổi độ→mét.

## Final planner validation (2026-10-19/20) — bẫy mới

Chi tiết: `docs/claude/final-planner-validation.md`, báo cáo `REPORT-GEOFABRIC-FINAL-PLANNER-VALIDATION.md`, cách đo máy thứ hai: `docs/claude/second-device-check.md`.
- Hợp đồng thời gian: thời gian browser đo bằng MỘT đồng hồ trong trang (`performance.now()`), không tính round trip của driver Playwright. Model và harness cùng định nghĩa; đổi định nghĩa = phải calibrate lại + held-out mới.
- Planner đã đóng băng ở commit `e909764` (model browser refit trên `ccal-*`, server có số hạng byte geometry/attribute). Held-out cuối seed 20261026 (`_s6`, 200 ca): browser median 1.23×, P90 2.66×; accuracy 95%; regret P90 1.00 (max 1.59 = 75 ms); 0 unsafe. KHÔNG chỉnh model sau khi xem kết quả; muốn đo lại phải seed mới + data draw mới.
- ADR-008 = Accepted (scoped: DuckDB server ↔ DuckDB-Wasm, ≤128 MiB tự động, 1 máy). ADR-010 vẫn Proposed: THIẾU máy vật lý thứ hai (user trả lời "chưa có", 2026-10-20). Chỉ khi chạy `?devicecheck=1` trên laptop khác thì mới đóng được.
- Đã cài (2026-10-19): Firefox cho Playwright (`npx playwright install firefox`, ~90 MB, ms-playwright/firefox-1555) — chỉ dùng cho device matrix, không nằm trong suite. `benchmarks/run_devicecheck.mjs` phải copy vào `web/` mới import được `@playwright/test`.
- Test e2e có 3 race đã sửa (chờ trạng thái ổn định rồi mới click): map polygon selection, engines fallback (chờ probe xong trước khi inject lỗi), data-pack click road/place/ward (layer refresh 2 lần cách ~1 s). Mọi test click phải chờ `contextInfo()` không còn `loading` và ổn định.
- `count_by` có test hồi quy vĩnh viễn (`templates.rs::count_by_ties_are_cut_by_the_group_key…`) — đừng xoá.
- Tên dữ liệu synthetic mới: `_s5` (held-out trước), `_s6` (held-out cuối, gồm 300mb/500mb/1gb); `gen_synthetic.py --only` để sinh một phần.

## PostGIS engine (2026-10-20; xác minh live 2026-10-21)

Chi tiết: `docs/claude/postgis.md`, ADR-013 (Accepted), `docs/postgis.md`. Báo cáo `REPORT-GEOFABRIC-POSTGIS-ENGINE.md` + `REPORT-GEOFABRIC-POSTGIS-LIVE-VERIFICATION.md`.
- Engine họ thứ hai nằm dưới CÙNG chuỗi PlanGraph→Planner→EngineDescriptor→ExecutionEngine (không endpoint riêng). Tuỳ chọn: không cấu hình = không có engine, không lỗi. Cấu hình: `GEOFABRIC_POSTGIS_ENABLED/URL/SOURCES/POOL` (URL chứa mật khẩu: không bao giờ in/commit).
- ĐÃ chạy trên PostgreSQL 16.9 + PostGIS 3.6.2 portable thật (2026-10-21): parity chặt 5 bộ dữ liệu HCMC (buildings 3.85 M), 17 plan giống hệt DuckDB, Demo B 27,624 id = oracle độc lập, timeout/cancel/pool chứng minh phía server. Driver thật = cargo feature `live` (`postgis-live` ở runtime/gateway). CHƯA có cost model PostGIS (R5d giữ); đo thật cho thấy không engine nào thắng mọi workload (PostGIS thắng cửa sổ nhỏ có index, DuckDB thắng semi-join mét lớn ~10x).
- Trait `EngineDescriptor` có thêm 2 hàm mặc định: `source_residency`, `calibration` (engine `Unmeasured` không bao giờ thắng engine đã đo theo chi phí: policy R5d).
- Đã thêm dependency (cargo): `postgres 0.19` (chỉ khi feature `live`), `sqlparser 0.55` (dev-dep của geo-engine-postgis, kiểm cú pháp PostgreSQL). Binary PostgreSQL 16.9 (EnterpriseDB zip) + PostGIS 3.6.2 (osgeo bundle) tải 2026-10-21 qua `tools/postgis_portable.py`, ghim SHA-256 trong `tools/postgis.lock.json`, cài ở `%LOCALAPPDATA%\geofabric-runtime\postgres` (ASCII, NGOÀI repo; initdb hỏng với đường dẫn có dấu), lắng nghe 127.0.0.1:5433, không service/PATH/admin. Gỡ: `python tools/postgis_portable.py stop` rồi xoá thư mục đó. Dữ liệu nạp bằng `scripts/postgis/load_data.py`; kết quả bằng chứng ở `benchmarks/results/postgis/`.
- arch_check có luật cách ly họ engine (PostGIS ↔ DuckDB/browser, gateway không dùng driver).

## Public release + static browser demo (2026-10-21)

Chi tiết: `docs/claude/public-release.md`, ADR-014, `docs/deployment-profiles.md`, báo cáo `REPORT-GEOFABRIC-PUBLIC-RELEASE.md`.
- Profile **static browser** = cùng UI/PlanGraph/DuckDB-Wasm, chỉ thay biên HTTP: `web/src/static/{profile,ir,gateway}.ts` tự trả `/api/*` trong trang (build `--mode static`, `GF_BASE=/geofabric/`). Không có Gateway/DuckDB server/PostGIS; thứ cần server (near, join, intersects) bị TỪ CHỐI "requires GeoFabric Server", không xấp xỉ.
- Gói dữ liệu demo công khai `data/public-demo/` (26 MB, ODbL derived, cắt bằng `data/scripts/make_public_demo.py`); KHÔNG đọc Overture S3 trực tiếp (bucket chỉ giữ 2 release mới nhất + app cần schema phẳng). Trang ghi công: `web/public/attribution.html` (sinh bởi `data/scripts/make_attribution_page.py`).
- Playwright static: `web/playwright.static.config.ts`, spec ở `web/e2e-static/` (PUBLIC_URL=... chạy thẳng trên site thật). Build base path: `GF_BASE` + `MSYS_NO_PATHCONV=1` trong Git Bash.
- Giấy phép mã nguồn: MIT OR Apache-2.0 (user chọn 2026-10-21). Lịch sử công khai viết lại sang noreply (user chọn); bản local gốc giữ ở nhánh sao lưu.
- ĐÃ CÔNG KHAI (2026-10-22): repo https://github.com/shadowhunter67/geofabric + demo https://shadowhunter67.github.io/geofabric/ . GitHub Pages: HEAD+Range trả 200 nên DuckDB-Wasm KHÔNG range-read được → lớp dữ liệu tải nguyên file (FilePool, 1 lần/phiên); `vite preview` che lỗi này, dùng `web/scripts/pages-like-server.mjs`. CI Rust trên windows-latest từng lỗi chập chờn ("Error code 1" ở truy vấn DuckDB đầu tiên) chưa tìm ra gốc; test dùng temp dir canonical (8.3). Lịch sử local gốc ở nhánh `backup/pre-public-history`.
