/* จัดกลุ่ม connection ในแถบซ้าย: ตั้งชื่อกลุ่มเอง (group) > host เดียวกันอยู่กลุ่มเดียวกันอัตโนมัติ */

type C = { host: string; url?: string; group?: string };

export const hostOf = (c: C) => {
  if (c.host) return c.host;
  try {
    return new URL(c.url ?? "").hostname || "อื่น ๆ"; // connection แบบเก่าที่เก็บเป็น URL ล้วน
  } catch {
    return "อื่น ๆ";
  }
};

export const groupOf = (c: C) => c.group?.trim() || hostOf(c);

/** เรียงกลุ่มตามลำดับที่เจอครั้งแรก ข้างในคงลำดับเดิมของ connection */
export const grouped = <T extends C>(conns: T[]) => {
  const m = new Map<string, T[]>();
  for (const c of conns) {
    const g = groupOf(c);
    m.set(g, [...(m.get(g) ?? []), c]);
  }
  return [...m].map(([name, items]) => ({ name, items }));
};

/** เปลี่ยนชื่อกลุ่ม = ตั้ง group ให้ทุกตัวในกลุ่มนั้น, ชื่อว่าง = กลับไปจัดตาม host */
export const renameGroup = <T extends C>(conns: T[], from: string, to: string) =>
  conns.map((c) => (groupOf(c) === from ? { ...c, group: to.trim() || undefined } : c));
