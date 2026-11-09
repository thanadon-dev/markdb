import { test } from "node:test";
import assert from "node:assert/strict";
import { shortType, typeKind } from "./coltype.ts";

test("จัดกลุ่ม type ไว้ใส่สี", () => {
  const cases: [string, string][] = [
    ["integer", "num"],
    ["bigint", "num"],
    ["int4", "num"],
    ["numeric(10,2)", "num"],
    ["character varying(50)", "text"],
    ["uuid", "text"],
    ["timestamp with time zone", "time"],
    ["boolean", "bool"],
    ["jsonb", "json"],
    ["integer[]", "json"],
    ["bytea", "other"],
    ["point", "other"],
  ];
  for (const [t, k] of cases) assert.equal(typeKind(t), k, t);
});

test("ย่อชื่อ type ยาว ๆ", () => {
  assert.equal(shortType("character varying(50)"), "varchar(50)");
  assert.equal(shortType("timestamp without time zone"), "timestamp");
  assert.equal(shortType("character(1)"), "char(1)");
  assert.equal(shortType("INT"), "int");
});
