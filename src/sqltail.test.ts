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
