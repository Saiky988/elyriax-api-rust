# Elyriax API v3.1 (Rust — Genshin + HSR + ZZZ)

Backend API hiệu năng cao cho hệ sinh thái Elyriax viết hoàn toàn bằng **Rust (Axum 0.8 + Tokio + SQLx MySQL)**, đã dọn sạch các module thừa và tích hợp toàn diện bộ công cụ tự động hóa **HoYoverse** cho cả 3 tựa game: **Genshin Impact**, **Honkai: Star Rail (HSR)** và **Zenless Zone Zero (ZZZ)**.

## Tính năng HoYoverse toàn diện (Genshin + HSR + ZZZ)

Tích hợp trực tiếp theo cấu trúc & nguồn dữ liệu từ [seriaati/hoyo-codes](https://github.com/seriaati/hoyo-codes) và [torikushiii/hoyoverse-api](https://github.com/torikushiii/hoyoverse-api):

- **Tổng hợp Giftcode đa nguồn (TTL Cache 5 phút)**:
  - `https://hoyo-codes.seria.moe/codes?game={genshin|hkrpg|nap}`
  - `https://api.ennead.cc/mihoyo/{genshin|starrail|zenless}/codes`
  - Fallback Fandom Wiki cho Genshin Impact
- **Auto Redeem Code thông minh**:
  - Hỗ trợ `webExchangeCdkey` (Genshin) và `webExchangeCdkeyHoyo` (HSR, ZZZ).
  - Xử lý **mã theo khu vực (Regional Codes)** như `ZZZSUSHIRO` (chỉ đánh dấu bỏ qua cho tài khoản/khu vực đó, không làm hỏng cache toàn cục) và **mã cùng nhóm prefix (Same-Family Codes)** trên ZZZ.
  - Giãn cách `5.5s` theo rate-limit HoYoLAB, tự retry 1 lần khi gặp `-2016`, dừng ngay khi cookie hết hạn (`-1071`/`-100`).
  - Lưu lịch sử chống nhập trùng theo `{game}:{account_id}` tại `data/hoyo_redeem_history.json` và tự động gửi email thông báo qua Resend (`templates/email/hoyo-redeem-success.html`).
- **Lịch sự kiện, Banner & Tin tức cộng đồng (TTL Cache 5–15 phút)**:
  - `calendar`: Sự kiện đang diễn ra, Banner nhân vật/vũ khí/Nón Ánh Sáng/W-Engine và thử thách Endgame (La Hoàn, Nhà Hát Giả Tưởng, Hồi Ức Hỗn Độn, Shiyu Defense, Deadly Assault) hỗ trợ `lang=vi-vn`, `en-us`, `ja-jp`, `zh-cn`...
  - `news`: Tin tức HoYoLAB phân loại `events`, `notices`, `info`.
- **Daily Check-in, Daily Note & Role/Stats**:
  - Chuẩn hóa đủ mã server cho cả 3 game (`os_*` cho Genshin, `prod_official_*` cho HSR, `prod_gf_*` cho ZZZ).
  - Tích hợp bộ sinh chữ ký động **HoYoLAB OS DS (MD5)** cho `game_record` API.
  - Tự động tạo bảng `hoyo_accounts` cho HSR & ZZZ khi khởi động (file SQL kèm theo tại `migrations/001_hoyo_accounts.sql`), giữ nguyên 100% bảng `genshin_accounts` và định dạng mã hóa cookie `AES-256-CBC`.
- **Scheduler tự động đa game (`Asia/Ho_Chi_Minh`)**:
  - Auto Check-in: `00:05` và dự phòng `06:00` hàng ngày.
  - Auto Redeem Code: mỗi 3 giờ (`00:15`, `03:15`, `06:15`, `09:15`, `15:15`, `18:15`, `21:15`) + `12:00` và `20:00`.
  - Auto Daily Note: mỗi 30 phút (`:00`, `:30`).

## Danh sách Endpoint HoYo (Genshin / HSR / ZZZ)

Thay `{prefix}` bằng `/v1/genshin`, `/v1/hsr`, `/v1/zzz`, hoặc `/v1/hoyo/{game}` (`genshin`, `hsr`/`starrail`, `zzz`/`zenless`):

| Method | Endpoint | Mô tả |
| :--- | :--- | :--- |
| `GET` | `/v1/hoyo/codes` | Lấy toàn bộ giftcode khả dụng của cả 3 game trong 1 request |
| `GET` | `{prefix}/codes` | Lấy giftcode khả dụng của game (`?refresh=true` để làm mới cache) |
| `GET` | `{prefix}/calendar?lang=vi-vn` | Lịch sự kiện, Banner và Endgame Challenges |
| `GET` | `{prefix}/news/{kind}?lang=vi-vn` | Tin tức HoYoLAB (`events`, `notices`, `info`) |
| `POST` | `{prefix}/checkin` | Điểm danh nhanh bằng cookie trực tiếp |
| `POST` | `{prefix}/redeem` | Nhập giftcode nhanh bằng cookie trực tiếp |
| `POST` | `{prefix}/role-stats` | Lấy thông số nhân vật/Agent bằng cookie trực tiếp |
| `POST` | `{prefix}/daily-note` | Lấy Daily Note (Nhựa / Sức Mạnh Khai Phá / Pin) |
| `GET/POST` | `{prefix}/accounts` | Xem danh sách / Thêm tài khoản game (có xác thực JWT/Session) |
| `GET/PATCH/DELETE` | `{prefix}/accounts/{id}` | Xem chi tiết / Bật tắt `auto_checkin`, `auto_redeem`, `auto_daily_note` / Xóa |
| `POST` | `{prefix}/accounts/{id}/checkin` | Điểm danh ngay cho tài khoản đã lưu |
| `POST` | `{prefix}/accounts/{id}/redeem` | Nhập 1 code cụ thể (`{ "code": "..." }`) cho tài khoản |
| `POST` | `{prefix}/accounts/{id}/auto-redeem` | Tự quét và nhập toàn bộ code mới chưa dùng cho tài khoản |
| `GET` | `{prefix}/accounts/{id}/redeem-history` | Lịch sử giftcode đã nhập của tài khoản |
| `GET` | `/v1/admin/scheduler/status` | Xem trạng thái chạy và thống kê theo từng game của scheduler |

## Biên dịch & Triển khai trên VPS

```bash
cp .env.example .env
nano .env   # Giữ nguyên GENSHIN_COOKIE_SECRET cũ nếu dùng DB có sẵn

cargo check
cargo build --release
./target/release/elyriax-api
```
