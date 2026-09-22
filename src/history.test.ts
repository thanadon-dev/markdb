import { test } from "node:test";
import assert from "node:assert/strict";
import { add, revertPlan, reverseOf, show, type Entry } from "./history.ts";

/* ทุกเทสต์ในไฟล์นี้เป็น logic ล้วน ไม่แตะ database จริงสักตัว */

const base = {
  id: "1",
  at: 1000,
  conn: "c1",
  connName: "local",
  table: '"public"."users"',
};

const upd: Entry = {
  ...base,
  kind: "update",
  column: "email",
  before: "a@x.com",
  after: "พิมพ์มั่ว",
  keys: [{ column: "id", value: "7" }],
};

test("ย้อน update = เขียนค่าเดิมกลับผ่าน update_cell เดิม", () => {
  const p = revertPlan(upd);
  assert.deepEqual(p, {
    cmd: "update_cell",
    args: {
      table: '"public"."users"',
      column: "email",
      value: "a@x.com",
      keys: [{ column: "id", value: "7" }],
    },
    summary: 'update "public"."users" set email = a@x.com',
  });
});

test("ค่าเดิมเป็น NULL ก็ย้อนกลับไปเป็น NULL ได้ ไม่ใช่สตริงว่าง", () => {
  const p = revertPlan({ ...upd, before: null });
  assert.equal("args" in p && p.args.value, null);
  assert.equal(show(null), "NULL");
  assert.equal(show(""), "(ว่าง)");
});

test("ย้อน delete = insert ทุกคอลัมน์ที่เก็บไว้กลับเข้าไป", () => {
  const del: Entry = {
    ...base,
    kind: "delete",
    row: { id: "7", email: "a@x.com", note: null },
  };
  const p = revertPlan(del);
  assert.equal("cmd" in p && p.cmd, "insert_row");
  assert.deepEqual("args" in p && p.args.values, [
    { column: "id", value: "7" },
    { column: "email", value: "a@x.com" },
    { column: "note", value: null },
  ]);
});

test("truncate / drop ย้อนไม่ได้ ต้องบอกเหตุผล ไม่ใช่เงียบ ๆ", () => {
  for (const kind of ["truncate", "drop"] as const) {
    const p = revertPlan({ ...base, kind });
    assert.ok("reason" in p && p.reason.includes("Backup"));
  }
});

test("ไม่มี pk / ไม่มีค่าแถว = ย้อนไม่ได้ ไม่ใช่ยิงคำสั่งมั่ว", () => {
  assert.ok("reason" in revertPlan({ ...upd, keys: [] }));
  assert.ok("reason" in revertPlan({ ...base, kind: "delete" }));
  assert.ok("reason" in revertPlan({ ...base, kind: "delete", row: {} }));
});

test("ย้อนซ้ำรายการเดิมไม่ได้", () => {
  assert.deepEqual(revertPlan({ ...upd, undone: true }), { reason: "รายการนี้ย้อนกลับไปแล้ว" });
});

test("รายการย้อนกลับสลับ before/after เพื่อให้ย้อนของย้อนได้อีก", () => {
  const r = reverseOf(upd, "2", 2000)!;
  assert.equal(r.before, "พิมพ์มั่ว");
  assert.equal(r.after, "a@x.com");
  assert.equal(r.isRevert, true);
  assert.equal(r.undone, false);
  // ย้อนอันนี้ต่อ ต้องได้ค่ากลับไปเป็นที่พิมพ์มั่ว
  assert.equal("args" in revertPlan(r) && revertPlan(r).args.value, "พิมพ์มั่ว");
  assert.equal(reverseOf({ ...base, kind: "drop" }, "2", 2000), null);
});

test("ประวัติเรียงใหม่สุดขึ้นก่อน และไม่โตไม่จำกัด", () => {
  let list: Entry[] = [];
  for (let i = 0; i < 305; i++) list = add(list, { ...upd, id: String(i), at: i });
  assert.equal(list.length, 300);
  assert.equal(list[0].id, "304");
});
