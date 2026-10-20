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

/** query ที่กดดาวเก็บไว้ — ไม่ถูกตัดทิ้งตอนประวัติเกิน 500 */
export type Saved = Ran & { name: string };

export const KEY = "markdb.queries.v1";
export const SAVED_KEY = "markdb.saved.v1";
const CAP = 500;

export const load = <T = Ran>(key = KEY): T[] => {
  try {
    const a = JSON.parse(localStorage.getItem(key) || "[]");
    return Array.isArray(a) ? a : [];
  } catch {
    return [];
  }
};

export const save = (list: Ran[], key = KEY) => {
  try {
    localStorage.setItem(key, JSON.stringify(list));
  } catch {
    /* เต็มก็ช่างมัน */
  }
};

const norm = (s: string) => s.trim().replace(/;\s*$/, "").replace(/\s+/g, " ");

const isSame = (a: Pick<Ran, "conn" | "sql">, b: Pick<Ran, "conn" | "sql">) => a.conn === b.conn && norm(a.sql) === norm(b.sql);

export const isSaved = (saved: Saved[], r: Pick<Ran, "conn" | "sql">) => saved.some((x) => isSame(x, r));

/** ชื่อตั้งต้น: comment บรรทัดแรก (-- …) ถ้ามี ไม่งั้นต้น SQL */
export const nameOf = (sql: string) => {
  const c = sql.trim().match(/^--\s*(.+)/);
  const s = (c ? c[1] : sql).trim().replace(/\s+/g, " ");
  return s.length > 60 ? s.slice(0, 59) + "…" : s;
};

/** กดดาว: ยังไม่มี = เก็บขึ้นบนสุด, มีแล้ว = เอาออก */
export const toggleSaved = (saved: Saved[], r: Omit<Ran, "n"> & { n?: number }): Saved[] =>
  isSaved(saved, r) ? saved.filter((x) => !isSame(x, r)) : [{ ...r, n: r.n ?? 1, name: nameOf(r.sql) }, ...saved];

export const add = (list: Ran[], r: Omit<Ran, "n">): Ran[] => {
  const same = list.find((x) => x.conn === r.conn && norm(x.sql) === norm(r.sql));
  const rest = list.filter((x) => x !== same);
  return [{ ...r, n: (same?.n ?? 0) + 1 }, ...rest].slice(0, CAP);
};

/** ค้นทุกคำที่พิมพ์ (คั่นด้วยช่องว่าง) ต้องเจอใน SQL หรือชื่อ connection */
export const search = <T extends Ran>(list: T[], q: string) => {
  const words = q.toLowerCase().split(/\s+/).filter(Boolean);
  return words.length
    ? list.filter((r) => {
        const hay = (r.sql + " " + r.connName + " " + ("name" in r ? r.name : "")).toLowerCase();
        return words.every((w) => hay.includes(w));
      })
    : list;
};
