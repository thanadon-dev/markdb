/* แถวใหม่ท้ายตาราง: คำใบ้สีเทาในแต่ละช่อง + ค่าที่ Tab เติมให้
   ช่องที่ปล่อยว่างจะไม่ส่งไปตอน insert — DB ใส่ DEFAULT ของมันเอง (serial, gen_random_uuid(), now()) */

export type ColMeta = { name: string; data_type: string; nullable: boolean; default: string };

/** hint = ตัวหนังสือเทาในช่องว่าง, fill = ค่าที่ Tab เติม (null = ไม่เติม ปล่อยให้ DB) */
export type Suggest = { hint: string; fill: string | null };

/* default ที่เป็นค่าคงที่ เช่น 'abc'::text, true, 0, NULL::integer → ค่านั้น
   ที่เป็นฟังก์ชัน (now(), nextval(...), gen_random_uuid()) → undefined ให้ DB คิดเอง */
export const defaultLiteral = (d: string): string | null | undefined => {
  let s = d.trim();
  for (let prev = ""; prev !== s; ) {
    prev = s;
    s = s.replace(/(::[\w\s".[\]]+)+$/, "").trim();
    if (s.startsWith("(") && s.endsWith(")")) s = s.slice(1, -1).trim();
  }
  const str = /^'((?:[^']|'')*)'$/.exec(s);
  if (str) return str[1].replace(/''/g, "'");
  if (/^(true|false)$/i.test(s)) return s.toLowerCase();
  if (/^-?\d+(\.\d+)?$/.test(s)) return s;
  if (/^null$/i.test(s)) return null;
  return undefined;
};

const pad = (n: number) => String(n).padStart(2, "0");
const day = (d: Date) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
const clock = (d: Date) => `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
const offset = (d: Date) => {
  const m = -d.getTimezoneOffset();
  return `${m < 0 ? "-" : "+"}${pad(Math.floor(Math.abs(m) / 60))}:${pad(Math.abs(m) % 60)}`;
};

export const suggest = (c: ColMeta, now = new Date(), uuid = () => crypto.randomUUID()): Suggest => {
  const t = c.data_type.toLowerCase();
  if (c.default) {
    const lit = defaultLiteral(c.default);
    if (lit === undefined) return { hint: /nextval|identity/i.test(c.default) ? "auto" : c.default, fill: null };
    if (lit === null) return { hint: "NULL", fill: null };
    return { hint: lit, fill: lit };
  }
  if (t === "uuid") return { hint: "Tab → uuid", fill: uuid() };
  if (t.startsWith("timestamp")) {
    const v = `${day(now)} ${clock(now)}`;
    return { hint: "Tab → ตอนนี้", fill: t.includes("with time zone") || t === "timestamptz" ? v + offset(now) : v };
  }
  if (t === "date") return { hint: "Tab → วันนี้", fill: day(now) };
  if (t.startsWith("time")) return { hint: "Tab → ตอนนี้", fill: clock(now) };
  if (t === "boolean" || t === "bool") {
    const v = /^is_?active$/i.test(c.name) ? "true" : "false";
    return { hint: v, fill: v };
  }
  return { hint: c.nullable ? "NULL" : t, fill: null };
};
