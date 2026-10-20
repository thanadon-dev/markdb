/* จัดรูป SQL ตามชนิด database (MySQL คง backtick, Postgres รู้จัก ::cast ฯลฯ)
   import เฉพาะ 4 dialect ที่ใช้ ไม่ลากทั้งหมดของ sql-formatter เข้า bundle */
import { formatDialect, mysql, postgresql, redshift, sqlite } from "sql-formatter";

const DIALECTS = { postgres: postgresql, redshift, sqlite, mysql };

/** อ่านไม่ออก (syntax ที่ formatter ไม่รู้จัก) → error ให้ผู้ใช้เห็น ไม่แก้ SQL ให้พัง */
export const formatSql = (sql: string, engine: keyof typeof DIALECTS = "postgres") =>
  formatDialect(sql, { dialect: DIALECTS[engine] ?? postgresql, tabWidth: 2 });
