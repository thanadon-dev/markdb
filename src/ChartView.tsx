/* กราฟจากผลลัพธ์บนจอ — แท่ง (เทียบหมวด) หรือเส้น (ตามเวลา) วาดด้วย SVG ล้วน
   สีชุดข้อมูลมาจาก --series-1..4 (ตรวจสีตาบอดสี/contrast แล้วทั้งธีมมืดและสว่าง)
   ค่าจริงดูได้จาก tooltip และสลับกลับไปดูเป็นตาราง (Grid) ได้เสมอ */
import { useEffect, useMemo, useRef, useState } from "react";
import { ChartBar, ChartLine } from "@phosphor-icons/react";
import { compact, defaults, MAX_BARS, MAX_POINTS, MAX_SERIES, numericCols, points, ticks, type ChartConf } from "./chart";

type Res = { columns: string[]; rows: Record<string, unknown>[] };

const M = { top: 16, right: 16, bottom: 30, left: 52 };
const color = (i: number) => `var(--series-${i + 1})`;

export function ChartView({ res }: { res: Res }) {
  const key = res.columns.join("\u0000");
  const [conf, setConf] = useState<ChartConf>(() => defaults(res.columns, res.rows));
  // คอลัมน์เปลี่ยน (รัน query ใหม่) → เลือกค่าตั้งต้นใหม่
  const [forKey, setForKey] = useState(key);
  if (forKey !== key) {
    setForKey(key);
    setConf(defaults(res.columns, res.rows));
  }
  const nums = useMemo(() => numericCols(res.columns, res.rows), [res]);

  const box = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: 600, h: 300 });
  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setSize({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const [hover, setHover] = useState<{ i: number; px: number; py: number } | null>(null);

  const pts = useMemo(() => points(res.rows, conf), [res, conf]);
  const lineEnds = conf.kind === "line" && conf.ys.length > 1;
  const W = size.w - M.left - M.right - (lineEnds ? 70 : 0);
  const H = size.h - M.top - M.bottom;
  const vals = pts.flatMap((p) => p.ys.filter((v): v is number => v !== null));
  const tk = ticks(vals.length ? Math.min(...vals) : 0, vals.length ? Math.max(...vals) : 1);
  const lo = tk[0];
  const hi = tk[tk.length - 1];
  const y = (v: number) => M.top + H - ((v - lo) / (hi - lo)) * H;
  const n = pts.length;
  const band = W / Math.max(n, 1);
  const cx = (i: number) => M.left + (conf.kind === "bar" ? band * (i + 0.5) : n > 1 ? (W * i) / (n - 1) : W / 2);
  // ป้ายแกน x ไม่ให้ชนกัน: เว้นทีละกี่ป้าย
  const every = Math.max(1, Math.ceil((n * 64) / Math.max(W, 1)));
  const barW = Math.min(24, Math.max(2, (band * 0.8 - (conf.ys.length - 1) * 2) / conf.ys.length));
  const groupW = barW * conf.ys.length + (conf.ys.length - 1) * 2;
  const base = y(Math.max(lo, Math.min(0, hi)));

  const cut =
    res.rows.length > (conf.kind === "bar" ? MAX_BARS : MAX_POINTS)
      ? `แสดง ${n.toLocaleString()} จาก ${res.rows.length.toLocaleString()} แถวแรก`
      : "";

  const onMove = (e: React.MouseEvent<SVGSVGElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    const px = e.clientX - r.left;
    const i =
      conf.kind === "bar"
        ? Math.floor((px - M.left) / band)
        : Math.round(((px - M.left) / Math.max(W, 1)) * (n - 1));
    if (i < 0 || i >= n || px < M.left || px > M.left + W) return setHover(null);
    setHover({ i, px, py: e.clientY - r.top });
  };

  // แท่ง: ปลายมน 4px ฝั่งข้อมูล เหลี่ยมที่เส้นฐาน
  const bar = (x0: number, v: number) => {
    const y1 = y(v);
    const up = y1 < base;
    const h = Math.abs(base - y1);
    const rr = Math.min(4, h, barW / 2);
    const top = up ? y1 : base;
    return up
      ? `M${x0},${base}V${top + rr}Q${x0},${top} ${x0 + rr},${top}H${x0 + barW - rr}Q${x0 + barW},${top} ${x0 + barW},${top + rr}V${base}Z`
      : `M${x0},${base}V${top + h - rr}Q${x0},${top + h} ${x0 + rr},${top + h}H${x0 + barW - rr}Q${x0 + barW},${top + h} ${x0 + barW},${top + h - rr}V${base}Z`;
  };

  return (
    <div className="result chart">
      <div className="chbar">
        <div className="seg">
          <button className={conf.kind === "bar" ? "on" : ""} onClick={() => setConf({ ...conf, kind: "bar" })}>
            <ChartBar size={14} weight="duotone" /> แท่ง
          </button>
          <button className={conf.kind === "line" ? "on" : ""} onClick={() => setConf({ ...conf, kind: "line" })}>
            <ChartLine size={14} weight="duotone" /> เส้น
          </button>
        </div>
        <label>
          แกน X
          <select value={conf.x} onChange={(e) => setConf({ ...conf, x: e.target.value, ys: conf.ys.filter((c) => c !== e.target.value) })}>
            {res.columns.map((c) => (
              <option key={c}>{c}</option>
            ))}
          </select>
        </label>
        <div className="chys">
          แกน Y
          {nums
            .filter((c) => c !== conf.x)
            .map((c) => {
              const on = conf.ys.includes(c);
              const i = conf.ys.indexOf(c);
              return (
                <button
                  key={c}
                  className={on ? "on" : ""}
                  disabled={!on && conf.ys.length >= MAX_SERIES}
                  title={!on && conf.ys.length >= MAX_SERIES ? `เลือกได้สูงสุด ${MAX_SERIES} คอลัมน์` : ""}
                  onClick={() => setConf({ ...conf, ys: on ? conf.ys.filter((x) => x !== c) : [...conf.ys, c] })}
                >
                  {on && <i style={{ background: color(i) }} />}
                  {c}
                </button>
              );
            })}
          {!nums.filter((c) => c !== conf.x).length && <span className="chdim">ไม่มีคอลัมน์ตัวเลข</span>}
        </div>
        {cut && <span className="chdim">{cut}</span>}
      </div>

      <div className="chplot" ref={box}>
        {!conf.ys.length || !n ? (
          <div className="hhint">เลือกคอลัมน์ตัวเลขอย่างน้อย 1 ตัวเป็นแกน Y</div>
        ) : (
          <svg width={size.w} height={size.h} onMouseMove={onMove} onMouseLeave={() => setHover(null)}>
            {tk.map((t) => (
              <g key={t}>
                <line x1={M.left} x2={M.left + W} y1={y(t)} y2={y(t)} className="chgrid" />
                <text x={M.left - 8} y={y(t)} className="chtick" textAnchor="end" dominantBaseline="middle">
                  {compact(t)}
                </text>
              </g>
            ))}
            {pts.map((p, i) =>
              i % every ? null : (
                <text key={i} x={cx(i)} y={M.top + H + 18} className="chtick" textAnchor="middle">
                  {p.x.length > 12 ? p.x.slice(0, 11) + "…" : p.x}
                </text>
              ),
            )}
            {hover && <line x1={cx(hover.i)} x2={cx(hover.i)} y1={M.top} y2={M.top + H} className="chcross" />}
            {conf.kind === "bar"
              ? pts.map((p, i) =>
                  p.ys.map((v, s) =>
                    v === null ? null : (
                      <path key={`${i}-${s}`} d={bar(cx(i) - groupW / 2 + s * (barW + 2), v)} style={{ fill: color(s) }} />
                    ),
                  ),
                )
              : conf.ys.map((_, s) => {
                  const seg = pts.map((p, i) => (p.ys[s] === null ? null : `${cx(i)},${y(p.ys[s]!)}`)).filter(Boolean);
                  const last = [...pts.keys()].reverse().find((i) => pts[i].ys[s] !== null);
                  return (
                    <g key={s}>
                      <polyline points={seg.join(" ")} className="chline" style={{ stroke: color(s) }} />
                      {n <= 60 &&
                        pts.map((p, i) =>
                          p.ys[s] === null ? null : <circle key={i} cx={cx(i)} cy={y(p.ys[s]!)} r={4} className="chdot" style={{ fill: color(s) }} />,
                        )}
                      {lineEnds && last !== undefined && (
                        <text x={cx(last) + 8} y={y(pts[last].ys[s]!)} className="chend" dominantBaseline="middle">
                          {conf.ys[s]}
                        </text>
                      )}
                    </g>
                  );
                })}
          </svg>
        )}
        {hover && pts[hover.i] && (
          <div className="chtip" style={{ left: Math.min(hover.px + 14, size.w - 200), top: Math.max(4, hover.py - 10) }}>
            <b>{pts[hover.i].x}</b>
            {conf.ys.map((c, s) => (
              <div key={c}>
                <i style={{ background: color(s) }} />
                <span>{c}</span>
                <b>{pts[hover.i].ys[s] === null ? "NULL" : pts[hover.i].ys[s]!.toLocaleString()}</b>
              </div>
            ))}
          </div>
        )}
      </div>

      {conf.ys.length > 1 && (
        <div className="chlegend">
          {conf.ys.map((c, s) => (
            <span key={c}>
              <i style={{ background: color(s) }} />
              {c}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}
