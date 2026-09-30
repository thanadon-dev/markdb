/* ประวัติ SQL ที่รัน (คนละอันกับ history.ts ที่เก็บการแก้ข้อมูลไว้ย้อนกลับ)
   เก็บใน localStorage ของเครื่องนี้ — รันคำสั่งเดิมซ้ำแค่ขยับขึ้นบนสุด ไม่เพิ่มแถวใหม่ */

export type Ran = {
  id: string;
  at: number;
  conn: string;
  connName: string;
  sql: string;
  ms?: number;
  rows?: number;
  err?: string;
  /** รันซ้ำกี่ครั้ง */
  n: number;
};

export const KEY = "markdb.queries.v1";
const CAP = 500;

export const load = (): Ran[] => {
  try {
    const a = JSON.parse(localStorage.getItem(KEY) || "[]");
    return Array.isArray(a) ? a : [];
  } catch {
    return [];
  }
};

export const save = (list: Ran[]) => {
  try {
    localStorage.setItem(KEY, JSON.stringify(list));
  } catch {
    /* เต็มก็ช่างมัน */
  }
};

const norm = (s: string) => s.trim().replace(/;\s*$/, "").replace(/\s+/g, " ");

export const add = (list: Ran[], r: Omit<Ran, "n">): Ran[] => {
  const same = list.find((x) => x.conn === r.conn && norm(x.sql) === norm(r.sql));
  const rest = list.filter((x) => x !== same);
  return [{ ...r, n: (same?.n ?? 0) + 1 }, ...rest].slice(0, CAP);
};

/** ค้นทุกคำที่พิมพ์ (คั่นด้วยช่องว่าง) ต้องเจอใน SQL หรือชื่อ connection */
export const search = (list: Ran[], q: string) => {
  const words = q.toLowerCase().split(/\s+/).filter(Boolean);
  return words.length
    ? list.filter((r) => {
        const hay = (r.sql + " " + r.connName).toLowerCase();
        return words.every((w) => hay.includes(w));
      })
    : list;
};
