import { test } from "node:test";
import assert from "node:assert/strict";
import { fromLite, fromPg, hottest } from "./plan.ts";

const pg = JSON.stringify([
  {
    Plan: {
      "Node Type": "Hash Join",
      "Join Type": "Inner",
      "Total Cost": 120.5,
      "Plan Rows": 10,
      "Actual Rows": 900,
      "Actual Loops": 1,
      "Actual Total Time": 50,
      "Hash Cond": "(o.customer_id = c.id)",
      Plans: [
        {
          "Node Type": "Seq Scan",
          "Relation Name": "orders",
          Schema: "public",
          Alias: "o",
          "Plan Rows": 1000,
          "Actual Rows": 1000,
          "Actual Loops": 1,
          "Actual Total Time": 40,
          Filter: "(total > 5)",
          "Rows Removed by Filter": 50000,
        },
        {
          "Node Type": "Index Scan",
          "Relation Name": "customer",
          Alias: "customer",
          "Index Name": "customer_pkey",
          "Plan Rows": 1,
          "Actual Rows": 1,
          "Actual Loops": 1,
          "Actual Total Time": 2,
        },
      ],
    },
    "Planning Time": 0.3,
    "Execution Time": 51,
  },
]);

test("Postgres: ต้นไม้ + เวลาเฉพาะ node + เตือนประมาณพลาด/สแกนทั้งตาราง", () => {
  const v = fromPg(pg);
  assert.equal(v.analyzed, true);
  assert.equal(v.execMs, 51);
  assert.deepEqual(v.nodes.map((n) => [n.depth, n.label]), [[0, "Hash Join (Inner)"], [1, "Seq Scan"], [1, "Index Scan"]]);
  assert.equal(v.nodes[0].selfMs, 8); // 50 - 40 - 2
  assert.equal(v.nodes[1].detail, "public.orders o");
  assert.equal(v.nodes[2].detail, "customer · index customer_pkey");
  assert.equal(v.nodes[0].cond, "(o.customer_id = c.id)");
  assert.match(v.nodes[0].warn!, /ประมาณไว้ 10 แถว แต่จริง 900/);
  assert.match(v.nodes[1].warn!, /อ่านทั้งตาราง/);
  assert.equal(v.nodes[2].warn, undefined);
  assert.equal(hottest(v), 1);
});

test("Postgres ไม่ analyze: ไม่มีเวลา ไม่เตือนเรื่องแถวจริง", () => {
  const v = fromPg(JSON.stringify([{ Plan: { "Node Type": "Seq Scan", "Relation Name": "t", "Plan Rows": 5, "Total Cost": 1 } }]));
  assert.equal(v.analyzed, false);
  assert.equal(v.nodes[0].selfMs, undefined);
  assert.equal(v.nodes[0].warn, undefined);
  assert.equal(hottest(v), -1);
});

test("SQLite: ต่อ parent เป็นความลึก, SCAN ไม่มี index = เตือน", () => {
  const v = fromLite(
    JSON.stringify([
      { id: 2, parent: 0, detail: "SCAN orders" },
      { id: 4, parent: 0, detail: "SEARCH customer USING INTEGER PRIMARY KEY (rowid=?)" },
      { id: 6, parent: 4, detail: "SCAN t USING COVERING INDEX ix" },
    ]),
  );
  assert.deepEqual(v.nodes.map((n) => n.depth), [0, 0, 1]);
  assert.ok(v.nodes[0].warn);
  assert.equal(v.nodes[1].warn, undefined);
  assert.equal(v.nodes[2].warn, undefined);
});
