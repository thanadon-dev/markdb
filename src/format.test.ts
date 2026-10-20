import { test } from "node:test";
import assert from "node:assert/strict";
import { formatSql } from "./format.ts";

test("จัดรูปตามชนิด database, comment ไม่หาย", () => {
  const pg = formatSql("select a,b from t where x::int = 1 -- note\nand y in (1,2)");
  assert.match(pg, /^select\n  a,\n  b\nfrom\n  t\nwhere\n  x::int = 1 -- note\n  and y in \(1, 2\)$/);
  assert.match(formatSql("select `a` from `t`", "mysql"), /`a`[\s\S]*`t`/);
  assert.throws(() => formatSql("select 'unclosed"));
});
