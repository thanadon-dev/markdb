/* แปลงแผนการรัน query (EXPLAIN) เป็นรายการ node เรียงแบบต้นไม้ ให้ ExplainView วาด
   Postgres: JSON (มีเวลา/แถวจริงถ้า analyze), SQLite: แถวของ EXPLAIN QUERY PLAN */

export type PlanNode = {
  depth: number;
  label: string;
  detail: string;
  /** เงื่อนไขที่ node นี้ใช้ (index cond / filter / join cond) */
  cond?: string;
  cost?: number;
  /** แถวที่ planner ประมาณไว้ (รวมทุก loop) */
  est?: number;
  /** แถวจริง (รวมทุก loop) — มีเฉพาะ analyze */
  rows?: number;
  /** เวลาของ node นี้เอง ไม่รวม node ลูก (ms) — มีเฉพาะ analyze */
  selfMs?: number;
  warn?: string;
};

export type PlanView = { nodes: PlanNode[]; analyzed: boolean; planningMs?: number; execMs?: number };

const n0 = (n: number) => Math.round(n).toLocaleString();

type Pg = Record<string, any> & { Plans?: Pg[] };

const labelOf = (n: Pg) => {
  const t: string = n["Node Type"];
  if (n["Join Type"] && (t.includes("Join") || t === "Nested Loop")) return `${t} (${n["Join Type"]})`;
  if (t === "Aggregate" && n["Strategy"]) return `Aggregate (${n["Strategy"]})`;
  return n["Parallel Aware"] ? `Parallel ${t}` : t;
};

const detailOf = (n: Pg) => {
  const parts: string[] = [];
  if (n["Relation Name"]) {
    const rel = (n["Schema"] ? `${n["Schema"]}.` : "") + n["Relation Name"];
    parts.push(n["Alias"] && n["Alias"] !== n["Relation Name"] ? `${rel} ${n["Alias"]}` : rel);
  }
  if (n["Index Name"]) parts.push(`index ${n["Index Name"]}`);
  if (n["CTE Name"]) parts.push(`CTE ${n["CTE Name"]}`);
  if (n["Subplan Name"]) parts.push(n["Subplan Name"]);
  return parts.join(" · ");
};

export const fromPg = (json: string): PlanView => {
  const root = JSON.parse(json)[0];
  const nodes: PlanNode[] = [];
  const total = (n: Pg) => (n["Actual Total Time"] ?? 0) * (n["Actual Loops"] ?? 1);
  const walk = (n: Pg, depth: number) => {
    const kids = n.Plans ?? [];
    const analyzed = n["Actual Rows"] !== undefined;
    const loops = analyzed ? n["Actual Loops"] ?? 1 : 1;
    const est = (n["Plan Rows"] ?? 0) * loops;
    const rows = analyzed ? n["Actual Rows"] * loops : undefined;
    let warn: string | undefined;
    const removed = n["Rows Removed by Filter"] ?? 0;
    if (n["Node Type"] === "Seq Scan" && removed > 1000 && removed > (rows ?? 0) * 10)
      warn = `อ่านทั้งตารางแล้วทิ้ง ${n0(removed * loops)} แถว — คอลัมน์ในเงื่อนไขอาจควรมี index`;
    else if (rows !== undefined && loops > 0 && n["Actual Loops"] > 0) {
      const a = Math.max(rows, 1);
      const e = Math.max(est, 1);
      if (a / e >= 10 || e / a >= 10) warn = `ประมาณไว้ ${n0(est)} แถว แต่จริง ${n0(rows)} — สถิติอาจเก่า ลอง ANALYZE ตาราง`;
    }
    nodes.push({
      depth,
      label: labelOf(n),
      detail: detailOf(n),
      cond: n["Index Cond"] ?? n["Hash Cond"] ?? n["Merge Cond"] ?? n["Join Filter"] ?? n["Recheck Cond"] ?? n["Filter"],
      cost: n["Total Cost"],
      est,
      rows,
      selfMs: analyzed ? Math.max(0, total(n) - kids.reduce((s, k) => s + total(k), 0)) : undefined,
      warn,
    });
    for (const k of kids) walk(k, depth + 1);
  };
  walk(root.Plan, 0);
  return {
    nodes,
    analyzed: root["Execution Time"] !== undefined,
    planningMs: root["Planning Time"],
    execMs: root["Execution Time"],
  };
};

/** SQLite: แถว (id, parent, detail) — SCAN = อ่านทั้งตาราง, SEARCH = ใช้ index */
export const fromLite = (json: string): PlanView => {
  const rows: { id: number; parent: number; detail: string }[] = JSON.parse(json);
  const depth = new Map<number, number>();
  const nodes = rows.map((r) => {
    const d = (depth.get(r.parent) ?? -1) + 1;
    depth.set(r.id, d);
    const scan = /^SCAN (?!CONSTANT)/.test(r.detail) && !/USING (COVERING )?INDEX/.test(r.detail);
    return { depth: d, label: r.detail, detail: "", warn: scan ? "อ่านทั้งตาราง — ถ้าตารางใหญ่ คอลัมน์ในเงื่อนไขอาจควรมี index" : undefined };
  });
  return { nodes, analyzed: false };
};

/** node ที่กินเวลามากที่สุด (เฉพาะ analyze) — -1 ถ้าไม่มี */
export const hottest = (v: PlanView) => {
  let best = -1;
  v.nodes.forEach((n, i) => {
    if ((n.selfMs ?? 0) > (best < 0 ? 0 : v.nodes[best].selfMs ?? 0)) best = i;
  });
  return best;
};
