/* ด่านก่อนรัน SQL: คำสั่งไหนแก้ข้อมูล และ UPDATE/DELETE ไหนไม่มี WHERE (โดนทั้งตาราง)
   ponytail: ดูจากคำแรก + หา where แบบ regex หลังตัด string/comment — ไม่ได้ parse จริง
   where ที่อยู่ใน subquery อย่างเดียวจะถูกนับว่ามี where (หลุดเตือน) ถ้าเจอบ่อยค่อยนับวงเล็บ */

const WRITE_FIRST =
  /^(insert|update|delete|merge|truncate|drop|alter|create|grant|revoke|copy|call|do|vacuum|reindex|cluster|comment|refresh|lock|unload)\b/;

/** ตัด comment และเนื้อใน string ทิ้ง เหลือโครงคำสั่งตัวเล็ก ไว้หา keyword */
export const skeleton = (sql: string) =>
  sql
    .replace(/--[^\n]*|\/\*[\s\S]*?\*\//g, " ")
    .replace(/'(?:[^']|'')*'/g, "''")
    .replace(/"(?:[^"]|"")*"/g, '""')
    .trim()
    .toLowerCase();

export type Risk = { write: boolean; noWhere: string | null };

/** write = แก้ข้อมูล/โครงสร้าง, noWhere = "update"/"delete" ที่ไม่มี where (ทั้งตาราง) */
export const risk = (stmt: string): Risk => {
  const s = skeleton(stmt);
  const first = /^\w+/.exec(s)?.[0] ?? "";
  // with ... delete/update/insert — CTE ที่เขียนข้อมูลก็นับเป็นการแก้
  const write = WRITE_FIRST.test(s) || (first === "with" && /\b(insert|update|delete|merge)\b/.test(s));
  const noWhere = (first === "update" || first === "delete") && !/\bwhere\b/.test(s) ? first : null;
  return { write, noWhere };
};
