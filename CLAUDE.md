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

**Quy tắc duy trì:** mục mới/lịch sử ghi vào `docs/claude/`, KHÔNG nối dài CLAUDE.md.


## Process safety (mandatory, added 2026-10-16)

NEVER terminate processes globally by executable name.

Forbidden: `taskkill /IM node.exe`, `taskkill /IM python.exe`, `pkill node`, `killall python` (and any other name-based kill).

Only terminate processes this project started, by a verified PID or an owned process handle (`taskkill /F /PID <pid>`; check with `netstat -ano` / `tasklist` first, and check that a port is free before starting a server on it: a stale server on the same port silently serves every test, see `docs/claude/planner-hardening.md`).

Do not change the global Node installation, the nvm default, or any shared toolchain. If a specific version is needed, put it on PATH for the command only (here Node 24 at `C:/Users/<user>/AppData/Local/nvm/v24.16.0`; never `nvm use`).
