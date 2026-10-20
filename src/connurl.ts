/* แตก connection URL (postgres://user:pass@host:port/db?sslmode=require) ออกเป็นช่องในฟอร์ม
   เดิมเก็บ URL ล้วนแล้วใช้ตรง ๆ ช่อง Database / Port / User ที่แก้ทีหลังเลยไม่มีผล
   → connection ที่ host เดียวกันแต่คนละ database ไปต่อ database เดิมกันหมด */

export type UrlParts = { host: string; port: string; user: string; pass: string; db: string; ssl: boolean };

// shortcut: query param อื่นนอกจาก sslmode (เช่น options=) ถูกทิ้ง — มีคนต้องใช้ค่อยเพิ่มช่องในฟอร์ม
export const fromUrl = (url: string): UrlParts | null => {
  let u: URL;
  try {
    u = new URL(url.trim());
  } catch {
    return null;
  }
  if (!/^postgres(ql)?:$/.test(u.protocol) || !u.hostname) return null;
  const ssl = u.searchParams.get("sslmode");
  return {
    host: u.hostname,
    port: u.port,
    user: decodeURIComponent(u.username),
    pass: decodeURIComponent(u.password),
    db: decodeURIComponent(u.pathname.slice(1)),
    ssl: !!ssl && ssl !== "disable",
  };
};
