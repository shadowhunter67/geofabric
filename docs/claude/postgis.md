# PostGIS engine (2026-10-20)

Báo cáo: `REPORT-GEOFABRIC-POSTGIS-ENGINE.md`. ADR-013. Hướng dẫn vận hành: `docs/postgis.md`. Kiến trúc: `docs/architecture/engine-postgis.md`.

## Trạng thái thật (đừng nói quá)
- ĐÃ có: crate `geo-engine-postgis` (descriptor, capability matrix, lowering PostgreSQL có tham số hoá, catalog nguồn "resident", ánh xạ CRS→SRID, probe, ánh xạ lỗi theo SQLSTATE, `PgConnector` + mock, driver thật sau feature `live`), tích hợp planner (`source_residency`, `calibration`, policy R5d), runtime ghi health đúng engine lỗi, gateway cấu hình tuỳ chọn + `/api/state.postgis`, dòng PostGIS ở Developer, arch_check chặn lẫn họ engine. 34 test mới không cần DB.
- CHƯA từng chạy với PostgreSQL thật (user từ chối tải 440 MB, 2026-10-20): không import dữ liệu, không parity, không digest chéo engine, không Demo B trên PostGIS, không benchmark, không cost model đo được, driver `live` chỉ compile-check, script `tools/postgis_portable.py` + `scripts/postgis/load_data.py` chưa chạy.
- Bước kế tiếp đúng: cho phép tải portable → chạy `postgis_portable.py setup/start` → `load_data.py` → `cargo test -p geo-engine-postgis --features live --test live` → cross-check digest DuckDB vs PostGIS + Demo B 27,624 → benchmark → calibrate PostGIS → held-out.

## Bẫy
- Engine mới = `Unmeasured`: planner không bao giờ chọn nó theo chi phí dự đoán trước engine đã đo (R5d); chỉ chọn khi hint tường minh.
- Dataset chỉ có trong PostGIS (không có file) CHƯA plan được vì planner đọc footer Parquet (P1: cần source-metadata provider).
- PostgreSQL khác mặc định DuckDB ở: `NULLS LAST`, collation (dùng `COLLATE "C"` + cluster locale C), endianness WKB (`'NDR'`), kiểu tham số (cast tường minh), ép kiểu so sánh (lowering từ chối, không đoán). Đừng bỏ.
- `geography` bị loại có chủ đích; mét = UTM + `ST_DWithin` trong `EXISTS`.
- Runtime ghi lỗi thực thi theo engine đã chạy (trước đây gateway ghi cứng "duckdb"); đừng thêm lại ghi ở gateway.
- Test e2e smoke phải chọn engine "duckdb" tường minh (planner có thể chọn browser khi đã có probe).
