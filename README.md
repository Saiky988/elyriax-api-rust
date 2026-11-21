# Elyriax API v3.0 (Rust)

Backend API hiệu năng cao cho hệ sinh thái Elyriax, được chuyển đổi hoàn toàn từ Python FastAPI sang **Rust (Axum 0.8 + Tokio + SQLx MySQL)**, dọn sạch toàn bộ các module đã loại bỏ và nâng cấp toàn diện hệ thống **Auto Redeem Code Genshin Impact**.

## Các phần đã dọn sạch khỏi dự án

- **Discord Bot & Discord Profile Cache**: Loại bỏ hoàn toàn `bot.start`/`bot.close`, `DISCORD_BOT_TOKEN`, `GUILD_ID`, `USER_TOKEN`, `profile_cache.py`, thư mục `data/profile/` và `discord.py` (giữ nguyên **Discord OAuth** cho đăng nhập/liên kết tài khoản).
- **Vietsub Studio / FFmpeg / Render Jobs**: Loại bỏ `app/utils/ffmpeg.py`, `job_manager`, `static-ffmpeg`, `yt-dlp`, `GROQ_API_KEY`, `GEMINI_API_KEY`.
- **Stream Token SQLite Cache & Asset thừa**: Loại bỏ `create_stream_token`/`decode_stream_token`, `stream_tokens.db` và file ảnh không sử dụng trong `assets/`.
- **Mục tài liệu cũ không còn tồn tại**: Gỡ bỏ các mô tả thừa (`/v1/truyen`, `/v1/crawl/hentai`, `/v1/translate`).

## Nâng cấp hiệu năng & kiến trúc Rust

- **Runtime & HTTP**: Chạy đa luồng không chặn với **Tokio + Axum 0.8**, tích hợp nén phản hồi **Gzip / Brotli**, CORS, giới hạn body upload 100 MiB và **Graceful Shutdown** (SIGINT/SIGTERM).
- **HTTP Connection Pool dùng chung**: Một `reqwest::Client` (HTTP/2 + Rustls + Keep-Alive) chia sẻ toàn ứng dụng thay vì khởi tạo client mới trên từng request.
- **SQLx MySQL Async Pool**: Tối ưu pool kết nối, `test_before_acquire`, prepared statements và khóa giao dịch ACID (`FOR UPDATE`) cho giỏ hàng, đơn hàng, tồn kho và ví SePay.
- **Tương thích 100% dữ liệu cũ**: Thuật toán mã hóa cookie Genshin **AES-256-CBC (PKCS7, khóa SHA-256)** giữ nguyên định dạng `{iv_hex}:{ciphertext_hex}` nên toàn bộ cookie đang lưu trong MySQL hoạt động ngay không cần migrate.
- **In-Memory TTL Cache & Atomic File Persistence**: Cache 10 phút cho giftcode Genshin, 1 giờ cho danh mục đơn vị EVN, cache logo VietQR trong RAM, và ghi file JSON theo cơ chế `.tmp` -> `rename` chống hỏng file.
- **CPU Offloading**: Ghép logo VietQR (`image`) và nén Modpack Minecraft ZIP (`zip`) chạy trên `tokio::task::spawn_blocking`, không làm nghẽn event loop.

## Hệ thống Auto Redeem Code Genshin Impact

- **Tổng hợp giftcode đa nguồn** (có TTL cache 10 phút, tự động lọc trùng):
  - Nguồn chính JSON: [hoyo-codes.seria.moe](https://github.com/seriaati/hoyo-codes) (`https://hoyo-codes.seria.moe/codes?game=genshin`)
  - Nguồn dự phòng JSON: [api.ennead.cc](https://github.com/torikushiii/hoyoverse-api) (`https://api.ennead.cc/mihoyo/genshin/codes`)
  - Nguồn dự phòng HTML: Genshin Impact Fandom Wiki (`Promotional_Code`)
- **Chống nhập trùng & xử lý rate-limit chuẩn HoYoLAB**:
  - Lưu lịch sử code đã nhập theo từng tài khoản và danh sách code hết hạn toàn cục tại `data/genshin_redeem_history.json` (tối đa 50 log gần nhất/tài khoản).
  - Tự động giãn cách `5.5s` giữa các lượt redeem, tự retry 1 lần khi gặp mã `-2016` (cooldown), dừng ngay nếu cookie hết hạn (`-1071`/`-100`).
  - Hỗ trợ cả định dạng server ngắn (`asia`, `na`, `eu`, `tw`, `hk`) lẫn định dạng DB (`os_asia`, `os_usa`, `os_euro`, `os_cht`).
  - Tự động gửi email thông báo qua Resend (`templates/email/genshin-redeem-success.html`) khi nhập thành công code mới.
- **Lịch chạy tự động (múi giờ `Asia/Ho_Chi_Minh`)**:
  - Auto Check-in: `00:05` và dự phòng `06:00` hàng ngày.
  - Auto Redeem Code: mỗi 3 giờ (`00:15`, `03:15`, `06:15`, `09:15`, `15:15`, `18:15`, `21:15`) cùng 2 khung giờ chính `12:00` và `20:00`.
  - Auto Daily Note: mỗi 30 phút (`:00`, `:30`).

### Các Endpoint mới & nâng cấp

| Method | Endpoint | Mô tả |
| :--- | :--- | :--- |
| `GET` | `/health` | Kiểm tra trạng thái server & kết nối MySQL |
| `GET` | `/v1/host/ws` | WebSocket đẩy thông số hệ thống realtime mỗi 2 giây |
| `POST` | `/v1/genshin/accounts/{account_id}/redeem` | Nhập 1 giftcode cụ thể (`{ "code": "..." }`) cho tài khoản |
| `POST` | `/v1/genshin/accounts/{account_id}/auto-redeem` | Tự quét và nhập toàn bộ code khả dụng chưa dùng cho tài khoản (`?refresh=true` để bỏ qua cache) |
| `GET` | `/v1/genshin/accounts/{account_id}/redeem-history` | Xem danh sách code đã nhập & log gần nhất |
| `POST` | `/v1/genshin/auto-redeem/trigger` | Admin kích hoạt auto-redeem toàn hệ thống ngay lập tức |
| `GET` | `/v1/admin/scheduler/status` | Xem trạng thái chạy và kết quả gần nhất của các cron job |
| `POST` | `/v1/admin/scheduler/test-redeem` | Admin test tiến trình auto-redeem |
| `POST` | `/v1/admin/scheduler/test-daily-note` | Admin test tiến trình đồng bộ daily note |

## Hướng dẫn biên dịch & triển khai

### 1. Cài đặt Rust trên Ubuntu VPS

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

### 2. Cấu hình biến môi trường

```bash
cp .env.example .env
nano .env
```

> Giữ nguyên `GENSHIN_COOKIE_SECRET` giống phiên bản cũ nếu kết nối vào cơ sở dữ liệu MySQL đang có sẵn cookie Genshin đã mã hóa.

### 3. Kiểm tra & Build Release

```bash
cargo check
cargo build --release
./target/release/elyriax-api
```

### 4. Chạy nền bằng Systemd hoặc Docker

```bash
# Systemd
sudo cp elyriax.service /etc/systemd/system/elyriax.service
sudo systemctl daemon-reload
sudo systemctl enable --now elyriax

# Hoặc Docker
docker build -t elyriax-api:3.0 .
docker run -d --name elyriax-api --env-file .env -p 25243:25243 elyriax-api:3.0
```
