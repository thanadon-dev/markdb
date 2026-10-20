import { test } from "node:test";
import assert from "node:assert/strict";
import { fromFile, toFile } from "./connfile.ts";

const BLANK = { id: "", name: "", engine: "postgres" as const, host: "localhost", port: "5432", user: "postgres", pass: "", db: "postgres", ssl: false };
let n = 0;
const uid = () => "n" + ++n;

test("export ไม่มี password / id", () => {
  const out = toFile([{ ...BLANK, id: "a", name: "dev", host: "10.0.0.1", pass: "secret", group: "DEV" }]);
  const text = JSON.stringify(out);
  assert.ok(!text.includes("secret"));
  assert.ok(!text.includes('"id"'));
  assert.equal(out[0].group, "DEV");
});

test("import: ข้ามตัวซ้ำ, ได้ id ใหม่, password ว่าง, ช่องแปลกถูกทิ้ง", () => {
  const have = [{ ...BLANK, id: "a", host: "10.0.0.1", db: "db1" }];
  const file = JSON.stringify([
    { name: "same", host: "10.0.0.1", port: "5432", user: "postgres", db: "DB1" },
    { name: "uat", host: "10.0.0.1", db: "db2", pass: "leak", engine: "redshift", prod: true, group: " UAT ", evil: "x" },
  ]);
  const { add, skipped } = fromFile(file, have, BLANK, uid);
  assert.equal(skipped, 1);
  assert.equal(add.length, 1);
  assert.equal(add[0].pass, "");
  assert.equal(add[0].engine, "redshift");
  assert.equal(add[0].group, "UAT");
  assert.equal(add[0].prod, true);
  assert.ok(add[0].id.startsWith("n"));
  assert.ok(!("evil" in add[0]));
});

test("import: ไฟล์ผิดรูปแบบ → error", () => {
  assert.throws(() => fromFile("{}", [], BLANK, uid), /ไม่ใช่ไฟล์/);
  assert.throws(() => fromFile('[{"name":"x"}]', [], BLANK, uid), /host และ db/);
  assert.throws(() => fromFile("not json", [], BLANK, uid));
});
