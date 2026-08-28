/* หา statement ที่เคอร์เซอร์อยู่ — ตัดด้วย ; แต่ข้าม ; ที่อยู่ใน string หรือ comment
   ponytail: ไม่รองรับ dollar-quote ($$...$$) — เจอเคสนั้นค่อยลากคลุมเอา */
export const stmtAt = (doc: string, pos: number) => {
  let start = 0;
  for (let i = 0; i < doc.length; i++) {
    const c = doc[i];
    if (c === "'" || c === '"') {
      const q = c;
      i++;
      while (i < doc.length) {
        if (doc[i] === q) {
          if (doc[i + 1] !== q) break; // '' คือ escape ไม่ใช่ปิด string
          i++;
        }
        i++;
      }
      continue;
    }
    if (c === "-" && doc[i + 1] === "-") {
      while (i < doc.length && doc[i] !== "\n") i++;
      continue;
    }
    if (c === "/" && doc[i + 1] === "*") {
      i += 2;
      while (i < doc.length && !(doc[i] === "*" && doc[i + 1] === "/")) i++;
      i++;
      continue;
    }
    if (c === ";") {
      if (i >= pos) return doc.slice(start, i);
      start = i + 1;
    }
  }
  return doc.slice(start);
};

/* ตารางที่ผลลัพธ์ของ query นี้เขียนกลับไปได้ — ต้องเป็น select จากตารางเดียวจริง ๆ
   join / union / subquery ใน from จะไม่ match แล้วปิดการแก้ค่าไปเลย เพราะไม่รู้ว่า
   แถวที่เห็นมาจากตารางไหน (ฝั่ง server ยังกันซ้ำอีกชั้นด้วยการนับแถวที่โดนก่อน commit) */
const CLAUSE_RE =
  /\b(where|group|having|order|limit|offset|window|union|except|intersect|fetch|for)\b/i;
const ONE_TABLE_RE = /^"?([\w$]+)"?(?:\s*\.\s*"?([\w$]+)"?)?(?:\s+(?:as\s+)?[a-z_][\w$]*)?$/i;

export const targetTable = (sql: string): { schema?: string; name: string } | null => {
  const clean = sql.replace(/--[^\n]*/g, " ").replace(/\/\*[\s\S]*?\*\//g, " ").trim();
  if (!/^select\b/i.test(clean)) return null;
  const i = clean.search(/\bfrom\b/i);
  if (i < 0) return null;
  let rest = clean.slice(i + 4);
  const c = rest.search(CLAUSE_RE);
  if (c >= 0) {
    // union/except/intersect = แถวมาจากหลายตาราง เขียนกลับไม่ได้
    if (/^(union|except|intersect)/i.test(rest.slice(c))) return null;
    rest = rest.slice(0, c);
  }
  const m = ONE_TABLE_RE.exec(rest.trim().replace(/;+$/, "").trim());
  if (!m) return null;
  return m[2] ? { schema: m[1], name: m[2] } : { name: m[1] };
};
