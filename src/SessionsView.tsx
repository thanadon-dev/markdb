/* session ที่ต่อ database อยู่ + ใครโดน lock ขวาง — ยกเลิก query / ตัด session ได้
   รีเฟรชเองทุก 3 วิ ตอนเปิดอยู่ */
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ArrowClockwise, LockSimple, Pulse, WarningCircle, X } from "@phosphor-icons/react";

type Session = {
  pid: number;
  usr: string;
  db: string;
  client: string;
  state: string;
  wait: string;
  secs: number | string;
  query: string;
  blocked_by: string;
};

const IDLE = new Set(["idle", "Sleep"]);

const dur = (v: number | string) => {
  const s = Number(v) || 0;
  return s < 60 ? `${s.toFixed(1)} วิ` : s < 3600 ? `${Math.floor(s / 60)} นาที` : `${(s / 3600).toFixed(1)} ชม.`;
};

export function SessionsView({ conn, name, onClose }: { conn: string; name: string; onClose: () => void }) {
  const [rows, setRows] = useState<Session[] | null>(null);
  const [err, setErr] = useState("");
  const [hideIdle, setHideIdle] = useState(true);
  const [auto, setAuto] = useState(true);
  // กดครั้งแรก = ถาม, กดซ้ำปุ่มเดิม = ทำจริง
  const [arm, setArm] = useState<string>("");
  const [msg, setMsg] = useState("");

  const load = useCallback(async () => {
    try {
      setRows(await invoke<Session[]>("sessions", { conn }));
      setErr("");
    } catch (e) {
      setErr(String(e));
    }
  }, [conn]);

  useEffect(() => {
    load();
    if (!auto) return;
    const t = setInterval(load, 3000);
    return () => clearInterval(t);
  }, [load, auto]);

  const kill = async (pid: number, terminate: boolean) => {
    const key = `${pid}:${terminate}`;
    if (arm !== key) return setArm(key);
    setArm("");
    try {
      const ok = await invoke<boolean>("kill_session", { conn, pid, terminate });
      setMsg(ok ? `${terminate ? "ตัด session" : "ยกเลิก query ของ"} pid ${pid} แล้ว` : `pid ${pid} จบไปก่อนแล้ว`);
      load();
    } catch (e) {
      setMsg(String(e));
    }
  };

  const all = rows ?? [];
  const blockers = new Set(all.flatMap((r) => (r.blocked_by ? r.blocked_by.split(",").map(Number) : [])));
  const shown = all.filter((r) => !hideIdle || !IDLE.has(r.state) || blockers.has(r.pid));
  const waiting = all.filter((r) => r.blocked_by).length;

  return (
    <div className="overlay">
      <div className="modal wide sessions">
        <button className="modal-x" onClick={onClose} title="ปิด (Esc)">
          <X size={14} weight="bold" />
        </button>
        <h3>
          <Pulse size={17} weight="duotone" /> Session ที่ต่ออยู่ · {name}
        </h3>
        <p>
          {all.length} session · แสดง {shown.length}
          {waiting > 0 && <b className="sxwait"> · {waiting} ตัวรอ lock อยู่</b>}
        </p>
        <div className="sxbar">
          <label className="check">
            <input type="checkbox" checked={hideIdle} onChange={(e) => setHideIdle(e.target.checked)} />
            ซ่อนที่ว่างอยู่
          </label>
          <label className="check">
            <input type="checkbox" checked={auto} onChange={(e) => setAuto(e.target.checked)} />
            รีเฟรชทุก 3 วิ
          </label>
          <button className="btn sm" onClick={load}>
            <ArrowClockwise size={13} /> รีเฟรช
          </button>
        </div>
        {err && <div className="err">{err}</div>}
        {msg && <div className="sxmsg">{msg}</div>}
        <div className="sxlist">
          {!rows && !err && <div className="hhint">กำลังโหลด…</div>}
          {rows && !shown.length && <div className="hhint">ไม่มี session ที่ทำงานอยู่</div>}
          {shown.map((r) => {
            const stale = r.state === "idle in transaction" && Number(r.secs) > 60;
            return (
              <div key={r.pid} className={"sxrow" + (r.blocked_by ? " waiting" : "") + (blockers.has(r.pid) ? " blocker" : "")}>
                <div className="sxhead">
                  <b>{r.pid}</b>
                  <span>
                    {r.usr}
                    {r.db && ` @ ${r.db}`}
                  </span>
                  {r.client && <span className="sxdim">{r.client}</span>}
                  <span className={"sxstate" + (r.state === "active" || r.state === "Query" ? " on" : "")}>{r.state || "—"}</span>
                  {r.wait && <span className="sxdim">{r.wait}</span>}
                  <span className="sxtime">{dur(r.secs)}</span>
                </div>
                {r.blocked_by && (
                  <div className="sxnote sxbad">
                    <LockSimple size={12} weight="fill" /> รอ lock จาก pid {r.blocked_by}
                  </div>
                )}
                {blockers.has(r.pid) && (
                  <div className="sxnote sxamber">
                    <WarningCircle size={12} weight="fill" /> ตัวนี้ถือ lock ที่คนอื่นรออยู่
                  </div>
                )}
                {stale && (
                  <div className="sxnote sxamber">
                    <WarningCircle size={12} weight="fill" /> เปิด transaction ค้างไว้โดยไม่ทำอะไร — อาจถือ lock อยู่
                  </div>
                )}
                {r.query && <pre title={r.query}>{r.query}</pre>}
                <div className="sxact">
                  <button className={"btn sm" + (arm === `${r.pid}:false` ? " danger" : "")} onClick={() => kill(r.pid, false)}>
                    {arm === `${r.pid}:false` ? "กดอีกครั้งเพื่อยกเลิก query" : "ยกเลิก query"}
                  </button>
                  <button className={"btn sm" + (arm === `${r.pid}:true` ? " danger" : "")} onClick={() => kill(r.pid, true)}>
                    {arm === `${r.pid}:true` ? "กดอีกครั้งเพื่อตัด session" : "ตัด session"}
                  </button>
                </div>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
