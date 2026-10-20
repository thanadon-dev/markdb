import { test } from "node:test";
import assert from "node:assert/strict";
import { fromUrl } from "./connurl.ts";

test("แตก URL เป็นช่อง: host เดียวกัน คนละ database ได้ค่าแยกกัน", () => {
  assert.deepEqual(fromUrl("postgres://us%40r:p%3Ass@10.1.2.3:6543/my%20db?sslmode=require"), {
    host: "10.1.2.3",
    port: "6543",
    user: "us@r",
    pass: "p:ss",
    db: "my db",
    ssl: true,
  });
  assert.equal(fromUrl("postgresql://u@10.1.2.3/db2")?.db, "db2");
  assert.equal(fromUrl("postgres://u@h/db?sslmode=disable")?.ssl, false);
  assert.equal(fromUrl("postgres://h")?.port, "");
});

test("ไม่ใช่ URL ของ postgres → null", () => {
  assert.equal(fromUrl("localhost"), null);
  assert.equal(fromUrl("mysql://u@h/db"), null);
  assert.equal(fromUrl("postgres://"), null);
});
