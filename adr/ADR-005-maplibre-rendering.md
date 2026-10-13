# ADR-005 — MapLibre cho rendering

Status: Proposed

Lý do: chuẩn mở, WebGL, hệ sinh thái lớn. Cách nạp kết quả vào bản đồ (GeoJSON ở biên hiển thị / vector tile động / deck.gl + GeoArrow) cần thử nghiệm; GeoJSON chỉ chấp nhận ở biên render, không trong runtime. Core không được import renderer.

## Evidence review (hardening, 2026-10-13)

Evidence: browser smoke test passes (map, results, plan, inspector). Open: GeoJSON at the boundary is a stop-gap (about 0.6 s for 20 000 polygons in a debug build), vector-tile / GeoArrow delivery is not evaluated. **Stays Proposed.**
