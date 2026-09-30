import { test } from "node:test";
import assert from "node:assert/strict";
import { defaultLiteral, suggest } from "./newrow.ts";

test("default ค่าคงที่ vs ฟังก์ชัน", () => {
  assert.equal(defaultLiteral("'draft'::character varying"), "draft");
  assert.equal(defaultLiteral("'it''s'::text"), "it's");
  assert.equal(defaultLiteral("true"), "true");
  assert.equal(defaultLiteral("(0)::numeric"), "0");
  assert.equal(defaultLiteral("NULL::integer"), null);
  assert.equal(defaultLiteral("now()"), undefined);
  assert.equal(defaultLiteral("nextval('t_id_seq'::regclass)"), undefined);
  assert.equal(defaultLiteral("gen_random_uuid()"), undefined);
});

test("Tab เติมตามชนิดคอลัมน์", () => {
  const now = new Date(2026, 8, 30, 14, 5, 9);
  const s = (name: string, data_type: string, dflt = "", nullable = true) =>
    suggest({ name, data_type, default: dflt, nullable }, now, () => "u-1");
  assert.deepEqual(s("id", "integer", "nextval('t_id_seq'::regclass)"), { hint: "auto", fill: null });
  assert.deepEqual(s("id", "uuid", "gen_random_uuid()"), { hint: "gen_random_uuid()", fill: null });
  assert.equal(s("ref", "uuid").fill, "u-1");
  assert.equal(s("at", "timestamp without time zone").fill, "2026-09-30 14:05:09");
  assert.match(s("at", "timestamp with time zone").fill!, /^2026-09-30 14:05:09[+-]\d\d:\d\d$/);
  assert.equal(s("d", "date").fill, "2026-09-30");
  assert.equal(s("IsActive", "boolean").fill, "true");
  assert.equal(s("is_active", "boolean").fill, "true");
  assert.equal(s("IsDelete", "boolean").fill, "false");
  assert.equal(s("IsActive", "boolean", "false").fill, "false"); // default ของ DB ชนะชื่อคอลัมน์
  assert.deepEqual(s("status", "text", "'new'::text"), { hint: "new", fill: "new" });
  assert.deepEqual(s("note", "text"), { hint: "NULL", fill: null });
  assert.deepEqual(s("name", "text", "", false), { hint: "text", fill: null });
});

test("อ่าน TSV จาก Excel", async () => {
  const { parseTsv } = await import("./newrow.ts");
  assert.deepEqual(parseTsv("a\tb\r\n1\t2\r\n"), [["a", "b"], ["1", "2"]]);
  assert.deepEqual(parseTsv('"x\ty"\t"line1\nline2"\t"say ""hi"""\n'), [["x\ty", "line1\nline2", 'say "hi"']]);
  assert.deepEqual(parseTsv("a\t\tc"), [["a", "", "c"]]);
  assert.deepEqual(parseTsv('5" pipe\tz'), [['5" pipe', "z"]]);
  assert.deepEqual(parseTsv("only"), [["only"]]);
});

test("วางทับ: ค่าเดียวเติมทั้งช่วง, หลายค่าวางจากมุม และตัดส่วนเกิน", async () => {
  const { pasteCells } = await import("./newrow.ts");
  const fill = pasteCells([["x"]], { r1: 0, c1: 1, r2: 1, c2: 2 }, 10, 5);
  assert.deepEqual(fill, { cells: [{ r: 0, c: 1, v: "x" }, { r: 0, c: 2, v: "x" }, { r: 1, c: 1, v: "x" }, { r: 1, c: 2, v: "x" }], cut: 0 });
  const block = pasteCells([["a", "b"], ["c", "d"]], { r1: 2, c1: 4, r2: 2, c2: 4 }, 3, 5);
  assert.deepEqual(block, { cells: [{ r: 2, c: 4, v: "a" }], cut: 3 });
});
