import { test } from "node:test";
import assert from "node:assert/strict";
import { pretty, tokens } from "./value.ts";

test("JSON ในข้อความถูกจัดรูป ส่วนที่ไม่ใช่ JSON โชว์ตามจริง", () => {
  assert.deepEqual(pretty('{"a":1,"b":[true,null]}'), {
    kind: "json",
    text: '{\n  "a": 1,\n  "b": [\n    true,\n    null\n  ]\n}',
  });
  assert.equal(pretty({ x: "y" }).kind, "json");
  assert.deepEqual(pretty("[abc"), { kind: "text", text: "[abc" });
  assert.deepEqual(pretty("123"), { kind: "text", text: "123" });
  assert.deepEqual(pretty(5), { kind: "text", text: "5" });
  assert.deepEqual(pretty(null), { kind: "null", text: "NULL" });
});

test("แยกท่อนสำหรับใส่สี: key / string / ตัวเลข / true-false-null", () => {
  const text = pretty('{"body":"<a class=\\"x\\">1</a>","n":-1.5e3,"ok":false,"z":null}').text;
  const ts = tokens(text);
  assert.equal(ts.map((t) => t.s).join(""), text);
  const of = (t: string) => ts.filter((x) => x.t === t).map((x) => x.s);
  assert.deepEqual(of("key"), ['"body"', '"n"', '"ok"', '"z"']);
  assert.deepEqual(of("str"), ['"<a class=\\"x\\">1</a>"']);
  assert.deepEqual(of("num"), ["-1500"]);
  assert.deepEqual(of("lit"), ["false", "null"]);
});
