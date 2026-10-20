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

test("MySQL / MariaDB / SQLite บอกชนิดมาด้วย, postgres ไม่บอก (อาจเป็น Redshift)", () => {
  assert.deepEqual(fromUrl("mysql://root:pw@10.0.0.5:3307/shop?ssl-mode=REQUIRED"), {
    engine: "mysql",
    host: "10.0.0.5",
    port: "3307",
    user: "root",
    pass: "pw",
    db: "shop",
    ssl: true,
  });
  assert.equal(fromUrl("mariadb://u@h/db")?.engine, "mysql");
  assert.equal(fromUrl("postgres://u@h/db")?.engine, undefined);
  assert.ok(!("engine" in fromUrl("postgres://u@h/db")!));
  assert.deepEqual(fromUrl("sqlite:C:\\data\\app.db"), { engine: "sqlite", host: "", port: "", user: "", pass: "", db: "C:\\data\\app.db", ssl: false });
  assert.equal(fromUrl("sqlite:///home/me/a.db")?.db, "/home/me/a.db");
});

test("ไม่ใช่ URL ที่รู้จัก → null", () => {
  assert.equal(fromUrl("localhost"), null);
  assert.equal(fromUrl("oracle://u@h/db"), null);
  assert.equal(fromUrl("postgres://"), null);
  assert.equal(fromUrl("sqlite:"), null);
});
