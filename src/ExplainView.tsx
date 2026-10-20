/* แผนการรัน query แบบต้นไม้ — node ที่กินเวลามากสุดถูกไฮไลต์ และเตือนจุดที่น่าจะช้า */
import { Lightning, TreeStructure, WarningCircle, X } from "@phosphor-icons/react";
import { fromLite, fromPg, hottest, type PlanView } from "./plan";

export type Plan = { format: "pg" | "sqlite" | "text"; text: string; analyzed: boolean; sql: string };

const n0 = (n?: number) => (n === undefined ? "" : Math.round(n).toLocaleString());

export function ExplainView({
  plan,
  canAnalyze,
  busy,
  onAnalyze,
  onClose,
}: {
  plan: Plan;
  canAnalyze: boolean;
  busy: boolean;
  onAnalyze: () => void;
  onClose: () => void;
}) {
  let view: PlanView | null = null;
  let bad = "";
  try {
    view = plan.format === "pg" ? fromPg(plan.text) : plan.format === "sqlite" ? fromLite(plan.text) : null;
  } catch (e) {
    bad = String(e);
  }
  const hot = view ? hottest(view) : -1;
  const maxMs = view ? Math.max(0, ...view.nodes.map((n) => n.selfMs ?? 0)) : 0;
  const warns = view?.nodes.filter((n) => n.warn).length ?? 0;

  return (
    <div className="result explain">
      <div className="exhead">
        <TreeStructure size={16} weight="duotone" />
        <b>Query plan</b>
        <span>
          {view?.analyzed
            ? `รันจริง ${view.execMs?.toFixed(1)} ms · วางแผน ${view.planningMs?.toFixed(1)} ms`
            : "ค่าประมาณของ planner — ยังไม่ได้รันจริง"}
          {warns > 0 && ` · มีจุดที่ควรดู ${warns} จุด`}
        </span>
        {canAnalyze && !plan.analyzed && (
          <button
            className="btn sm"
            disabled={busy}
            onClick={onAnalyze}
            title="รันคำสั่งจริงเพื่อจับเวลาและจำนวนแถวจริง แล้ว rollback ทิ้ง — คำสั่งที่ช้าก็จะช้าจริง"
          >
            <Lightning size={13} weight="duotone" /> Explain Analyze
          </button>
        )}
        <button className="btn ghost sm" onClick={onClose} title="ปิด กลับไปดูผลลัพธ์">
          <X size={13} />
        </button>
      </div>

      {!view ? (
        <pre className="extext">{bad ? `${bad}\n\n${plan.text}` : plan.text}</pre>
      ) : (
        <div className="exlist">
          {view.nodes.map((n, i) => (
            <div key={i} className={"exrow" + (i === hot ? " hot" : "")} style={{ paddingLeft: 12 + n.depth * 20 }}>
              <div className="exmain">
                <b>{n.label}</b>
                {n.detail && <span className="exdetail">{n.detail}</span>}
                <span className="exnums">
                  {n.rows !== undefined ? (
                    <>
                      <b>{n0(n.rows)}</b> แถว <em>(ประมาณ {n0(n.est)})</em>
                    </>
                  ) : n.est !== undefined ? (
                    <>ประมาณ {n0(n.est)} แถว</>
                  ) : null}
                  {n.cost !== undefined && !view.analyzed && <em> · cost {n0(n.cost)}</em>}
                  {n.selfMs !== undefined && <b className="exms">{n.selfMs.toFixed(1)} ms</b>}
                </span>
              </div>
              {n.selfMs !== undefined && maxMs > 0 && (
                <div className="exbar">
                  <i style={{ width: `${(n.selfMs / maxMs) * 100}%` }} />
                </div>
              )}
              {n.cond && <code className="excond">{n.cond}</code>}
              {n.warn && (
                <div className="exwarn">
                  <WarningCircle size={13} weight="fill" /> {n.warn}
                </div>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
