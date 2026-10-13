# ADR-005 — MapLibre cho rendering

Status: Proposed

Lý do: chuẩn mở, WebGL, hệ sinh thái lớn. Cách nạp kết quả vào bản đồ (GeoJSON ở biên hiển thị / vector tile động / deck.gl + GeoArrow) cần thử nghiệm; GeoJSON chỉ chấp nhận ở biên render, không trong runtime. Core không được import renderer.
