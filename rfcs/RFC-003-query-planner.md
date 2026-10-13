# RFC-003 — Query Planner

Status: Proposed

- MVP: rule-based. Input: IR + catalog (stats nguồn) + danh sách engine/capabilities. Output: `PhysicalPlan` (engine được chọn, các bước) + `DecisionLog`.
- Quy tắc ví dụ: đẩy bbox/filter xuống Source (row-group pruning); chiếu cột sớm; chọn engine đầu tiên đáp ứng capabilities, ưu tiên theo kích thước dữ liệu (nhỏ→browser, lớn→server, giao dịch→PostGIS) khi có engine tương ứng.
- Mọi quyết định ghi vào `DecisionLog` (rule, lý do, lựa chọn bị loại) → hiển thị ở tab Execution Plan (Constitution 13).
- Hướng cost-based: dataset size, ước lượng rows, bbox, row-group stats, network, memory, CPU, cache, index, capability, locality. Chỉ làm khi benchmark chứng minh rule-based không đủ (Constitution 7).
- Planner không import renderer/UI; không hard-code DuckDB.
