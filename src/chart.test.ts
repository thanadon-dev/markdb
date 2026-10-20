import { test } from "node:test";
import assert from "node:assert/strict";
import { compact, defaults, numericCols, points, ticks } from "./chart.ts";

const rows = [
  { id: 1, region: "north", sales: 120, cost: "80.5", note: null },
  { id: 2, region: "south", sales: 90, cost: "70", note: "x" },
];

test("เลือกค่าตั้งต้น: X = คอลัมน์ข้อความ, Y = ตัวเลขที่ไม่ใช่ id, ข้อความตัวเลขนับเป็นตัวเลข", () => {
  const cols = ["id", "region", "sales", "cost", "note"];
  assert.deepEqual(numericCols(cols, rows), ["id", "sales", "cost"]);
  assert.deepEqual(defaults(cols, rows), { kind: "bar", x: "region", ys: ["sales", "cost"] });
  // วันที่ → กราฟเส้น
  const daily = [{ day: "2026-10-01", n: 3 }, { day: "2026-10-02", n: 5 }];
  assert.equal(defaults(["day", "n"], daily).kind, "line");
  // ไม่มีอะไรนอกจาก id กับตัวเลข → ยังได้ Y สักตัว
  assert.deepEqual(defaults(["id", "n"], [{ id: 1, n: 2 }]).ys, ["n"]);
});

test("แกนตัวเลขกลม ๆ และครอบ 0", () => {
  assert.deepEqual(ticks(0, 97), [0, 20, 40, 60, 80, 100]);
  assert.deepEqual(ticks(-30, 45), [-40, -20, 0, 20, 40, 60]);
  assert.deepEqual(ticks(0.2, 0.9), [0, 0.2, 0.4, 0.6, 0.8, 1]);
  assert.deepEqual(ticks(5, 5), [0, 1, 2, 3, 4, 5]);
  assert.deepEqual(ticks(0, 0), [0, 0.2, 0.4, 0.6, 0.8, 1]);
  assert.equal(compact(1500), "1.5K");
  assert.equal(compact(12.5), "12.5");
});

test("จุดของกราฟ: x เป็นข้อความ, ค่าที่ไม่ใช่ตัวเลขเป็น null", () => {
  assert.deepEqual(points(rows, { kind: "bar", x: "region", ys: ["sales", "note"] }), [
    { x: "north", ys: [120, null] },
    { x: "south", ys: [90, null] },
  ]);
});
