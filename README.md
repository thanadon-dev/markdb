<div align="center">

# MarkDB

**SQL client ที่เบาและเร็ว สำหรับ PostgreSQL และ Amazon Redshift**

[![release](https://img.shields.io/github/v/release/thanadon-dev/markdb?style=flat-square&color=222)](https://github.com/thanadon-dev/markdb/releases/latest)
[![downloads](https://img.shields.io/github/downloads/thanadon-dev/markdb/total?style=flat-square&color=222)](https://github.com/thanadon-dev/markdb/releases)
![platform](https://img.shields.io/badge/Windows-x64-222?style=flat-square)
![stack](https://img.shields.io/badge/Tauri_v2_·_Rust_·_React-222?style=flat-square)

[ดาวน์โหลดเวอร์ชันล่าสุด](https://github.com/thanadon-dev/markdb/releases/latest)

</div>

---

## ทำไมต้อง MarkDB

ตัวติดตั้งไม่ถึง 5 MB เปิดเร็ว กินแรมน้อย เพราะใช้ Tauri (WebView ของระบบ + Rust) แทน Electron หรือ Java

| | ไบนารี | แรม |
|---|---|---|
| **MarkDB** | **~8.7 MB** | **~25 MB** |
| pgAdmin | ~250 MB | ~400 MB |
| DBeaver | ~500 MB | ~700 MB |

## รองรับ

| Database | สถานะ |
|---|---|
| PostgreSQL 12+ | ครบทุกฟีเจอร์ |
| Amazon Redshift | เลือกตอนสร้าง connection ตั้งพอร์ตและ SSL ให้เอง (บางฟีเจอร์ยังไม่รองรับ ดูด้านล่าง) |

## ฟีเจอร์

**แก้ข้อมูล**
- แก้ค่าใน cell ได้เลย: ดับเบิลคลิกแล้วกด `Enter` ระบบอัปเดตผ่าน primary key ใน transaction และเช็คว่าโดนแถวเดียวพอดี
- แถวว่างท้ายตารางสำหรับเพิ่มข้อมูล: กด `Tab` แล้วระบบเติม uuid, เวลาปัจจุบัน และค่า default ให้
- วางข้อมูลจาก Excel หรือ Google Sheets ลงช่วง cell ที่เลือก หรือลงแถวใหม่
- ย้อนการแก้ การเพิ่มแถว และ DELETE ได้จากประวัติการเปลี่ยนแปลง

**เขียน SQL**
- Autocomplete รู้จักชื่อตาราง คอลัมน์ของตารางใน `FROM` และค่าที่มีจริงในคอลัมน์ตอนพิมพ์ `where col = '`
- ประวัติ SQL ที่รันไปแล้ว ค้นหาได้ (`Ctrl+H`)
- กรองจาก cell ได้ด้วยการคลิกขวา (เท่ากับ, ไม่เท่ากับ, มีคำนี้, เป็น NULL) และแบ่งหน้า หรือ order by จากหัวคอลัมน์
- AI ช่วยเขียนและแก้ SQL ที่ error ผ่าน [Claude Code](https://claude.com/claude-code) ที่ login ไว้ในเครื่อง ตัวแอปไม่ต้องเก็บ API key

**กันพลาด**
- ติดป้าย Production หรืออ่านอย่างเดียวให้แต่ละ connection ได้ บน Postgres ใช้ read-only transaction บังคับที่ระดับ session
- เตือนก่อนรันคำสั่งเขียนบน PROD และ `UPDATE` / `DELETE` ที่ไม่มี `WHERE`
- ก่อนลบหรือ drop ต้องรอ 3 วินาทีก่อนกดยืนยัน

**เครื่องมือ**
- ER diagram ที่อ่าน foreign key ทั้ง database
- Backup และ restore ทั้ง database เป็นไฟล์ `.sql` เดียว โดยไม่ต้องมี `pg_dump`
- Sync ตารางข้าม connection (เช่น dev ไป uat) ทั้งโครงสร้างและข้อมูล
- Import และ export เป็น CSV, SQL หรือ JSON (CSV เป็น UTF-8 + BOM เปิดใน Excel แล้วอ่านภาษาไทยได้)

**ใช้งานทั่วไป**
- จัดกลุ่ม connection ตาม host ให้อัตโนมัติ ตั้งชื่อกลุ่มเองหรือพับเก็บได้
- ตารางหลักพันแถวเลื่อนลื่นเพราะ virtualize grid
- อัปเดตตัวเองเมื่อมีเวอร์ชันใหม่

## ติดตั้ง

ดาวน์โหลด `MarkDB_x.y.z_x64-setup.exe` จาก [Releases](https://github.com/thanadon-dev/markdb/releases/latest) แล้วติดตั้ง

> ใช้ได้บน Windows x64 ที่มี WebView2 (Windows 11 และ Windows 10 ที่อัปเดตแล้วมีมาให้)
> ตัวติดตั้งยังไม่ได้ code sign จึงมี SmartScreen เตือนตอนเปิดครั้งแรก ให้กด More info แล้วกด Run anyway

## ข้อจำกัดที่ควรรู้

- **password ของ connection เก็บใน `localStorage` แบบ plaintext** จึงยังไม่ควรใช้กับเครื่องที่หลายคนใช้ร่วมกัน
- backup ยังไม่รวม trigger, function, extension, GRANT และ partition
- Redshift ยัง backup ทั้ง database ไม่ได้ import CSV ใช้ INSERT แทน `COPY` และโหมดอ่านอย่างเดียวบังคับแค่ในแอป
- รองรับแค่ Windows x64

## พัฒนา

ต้องมี [Node.js](https://nodejs.org) 22.18+ (เทสต์รันไฟล์ `.ts` ตรง ๆ), [Rust](https://rustup.rs) stable และ [สิ่งที่ Tauri ต้องใช้บน Windows](https://v2.tauri.app/start/prerequisites/)

```bash
npm install
npm run tauri dev      # เปิดแอปแบบ dev (hot reload)
npm run tauri build    # build ตัวติดตั้ง → src-tauri/target/release/bundle/
```

### เทสต์

```bash
npm test                                   # เทสต์ฝั่ง TypeScript (node --test)
cd src-tauri && cargo test                 # เทสต์ฝั่ง Rust
# เทสต์ที่ต้องต่อ Postgres จริง (ใช้ DB ทิ้งได้เท่านั้น เพราะเทสต์สร้างและลบตาราง)
MARKDB_TEST_PG=postgres://user:pass@localhost:5432/db cargo test -- --ignored
```

### โครงสร้าง

| ไฟล์ | หน้าที่ |
|---|---|
| `src-tauri/src/lib.rs` | คำสั่งที่คุยกับ DB ทั้งหมด (sqlx) |
| `src-tauri/src/sync.rs` | sync ตารางข้าม connection |
| `src-tauri/src/ai.rs` | เรียก `claude` CLI สำหรับ AI |
| `src/App.tsx` | UI |
| `src/*.ts` | logic ล้วนที่เทสต์ได้ เช่น ตัดคำสั่ง SQL (`sqlsplit`), เติม where/order/limit (`sqltail`), กันพลาด (`guard`), ประวัติ (`history`, `queries`) |
| `src/styles.css` | ธีม |

## ร่วมพัฒนา

ยินดีรับ issue และ pull request

- **bug**: เปิด issue บอกเวอร์ชัน, DB (Postgres เวอร์ชันไหน หรือ Redshift), ขั้นตอนที่ทำให้เกิด และสิ่งที่คาดว่าจะเกิด
- **ฟีเจอร์ใหญ่**: เปิด issue คุยกันก่อนลงมือ จะได้ไม่ทำซ้ำหรือทำไปแล้วไม่ตรงทาง
- **pull request**: หนึ่ง PR ต่อหนึ่งเรื่อง รัน `npm test` และ `cargo test` ให้ผ่านก่อนส่ง ถ้าแก้ logic ให้เพิ่มเทสต์ด้วย
- logic ที่ไม่ผูกกับ UI ให้แยกไว้ใน `src/*.ts` แล้วเขียนเทสต์คู่กันเป็น `*.test.ts`
- ห้ามใส่ password, connection string หรือข้อมูลจริงจาก DB ลงในโค้ด เทสต์ หรือ issue

## License

[MIT](LICENSE)
