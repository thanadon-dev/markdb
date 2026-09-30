import { test } from "node:test";
import assert from "node:assert/strict";
import { add, search, type Ran } from "./queries.ts";

test("รันซ้ำ = ขยับขึ้นบนสุด ไม่เพิ่มแถว, ค้นได้หลายคำ", () => {
  const r = (id: string, sql: string, conn = "c1") => ({ id, at: 0, conn, connName: conn === "c1" ? "dev" : "uat", sql });
  let l: Ran[] = [];
  l = add(l, r("1", "select * from a"));
  l = add(l, r("2", "select * from b"));
  l = add(l, r("3", "select *  from a;"));
  assert.deepEqual(l.map((x) => [x.id, x.n]), [["3", 2], ["2", 1]]);
  l = add(l, r("4", "select * from a", "c2")); // คนละ connection ถือเป็นคนละรายการ
  assert.equal(l.length, 3);
  assert.deepEqual(search(l, "FROM a").map((x) => x.id), ["4", "3"]);
  assert.deepEqual(search(l, "a uat").map((x) => x.id), ["4"]);
  assert.equal(search(l, "  ").length, 3);
});
