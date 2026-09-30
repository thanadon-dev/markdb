/* ส่วนท้ายของ SELECT: ORDER BY / LIMIT / OFFSET — แยกออกมาแก้แล้วประกอบคืน
   ใช้กับปุ่มแบ่งหน้าและ "order by" จากหัวคอลัมน์ ซึ่งแก้ SQL ของผู้ใช้ตรง ๆ ให้เห็นว่ารันอะไร
   นับเฉพาะคำที่อยู่ชั้นนอกสุด — ข้างใน (…) / string / comment ไม่นับ
   (subquery หรือ over (order by …) จึงไม่โดนแตะ) */

export type Tail = {
  head: string;
  order: string | null;
  limit: number | null;
  offset: number | null;
  semi: boolean;
};

const KW = /^(order\s+by|limit|offset)\b/i;

/* ตำแหน่งของ keyword (ค่าเริ่มต้น order by / limit / offset) ที่อยู่ชั้นนอกสุด */
const topLevel = (s: string, kw = KW) => {
  const hits: { at: number; kw: string; len: number }[] = [];
  let depth = 0;
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (c === "'" || c === '"') {
      const close = s.indexOf(c, i + 1);
      i = close < 0 ? s.length : close; // '' / "" ข้างในคือ escape — วนต่อก็ปิดคู่ถัดไปพอดี
    } else if (c === "-" && s[i + 1] === "-") {
      const nl = s.indexOf("\n", i);
      i = nl < 0 ? s.length : nl;
    } else if (c === "/" && s[i + 1] === "*") {
      const end = s.indexOf("*/", i + 2);
      i = end < 0 ? s.length : end + 1;
    } else if (c === "(") depth++;
    else if (c === ")") depth--;
    else if (depth === 0 && /\w/.test(c) && !/\w/.test(s[i - 1] ?? "")) {
      const m = s.slice(i).match(kw);
      if (m) {
        hits.push({ at: i, kw: m[1].toLowerCase().replace(/\s+/, " "), len: m[0].length });
        i += m[0].length - 1;
      }
    }
  }
  return hits;
};

/** null = ส่วนท้ายอ่านไม่ออก (เช่น limit $1, fetch first) — ไม่แตะ SQL นั้น */
export const parseTail = (sql: string): Tail | null => {
  let s = sql.trimEnd();
  const semi = s.endsWith(";");
  if (semi) s = s.slice(0, -1).trimEnd();
  const hits = topLevel(s);
  const t: Tail = { head: s, order: null, limit: null, offset: null, semi };
  if (!hits.length) return t;
  t.head = s.slice(0, hits[0].at).trimEnd();
  for (let k = 0; k < hits.length; k++) {
    const h = hits[k];
    const body = s.slice(h.at + h.len, hits[k + 1]?.at ?? s.length).trim();
    if (h.kw === "order by") {
      if (!body) return null;
      t.order = body;
    } else {
      const n = /^\d+$/.test(body) ? Number(body) : h.kw === "limit" && /^all$/i.test(body) ? null : NaN;
      if (Number.isNaN(n)) return null;
      if (h.kw === "limit") t.limit = n;
      else t.offset = n;
    }
  }
  return t;
};

export const buildTail = (t: Tail) =>
  t.head +
  (t.order ? `\norder by ${t.order}` : "") +
  (t.limit !== null ? `\nlimit ${t.limit}` : "") +
  (t.offset ? ` offset ${t.offset}` : "") +
  (t.semi ? ";" : "");

/** แก้เฉพาะส่วนที่ส่งมา แล้วคืน SQL ใหม่ — null ถ้าส่วนท้ายเดิมอ่านไม่ออก */
export const withTail = (sql: string, patch: Partial<Omit<Tail, "head" | "semi">>) => {
  const t = parseTail(sql);
  return t ? buildTail({ ...t, ...patch }) : null;
};

export const quoteIdent = (c: string) => `"${c.replace(/"/g, '""')}"`;

export type FilterOp = "eq" | "ne" | "null" | "notnull" | "like";

const lit = (v: unknown) =>
  typeof v === "number" || typeof v === "boolean"
    ? String(v)
    : `'${(typeof v === "string" ? v : JSON.stringify(v)).replace(/'/g, "''")}'`;

/** เงื่อนไขจากค่าใน cell — like = มีคำนี้อยู่ (ไม่สนตัวเล็กใหญ่) */
export const filterCond = (col: string, op: FilterOp, v: unknown) => {
  const c = quoteIdent(col);
  if (op === "null") return `${c} is null`;
  if (op === "notnull") return `${c} is not null`;
  if (op === "like") {
    const t = (typeof v === "string" ? v : JSON.stringify(v)).replace(/[\\%_]/g, (m) => "\\" + m);
    return `${c}::varchar ilike ${lit(`%${t}%`)}`;
  }
  return `${c} ${op === "eq" ? "=" : "<>"} ${lit(v)}`;
};

const CLAUSE = /^(where|group\s+by|having|window|union|except|intersect)\b/i;
// select list ที่เป็นแค่ * หรือชื่อคอลัมน์ล้วน — ชื่อบนจอตรงกับชื่อจริง ใส่ where ตรง ๆ ได้
const PLAIN = /^select\s+(distinct\s+)?(("[^"]*"|[\w.*]+)\s*,\s*)*("[^"]*"|[\w.*]+)\s+from\s/i;

/** เพิ่มเงื่อนไขเข้า where ของ SELECT แล้วรีเซ็ต offset — null ถ้าส่วนท้ายอ่านไม่ออก
    query ที่มี group by / union / alias → ห่อเป็น subquery แทน (ชื่อคอลัมน์บนจอยังใช้ได้) */
export const addWhere = (sql: string, cond: string) => {
  const t = parseTail(sql);
  if (!t) return null;
  const hits = topLevel(t.head, CLAUSE).map((h) => ({ ...h, kw: h.kw.split(/\s/)[0] }));
  const plain = PLAIN.test(t.head.replace(/^\s*(--[^\n]*\n\s*)*/, "")) && hits.every((h) => h.kw === "where");
  let head: string;
  if (!plain) {
    head = `select *\nfrom (\n${t.head}\n) _f\nwhere ${cond}`;
  } else if (!hits.length) {
    head = `${t.head}\nwhere ${cond}`;
  } else {
    const w = hits[0];
    const body = t.head.slice(w.at + w.len).trim();
    // มี or ชั้นนอกอยู่แล้ว ต้องครอบวงเล็บ ไม่งั้น and ใหม่ไปผูกกับแค่ท่อนท้าย
    const hasOr = topLevel(body, /^(or)\b/i).length > 0;
    head = `${t.head.slice(0, w.at)}where ${hasOr ? `(${body})` : body}\n  and ${cond}`;
  }
  return buildTail({ ...t, head, offset: null });
};
