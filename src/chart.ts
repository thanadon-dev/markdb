/* กราฟจากผลลัพธ์ — เลือกคอลัมน์ตั้งต้นให้ + สเกลแกนที่ตัวเลขกลม ๆ อ่านง่าย */

type Row = Record<string, unknown>;

export type ChartKind = "bar" | "line";
export type ChartConf = { kind: ChartKind; x: string; ys: string[] };

/** ชุดข้อมูลสูงสุด — palette ผ่านการตรวจสีสำหรับ 4 ชุดแรกที่วางติดกัน */
export const MAX_SERIES = 4;
export const MAX_BARS = 60;
export const MAX_POINTS = 2000;

const isNum = (v: unknown) =>
  typeof v === "number" || (typeof v === "string" && v.trim() !== "" && Number.isFinite(Number(v)));
const isDate = (v: unknown) => typeof v === "string" && /^\d{4}-\d{2}-\d{2}/.test(v);
const idLike = (c: string) => /(^id$|_id$|[a-z]Id$)/.test(c);

export const numericCols = (cols: string[], rows: Row[]) =>
  cols.filter((c) => rows.some((r) => r[c] != null) && rows.every((r) => r[c] == null || isNum(r[c])));

/** X = คอลัมน์แรกที่ไม่ใช่ตัวเลข (ไม่มีก็คอลัมน์แรก), Y = ตัวเลขที่ไม่ใช่ id, วันที่ → กราฟเส้น */
export const defaults = (cols: string[], rows: Row[]): ChartConf => {
  const nums = numericCols(cols, rows);
  const x = cols.find((c) => !nums.includes(c)) ?? cols[0];
  const ys = nums.filter((c) => c !== x && !idLike(c));
  const time = rows.length > 1 && rows.every((r) => r[x] == null || isDate(r[x]));
  return {
    kind: time ? "line" : "bar",
    x,
    ys: (ys.length ? ys : nums.filter((c) => c !== x)).slice(0, MAX_SERIES),
  };
};

/** สเกลแกน: ขั้นละ 1 / 2 / 2.5 / 5 × 10^n ครอบช่วงข้อมูล (และครอบ 0 เสมอ — แท่งงอกจากเส้นฐานเดียว) */
export const ticks = (lo: number, hi: number, n = 5) => {
  lo = Math.min(0, lo);
  hi = Math.max(0, hi);
  if (hi === lo) hi = lo + 1;
  const raw = (hi - lo) / n;
  const mag = 10 ** Math.floor(Math.log10(raw));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((s) => s >= raw)!;
  const out: number[] = [];
  for (let v = Math.floor(lo / step) * step; v <= hi + step * 1e-9; v += step) out.push(Number(v.toPrecision(12)));
  if (out[out.length - 1] < hi) out.push(Number((out[out.length - 1] + step).toPrecision(12)));
  return out;
};

const fmt = new Intl.NumberFormat("en", { notation: "compact", maximumFractionDigits: 1 });
export const compact = (v: number) => (Math.abs(v) < 1000 ? String(Number(v.toPrecision(6))) : fmt.format(v));

/** แถว → จุด (x เป็นข้อความเสมอ, y ที่ว่าง/ไม่ใช่ตัวเลข = null) */
export const points = (rows: Row[], c: ChartConf) =>
  rows.slice(0, c.kind === "bar" ? MAX_BARS : MAX_POINTS).map((r) => ({
    x: r[c.x] == null ? "NULL" : String(r[c.x]),
    ys: c.ys.map((y) => (isNum(r[y]) ? Number(r[y]) : null)),
  }));
