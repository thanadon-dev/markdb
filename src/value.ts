/* ค่าใน cell สำหรับแผง Value — ข้อความที่เป็น JSON จัดรูปให้อ่านง่าย แล้วแยกท่อนไว้ใส่สี */

export type Pretty = { kind: "json" | "text" | "null"; text: string };

export const pretty = (v: unknown): Pretty => {
  if (v === null || v === undefined) return { kind: "null", text: "NULL" };
  if (typeof v === "object") return { kind: "json", text: JSON.stringify(v, null, 2) };
  const s = String(v);
  // เดาจากตัวแรกก่อน ไม่งั้นตัวเลข / true ล้วน ๆ จะกลายเป็น "JSON" ไปหมด
  if (/^\s*[[{]/.test(s)) {
    try {
      return { kind: "json", text: JSON.stringify(JSON.parse(s), null, 2) };
    } catch {
      /* ขึ้นต้นด้วย { แต่ไม่ใช่ JSON — โชว์ตามจริง */
    }
  }
  return { kind: "text", text: s };
};

export type Tok = { t: "key" | "str" | "num" | "lit" | "punct"; s: string };

const RE =
  /(?<key>"(?:\\.|[^"\\])*"(?=\s*:))|(?<str>"(?:\\.|[^"\\])*")|(?<num>-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)|(?<lit>\b(?:true|false|null)\b)/g;

/** แยก JSON ที่จัดรูปแล้วเป็นท่อน — ต่อ s ทุกท่อนกลับได้ข้อความเดิมเป๊ะ */
export const tokens = (text: string): Tok[] => {
  const out: Tok[] = [];
  let at = 0;
  for (const m of text.matchAll(RE)) {
    if (m.index > at) out.push({ t: "punct", s: text.slice(at, m.index) });
    const g = m.groups!;
    out.push({ t: g.key ? "key" : g.str ? "str" : g.num ? "num" : "lit", s: m[0] });
    at = m.index + m[0].length;
  }
  if (at < text.length) out.push({ t: "punct", s: text.slice(at) });
  return out;
};
