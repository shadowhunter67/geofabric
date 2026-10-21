# Public release + static demo (2026-10-21)

Quyết định của user (AskUserQuestion, 2026-10-21): giấy phép **MIT OR Apache-2.0**; lịch sử công khai **viết lại sang noreply**; dữ liệu demo "đọc từ nguồn Overture" - ĐÃ chuyển sang gói con đi kèm vì: (1) bucket chỉ giữ 2 release mới nhất (liệt kê 2026-10-21: 2026-09-23.0, 2026-09-23.1) nên URL ghim sẽ chết; (2) app cần schema phẳng đã chuẩn hoá, không phải schema lồng của Overture. CORS+Range của bucket đều OK (`Access-Control-Allow-Origin: *`).

## Kiểm toán trước khi công khai
- Lịch sử 48 commit quét bằng `git grep` mọi revision: không token/khóa riêng/chuỗi kết nối có mật khẩu thật; chỉ placeholder (`u:secret-pw`, `dev:dev`, `SECRET-PW-123` trong test). Mật khẩu `credentials.json` thật của PG không có trong lịch sử. Không từng track `.env*`. Blob lớn nhất 679 KB.
- Rủi ro đã xử lý: email cá nhân + tên máy trong author (rewrite noreply); đường dẫn `C:\Users\<user>\...` và tên máy trong vài log/JSON (scrub trong history rewrite).

## Bẫy
- Playwright `channel: chromium` cho CI; local dùng chrome/msedge. Gateway khởi động bằng `--no-browser` TẮT engine browser (test browser sẽ thấy placement "server"); gateway sống lâu mang trạng thái "browser unhealthy" từ test crash-report: mỗi lượt Playwright full-stack nên khởi động gateway mới.
- Git Bash đổi `/geofabric/` thành đường dẫn Windows: build base path bằng `GF_BASE` + `MSYS_NO_PATHCONV=1`.
- DuckDB-Wasm tải extension `parquet` từ `extensions.duckdb.org` lần đầu: là origin ngoài hợp lệ, đã ghi trong privacy/audit.
- Workflow Rust chạy trên `windows-latest` vì libduckdb prebuilt (đã ghim sha256) là bản Windows.
- Hook của harness chặn: tên file `.env*` trong lệnh bash, và mọi lệnh có chuỗi xoá đệ quy kể cả trong nội dung heredoc. Dùng Write tool; profile static đi qua Vite `define`, không dùng file env.
