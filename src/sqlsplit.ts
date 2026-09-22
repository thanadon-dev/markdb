/* ซอย doc เป็น statement — ตัดด้วย ; และด้วยบรรทัดว่าง (เคาะ Enter คั่นไว้ 1 บรรทัด)
   ข้าม ; ที่อยู่ใน string หรือ comment
   to = ตำแหน่งท้ายสุดที่ยังนับว่าเคอร์เซอร์อยู่ใน statement นี้ (รวมตัวคั่นด้วย)
   ponytail: ไม่รองรับ dollar-quote ($$...$$) — เจอเคสนั้นค่อยลากคลุมเอา */
const split = (doc: string) => {
  const out: { from: number; end: number; bound: number }[] = [];
  let start = 0;
  // ตัดหัวท้ายที่เป็นช่องว่างออกตั้งแต่ตอนซอย ตำแหน่งที่ได้จะเอาไปไฮไลต์ได้ตรง ๆ
  const push = (end: number, next: number) => {
    let a = start;
    let b = end;
    while (a < b && /\s/.test(doc[a])) a++;
    while (b > a && /\s/.test(doc[b - 1])) b--;
    if (a < b) out.push({ from: a, end: b, bound: next });
    start = next;
  };
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
      push(i, i + 1); // ตัว ; ไม่ต้องส่งไปด้วย แต่เคอร์เซอร์ที่ยืนหลัง ; ยังนับเป็นคำสั่งนี้
      continue;
    }
    if (c === "\n") {
      let j = i + 1;
      while (j < doc.length && (doc[j] === " " || doc[j] === "\t" || doc[j] === "\r")) j++;
      if (doc[j] === "\n") {
        push(i, j + 1); // มีบรรทัดว่างคั่น = คนละคำสั่ง ถึงจะไม่มี ; ก็ตาม
        i = j;
      }
    }
  }
  push(doc.length, doc.length);
  return out;
};

/* ช่วงของ statement ที่เคอร์เซอร์ยืนอยู่ — ยืนหลังบรรทัดสุดท้ายของก้อนไหน ก็ได้ก้อนนั้น */
export const stmtRangeAt = (doc: string, pos: number) => {
  const segs = split(doc);
  const hit = segs.find((s) => pos <= s.bound) ?? segs[segs.length - 1];
  return hit ? { from: hit.from, to: hit.end } : { from: 0, to: 0 };
};

export const stmtAt = (doc: string, pos: number) => {
  const { from, to } = stmtRangeAt(doc, pos);
  return doc.slice(from, to);
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
