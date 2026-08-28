import { test } from "node:test";
import assert from "node:assert/strict";
import { stmtAt, targetTable } from "./sqlsplit.ts";

const at = (doc: string, marker: string) => stmtAt(doc, doc.indexOf(marker)).trim();

test("เลือก statement ตามตำแหน่งเคอร์เซอร์", () => {
  const doc = "select 1;\nselect 2;\nselect 3";
  assert.equal(at(doc, "1"), "select 1");
  assert.equal(at(doc, "2"), "select 2");
  assert.equal(at(doc, "3"), "select 3");
});

test("; ใน string ไม่นับเป็นตัวคั่น", () => {
  const doc = "select * from t where a = 'x;y' and b = 2;\nselect 9";
  assert.equal(at(doc, "where"), "select * from t where a = 'x;y' and b = 2");
  assert.equal(at(doc, "9"), "select 9");
});

test("'' คือ escape ไม่ใช่ปิด string", () => {
  const doc = "select 'it''s; ok' as a;\nselect 2";
  assert.equal(at(doc, "as a"), "select 'it''s; ok' as a");
});

test("; ใน comment ไม่นับ", () => {
  const doc = "select 1 -- a; b\nfrom t;\nselect 2";
  assert.equal(at(doc, "from t"), "select 1 -- a; b\nfrom t");
  const blk = "select 1 /* a; b */ from t;\nselect 2";
  assert.equal(at(blk, "from t"), "select 1 /* a; b */ from t");
});

test("ไม่มี ; เลย = ทั้งก้อน", () => {
  assert.equal(at("select * from t", "from"), "select * from t");
});

const tt = (sql: string) => {
  const t = targetTable(sql);
  return t ? (t.schema ? `${t.schema}.${t.name}` : t.name) : null;
};

test("หาตารางที่เขียนกลับได้จาก select ธรรมดา", () => {
  assert.equal(tt("select * from users"), "users");
  assert.equal(tt("select * from users;"), "users");
  assert.equal(tt('select *\nfrom "public"."users"\nlimit 500;'), "public.users");
  assert.equal(tt("select id, name from sales.orders where id = 3 order by id"), "sales.orders");
  assert.equal(tt("SELECT * FROM Users u WHERE u.id = 1"), "Users");
  assert.equal(tt("select * from users limit 10"), "users");
  assert.equal(tt("select * from users -- from orders\n"), "users");
});

test("ปิดการแก้ค่าเมื่อไม่รู้ว่าแถวมาจากตารางไหน", () => {
  assert.equal(tt("select * from a join b on a.id = b.a_id"), null);
  assert.equal(tt("select * from a, b"), null);
  assert.equal(tt("select * from (select 1) x"), null);
  assert.equal(tt("select * from a union select * from b"), null);
  assert.equal(tt("insert into users values (1)"), null);
  assert.equal(tt("update users set a = 1"), null);
  assert.equal(tt("select 1"), null);
  assert.equal(tt(""), null);
});
