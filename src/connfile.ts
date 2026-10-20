/* แชร์รายการ connection ให้คนในทีมเป็นไฟล์ .json — ไม่มี password เด็ดขาด
   import: ไฟล์มาจากคนอื่น จึงหยิบเฉพาะช่องที่รู้จักและชนิดถูก, ข้ามตัวที่ซ้ำ (host+port+db+user เดียวกัน) */

type C = {
  id: string;
  name: string;
  engine: "postgres" | "redshift" | "sqlite" | "mysql";
  host: string;
  port: string;
  user: string;
  pass: string;
  db: string;
  ssl: boolean;
  url?: string;
  prod?: boolean;
  readonly?: boolean;
  group?: string;
  ssh?: { on: boolean; host: string; port: string; user: string; auth: "password" | "key"; keyPath?: string; fingerprint?: string };
  sshPass?: string;
};

const same = (c: Pick<C, "host" | "port" | "db" | "user">) => [c.host, c.port, c.db, c.user].join("\n").toLowerCase();

export const toFile = (cs: C[]) =>
  cs.map(({ name, engine, host, port, user, db, ssl, prod, readonly, group, ssh }) => ({
    name, engine, host, port, user, db, ssl, prod, readonly, group,
    // SSH: ส่งที่อยู่ + user + host key ไปด้วย (ไม่มี password / passphrase) — path ของ key เป็นของเครื่องเรา ไม่ส่ง
    ssh: ssh && { on: ssh.on, host: ssh.host, port: ssh.port, user: ssh.user, auth: ssh.auth, fingerprint: ssh.fingerprint },
  }));

const sshOf = (v: unknown): C["ssh"] => {
  if (!v || typeof v !== "object") return undefined;
  const s = v as Record<string, unknown>;
  if (typeof s.host !== "string" || typeof s.user !== "string") return undefined;
  return {
    on: s.on === true,
    host: s.host,
    port: typeof s.port === "string" ? s.port : "22",
    user: s.user,
    auth: s.auth === "key" ? "key" : "password",
    fingerprint: typeof s.fingerprint === "string" ? s.fingerprint : undefined,
  };
};

export const fromFile = <T extends C>(text: string, have: T[], blank: T, uid: () => string) => {
  const data: unknown = JSON.parse(text);
  if (!Array.isArray(data)) throw new Error("ไม่ใช่ไฟล์รายการ connection ของ MarkDB");
  const str = (v: unknown) => (typeof v === "string" ? v : undefined);
  const seen = new Set(have.map(same));
  const add: T[] = [];
  let skipped = 0;
  for (const x of data as Record<string, unknown>[]) {
    if (!x || typeof x.host !== "string" || typeof x.db !== "string")
      throw new Error("ไฟล์ไม่ถูกรูปแบบ — ทุกรายการต้องมี host และ db");
    const c: T = {
      ...blank,
      id: uid(),
      name: str(x.name) ?? x.db,
      engine: x.engine === "redshift" || x.engine === "sqlite" || x.engine === "mysql" ? x.engine : "postgres",
      host: x.host,
      port: str(x.port) ?? blank.port,
      user: str(x.user) ?? blank.user,
      pass: "",
      db: x.db,
      ssl: x.ssl === true,
      prod: x.prod === true || undefined,
      readonly: x.readonly === true || undefined,
      group: str(x.group)?.trim() || undefined,
      ssh: sshOf(x.ssh),
      sshPass: undefined,
    };
    if (seen.has(same(c))) {
      skipped++;
      continue;
    }
    seen.add(same(c));
    add.push(c);
  }
  return { add, skipped };
};
