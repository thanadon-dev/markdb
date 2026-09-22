/* ประวัติคำสั่งที่เปลี่ยนข้อมูล เก็บไว้ใน localStorage ของเครื่องนี้เท่านั้น
   ponytail: ไม่ใช่ audit log ระดับ server — คนอื่นแก้จากที่อื่นจะไม่โผล่ที่นี่
   และเก็บได้เฉพาะค่าที่หน้าจอเห็นตอนนั้น (select บางคอลัมน์ = ย้อนได้แค่นั้น) */

export type Val = string | null;

export type Entry = {
  id: string;
  at: number;
  conn: string;
  connName: string;
  table: string;
  kind: "update" | "delete" | "truncate" | "drop";
  /** update: คอลัมน์ที่แก้ */
  column?: string;
  /** update: ค่าก่อนแก้ = ค่าที่จะย้อนกลับไป */
  before?: Val;
  /** update: ค่าหลังแก้ */
  after?: Val;
  /** pk ของแถวตอนนั้น — ไม่มี = ย้อนไม่ได้ */
  keys?: { column: string; value: Val }[];
  /** delete: ค่าทั้งแถวที่ถูกลบ */
  row?: Record<string, Val>;
  /** ย้อนกลับไปแล้ว */
  undone?: boolean;
  /** รายการนี้เกิดจากการกดย้อนกลับ */
  isRevert?: boolean;
};

export type Plan =
  | { cmd: "update_cell" | "insert_row"; args: Record<string, unknown>; summary: string }
  | { reason: string };

export const KEY = "markdb.history.v1";
const CAP = 300;

export const show = (v: Val | undefined) =>
  v === null || v === undefined ? "NULL" : v === "" ? "(ว่าง)" : v;

export const load = (): Entry[] => {
  try {
    const a = JSON.parse(localStorage.getItem(KEY) || "[]");
    return Array.isArray(a) ? a : [];
  } catch {
    return []; // localStorage เสีย — เริ่มประวัติใหม่ ดีกว่าแอปเปิดไม่ขึ้น
  }
};

export const save = (list: Entry[]) => {
  try {
    localStorage.setItem(KEY, JSON.stringify(list.slice(0, CAP)));
  } catch {
    /* เต็มก็ช่างมัน ประวัติไม่ใช่ข้อมูลหลัก */
  }
};

export const add = (list: Entry[], e: Entry) => [e, ...list].slice(0, CAP);

/* คำสั่งที่จะพาข้อมูลกลับไปเป็นเหมือนก่อนหน้า — ใช้ command เดิมที่มีอยู่แล้ว
   ไม่ได้ต่อ SQL เองใหม่ จะได้ผ่านด่านนับแถว (ต้องโดน 1 แถว) เหมือนตอนแก้ปกติ */
export const revertPlan = (e: Entry): Plan => {
  if (e.undone) return { reason: "รายการนี้ย้อนกลับไปแล้ว" };
  if (e.kind === "update") {
    if (!e.keys?.length) return { reason: "ไม่ได้เก็บ primary key ของแถวนี้ไว้ ย้อนกลับไม่ได้" };
    return {
      cmd: "update_cell",
      args: { table: e.table, column: e.column, value: e.before ?? null, keys: e.keys },
      summary: `update ${e.table} set ${e.column} = ${show(e.before)}`,
    };
  }
  if (e.kind === "delete") {
    if (!e.row || !Object.keys(e.row).length)
      return { reason: "ไม่ได้เก็บค่าของแถวไว้ ย้อนกลับไม่ได้" };
    return {
      cmd: "insert_row",
      args: {
        table: e.table,
        values: Object.entries(e.row).map(([column, value]) => ({ column, value })),
      },
      summary: `insert into ${e.table} (${Object.keys(e.row).length} คอลัมน์)`,
    };
  }
  return {
    reason:
      e.kind === "truncate"
        ? "TRUNCATE ย้อนกลับไม่ได้ — ข้อมูลไม่ได้ถูกเก็บไว้ ต้องกู้จากไฟล์ Backup"
        : "DROP ย้อนกลับไม่ได้ — ทั้งตารางหายไปแล้ว ต้องกู้จากไฟล์ Backup",
  };
};

/* รายการที่บันทึกหลังกดย้อนกลับสำเร็จ — ย้อนของย้อนได้อีกที ประวัติจึงตรงกับของจริงเสมอ */
export const reverseOf = (e: Entry, id: string, at: number): Entry | null =>
  e.kind === "update"
    ? { ...e, id, at, before: e.after ?? null, after: e.before ?? null, undone: false, isRevert: true }
    : null;
