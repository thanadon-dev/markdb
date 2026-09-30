import { test } from "node:test";
import assert from "node:assert/strict";
import { risk } from "./guard.ts";

test("แยกคำสั่งอ่าน/เขียน และ update/delete ที่ไม่มี where", () => {
  assert.deepEqual(risk("select * from t"), { write: false, noWhere: null });
  assert.deepEqual(risk("-- ลบ\nDELETE FROM t"), { write: true, noWhere: "delete" });
  assert.deepEqual(risk("delete from t where id = 1"), { write: true, noWhere: null });
  assert.deepEqual(risk("update t set note = 'where'"), { write: true, noWhere: "update" }); // where ใน string ไม่นับ
  assert.deepEqual(risk('update t set "where" = 1'), { write: true, noWhere: "update" });
  assert.deepEqual(risk("update t set a = 1 /* where */"), { write: true, noWhere: "update" });
  assert.equal(risk("with x as (delete from t returning *) select * from x").write, true);
  assert.equal(risk("with x as (select 1) select * from x").write, false);
  assert.equal(risk("truncate t").write, true);
  assert.equal(risk("insert into t values (1)").noWhere, null);
  assert.equal(risk("explain select 1").write, false);
});
