# GeoFabric Constitution

Các nguyên tắc bất biến. Thay đổi cần ADR/RFC được duyệt (điều 15).

1. GeoJSON is a boundary format, not a core execution format.
2. CRS must never be implicit.
3. Distance and area units must never be implicit.
4. Spatial IR must not depend on DuckDB, PostGIS, MapLibre or any specific execution engine.
5. Execution engines must be replaceable adapters.
6. Browser, local server and future distributed execution must use the same logical plan.
7. Every optimization must be justified by benchmark evidence.
8. Do not create a new geospatial file format unless existing standards cannot satisfy the requirement.
9. AI must never directly execute arbitrary generated spatial SQL without validation.
10. Core runtime must be usable without a proprietary cloud service.
11. Core packages must not depend on UI packages.
12. Storage layer must not depend on renderer.
13. Planner decisions must be observable.
14. Data lineage must be retained where possible.
15. Architecture changes require explicit ADR or RFC.

## Enforcement

Các điều 4, 11, 12 và "no cloud SDK in core" được kiểm bằng `tools/arch_check.py` (chạy trong CI / `make check`). Điều 2, 3 được ép bằng kiểu dữ liệu của `geo-ir` (không có constructor thiếu CRS/unit).
