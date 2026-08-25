import { test } from "node:test";
import assert from "node:assert/strict";
import { stmtAt } from "./sqlsplit.ts";

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
