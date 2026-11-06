/* แผงด้านขวาของผลลัพธ์: ค่าเต็มของ cell ที่เลือก — JSON จัดรูปและใส่สีให้ */
import { useState } from "react";
import { BracketsCurly, Copy, X } from "@phosphor-icons/react";
import { pretty, tokens } from "./value";

// ponytail: เกินนี้ไม่ใส่สี — span เป็นแสนตัวทำให้คลิกเปลี่ยน cell แล้วหน่วง
const COLOR_MAX = 200_000;

export function ValueView({
  cell,
  onCopy,
  onClose,
}: {
  cell: { col: string; v: unknown } | null;
  onCopy: (text: string) => void;
  onClose: () => void;
}) {
  const [raw, setRaw] = useState(false);
  const p = cell ? pretty(cell.v) : null;
  const json = p?.kind === "json" && !raw;
  const text = !cell || !p ? "" : p.kind === "json" && raw ? String(typeof cell.v === "object" ? JSON.stringify(cell.v) : cell.v) : p.text;

  return (
    <aside className="valview">
      <div className="valhead">
        <BracketsCurly size={14} weight="duotone" />
        <b>{cell ? cell.col : "Value"}</b>
        {p?.kind === "json" && (
          <div className="seg">
            <button className={raw ? "" : "on"} onClick={() => setRaw(false)} title="จัดรูป JSON">
              JSON
            </button>
            <button className={raw ? "on" : ""} onClick={() => setRaw(true)} title="ข้อความตามที่เก็บใน database">
              Text
            </button>
          </div>
        )}
        <span className="spacer" />
        <button disabled={!cell || p?.kind === "null"} onClick={() => onCopy(text)} title="คัดลอก">
          <Copy size={13} />
        </button>
        <button onClick={onClose} title="ปิด">
          <X size={13} />
        </button>
      </div>
      {!cell ? (
        <div className="valempty">คลิก cell เพื่อดูค่าเต็ม</div>
      ) : (
        <pre className={"valbody" + (p?.kind === "null" ? " null" : "")}>
          {json && text.length <= COLOR_MAX
            ? tokens(text).map((t, i) => (t.t === "punct" ? t.s : <span key={i} className={"j" + t.t}>{t.s}</span>))
            : text}
        </pre>
      )}
    </aside>
  );
}
