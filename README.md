# SparkDB

Postgres client ที่เบาและเร็ว — Tauri v2 + Rust + React

ไบนารี ~8.7 MB, กินแรม ~25 MB (pgAdmin ~400 MB, DBeaver ~700 MB)

## ฟีเจอร์

- **ดับเบิลคลิกตาราง** → เปิดแท็บพร้อมรัน `SELECT` ทันที
- **แก้ค่าในตารางตรง ๆ** — ดับเบิลคลิก cell, `Enter`/`Ctrl+S` บันทึก ทำงานผ่าน primary key ใน transaction ที่ยืนยันว่าโดนแค่ 1 แถว
- **Autocomplete** — ชื่อตาราง, ชื่อคอลัมน์ของตารางใน `FROM`, และ**ค่าจริงในคอลัมน์**ตอนพิมพ์ `where col = '` เลือกด้วย `Tab` หรือ `Enter`
- **Backup / Restore** ทั้ง database เป็นไฟล์ `.sql` เดียว ไม่ต้องมี `pg_dump`
- **Import / Export** CSV (UTF-8 + BOM อ่านภาษาไทยใน Excel ได้) และ SQL
- **คลิกขวาที่ตาราง** → properties: คอลัมน์, type, nullable, default, ขนาด, จำนวนแถวโดยประมาณ
- Grid virtualize — ผลลัพธ์หลักพันแถวเลื่อนลื่น ไม่หน่วง
- ธีมดำสนิท ตัวอักษรขาว, SQL keyword ไฮไลต์สีและตัวใหญ่กว่า

## Requirements

- Windows x64 (WebView2 — Win11/Win10 ที่อัปเดตแล้วมีมาให้)
- Postgres 12+

## Development

```bash
npm install
npm run tauri dev
```

## Build

```bash
npm run tauri build
```

ผลลัพธ์:
- `src-tauri/target/release/sparkdb.exe` — portable
- `src-tauri/target/release/bundle/nsis/*.exe` — installer
- `src-tauri/target/release/bundle/msi/*.msi`

## โครงสร้าง

| ไฟล์ | หน้าที่ |
|---|---|
| `src-tauri/src/lib.rs` | คำสั่งทั้งหมดที่คุยกับ Postgres (sqlx) |
| `src/App.tsx` | UI ทั้งหมด |
| `src/styles.css` | ธีม |
| `logo-source.png` | ต้นฉบับ icon (`npm run tauri icon logo-source.png`) |

## ที่ยังไม่ทำ

- password ของ connection เก็บใน `localStorage` เป็น plaintext — ยังไม่เหมาะกับ DB production ที่แชร์กันหลายคน
- backup ไม่ครอบคลุม trigger, function, extension, GRANT, partition
- รองรับเฉพาะ Postgres และเฉพาะ x64
- ยังไม่ได้ code sign — SmartScreen จะเตือนตอนเปิดครั้งแรก
