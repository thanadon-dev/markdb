import { test } from "node:test";
import assert from "node:assert/strict";
import { parseTail, withTail } from "./sqltail.ts";

test("แยก order by / limit / offset ส่วนท้าย", () => {
  const t = parseTail('select *\nfrom "public"."t"\norder by id desc\nlimit 15 offset 30;');
  assert.deepEqual(t, {
    head: 'select *\nfrom "public"."t"',
    order: "id desc",
    limit: 15,
    offset: 30,
    semi: true,
  });
});

test("เปลี่ยน limit แล้วรูปแบบเดิมไม่เพี้ยน", () => {
  assert.equal(withTail('select *\nfrom "public"."t"\nlimit 15;', { limit: 50 }), 'select *\nfrom "public"."t"\nlimit 50;');
  assert.equal(withTail("select * from t", { limit: 15 }), "select * from t\nlimit 15");
  assert.equal(withTail("select * from t limit 15", { limit: 15, offset: 15 }), "select * from t\nlimit 15 offset 15");
  // แสดงทั้งหมด = เอา limit/offset ออก
  assert.equal(withTail("select * from t limit 15 offset 45;", { limit: null, offset: null }), "select * from t;");
});

test("order by ใส่ก่อน limit และแทนของเดิม", () => {
  assert.equal(withTail("select * from t limit 15", { order: '"cc" desc', offset: null }), 'select * from t\norder by "cc" desc\nlimit 15');
  assert.equal(withTail("select * from t order by a, b limit 5", { order: null }), "select * from t\nlimit 5");
});

test("ข้างใน () string และ comment ไม่นับ", () => {
  const sql =
    "select id, row_number() over (order by cc) from t where x in (select y from u order by y limit 1) and n = 'limit 9' -- limit 3\n/* order by z */";
  const t = parseTail(sql);
  assert.equal(t?.head, sql);
  assert.equal(t?.limit, null);
  assert.equal(withTail(sql, { limit: 15 }), sql + "\nlimit 15");
});

test("คอลัมน์ชื่อคล้ายคีย์เวิร์ดไม่โดน", () => {
  assert.equal(parseTail("select limited, offsets from t")?.limit, null);
});

test("ส่วนท้ายที่อ่านไม่ออกไม่แตะ", () => {
  assert.equal(parseTail("select * from t limit $1"), null);
  assert.equal(parseTail("select * from t order by"), null);
  assert.equal(parseTail("select * from t limit all")?.limit, null);
});

test("กรองจาก cell: เพิ่ม where / and / ห่อ subquery", async () => {
  const { addWhere, filterCond } = await import("./sqltail.ts");
  assert.equal(filterCond("name", "eq", "o'b"), `"name" = 'o''b'`);
  assert.equal(filterCond("id", "ne", 5), `"id" <> 5`);
  assert.equal(filterCond("ok", "eq", true), `"ok" = true`);
  assert.equal(filterCond("x", "null", null), `"x" is null`);
  assert.equal(filterCond("n", "like", "50%_a"), String.raw`"n"::varchar ilike '%50\%\_a%'`);
  assert.equal(filterCond("n", "like", String.raw`c:\x`), String.raw`"n"::varchar ilike '%c:\\x%'`);
  assert.equal(
    addWhere('select *\nfrom "public"."t"\nlimit 15 offset 30;', `"id" = 1`),
    'select *\nfrom "public"."t"\nwhere "id" = 1\nlimit 15;',
  );
  assert.equal(
    addWhere("select * from t where a = 1\norder by b\nlimit 5", `"c" = 2`),
    'select * from t where a = 1\n  and "c" = 2\norder by b\nlimit 5',
  );
  assert.equal(
    addWhere("select * from t where a = 1 or b = 2", `"c" = 2`),
    'select * from t where (a = 1 or b = 2)\n  and "c" = 2',
  );
  // where ใน subquery ไม่นับ, or ใน subquery ไม่ต้องครอบ
  assert.equal(
    addWhere("select id, name from t where id in (select id from u where a or b)", `"name" = 'x'`),
    `select id, name from t where id in (select id from u where a or b)\n  and "name" = 'x'`,
  );
  assert.equal(
    addWhere("select kind, count(*) as n from t group by kind\nlimit 15", `"n" = 3`),
    'select *\nfrom (\nselect kind, count(*) as n from t group by kind\n) _f\nwhere "n" = 3\nlimit 15',
  );
  assert.equal(addWhere("select a as b from t", `"b" = 1`), 'select *\nfrom (\nselect a as b from t\n) _f\nwhere "b" = 1');
});
