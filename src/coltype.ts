/* จัดกลุ่ม type ของคอลัมน์ไว้ใส่สี + ย่อชื่อยาว ๆ ให้อ่านง่ายบนหัวตาราง / sidebar */
export type TypeKind = "num" | "text" | "time" | "bool" | "json" | "other";

export const typeKind = (t: string): TypeKind => {
  const s = t.toLowerCase();
  if (/json|xml|\[\]|^array/.test(s)) return "json";
  if (/bool|^bit\b/.test(s)) return "bool";
  if (/date|time|interval|year/.test(s)) return "time";
  if (/(^|[^a-z])(tiny|small|medium|big)?int|serial|numeric|decimal|float|double|real|money|number/.test(s)) return "num";
  if (/char|text|string|uuid|enum|citext|clob/.test(s)) return "text";
  return "other";
};

export const shortType = (t: string) =>
  t
    .toLowerCase()
    .replace("character varying", "varchar")
    .replace("timestamp without time zone", "timestamp")
    .replace("timestamp with time zone", "timestamptz")
    .replace("time without time zone", "time")
    .replace("time with time zone", "timetz")
    .replace("double precision", "double")
    .replace(/^character\b/, "char");
