<div align="center">

# ⚡ MarkDB

**SQL client ที่เบาและเร็ว** — Postgres & Amazon Redshift

[![release](https://img.shields.io/github/v/release/thanadon-dev/markdb?style=flat-square&color=222)](https://github.com/thanadon-dev/markdb/releases/latest)
[![downloads](https://img.shields.io/github/downloads/thanadon-dev/markdb/total?style=flat-square&color=222)](https://github.com/thanadon-dev/markdb/releases)
![platform](https://img.shields.io/badge/Windows-x64-222?style=flat-square)
![stack](https://img.shields.io/badge/Tauri_v2_·_Rust_·_React-222?style=flat-square)

### [⬇️ ดาวน์โหลดเวอร์ชันล่าสุด](https://github.com/thanadon-dev/markdb/releases/latest)

</div>

---

## 🪶 เบากว่าเยอะ

| | ไบนารี | แรม |
|---|---|---|
| ⚡ **MarkDB** | **~8.7 MB** | **~25 MB** |
| 🐘 pgAdmin | ~250 MB | ~400 MB |
| 🦫 DBeaver | ~500 MB | ~700 MB |

## 🗄️ รองรับ

| | |
|---|---|
| 🐘 **PostgreSQL** 12+ | เต็มทุกฟีเจอร์ |
| 🟥 **Amazon Redshift** | เลือกตอนสร้าง connection · พอร์ต + SSL ตั้งให้เอง |

## ✨ ฟีเจอร์เด่น

| | |
|---|---|
| ✏️ **แก้ค่าในตารางตรง ๆ** | ดับเบิลคลิก cell แก้แล้ว `Enter` — ยิงผ่าน primary key ใน transaction ที่ยืนยันว่าโดนแค่แถวเดียว |
| 💡 **Autocomplete ฉลาด** | ชื่อตาราง · คอลัมน์ของตารางใน `FROM` · และ **ค่าจริงในคอลัมน์** ตอนพิมพ์ `where col = '` |
| 🖱️ **คลิกขวาได้ทุกที่** | ที่ตาราง → Open · New query · Properties · ER diagram<br>ที่แถว → Copy row · Copy as JSON · Export as JSON |
| ✅ **เลือกแถวแบบสเปรดชีต** | คลิก = แถวเดียว · `Ctrl+คลิก` = ทีละแถว · `Shift+คลิก` = ทั้งช่วง |
| 🕸️ **ER diagram** | อ่าน foreign key ทั้ง database มาวาดให้ คลิกที่ node เปิดตารางได้เลย |
| 💾 **Backup / Restore** | ทั้ง database เป็นไฟล์ `.sql` เดียว ไม่ต้องมี `pg_dump` |
| 📤 **Import / Export** | CSV (UTF-8 + BOM เปิดใน Excel อ่านไทยได้) · SQL · JSON |
| 🚀 **ลื่นทุกขนาด** | grid virtualize หลักพันแถวเลื่อนไม่หน่วง |
| 🌑 **ธีมดำสนิท** | SQL keyword ไฮไลต์สีและตัวใหญ่กว่า |
| 🔄 **อัปเดตเอง** | เปิดแอปแล้วเจอเวอร์ชันใหม่ กดทีเดียวจบ |

## 📦 ติดตั้ง

โหลด `MarkDB_x.y.z_x64-setup.exe` จาก [Releases](https://github.com/thanadon-dev/markdb/releases/latest) แล้วติดตั้งได้เลย

> Windows x64 · ต้องมี WebView2 (Win11 / Win10 ที่อัปเดตแล้วมีมาให้)
> ยังไม่ได้ code sign — SmartScreen จะเตือนครั้งแรก กด More info → Run anyway

## 🛠️ Development

```bash
npm install
npm run tauri dev      # dev
npm run tauri build    # build → src-tauri/target/release/bundle/
```

| ไฟล์ | หน้าที่ |
|---|---|
| `src-tauri/src/lib.rs` | ทุกคำสั่งที่คุยกับ DB (sqlx) |
| `src/App.tsx` | UI ทั้งหมด |
| `src/styles.css` | ธีม |

## 🚢 ปล่อยเวอร์ชันใหม่

เลื่อนเลขให้ตรงกันใน `package.json` + `src-tauri/tauri.conf.json` แล้ว

```bash
git commit -am "v0.1.7" && git tag v0.1.7 && git push origin main --tags
```

GitHub Actions จะ build → เซ็น → สร้าง Release พร้อม `latest.json` ให้เอง
ต้องมี secret `TAURI_SIGNING_PRIVATE_KEY` (เนื้อไฟล์ key ทั้งไฟล์)

> ⚠️ private key อยู่ที่ `~/.tauri/markdb.key` **นอก repo** — หายแล้วปล่อยอัปเดตให้เครื่องที่ลงไปแล้วไม่ได้อีกเลย สำรองไว้ให้ดี
> 💤 ถ้า Actions รันไม่ได้ (เช่น billing ล็อก) build ในเครื่องแล้วอัปไฟล์ขึ้น Release เองได้ ผลลัพธ์เหมือนกัน

## 🚧 ที่ยังไม่ทำ

- password เก็บใน `localStorage` เป็น plaintext — ยังไม่เหมาะกับ DB production ที่แชร์กันหลายคน
- backup ไม่ครอบคลุม trigger · function · extension · GRANT · partition
- Redshift: ยัง backup ทั้ง database ไม่ได้ และ import CSV ใช้ INSERT แทน `COPY`
- Windows x64 เท่านั้น

<!-- smoke test -->
