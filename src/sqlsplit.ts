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
