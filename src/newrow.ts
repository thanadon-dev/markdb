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

/* ข้อความที่ก๊อปจาก Excel / Google Sheets / MarkDB เอง: แถวคั่นด้วย newline คอลัมน์คั่นด้วย tab
   ช่องที่มี tab / ขึ้นบรรทัด / " อยู่ข้างใน จะถูกครอบด้วย "…" และ " ข้างในเป็น "" */
export const parseTsv = (text: string): string[][] => {
  const rows: string[][] = [];
  let row: string[] = [];
  let cell = "";
  let i = 0;
  const s = text.replace(/\r\n?/g, "\n");
  while (i < s.length) {
    const c = s[i];
    if (c === '"' && cell === "") {
      const m = /^"((?:[^"]|"")*)"(?=\t|\n|$)/.exec(s.slice(i));
      if (m) {
        cell = m[1].replace(/""/g, '"');
        i += m[0].length;
        continue;
      }
    }
    if (c === "\t") {
      row.push(cell);
      cell = "";
    } else if (c === "\n") {
      row.push(cell);
      rows.push(row);
      row = [];
      cell = "";
    } else cell += c;
    i++;
  }
  if (cell !== "" || row.length) {
    row.push(cell);
    rows.push(row);
  }
  return rows;
};

/** ช่องที่จะโดนวางทับ: ค่าเดียว + คลุมหลายช่อง = เติมทุกช่องที่คลุม (แบบ Excel)
    นอกนั้นวางเริ่มที่มุมซ้ายบนของที่คลุม ส่วนที่เกินขอบตารางตัดทิ้ง */
export const pasteCells = (
  data: string[][],
  box: { r1: number; c1: number; r2: number; c2: number },
  rows: number,
  cols: number,
) => {
  const out: { r: number; c: number; v: string }[] = [];
  const one = data.length === 1 && data[0].length === 1;
  const h = one ? box.r2 - box.r1 + 1 : data.length;
  const w = one ? box.c2 - box.c1 + 1 : Math.max(...data.map((d) => d.length));
  let cut = 0;
  for (let y = 0; y < h; y++)
    for (let x = 0; x < w; x++) {
      const v = one ? data[0][0] : data[y][x];
      if (v === undefined) continue;
      const r = box.r1 + y;
      const c = box.c1 + x;
      if (r >= rows || c >= cols) cut++;
      else out.push({ r, c, v });
    }
  return { cells: out, cut };
};
