import { test } from "node:test";
import assert from "node:assert/strict";
import { grouped, renameGroup } from "./groups.ts";

test("host เดียวกันรวมกลุ่มเอง, ตั้งชื่อเองชนะ host, เปลี่ยนชื่อทั้งกลุ่ม", () => {
  const cs = [
    { id: "a", host: "10.0.0.1" },
    { id: "b", host: "10.0.0.2" },
    { id: "c", host: "10.0.0.1" },
    { id: "d", host: "", url: "postgres://u@db.local:5432/x" },
    { id: "e", host: "10.0.0.2", group: "UAT" },
  ];
  assert.deepEqual(
    grouped(cs).map((g) => [g.name, g.items.map((c) => c.id)]),
    [["10.0.0.1", ["a", "c"]], ["10.0.0.2", ["b"]], ["db.local", ["d"]], ["UAT", ["e"]]],
  );
  const r = renameGroup(cs, "10.0.0.1", " DEV ");
  assert.deepEqual(grouped(r).map((g) => g.name), ["DEV", "10.0.0.2", "db.local", "UAT"]);
  // ชื่อว่าง = กลับไปจัดตาม host
  assert.deepEqual(grouped(renameGroup(r, "DEV", "")).map((g) => g.name)[0], "10.0.0.1");
});
