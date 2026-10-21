# PostGIS engine (2026-10-20)

Báo cáo: `REPORT-GEOFABRIC-POSTGIS-ENGINE.md`. ADR-013. Hướng dẫn vận hành: `docs/postgis.md`. Kiến trúc: `docs/architecture/engine-postgis.md`.

## Trạng thái thật (đừng nói quá)
- ĐÃ có: crate `geo-engine-postgis` (descriptor, capability matrix, lowering PostgreSQL có tham số hoá, catalog nguồn "resident", ánh xạ CRS→SRID, probe, ánh xạ lỗi theo SQLSTATE, `PgConnector` + mock, driver thật sau feature `live`), tích hợp planner (`source_residency`, `calibration`, policy R5d), runtime ghi health đúng engine lỗi, gateway cấu hình tuỳ chọn + `/api/state.postgis`, dòng PostGIS ở Developer, arch_check chặn lẫn họ engine. 34 test mới không cần DB.
- ĐÃ chạy live 2026-10-21 (PG 16.9 / PostGIS 3.6.2 portable): xem `REPORT-GEOFABRIC-POSTGIS-LIVE-VERIFICATION.md`. Parity chặt + 17 plan giống DuckDB + Demo B = oracle; 8 test live (`--features live`); benchmark cold/warm.
- CHƯA: cost model PostGIS đo được (R5d giữ), held-out, shared-lowering (RFC-004), máy thứ hai.
- Bước kế tiếp đúng: cost model PostGIS có đầu vào selectivity/dạng semi-join (không dùng một hệ số hằng) → held-out → mới bỏ R5d.

## Bẫy
- BboxFilter trên PostGIS = giao hộp *covering* (Overture đệm ~1.6 m), KHÔNG phải envelope hình học: đã lệch 41,150 vs 41,165. Giữ cột covering `__cov_*`; `&&` chỉ là gợi ý index.
- Nhánh phải của semi-join bỏ envelope assist (thăm mỗi hàng trái; 10.4 s → 0.9 s). Đừng thêm lại.
- Driver `postgres` đồng bộ: chạy trong `spawn_blocking`, không trong runtime async (panic).
- Chạy live cần ASCII runtime dir ngoài repo; `Get-Process`/kill chỉ theo PID (postmaster.pid), không `taskkill /IM postgres.exe`.
- Cold = restart PG (cache OS vẫn ấm) — ghi rõ khi báo cáo benchmark.
- Engine mới = `Unmeasured`: planner không bao giờ chọn nó theo chi phí dự đoán trước engine đã đo (R5d); chỉ chọn khi hint tường minh.
- Dataset chỉ có trong PostGIS (không có file) CHƯA plan được vì planner đọc footer Parquet (P1: cần source-metadata provider).
- PostgreSQL khác mặc định DuckDB ở: `NULLS LAST`, collation (dùng `COLLATE "C"` + cluster locale C), endianness WKB (`'NDR'`), kiểu tham số (cast tường minh), ép kiểu so sánh (lowering từ chối, không đoán). Đừng bỏ.
- `geography` bị loại có chủ đích; mét = UTM + `ST_DWithin` trong `EXISTS`.
- Runtime ghi lỗi thực thi theo engine đã chạy (trước đây gateway ghi cứng "duckdb"); đừng thêm lại ghi ở gateway.
- Test e2e smoke phải chọn engine "duckdb" tường minh (planner có thể chọn browser khi đã có probe).
