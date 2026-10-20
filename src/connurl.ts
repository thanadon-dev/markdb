/* แตก connection URL (postgres://user:pass@host:port/db?sslmode=require) ออกเป็นช่องในฟอร์ม
   เดิมเก็บ URL ล้วนแล้วใช้ตรง ๆ ช่อง Database / Port / User ที่แก้ทีหลังเลยไม่มีผล
   → connection ที่ host เดียวกันแต่คนละ database ไปต่อ database เดิมกันหมด
   mysql:// / mariadb:// / sqlite:<ไฟล์> บอกชนิด database มาด้วย (postgres:// ไม่บอก — อาจเป็น Redshift) */

export type UrlParts = {
  engine?: "mysql" | "sqlite";
  host: string;
  port: string;
  user: string;
  pass: string;
  db: string;
  ssl: boolean;
};

// shortcut: query param อื่นนอกจาก sslmode / ssl-mode (เช่น options=) ถูกทิ้ง — มีคนต้องใช้ค่อยเพิ่มช่องในฟอร์ม
export const fromUrl = (url: string): UrlParts | null => {
  const t = url.trim();
  // SQLite: path ของไฟล์ (มี \ และ : ของ drive) — URL parser อ่านไม่ได้ ตัดเอง
  if (/^sqlite:/i.test(t)) {
    const db = t.replace(/^sqlite:(\/\/)?/i, "");
    return db ? { engine: "sqlite", host: "", port: "", user: "", pass: "", db, ssl: false } : null;
  }
  let u: URL;
  try {
    u = new URL(t);
  } catch {
    return null;
  }
  const my = /^(mysql|mariadb):$/.test(u.protocol);
  if ((!my && !/^postgres(ql)?:$/.test(u.protocol)) || !u.hostname) return null;
  const ssl = u.searchParams.get("sslmode") ?? u.searchParams.get("ssl-mode");
  return {
    ...(my ? { engine: "mysql" as const } : {}),
    host: u.hostname,
    port: u.port,
    user: decodeURIComponent(u.username),
    pass: decodeURIComponent(u.password),
    db: decodeURIComponent(u.pathname.slice(1)),
    ssl: !!ssl && !/^(disable|disabled|prefer|preferred)$/i.test(ssl),
  };
};
