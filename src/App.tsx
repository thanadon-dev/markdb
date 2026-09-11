import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { getVersion } from "@tauri-apps/api/app";
import CodeMirror, { type ReactCodeMirrorRef } from "@uiw/react-codemirror";
import { stmtAt, targetTable } from "./sqlsplit";
import { PostgreSQL, sql as sqlLang } from "@codemirror/lang-sql";
import { createTheme } from "@uiw/codemirror-themes";
import { tags as t } from "@lezer/highlight";
import {
  acceptCompletion,
  autocompletion,
  completionKeymap,
  type CompletionContext,
} from "@codemirror/autocomplete";
import { keymap } from "@codemirror/view";
import { Prec } from "@codemirror/state";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  ArrowClockwise,
  ArrowCounterClockwise,
  CheckCircle,
  Database,
  DownloadSimple,
  Copy,
  BracketsCurly,
  Confetti,
  Eye,
  FloppyDisk,
  Lightning,
  LinkSimple,
  MagnifyingGlass,
  PencilSimple,
  Info,
  Play,
  Plug,
  Plus,
  RowsPlusBottom,
  Spinner,
  Table as TableIcon,
  Trash,
  TreeStructure,
  UploadSimple,
  WarningCircle,
  X,
} from "@phosphor-icons/react";
import "./styles.css";

type TableInfo = { schema: string; name: string; kind: string };
type QueryResult = {
  columns: string[];
  rows: Record<string, unknown>[];
  affected: number;
  elapsed_ms: number;
  truncated: boolean;
};
type Tab = {
  id: string;
  conn: string;
  title: string;
  sql: string;
  source?: string;
  res?: QueryResult;
  err?: string;
  running?: boolean;
};
type ColumnInfo = {
  name: string;
  data_type: string;
  nullable: boolean;
  default: string;
  pk: boolean;
};
type Relation = { dir: "out" | "in"; name: string; other: string; def: string };
type Edge = { src: string; src_cols: string; dst: string; dst_cols: string };
type TableProps = {
  columns: ColumnInfo[];
  approx_rows: number;
  size: string;
  relations: Relation[];
};
type Release = { tag_name: string; name: string; published_at: string; body: string };

const RELEASES_API = "https://api.github.com/repos/thanadon-dev/markdb/releases?per_page=20";

type Engine = "postgres" | "redshift";
type ConnInfo = { version: string; engine: Engine };

type Conn = {
  id: string;
  name: string;
  engine: Engine;
  host: string;
  port: string;
  user: string;
  pass: string;
  db: string;
  ssl: boolean;
  url?: string; // connection ที่บันทึกไว้แบบเดิม (เป็น URL ล้วน)
};

const CONNS_KEY = "markdb.conns";
const TABS_KEY = "markdb.tabs";
const ROW_H = 28;
const PORTS: Record<Engine, string> = { postgres: "5432", redshift: "5439" };

const BLANK: Conn = {
  id: "",
  name: "",
  engine: "postgres",
  host: "localhost",
  port: "5432",
  user: "postgres",
  pass: "",
  db: "postgres",
  ssl: false,
};

/* โลโก้สองตัวนี้วาดเอง ไม่ได้ก๊อป asset ของแบรนด์มา — ใช้แค่รูปทรงกับสีที่จำได้
   Postgres = หัวช้าง, Redshift = ลูกบาศก์ไล่สีแดง→ม่วง ("red shift" ตรงตัว) */
const PgLogo = ({ size = 16 }: { size?: number }) => (
  <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden>
    <path
      d="M12 2.6c4.5 0 7.4 2.4 7.4 6.2 0 2.1-.5 3.6-1.4 5.4-.8 1.6-1.2 2.7-1.2 4.1 0 1.5-1 2.5-2.4 2.5-1.2 0-2-.7-2.4-2-.4 1.3-1.2 2-2.4 2-1.4 0-2.4-1-2.4-2.5 0-1.4-.4-2.5-1.2-4.1-.9-1.8-1.4-3.3-1.4-5.4 0-3.8 3.1-6.2 7.4-6.2Z"
      fill="#336791"
    />
    <circle cx="9.1" cy="9.1" r="1.05" fill="#eaf3fb" />
    <path
      d="M14.5 12.3c1 .6 1.5 1.7 1.2 2.9"
      stroke="#9dc6ea"
      strokeWidth="1.3"
      strokeLinecap="round"
      fill="none"
    />
  </svg>
);

const RsLogo = ({ size = 16 }: { size?: number }) => (
  <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden>
    <defs>
      <linearGradient id="mdb-rs" x1="0" y1="0" x2="1" y2="1">
        <stop offset="0%" stopColor="#ff4d6d" />
        <stop offset="100%" stopColor="#8c4fff" />
      </linearGradient>
    </defs>
    <path d="M12 2.2 20.6 7v10L12 21.8 3.4 17V7Z" fill="url(#mdb-rs)" />
    <path d="M12 2.2 20.6 7 12 11.8 3.4 7Z" fill="#fff" opacity=".26" />
    <path d="M12 11.8v10L3.4 17V7Z" fill="#000" opacity=".22" />
  </svg>
);

const EngineLogo = ({ engine, size }: { engine: Engine; size?: number }) =>
  engine === "redshift" ? <RsLogo size={size} /> : <PgLogo size={size} />;

const mb = (n: number) => (n / 1_048_576).toFixed(1);

const uid = () => Math.random().toString(36).slice(2, 9);
const qname = (t: TableInfo) => `"${t.schema}"."${t.name}"`;

const connUrl = (c: Conn) => {
  if (c.url) return c.url;
  const auth = c.pass
    ? `${encodeURIComponent(c.user)}:${encodeURIComponent(c.pass)}@`
    : c.user
      ? `${encodeURIComponent(c.user)}@`
      : "";
  const port = c.port || PORTS[c.engine ?? "postgres"];
  // Redshift ปิดการต่อแบบไม่เข้ารหัสไว้ที่ cluster อยู่แล้ว — บังคับ sslmode ให้เลย
  const ssl = c.ssl || c.engine === "redshift";
  return `postgres://${auth}${c.host}:${port}/${encodeURIComponent(c.db)}${
    ssl ? "?sslmode=require" : ""
  }`;
};

const loadConns = (): Conn[] => {
  try {
    return JSON.parse(localStorage.getItem(CONNS_KEY) || "[]").map((c: Conn) => ({
      ...BLANK,
      ...c,
      engine: c.engine ?? "postgres",
    }));
  } catch {
    return [];
  }
};

type Meta = { tables: TableInfo[]; schema: Record<string, string[]> };

const newTab = (conn: string, over: Partial<Tab> = {}): Tab => ({
  id: uid(),
  conn,
  title: "Query",
  sql: "",
  ...over,
});

/* แท็บที่เปิดค้างไว้ตอนปิดโปรแกรม — เก็บแค่ตัว query ไม่เก็บผลลัพธ์ */
const loadTabs = () => {
  try {
    const s = JSON.parse(localStorage.getItem(TABS_KEY) || "null");
    if (s?.tabs?.length)
      return { tabs: s.tabs.map((t: Tab) => newTab(t.conn ?? "", t)) as Tab[], active: s.active as string };
  } catch {
    /* localStorage เสีย — เริ่มแท็บใหม่ */
  }
  const t = newTab("");
  return { tabs: [t], active: t.id };
};

const BOOT = loadTabs();

/* ดึงตารางจาก `from <schema>.<table>` ในตัว query — ใช้บอก CodeMirror ว่าคอลัมน์
   ของตารางไหนควรขึ้นเวลาพิมพ์ใน where/select โดยไม่ต้องพิมพ์ชื่อตารางนำ */
const FROM_RE = /\bfrom\s+("?)([\w$]+)\1(?:\s*\.\s*("?)([\w$]+)\3)?/i;
const tableInQuery = (sql: string) => {
  const m = FROM_RE.exec(sql);
  if (!m) return {};
  return m[4] ? { schema: m[2], table: m[4] } : { table: m[2] };
};

/* `where col = '…` / `col in ('…` — จับคอลัมน์กับสิ่งที่พิมพ์ไปแล้วในเครื่องหมายคำพูด */
const VALUE_RE =
  /(?:^|[\s(,])"?([\w$]+)"?\s*(?:=|<>|!=|ilike|like|in\s*\(\s*)\s*'([^']*)$/i;

/* เลือกตัวที่ไฮไลต์อยู่ได้ทั้ง Tab และ Enter (Enter มากับ completionKeymap อยู่แล้ว)
   acceptCompletion คืน false ตอนไม่มี popup เปิด — Tab จึงยังย่อหน้า Enter ยังขึ้นบรรทัดใหม่ปกติ */
const completionKeys = Prec.highest(
  keymap.of([{ key: "Tab", run: acceptCompletion }, ...completionKeymap]),
);

const MarkMark = () => (
  <svg
    width="19"
    height="19"
    viewBox="0 0 1024 1024"
    fill="none"
    stroke="currentColor"
    strokeWidth="74"
    strokeLinecap="round"
    strokeLinejoin="round"
    aria-hidden
  >
    <path d="M298 736L298 306L512 556L726 306L726 736" />
  </svg>
);

const cellText = (v: unknown) =>
  v === null || v === undefined
    ? "NULL"
    : typeof v === "object"
      ? JSON.stringify(v)
      : String(v);

/* ตัวเลขเทียบเป็นตัวเลข (ไม่งั้น "10" < "9"), ที่เหลือเทียบแบบภาษาไทย */
const compare = (a: unknown, b: unknown) => {
  if (typeof a === "number" && typeof b === "number") return a - b;
  if (typeof a === "boolean" && typeof b === "boolean") return Number(a) - Number(b);
  const sa = cellText(a);
  const sb = cellText(b);
  const na = Number(sa);
  const nb = Number(sb);
  if (sa !== "" && sb !== "" && !Number.isNaN(na) && !Number.isNaN(nb)) return na - nb;
  return sa.localeCompare(sb, "th");
};

const cellClass = (v: unknown) =>
  v === null || v === undefined
    ? "cell null"
    : typeof v === "number"
      ? "cell num"
      : typeof v === "boolean"
        ? "cell bool"
        : "cell";

const blackTheme = createTheme({
  theme: "dark",
  settings: {
    background: "#000000",
    foreground: "#ffffff",
    caret: "#ffffff",
    selection: "#2f5d9e",
    selectionMatch: "#1d3a63",
    lineHighlight: "#0b0b0b",
    gutterBackground: "#000000",
    gutterForeground: "#4a4a4a",
    gutterBorder: "transparent",
  },
  styles: [
    // keyword ใหญ่กว่าตัวอื่น 3px + ตัวหนา — กวาดตาหา SELECT/FROM/WHERE ได้ทันที
    { tag: t.keyword, color: "#7dd3fc", fontWeight: "700", fontSize: "16px" },
    { tag: [t.string, t.special(t.string)], color: "#a3e635" },
    { tag: t.comment, color: "#5a5a5a", fontStyle: "italic" },
    { tag: [t.number, t.bool, t.null], color: "#fbbf24" },
    { tag: [t.typeName, t.standard(t.name)], color: "#c084fc" },
    { tag: [t.propertyName, t.name], color: "#f0f0f0" },
    { tag: t.operator, color: "#94a3b8" },
    { tag: t.punctuation, color: "#7a7a7a" },
  ],
});

/* ---------- ER diagram ---------- */

const NW = 178;
const NH = 34;
const GX = 258;
const GY = 56;

/* วางตารางเป็นชั้น ๆ ตาม "ความลึกของการอ้างอิง": ตารางที่ไม่ชี้ไปหาใครอยู่ซ้ายสุด
   ตารางที่ชี้ไปหามันอยู่ถัดมาทางขวา — อ่านทิศทางความสัมพันธ์ได้จากซ้ายไปขวา
   ponytail: layout แบบชั้นธรรมดา ไม่ใช่ force-directed — เส้นอาจตัดกันบ้างถ้า FK เยอะมาก */
const layout = (edges: Edge[]) => {
  const names = [...new Set(edges.flatMap((e) => [e.src, e.dst]))].sort();
  const out = new Map<string, string[]>(names.map((n) => [n, []]));
  for (const e of edges) if (e.src !== e.dst) out.get(e.src)!.push(e.dst);

  const depth = new Map<string, number>();
  const busy = new Set<string>();
  const calc = (n: string): number => {
    const known = depth.get(n);
    if (known !== undefined) return known;
    if (busy.has(n)) return 0; // FK วนกลับมาหาตัวเอง — ตัดตรงนี้ไม่ให้ลูปไม่จบ
    busy.add(n);
    const kids = out.get(n) ?? [];
    const d = kids.length ? 1 + Math.max(...kids.map(calc)) : 0;
    busy.delete(n);
    depth.set(n, d);
    return d;
  };
  names.forEach(calc);

  const perCol = new Map<number, number>();
  const nodes = names.map((name) => {
    const d = depth.get(name) ?? 0;
    const row = perCol.get(d) ?? 0;
    perCol.set(d, row + 1);
    return { name, x: d * GX, y: row * GY, links: 0 };
  });
  const at = new Map(nodes.map((n) => [n.name, n]));
  for (const e of edges) {
    const a = at.get(e.src);
    const b = at.get(e.dst);
    if (a) a.links++;
    if (b && b !== a) b.links++;
  }

  const w = Math.max(...nodes.map((n) => n.x + NW), 1) + 40;
  const h = Math.max(...nodes.map((n) => n.y + NH), 1) + 40;
  return { nodes, at, w, h };
};

function ErDiagram({ edges, onOpen }: { edges: Edge[]; onOpen: (t: string) => void }) {
  const { nodes, at, w, h } = useMemo(() => layout(edges), [edges]);
  const [view, setView] = useState({ x: 20, y: 20, k: 1 });
  const [hot, setHot] = useState("");

  if (!edges.length)
    return <div className="empty">ยังไม่พบ foreign key ใน database นี้</div>;

  const drag = (e: React.MouseEvent) => {
    if (e.button !== 0) return;
    const x0 = e.clientX;
    const y0 = e.clientY;
    const v = view;
    const move = (ev: MouseEvent) =>
      setView({ ...v, x: v.x + ev.clientX - x0, y: v.y + ev.clientY - y0 });
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };

  return (
    <div
      className="er"
      onMouseDown={drag}
      onWheel={(e) =>
        setView((v) => ({ ...v, k: Math.min(2.4, Math.max(0.25, v.k * (e.deltaY < 0 ? 1.12 : 0.89))) }))
      }
    >
      <svg viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="xMidYMid meet">
        <defs>
          <marker id="tip" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto">
            <path d="M0,0 L8,4 L0,8 z" fill="#5a5a5a" />
          </marker>
        </defs>
        <g transform={`translate(${view.x},${view.y}) scale(${view.k})`}>
          {edges.map((e, i) => {
            const a = at.get(e.src)!;
            const b = at.get(e.dst)!;
            const on = hot === e.src || hot === e.dst;
            if (a === b)
              return (
                <path
                  key={i}
                  className={"erlink" + (on ? " on" : "")}
                  markerEnd="url(#tip)"
                  d={`M${a.x + NW},${a.y + 10} c40,-16 40,32 0,16`}
                />
              );
            const sx = a.x;
            const sy = a.y + NH / 2;
            const dx = b.x + NW;
            const dy = b.y + NH / 2;
            return (
              <path
                key={i}
                className={"erlink" + (on ? " on" : "")}
                markerEnd="url(#tip)"
                d={`M${sx},${sy} C${sx - 70},${sy} ${dx + 70},${dy} ${dx},${dy}`}
              >
                <title>{`${e.src}.${e.src_cols} → ${e.dst}.${e.dst_cols}`}</title>
              </path>
            );
          })}
          {nodes.map((n) => (
            <g
              key={n.name}
              className={"ernode" + (hot === n.name ? " on" : "")}
              transform={`translate(${n.x},${n.y})`}
              onMouseEnter={() => setHot(n.name)}
              onMouseLeave={() => setHot("")}
              onClick={() => onOpen(n.name)}
            >
              <rect width={NW} height={NH} rx="9" />
              <text x="12" y={NH / 2 + 4}>
                {n.name.length > 22 ? n.name.slice(0, 21) + "…" : n.name}
              </text>
              <text className="cnt" x={NW - 12} y={NH / 2 + 4} textAnchor="end">
                {n.links}
              </text>
              <title>{`${n.name} — คลิกเพื่อเปิด`}</title>
            </g>
          ))}
        </g>
      </svg>
      <div className="erhint">ลากเพื่อเลื่อน · สกรอลเพื่อซูม · คลิกตารางเพื่อเปิด</div>
    </div>
  );
}

/* ---------- result grid (memo: พิมพ์ใน editor แล้วตารางไม่ re-render) ---------- */

const Grid = memo(function Grid({
  res,
  pk,
  editable,
  filter,
  onEdit,
  onCopy,
  onSelect,
  onExportJson,
}: {
  res: QueryResult;
  pk: string[];
  editable: boolean;
  filter: string;
  onEdit: (rowIndex: number, column: string, value: string | null) => void;
  onCopy: (text: string) => void;
  onSelect: (rows: number[]) => void;
  onExportJson: (rows: Record<string, unknown>[]) => void;
}) {
  const parent = useRef<HTMLDivElement>(null);
  // cur ชี้ด้วย "ลำดับที่เห็นบนจอ" (index ใน order) ไม่ใช่ index จริงของแถว
  // การกดลูกศรจึงเดินตามที่ตาเห็นแม้กำลังกรองหรือเรียงอยู่
  const [cur, setCur] = useState<{ r: number; c: number } | null>(null);
  const [sel, setSel] = useState<Set<number>>(() => new Set());
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const anchor = useRef(-1); // แถว (ลำดับบนจอ) ที่ใช้เป็นจุดตั้งต้นของ shift+click
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [sort, setSort] = useState<{ col: string; dir: "asc" | "desc" } | null>(null);

  const widths = useMemo(
    () =>
      res.columns.map((c) => {
        const sample = res.rows
          .slice(0, 40)
          .reduce((m, r) => Math.max(m, cellText(r[c]).length), c.length);
        return Math.min(440, Math.max(88, sample * 7.4 + 28));
      }),
    [res],
  );
  const template = useMemo(() => widths.map((w) => `${w}px`).join(" "), [widths]);

  /* เรียงลำดับฝั่ง client บน "ลำดับ index" ไม่ใช่ตัว rows — index เดิมจึงยังใช้อ้าง
     ตอนแก้ค่าได้ถูกแถว และไม่ต้องยิง query ใหม่ */
  const order = useMemo(() => {
    const q = filter.trim().toLowerCase();
    let idx = res.rows.map((_, i) => i);
    // กรองจากข้อมูลที่โหลดมาแล้ว ไม่ยิง query ใหม่ — พิมพ์แล้วเห็นผลทันที
    if (q)
      idx = idx.filter((i) =>
        res.columns.some((c) => cellText(res.rows[i][c]).toLowerCase().includes(q)),
      );
    if (!sort) return idx;
    const sign = sort.dir === "asc" ? 1 : -1;
    return idx.sort((a, b) => {
      const va = res.rows[a][sort.col];
      const vb = res.rows[b][sort.col];
      const an = va === null || va === undefined;
      const bn = vb === null || vb === undefined;
      if (an || bn) return an && bn ? 0 : an ? 1 : -1; // ค่าว่างไปท้ายเสมอ ไม่ว่าเรียงทางไหน
      return compare(va, vb) * sign;
    });
  }, [res, sort, filter]);

  // ผูกกับ columns ไม่ใช่ res ทั้งก้อน — แก้ค่าทีนึง res เปลี่ยน object ใหม่ทุกครั้ง
  // ถ้า reset ตามนั้นเคอร์เซอร์จะเด้งหายทุกครั้งที่กด Enter บันทึก
  useEffect(() => {
    setCur(null);
    setEditing(false);
    setSel(new Set());
  }, [res.columns]);

  const rv = useVirtualizer({
    count: order.length,
    getScrollElement: () => parent.current,
    estimateSize: () => ROW_H,
    overscan: 12,
  });

  const cycleSort = (c: string) =>
    setSort((s) =>
      s?.col !== c ? { col: c, dir: "asc" } : s.dir === "asc" ? { col: c, dir: "desc" } : null,
    );

  const moveTo = (r: number, c: number) => {
    if (!order.length) return;
    const rr = Math.max(0, Math.min(order.length - 1, r));
    const cc = Math.max(0, Math.min(res.columns.length - 1, c));
    setCur({ r: rr, c: cc });
    anchor.current = rr;
    setSel(new Set([order[rr]]));
    onSelect([order[rr]]);
    rv.scrollToIndex(rr);
  };

  /* คลิกเลือกแถว: ธรรมดา = แถวเดียว, Ctrl = สลับทีละแถว, Shift = ทั้งช่วง */
  const pick = (vr: number, e: React.MouseEvent) => {
    const ri = order[vr];
    let next: Set<number>;
    if (e.shiftKey && anchor.current >= 0) {
      const [a, b] = anchor.current < vr ? [anchor.current, vr] : [vr, anchor.current];
      next = new Set(order.slice(a, b + 1));
    } else {
      if (e.ctrlKey || e.metaKey) {
        next = new Set(sel);
        if (!next.delete(ri)) next.add(ri);
      } else {
        next = new Set([ri]);
      }
      anchor.current = vr;
    }
    setSel(next);
    onSelect([...next]);
  };

  // เรียงตามที่เห็นบนจอ ไม่ใช่ตามลำดับที่คลิก — copy/export จะได้ตรงกับตา
  const picked = () => order.filter((i) => sel.has(i)).map((i) => res.rows[i]);
  const asTsv = (rows: Record<string, unknown>[]) =>
    rows.map((r) => res.columns.map((c) => cellText(r[c])).join("\t")).join("\n");

  const startEdit = (r: number, c: number, initial?: string) => {
    if (!editable) return;
    const v = res.rows[order[r]][res.columns[c]];
    setCur({ r, c });
    setDraft(initial ?? (v === null || v === undefined ? "" : cellText(v)));
    setEditing(true);
  };

  /* บันทึกแล้วไปต่อ: 1 = ลงแถวล่าง (Enter), 2 = ไปคอลัมน์ขวา (Tab) */
  const commit = (move: 1 | 2) => {
    if (!cur) return;
    const ri = order[cur.r];
    const col = res.columns[cur.c];
    setEditing(false);
    parent.current?.focus();
    if (draft !== cellText(res.rows[ri][col]))
      onEdit(ri, col, draft.toUpperCase() === "NULL" ? null : draft);
    moveTo(move === 1 ? cur.r + 1 : cur.r, move === 2 ? cur.c + 1 : cur.c);
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (editing) return;
    if (!cur) {
      if (e.key.startsWith("Arrow")) {
        e.preventDefault();
        moveTo(0, 0);
      }
      return;
    }
    const k = e.key;
    const jump = { ArrowDown: [1, 0], ArrowUp: [-1, 0], ArrowLeft: [0, -1], ArrowRight: [0, 1] }[k];
    if (jump) {
      e.preventDefault();
      moveTo(cur.r + jump[0], cur.c + jump[1]);
    } else if (k === "Tab") {
      e.preventDefault();
      moveTo(cur.r, cur.c + (e.shiftKey ? -1 : 1));
    } else if (k === "PageDown" || k === "PageUp") {
      e.preventDefault();
      moveTo(cur.r + (k === "PageDown" ? 20 : -20), cur.c);
    } else if (k === "Enter" || k === "F2") {
      e.preventDefault();
      startEdit(cur.r, cur.c);
    } else if (k === "Escape") {
      setCur(null);
    } else if ((e.ctrlKey || e.metaKey) && k.toLowerCase() === "c") {
      onCopy(cellText(res.rows[order[cur.r]][res.columns[cur.c]]));
    } else if (k.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
      // พิมพ์ตัวอักษรทับได้เลยแบบสเปรดชีต ไม่ต้องดับเบิลคลิกก่อน
      e.preventDefault();
      startEdit(cur.r, cur.c, k);
    }
  };

  if (!res.columns.length)
    return (
      <div className="empty">
        <Lightning size={30} weight="duotone" />
        <div>
          สำเร็จ — กระทบ <b>{res.affected}</b> แถว ({res.elapsed_ms} ms)
        </div>
      </div>
    );

  return (
    <div className="result grid" ref={parent} tabIndex={0} onKeyDown={onKey}>
      <div className="grid-head" style={{ gridTemplateColumns: template }}>
        {res.columns.map((c) => (
          <div
            key={c}
            className={(pk.includes(c) ? "pk" : "") + (sort?.col === c ? " sorted" : "")}
            title={`${c} — คลิกเพื่อเรียง (น้อย→มาก, มาก→น้อย, ยกเลิก)`}
            onClick={() => cycleSort(c)}
          >
            {pk.includes(c) ? `🔑 ${c}` : c}
            {sort?.col === c && <i>{sort.dir === "asc" ? "▲" : "▼"}</i>}
          </div>
        ))}
      </div>
      {!order.length && <div className="nomatch">ไม่มีแถวที่ตรงกับ “{filter}”</div>}
      <div className="grid-body" style={{ height: rv.getTotalSize() }}>
        {rv.getVirtualItems().map((vi) => {
          const ri = order[vi.index];
          const row = res.rows[ri];
          return (
            <div
              key={vi.key}
              className={"grid-row" + (sel.has(ri) ? " sel" : "")}
              style={{
                gridTemplateColumns: template,
                height: ROW_H,
                transform: `translateY(${vi.start}px)`,
              }}
              onMouseDown={(e) => {
                // คลิกพื้นที่ว่างขวาสุดของแถว (เลยคอลัมน์สุดท้าย) ให้เลือกแถวได้เหมือนกัน
                if (e.button === 0 && e.target === e.currentTarget) pick(vi.index, e);
              }}
              onContextMenu={(e) => {
                e.preventDefault();
                if (!sel.has(ri)) {
                  anchor.current = vi.index;
                  setSel(new Set([ri]));
                  onSelect([ri]);
                }
                setMenu({ x: e.clientX, y: e.clientY });
              }}
            >
              {res.columns.map((c, ci) => {
                const here = cur?.r === vi.index && cur.c === ci;
                if (here && editing)
                  return (
                    <div key={c} className="cell editing">
                      <input
                        autoFocus
                        value={draft}
                        onChange={(e) => setDraft(e.target.value)}
                        onBlur={() => setEditing(false)}
                        onKeyDown={(e) => {
                          const save =
                            e.key === "Enter" ||
                            ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s");
                          if (save) {
                            e.preventDefault();
                            commit(1);
                          } else if (e.key === "Tab") {
                            e.preventDefault();
                            commit(2);
                          } else if (e.key === "Escape") {
                            e.preventDefault();
                            setEditing(false);
                            parent.current?.focus();
                          }
                        }}
                      />
                    </div>
                  );
                return (
                  <div
                    key={c}
                    className={cellClass(row[c]) + (here ? " picked" : "")}
                    title={
                      editable
                        ? `${cellText(row[c])}

พิมพ์ทับได้เลย หรือกด Enter/F2 เพื่อแก้ (พิมพ์ NULL = ค่าว่าง)`
                        : cellText(row[c])
                    }
                    onMouseDown={(e) => {
                      if (e.button !== 0) return;
                      setCur({ r: vi.index, c: ci });
                      pick(vi.index, e);
                    }}
                    onDoubleClick={() =>
                      editable ? startEdit(vi.index, ci) : onCopy(cellText(row[c]))
                    }
                  >
                    {cellText(row[c])}
                  </div>
                );
              })}
            </div>
          );
        })}
      </div>

      {menu && (
        <>
          <div
            className="ctx-backdrop"
            onMouseDown={() => setMenu(null)}
            onContextMenu={(e) => {
              e.preventDefault();
              setMenu(null);
            }}
          />
          <div className="ctxmenu" style={{ left: menu.x, top: menu.y }}>
            <button onClick={() => (onCopy(asTsv(picked())), setMenu(null))}>
              <Copy size={14} weight="duotone" /> Copy {sel.size > 1 ? `${sel.size} rows` : "row"}
            </button>
            <button onClick={() => (onCopy(JSON.stringify(picked(), null, 2)), setMenu(null))}>
              <BracketsCurly size={14} weight="duotone" /> Copy as JSON
            </button>
            <button onClick={() => (onExportJson(picked()), setMenu(null))}>
              <DownloadSimple size={14} weight="duotone" /> Export as JSON…
            </button>
          </div>
        </>
      )}
    </div>
  );
});

/* ---------- app ---------- */

export default function App() {
  const [conns, setConns] = useState<Conn[]>(loadConns);
  const [activeConn, setActiveConn] = useState<string>("");
  const [live, setLive] = useState<Record<string, Meta>>({});
  const [filter, setFilter] = useState("");
  const [picked, setPicked] = useState<TableInfo | null>(null);
  const [tabs, setTabs] = useState<Tab[]>(BOOT.tabs);
  const [activeTab, setActiveTab] = useState<string>(BOOT.active);
  const [editorH, setEditorH] = useState(230);
  const [hasSel, setHasSel] = useState(false);
  // key ที่ใช้ชี้แถวเดียว ต่อ connection+ตาราง — ถามครั้งเดียวแล้วจำไว้
  const [keys, setKeys] = useState<Record<string, string[] | null>>({});
  const cmRef = useRef<ReactCodeMirrorRef>(null);
  const [toast, setToast] = useState("");
  const [form, setForm] = useState<Conn | null>(null);
  const [props, setProps] = useState<{ table: TableInfo; data: TableProps } | null>(null);
  const [exportOpen, setExportOpen] = useState(false);
  const [restoreFile, setRestoreFile] = useState<string | null>(null);
  const [test, setTest] = useState<{ ok: boolean; msg: string } | null>(null);
  const [testing, setTesting] = useState(false);
  const [version, setVersion] = useState("");
  const [update, setUpdate] = useState<Update | null>(null);
  const [pct, setPct] = useState<number | null>(null);
  const [updatesOpen, setUpdatesOpen] = useState(false);
  const [rels, setRels] = useState<Release[] | null>(null);
  const [relErr, setRelErr] = useState("");
  const [gridFilter, setGridFilter] = useState("");
  const [selRows, setSelRows] = useState<number[]>([]);
  const [confirmDel, setConfirmDel] = useState<Record<string, unknown> | null>(null);
  const [stage, setStage] = useState("");
  const [erOpen, setErOpen] = useState(false);
  const [tblMenu, setTblMenu] = useState<{ x: number; y: number; t: TableInfo } | null>(null);
  const [edges, setEdges] = useState<Edge[] | null>(null);
  const [addRow, setAddRow] = useState<{ cols: ColumnInfo[]; vals: Record<string, string> } | null>(
    null,
  );
  const [busy, setBusy] = useState(false);

  const tab = tabs.find((t) => t.id === activeTab) ?? tabs[0];
  const tabConn = tab?.conn || activeConn;
  const connected = !!live[activeConn];
  // sidebar โชว์ของ connection ที่เลือกอยู่ ส่วน autocomplete ใช้ของ connection ที่แท็บผูกไว้
  const tables = live[activeConn]?.tables ?? [];
  const schema = live[tabConn]?.schema ?? {};

  /* ตารางเป้าหมายอ่านจากตัว query เอง ไม่ผูกกับว่าเปิดแท็บมาแบบไหน — พิมพ์
     select เองในแท็บใหม่ก็แก้ค่าได้ และพอแก้ query ไปชี้ตารางอื่นมันก็ตามไปเอง */
  const target = useMemo(() => {
    const t = targetTable(tab?.sql ?? "");
    if (!t) return "";
    if (t.schema) return `"${t.schema}"."${t.name}"`;
    // ไม่ได้ระบุ schema — หาจากรายชื่อตารางจริงก่อน ค่อย fallback เป็น public
    const hit = (live[tabConn]?.tables ?? []).filter((x) => x.name === t.name);
    return hit.length === 1 ? `"${hit[0].schema}"."${t.name}"` : `"public"."${t.name}"`;
  }, [tab?.sql, live, tabConn]);

  const keyId = target ? `${tabConn}::${target}` : "";
  const rowKey = keyId ? keys[keyId] : null;

  useEffect(() => {
    if (!keyId || !live[tabConn] || keys[keyId] !== undefined) return;
    let alive = true;
    invoke<string[]>("list_pk", { conn: tabConn, table: target })
      .then((k) => alive && setKeys((m) => ({ ...m, [keyId]: k })))
      .catch(() => alive && setKeys((m) => ({ ...m, [keyId]: [] })));
    return () => {
      alive = false;
    };
  }, [keyId, target, tabConn, live, keys]);

  const missingKeys = (rowKey ?? []).filter((k) => !tab?.res?.columns.includes(k));

  const editable = !!(tab?.res?.columns.length && rowKey?.length && !missingKeys.length);

  const editReason = !tab?.res?.columns.length
    ? ""
    : !target
      ? "แก้ค่าได้เฉพาะ select จากตารางเดียว (มี join หรือ subquery จะปิดไว้)"
      : rowKey === undefined
        ? "กำลังตรวจ key ของตาราง…"
        : !rowKey?.length
          ? "ตารางนี้ไม่มี primary key หรือ unique index — แก้ค่าตรง ๆ ไม่ได้"
          : `ใส่ ${missingKeys.join(", ")} ไว้ใน select ด้วยถึงจะแก้ค่าได้`;

  const canAddRow = !!target;

  useEffect(() => {
    setSelRows([]);
    setGridFilter("");
  }, [tab?.id, tab?.res]);

  useEffect(() => setActiveTab((a) => a || tabs[0].id), [tabs]);
  useEffect(() => {
    getVersion().then(setVersion).catch(() => {});
  }, []);
  useEffect(() => localStorage.setItem(CONNS_KEY, JSON.stringify(conns)), [conns]);

  useEffect(() => {
    const slim = tabs.map((t) => ({
      id: t.id,
      conn: t.conn,
      title: t.title,
      sql: t.sql,
      source: t.source,
    }));
    localStorage.setItem(TABS_KEY, JSON.stringify({ tabs: slim, active: activeTab }));
  }, [tabs, activeTab]);
  // แก้ค่าในฟอร์มเมื่อไร ผลทดสอบเดิมถือว่าใช้ไม่ได้แล้ว
  useEffect(() => setTest(null), [form?.host, form?.port, form?.user, form?.pass, form?.db, form?.ssl, form?.url]);

  const say = useCallback((m: string) => {
    setToast(m);
    setTimeout(() => setToast(""), 3000);
  }, []);

  const patch = useCallback(
    (id: string, p: Partial<Tab>) =>
      setTabs((ts) => ts.map((t) => (t.id === id ? { ...t, ...p } : t))),
    [],
  );

  const loadMeta = useCallback(async (id: string) => {
    const [tbls, cols] = await Promise.all([
      invoke<TableInfo[]>("list_tables", { conn: id }),
      invoke<[string, string, string][]>("list_all_columns", { conn: id }),
    ]);
    const map: Record<string, string[]> = {};
    for (const [s, t, c] of cols) {
      (map[`${s}.${t}`] ??= []).push(c);
      (map[t] ??= []).push(c);
    }
    setLive((l) => ({ ...l, [id]: { tables: tbls, schema: map } }));
  }, []);

  const refresh = useCallback(async () => {
    try {
      await loadMeta(activeConn);
    } catch (e) {
      say(String(e));
    }
  }, [loadMeta, activeConn, say]);

  const doConnect = useCallback(
    async (c: Conn) => {
      setBusy(true);
      try {
        const info = await invoke<ConnInfo>("connect", { conn: c.id, url: connUrl(c) });
        setActiveConn(c.id);
        await loadMeta(c.id);
        // เลือกผิดก็ยังใช้ได้ — จำค่าที่ตรวจได้จริงไว้แทน
        if (info.engine !== c.engine) {
          setConns((cs) => cs.map((x) => (x.id === c.id ? { ...x, engine: info.engine } : x)));
          say(`เชื่อมต่อ ${c.name} แล้ว — ตรวจพบว่าเป็น ${info.engine}`);
        } else {
          say(`เชื่อมต่อ ${c.name} แล้ว`);
        }
      } catch (e) {
        say(String(e));
      } finally {
        setBusy(false);
      }
    },
    [loadMeta, say],
  );

  const dropConn = useCallback(async (id: string) => {
    await invoke("disconnect", { conn: id }).catch(() => {});
    setLive((l) => {
      const rest = { ...l };
      delete rest[id];
      setActiveConn((a) => (a === id ? (Object.keys(rest)[0] ?? "") : a));
      return rest;
    });
  }, []);

  const run = useCallback(
    async (id: string, sqlText: string, connId?: string) => {
      const c = connId ?? tabs.find((t) => t.id === id)?.conn ?? activeConn;
      if (!live[c]) return say("แท็บนี้ยังไม่ได้เชื่อมต่อ");
      if (!sqlText.trim()) return;
      patch(id, { running: true, err: undefined });
      try {
        const res = await invoke<QueryResult>("run_query", { conn: c, sql: sqlText });
        patch(id, { res, running: false });
      } catch (e) {
        patch(id, { err: String(e), running: false, res: undefined });
      }
    },
    [tabs, live, activeConn, patch, say],
  );

  const openTable = useCallback(
    (t: TableInfo, exec = true) => {
      const src = qname(t);
      const q = `select *\nfrom ${src}\nlimit 500;`;
      const nt = newTab(activeConn, { title: t.name, sql: q, source: src });
      setTabs((ts) => [...ts, nt]);
      setActiveTab(nt.id);
      if (exec) run(nt.id, q, activeConn);
    },
    [run, activeConn],
  );

  const showProps = useCallback(
    async (t: TableInfo) => {
      try {
        setProps({ table: t, data: await invoke<TableProps>("table_props", { conn: activeConn, table: qname(t) }) });
      } catch (e) {
        say(String(e));
      }
    },
    [activeConn, say],
  );

  /* เปิดฟอร์มเพิ่มแถว — ดึง type/default ของคอลัมน์มาโชว์เป็นคำใบ้ */
  const openAddRow = useCallback(async () => {
    if (!tab || !target) return;
    try {
      const props = await invoke<TableProps>("table_props", { conn: tab.conn, table: target });
      setAddRow({ cols: props.columns, vals: {} });
    } catch (e) {
      say(String(e));
    }
  }, [tab, target, say]);

  const saveNewRow = useCallback(async () => {
    if (!addRow || !tab || !target) return;
    // ส่งเฉพาะช่องที่กรอกจริง — ที่เหลือปล่อยให้ DEFAULT ของตารางทำงาน
    const values = Object.entries(addRow.vals)
      .filter(([, v]) => v !== "")
      .map(([column, v]) => ({ column, value: v.toUpperCase() === "NULL" ? null : v }));
    try {
      await invoke("insert_row", { conn: tab.conn, table: target, values });
      setAddRow(null);
      say("เพิ่มแถวแล้ว");
      run(tab.id, tab.sql);
    } catch (e) {
      say(String(e));
    }
  }, [addRow, tab, target, run, say]);

  const openEr = useCallback(async () => {
    setErOpen(true);
    setEdges(null);
    try {
      setEdges(await invoke<Edge[]>("er_edges", { conn: activeConn }));
    } catch (e) {
      setErOpen(false);
      say(String(e));
    }
  }, [activeConn, say]);

  const pkKeys = useCallback(
    (row: Record<string, unknown>) =>
      (rowKey ?? []).map((k) => ({
        column: k,
        value: row[k] === null || row[k] === undefined ? null : cellText(row[k]),
      })),
    [rowKey],
  );

  const doDelete = useCallback(async () => {
    if (!confirmDel || !tab || !target) return;
    const keys = pkKeys(confirmDel);
    setConfirmDel(null);
    try {
      await invoke("delete_row", { conn: tab.conn, table: target, keys });
      say("ลบแถวแล้ว");
      setSelRows([]);
      run(tab.id, tab.sql);
    } catch (e) {
      say(String(e));
    }
  }, [confirmDel, tab, target, pkKeys, run, say]);

  const editCell = useCallback(
    async (rowIndex: number, column: string, value: string | null) => {
      if (!tab?.res || !target || !rowKey?.length) return;
      const row = tab.res.rows[rowIndex];
      const keys = pkKeys(row);
      try {
        await invoke("update_cell", { conn: tab.conn, table: target, column, value, keys });
        const rows = tab.res.rows.slice();
        rows[rowIndex] = { ...row, [column]: value };
        patch(tab.id, { res: { ...tab.res, rows } });
        say(`อัปเดต ${column} แล้ว`);
      } catch (e) {
        say(String(e));
      }
    },
    [tab, target, rowKey, patch, pkKeys, say],
  );

  const addTab = useCallback(() => {
    const nt = newTab(activeConn);
    setTabs((ts) => [...ts, nt]);
    setActiveTab(nt.id);
  }, [activeConn]);

  const closeTab = useCallback((id: string) => {
    setTabs((ts) => {
      const left = ts.filter((t) => t.id !== id);
      const next = left.length ? left : [newTab("")];
      setActiveTab((a) => (a === id ? next[next.length - 1].id : a));
      return next;
    });
  }, []);

  const closeAll = useCallback(() => {
    const nt = newTab(activeConn);
    setTabs([nt]);
    setActiveTab(nt.id);
  }, [activeConn]);

  const exportAs = useCallback(
    async (kind: "csv" | "sql" | "json", only?: Record<string, unknown>[]) => {
      const rows = only ?? tab?.res?.rows;
      if (!tab?.res || !rows?.length) return say("ไม่มีผลลัพธ์ให้ export");
      const path = await save({
        defaultPath: `${tab.title}.${kind}`,
        filters: [{ name: kind.toUpperCase(), extensions: [kind] }],
      });
      if (!path) return;
      try {
        const n = await invoke<number>(`export_${kind}`, {
          path,
          table: tab.source ?? tab.title,
          columns: tab.res.columns,
          rows,
        });
        say(`export ${n} แถว → ${path}`);
      } catch (e) {
        say(String(e));
      }
    },
    [tab, say],
  );

  /* เช็คอัปเดตตอนเปิดแอป — เงียบไว้ถ้าเช็คไม่ได้ (ออฟไลน์/ยังไม่มี release) */
  const checkUpdate = useCallback(
    async (loud = false) => {
      try {
        const u = await check();
        setUpdate(u ?? null);
        if (u) setUpdatesOpen(true);
        else if (loud) say(`ใช้เวอร์ชันล่าสุดอยู่แล้ว (${version})`);
      } catch (e) {
        if (loud) say(String(e));
      }
    },
    [version, say],
  );

  /* ประวัติแต่ละเวอร์ชันดึงสดจาก GitHub Releases — ไม่ต้องฝัง changelog ไว้ในแอป
     เขียน release notes ที่ GitHub ที่เดียว แอปทุกเครื่องเห็นตรงกันทันที */
  const loadReleases = useCallback(async () => {
    setRels(null);
    setRelErr("");
    try {
      const r = await fetch(RELEASES_API);
      if (!r.ok) throw new Error(`GitHub ตอบกลับ ${r.status}`);
      setRels(await r.json());
    } catch (e) {
      setRelErr(String(e));
    }
  }, []);

  const openUpdates = useCallback(() => {
    setUpdatesOpen(true);
    loadReleases();
    checkUpdate();
  }, [loadReleases, checkUpdate]);

  useEffect(() => {
    checkUpdate();
    // เช็คครั้งเดียวตอนเปิด — ไม่ poll ซ้ำ
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const installUpdate = useCallback(async () => {
    if (!update) return;
    let total = 0;
    let got = 0;
    setPct(0);
    setStage("กำลังเริ่มดาวน์โหลด…");
    try {
      await update.downloadAndInstall((e) => {
        if (e.event === "Started") {
          total = e.data.contentLength ?? 0;
          setStage("กำลังดาวน์โหลด");
        } else if (e.event === "Progress") {
          got += e.data.chunkLength;
          setPct(total ? Math.round((got / total) * 100) : null);
          setStage(`กำลังดาวน์โหลด ${mb(got)}${total ? ` / ${mb(total)}` : ""} MB`);
        } else if (e.event === "Finished") {
          setPct(100);
          setStage("กำลังติดตั้ง…");
        }
      });
      setStage("กำลังรีสตาร์ท…");
      await relaunch();
    } catch (e) {
      setPct(null);
      setStage("");
      setUpdate(null);
      say(String(e));
    }
  }, [update, say]);

  const testConn = useCallback(async (c: Conn) => {
    setTesting(true);
    setTest(null);
    try {
      const info = await invoke<ConnInfo>("test_connection", { url: connUrl(c) });
      setTest({ ok: true, msg: `${info.engine} · ${info.version.split(" on ")[0]}` });
    } catch (e) {
      setTest({ ok: false, msg: String(e) });
    } finally {
      setTesting(false);
    }
  }, []);

  const backup = useCallback(async () => {
    if (!connected) return say("ยังไม่ได้เชื่อมต่อ");
    const name = conns.find((c) => c.id === activeConn)?.name ?? "database";
    const path = await save({
      defaultPath: `${name}-backup.sql`,
      filters: [{ name: "SQL", extensions: ["sql"] }],
    });
    if (!path) return;
    setBusy(true);
    say("กำลัง backup… ตารางใหญ่อาจใช้เวลาสักครู่");
    try {
      say(`backup เสร็จ — ${await invoke<string>("backup_database", { conn: activeConn, path })}`);
    } catch (e) {
      say(String(e));
    } finally {
      setBusy(false);
    }
  }, [connected, conns, activeConn, say]);

  const pickRestore = useCallback(async () => {
    if (!connected) return say("ยังไม่ได้เชื่อมต่อ");
    const path = await open({ multiple: false, filters: [{ name: "SQL", extensions: ["sql"] }] });
    if (typeof path === "string") setRestoreFile(path);
  }, [connected, say]);

  const doRestore = useCallback(async () => {
    if (!restoreFile) return;
    setRestoreFile(null);
    setBusy(true);
    say("กำลัง restore…");
    try {
      await invoke("import_sql", { path: restoreFile });
      say("restore สำเร็จ");
      refresh();
    } catch (e) {
      say(String(e));
    } finally {
      setBusy(false);
    }
  }, [restoreFile, refresh, say]);

  const importFile = useCallback(async () => {
    if (!connected) return say("ยังไม่ได้เชื่อมต่อ");
    const path = await open({
      multiple: false,
      filters: [{ name: "CSV / SQL", extensions: ["csv", "sql"] }],
    });
    if (typeof path !== "string") return;
    setBusy(true);
    try {
      if (path.toLowerCase().endsWith(".sql")) {
        await invoke("import_sql", { conn: activeConn, path });
        say("รันไฟล์ SQL เรียบร้อย");
        refresh();
      } else if (!picked) {
        say("เลือกตารางปลายทางในแถบซ้ายก่อน");
      } else {
        const n = await invoke<number>("import_csv", { conn: activeConn, path, table: qname(picked) });
        say(`นำเข้า ${n} แถว → ${picked.name}`);
      }
    } catch (e) {
      say(String(e));
    } finally {
      setBusy(false);
    }
  }, [connected, picked, refresh, say]);

  /* ลากคลุมไว้ = รันเฉพาะที่คลุม, ไม่ได้คลุม = รันเฉพาะคำสั่งที่เคอร์เซอร์อยู่ */
  const runNow = useCallback(() => {
    if (!tab) return;
    const st = cmRef.current?.view?.state;
    if (!st) return run(tab.id, tab.sql);
    const { from, to, head } = st.selection.main;
    const sel = st.sliceDoc(from, to).trim();
    run(tab.id, sel || stmtAt(st.doc.toString(), head));
  }, [tab, run]);

  const runRef = useRef(runNow);
  runRef.current = runNow;

  /* keyboard: Ctrl+Enter รัน, Ctrl+N แท็บใหม่ */
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      // CodeMirror จัดการไปแล้ว (โฟกัสอยู่ในตัวแก้ไข) — ไม่ต้องรันซ้ำ
      if (e.defaultPrevented) return;
      if (!(e.ctrlKey || e.metaKey)) return;
      // Ctrl+S ถูกจัดการที่ input ของ cell แล้ว — กันไม่ให้ webview เด้ง save page
      if (e.key.toLowerCase() === "s") {
        e.preventDefault();
      } else if (e.key === "Enter") {
        e.preventDefault();
        runNow();
      } else if (e.key.toLowerCase() === "n") {
        e.preventDefault();
        addTab();
      }
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [runNow, addTab]);

  const startDrag = (e: React.MouseEvent) => {
    const y0 = e.clientY;
    const h0 = editorH;
    const move = (ev: MouseEvent) =>
      setEditorH(Math.min(window.innerHeight - 220, Math.max(90, h0 + ev.clientY - y0)));
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };

  const shown = useMemo(() => {
    const f = filter.trim().toLowerCase();
    return f ? tables.filter((t) => t.name.toLowerCase().includes(f)) : tables;
  }, [tables, filter]);

  const ctx = tableInQuery(tab?.sql ?? "");
  const cmExt = useMemo(() => {
    const support = sqlLang({
      dialect: PostgreSQL,
      schema,
      defaultSchema: ctx.schema ?? "public",
      defaultTable: ctx.table,
      upperCaseKeywords: true,
    });
    const src = ctx.table ? `"${ctx.schema ?? "public"}"."${ctx.table}"` : "";

    // ค่าจริงจาก DB ตอนพิมพ์ในเครื่องหมายคำพูดหลัง = / like / in (
    const values = async (c: CompletionContext) => {
      if (!src) return null;
      const m = VALUE_RE.exec(c.state.doc.sliceString(Math.max(0, c.pos - 200), c.pos));
      if (!m) return null;
      try {
        const vals = await invoke<string[]>("column_values", {
          conn: tabConn,
          table: src,
          column: m[1],
          prefix: m[2],
        });
        if (!vals.length) return null;
        // ไม่ใส่ validFor: ให้ยิงถาม DB ใหม่ทุกตัวอักษร ไม่ใช่กรองเฉพาะชุดแรกที่ได้มา
        return {
          from: c.pos - m[2].length,
          options: vals.map((v) => ({ label: v, type: "text" })),
        };
      } catch {
        return null;
      }
    };

    return [
      support,
      support.language.data.of({ autocomplete: values }),
      autocompletion({ defaultKeymap: false }),
      // ต้องแย่ง Ctrl+Enter จาก keymap ของ CodeMirror ไม่งั้นมันแทรกบรรทัดใหม่ก่อนแล้วค่อยรัน
      Prec.highest(
        keymap.of([
          {
            key: "Mod-Enter",
            run: () => {
              runRef.current();
              return true;
            },
          },
        ]),
      ),
      completionKeys,
    ];
  }, [schema, ctx.schema, ctx.table, tabConn]);

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="brand">
          <MarkMark />
          MarkDB
          <button className="verbtn" title="ประวัติเวอร์ชัน / ตรวจหาอัปเดต" onClick={openUpdates}>
            v{version}
          </button>
        </div>

        <div className="side-section">
          <div className="side-label">
            <Plug size={13} weight="duotone" /> connections
            <button title="เพิ่ม connection" onClick={() => {
                setTest(null);
                setForm({ ...BLANK, id: uid() });
              }}>
              <Plus size={14} weight="bold" />
            </button>
          </div>
          {conns.map((c) => (
            <button
              key={c.id}
              className={"conn" + (activeConn === c.id ? " on" : "")}
              title={connUrl(c)}
              onClick={() => (live[c.id] ? setActiveConn(c.id) : doConnect(c))}
            >
              <EngineLogo engine={c.engine ?? "postgres"} size={15} />
              <span>{c.name}</span>
              <span className={"cdot" + (live[c.id] ? " on" : "")} />
              {live[c.id] && (
                <span
                  className="x"
                  title="ตัดการเชื่อมต่อ"
                  onClick={(e) => {
                    e.stopPropagation();
                    dropConn(c.id);
                  }}
                >
                  <Plug size={13} />
                </span>
              )}
              <span
                className="x"
                title="แก้ไข"
                onClick={(e) => {
                  e.stopPropagation();
                  setTest(null);
                  setForm(c);
                }}
              >
                <PencilSimple size={13} />
              </span>
              <span
                className="x"
                style={{ marginLeft: 4 }}
                title="ลบ"
                onClick={(e) => {
                  e.stopPropagation();
                  dropConn(c.id);
                  setConns((cs) => cs.filter((x) => x.id !== c.id));
                }}
              >
                <Trash size={13} />
              </span>
            </button>
          ))}
          {!conns.length && (
            <div style={{ color: "var(--dim)", padding: "6px 10px", fontSize: 12 }}>
              ยังไม่มี — กด + เพื่อเพิ่ม
            </div>
          )}
        </div>

        <div className="side-section" style={{ paddingBottom: 0 }}>
          <div className="side-label">
            <TableIcon size={13} weight="duotone" /> tables ({shown.length})
            <button title="ER diagram" onClick={openEr} disabled={!connected}>
              <TreeStructure size={14} weight="bold" />
            </button>
            <button title="รีเฟรช" onClick={refresh}>
              <ArrowClockwise size={14} weight="bold" />
            </button>
          </div>
          <div style={{ position: "relative", marginBottom: 6 }}>
            <MagnifyingGlass
              size={14}
              style={{ position: "absolute", left: 9, top: 9, color: "var(--dim)" }}
            />
            <input
              placeholder="ค้นหาตาราง"
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              style={{ paddingLeft: 28 }}
            />
          </div>
        </div>

        <div className="tables">
          {shown.map((t) => (
            <div
              key={t.schema + t.name}
              className={
                "tbl" + (picked?.name === t.name && picked?.schema === t.schema ? " on" : "")
              }
              title={`${t.schema}.${t.name} — ดับเบิลคลิก = SELECT, คลิกขวา = เมนู`}
              onClick={() => setPicked(t)}
              onDoubleClick={() => openTable(t)}
              onContextMenu={(e) => {
                e.preventDefault();
                setPicked(t);
                setTblMenu({ x: e.clientX, y: e.clientY, t });
              }}
            >
              {t.kind === "view" ? (
                <Eye size={15} weight="duotone" />
              ) : t.kind === "foreign" ? (
                <LinkSimple size={15} weight="duotone" />
              ) : (
                <TableIcon size={15} weight="duotone" />
              )}
              <span>{t.name}</span>
              {t.schema !== "public" && <em>{t.schema}</em>}
            </div>
          ))}
        </div>

        <div className="side-foot">
          <button
            className="btn sm"
            onClick={importFile}
            disabled={!connected}
            title="นำเข้า CSV/SQL"
          >
            <UploadSimple size={15} weight="duotone" /> Import
          </button>
          <button className="btn sm" onClick={() => setExportOpen(true)} disabled={!tab?.res}>
            <DownloadSimple size={15} weight="duotone" /> Export
          </button>
          <button
            className="btn sm"
            onClick={backup}
            disabled={!connected}
            title="dump ทั้ง database เป็นไฟล์ .sql"
          >
            <FloppyDisk size={15} weight="duotone" /> Backup
          </button>
          <button
            className="btn sm"
            onClick={pickRestore}
            disabled={!connected}
            title="รันไฟล์ .sql กลับเข้า database ที่เชื่อมต่ออยู่"
          >
            <ArrowCounterClockwise size={15} weight="duotone" /> Restore
          </button>
        </div>
      </aside>

      <main className="main">
        <div className="tabbar">
          {tabs.map((t) => (
            <div
              key={t.id}
              className={"tab" + (t.id === tab?.id ? " on" : "")}
              onClick={() => setActiveTab(t.id)}
            >
              {t.running ? (
                <Spinner size={13} className="spin" />
              ) : (
                <Lightning size={13} weight="duotone" />
              )}
              <span>{t.title}</span>
              {t.conn !== activeConn && conns.find((c) => c.id === t.conn) && (
                <em className="tabconn">{conns.find((c) => c.id === t.conn)!.name}</em>
              )}
              <span
                className="x"
                onClick={(e) => {
                  e.stopPropagation();
                  closeTab(t.id);
                }}
              >
                <X size={12} weight="bold" />
              </span>
            </div>
          ))}
          <button className="btn primary sm newq" onClick={addTab} title="New Query (Ctrl+N)">
            <Plus size={15} weight="bold" /> New Query
          </button>
          <button
            className="btn sm"
            onClick={closeAll}
            disabled={tabs.length === 1 && !tabs[0].sql}
            title="ปิดแท็บ query ทั้งหมด"
          >
            <X size={14} weight="bold" /> Close all
          </button>
        </div>

        <div className="toolbar">
          <button
            className="btn primary sm"
            onClick={runNow}
            disabled={!connected || tab?.running}
            title={hasSel ? "รันเฉพาะที่ลากคลุมไว้" : "รันเฉพาะคำสั่งที่เคอร์เซอร์อยู่ (คั่นด้วย ;)"}
          >
            <Play size={14} weight="fill" /> {hasSel ? "Run selection" : "Run"}{" "}
            <span style={{ opacity: 0.55 }}>Ctrl+↵</span>
          </button>
          <button
            className="btn sm"
            onClick={openAddRow}
            disabled={!canAddRow}
            title={canAddRow ? `เพิ่มแถวใน ${target}` : "ต้องเป็น select จากตารางเดียวถึงจะเพิ่มแถวได้"}
          >
            <RowsPlusBottom size={15} weight="duotone" /> Add row
          </button>
          <button
            className="btn sm"
            onClick={() =>
              selRows.length === 1 && tab?.res && setConfirmDel(tab.res.rows[selRows[0]])
            }
            disabled={!editable || selRows.length !== 1}
            title={
              !editable
                ? editReason || "ลบแถวตรง ๆ ไม่ได้กับผลลัพธ์นี้"
                : selRows.length !== 1
                  ? "คลิกเลือกแถวเดียวก่อน (ลบทีละแถว)"
                  : "ลบแถวที่เลือก"
            }
          >
            <Trash size={15} weight="duotone" /> Delete row
          </button>
          <div className="filterbox">
            <MagnifyingGlass size={13} />
            <input
              placeholder="กรองในผลลัพธ์"
              value={gridFilter}
              onChange={(e) => setGridFilter(e.target.value)}
            />
            {gridFilter && (
              <button onClick={() => setGridFilter("")} title="ล้าง">
                <X size={11} weight="bold" />
              </button>
            )}
          </div>
          <div className="spacer" />
          {editable ? (
            <span style={{ color: "var(--dim)", fontSize: 12 }}>
              <PencilSimple size={12} style={{ verticalAlign: -1 }} /> คลิกเลือก cell · พิมพ์ได้เลย
              หรือกด Enter/F2 · Enter บันทึกแล้วลงแถวถัดไป · Tab บันทึกแล้วไปขวา · Esc ยกเลิก
            </span>
          ) : (
            editReason && <span style={{ color: "var(--dim)", fontSize: 12 }}>{editReason}</span>
          )}
        </div>

        <div className="editor" style={{ height: editorH }}>
          <CodeMirror
            ref={cmRef}
            value={tab?.sql ?? ""}
            height={`${editorH}px`}
            theme={blackTheme}
            extensions={cmExt}
            onChange={(v) => tab && patch(tab.id, { sql: v })}
            onUpdate={(u) => {
              if (u.selectionSet || u.docChanged) setHasSel(!u.state.selection.main.empty);
            }}
            basicSetup={{
              foldGutter: false,
              highlightActiveLineGutter: false,
              autocompletion: false, // ใช้ตัวที่ตั้ง keymap เองใน cmExt แทน
            }}
          />
        </div>

        <div className="dragbar" onMouseDown={startDrag} />

        {tab?.err ? (
          <div className="result">
            <div className="err">{tab.err}</div>
          </div>
        ) : tab?.res ? (
          <Grid
            res={tab.res}
            pk={rowKey ?? []}
            editable={editable}
            filter={gridFilter}
            onEdit={editCell}
            onSelect={setSelRows}
            onExportJson={(rows) => exportAs("json", rows)}
            onCopy={(txt) => {
              navigator.clipboard?.writeText(txt);
              say("คัดลอกแล้ว");
            }}
          />
        ) : (
          <div className="result">
            <div className="empty">
              <Database size={34} weight="duotone" />
              <div>
                {connected
                  ? "ดับเบิลคลิกตารางทางซ้าย หรือพิมพ์ SQL แล้วกด Ctrl+Enter"
                  : "กด + เพิ่ม connection เพื่อเริ่มต้น"}
              </div>
            </div>
          </div>
        )}

        <div className="status">
          <span className={"dot" + (connected ? " on" : "")} />
          <span>
            {connected ? conns.find((c) => c.id === activeConn)?.name : "ไม่ได้เชื่อมต่อ"}
          </span>
          {tab?.res && (
            <>
              <span>
                <b>{tab.res.rows.length}</b> แถว
              </span>
              <span>
                <b>{tab.res.elapsed_ms}</b> ms
              </span>
              {selRows.length > 0 && (
                <span>
                  เลือก <b>{selRows.length}</b> แถว
                </span>
              )}
              {tab.res.truncated && <span>ตัดที่ 5000 แถว</span>}
            </>
          )}
          <span style={{ flex: 1 }} />
          {busy && <span>กำลังทำงาน…</span>}
        </div>
      </main>

      {form && (
        <div className="overlay" onClick={() => setForm(null)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <h3>
              <Plug size={17} weight="duotone" /> Connection
            </h3>
            <p>เลือกชนิดฐานข้อมูล แล้วกรอกทีละช่อง หรือวาง connection string ลงช่อง Host</p>

            <div className="engines">
              {(["postgres", "redshift"] as Engine[]).map((e) => (
                <button
                  key={e}
                  className={"engine" + (form.engine === e ? " on" : "")}
                  onClick={() =>
                    setForm({
                      ...form,
                      engine: e,
                      // พอร์ตยังเป็นค่าเริ่มต้นของอีกฝั่งอยู่ค่อยเปลี่ยนให้ ไม่ทับที่พิมพ์เอง
                      port: form.port === PORTS[form.engine] || !form.port ? PORTS[e] : form.port,
                    })
                  }
                >
                  <EngineLogo engine={e} size={26} />
                  <b>{e === "redshift" ? "Amazon Redshift" : "PostgreSQL"}</b>
                  <em>พอร์ต {PORTS[e]}</em>
                </button>
              ))}
            </div>

            <div className="field">
              <label>ชื่อเรียก</label>
              <input
                autoFocus
                placeholder="local dev"
                value={form.name}
                onChange={(e) => setForm({ ...form, name: e.target.value })}
              />
            </div>

            <div className="field row2">
              <div>
                <label>Host</label>
                <input
                  placeholder="localhost หรือ postgres://…"
                  value={form.url ?? form.host}
                  onChange={(e) => {
                    const v = e.target.value;
                    // วาง URL มา → เก็บเป็น URL ล้วนแทนการแยกช่อง
                    setForm(
                      v.startsWith("postgres")
                        ? { ...form, url: v }
                        : { ...form, url: undefined, host: v },
                    );
                  }}
                />
              </div>
              <div>
                <label>Port</label>
                <input
                  value={form.port}
                  onChange={(e) => setForm({ ...form, port: e.target.value })}
                />
              </div>
            </div>

            <div className="field row2 even">
              <div>
                <label>User</label>
                <input
                  value={form.user}
                  onChange={(e) => setForm({ ...form, user: e.target.value })}
                />
              </div>
              <div>
                <label>Password</label>
                <input
                  type="password"
                  value={form.pass}
                  onChange={(e) => setForm({ ...form, pass: e.target.value })}
                />
              </div>
            </div>

            <div className="field">
              <label>Database</label>
              <input value={form.db} onChange={(e) => setForm({ ...form, db: e.target.value })} />
            </div>

            <label className="check">
              <input
                type="checkbox"
                checked={form.ssl}
                onChange={(e) => setForm({ ...form, ssl: e.target.checked })}
              />
              ใช้ SSL (sslmode=require)
            </label>

            {test && (
              <div className={"testres " + (test.ok ? "ok" : "bad")}>
                {test.ok ? <CheckCircle size={16} weight="fill" /> : <WarningCircle size={16} weight="fill" />}
                <span>{test.msg}</span>
              </div>
            )}

            <div className="modal-foot">
              <button className="btn sm" onClick={() => setForm(null)}>
                ยกเลิก
              </button>
              <button className="btn sm" onClick={() => testConn(form)} disabled={testing}>
                {testing ? <Spinner size={14} className="spin" /> : <Plug size={14} weight="duotone" />}
                {testing ? "กำลังทดสอบ…" : "Test connection"}
              </button>
              <button
                className="btn primary sm"
                onClick={() => {
                  const c = { ...form, name: form.name.trim() || form.db || form.engine };
                  setConns((cs) =>
                    cs.some((x) => x.id === c.id)
                      ? cs.map((x) => (x.id === c.id ? c : x))
                      : [...cs, c],
                  );
                  setForm(null);
                  doConnect(c);
                }}
              >
                บันทึก & เชื่อมต่อ
              </button>
            </div>
          </div>
        </div>
      )}

      {addRow && (
        <div className="overlay" onClick={() => setAddRow(null)}>
          <div className="modal wide" onClick={(e) => e.stopPropagation()}>
            <h3>
              <RowsPlusBottom size={17} weight="duotone" /> เพิ่มแถวใน {tab?.title}
            </h3>
            <p>เว้นว่างไว้ = ใช้ค่า default ของคอลัมน์ · พิมพ์ NULL = ใส่ค่าว่าง</p>
            <div className="cols">
              {addRow.cols.map((c) => (
                <label className="addrow" key={c.name}>
                  <span>
                    {c.pk && "🔑 "}
                    {c.name}
                    <em>{c.data_type}</em>
                  </span>
                  <input
                    value={addRow.vals[c.name] ?? ""}
                    placeholder={c.default || (c.nullable ? "NULL" : "ต้องกรอก")}
                    onChange={(e) =>
                      setAddRow({ ...addRow, vals: { ...addRow.vals, [c.name]: e.target.value } })
                    }
                  />
                </label>
              ))}
            </div>
            <div className="modal-foot">
              <button className="btn sm" onClick={() => setAddRow(null)}>
                ยกเลิก
              </button>
              <button className="btn primary sm" onClick={saveNewRow}>
                เพิ่มแถว
              </button>
            </div>
          </div>
        </div>
      )}

      {tblMenu && (
        <>
          <div
            className="ctx-backdrop"
            onMouseDown={() => setTblMenu(null)}
            onContextMenu={(e) => {
              e.preventDefault();
              setTblMenu(null);
            }}
          />
          <div className="ctxmenu" style={{ left: tblMenu.x, top: tblMenu.y }}>
            <div className="ctxhead">
              {tblMenu.t.schema}.{tblMenu.t.name}
            </div>
            <button onClick={() => (openTable(tblMenu.t), setTblMenu(null))}>
              <Play size={13} weight="fill" /> Open (run SELECT)
            </button>
            <button onClick={() => (openTable(tblMenu.t, false), setTblMenu(null))}>
              <Plus size={14} weight="bold" /> New query
            </button>
            <button onClick={() => (showProps(tblMenu.t), setTblMenu(null))}>
              <Info size={14} weight="duotone" /> Properties
            </button>
            <button onClick={() => (openEr(), setTblMenu(null))}>
              <TreeStructure size={14} weight="duotone" /> ER diagram
            </button>
          </div>
        </>
      )}

      {erOpen && (
        <div className="overlay" onClick={() => setErOpen(false)}>
          <div className="modal ermodal" onClick={(e) => e.stopPropagation()}>
            <h3>
              <TreeStructure size={17} weight="duotone" /> ER diagram
              <button className="btn ghost sm" style={{ marginLeft: "auto" }} onClick={() => setErOpen(false)}>
                <X size={14} weight="bold" />
              </button>
            </h3>
            {edges === null ? (
              <div className="empty">
                <Spinner size={26} className="spin" />
                <div>กำลังอ่านความสัมพันธ์…</div>
              </div>
            ) : (
              <ErDiagram
                edges={edges}
                onOpen={(name) => {
                  const clean = name.replace(/"/g, "");
                  const [a, b] = clean.split(".");
                  openTable(
                    b ? { schema: a, name: b, kind: "table" } : { schema: "public", name: a, kind: "table" },
                  );
                  setErOpen(false);
                }}
              />
            )}
          </div>
        </div>
      )}

      {confirmDel && (
        <div className="overlay" onClick={() => setConfirmDel(null)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <h3>
              <Trash size={17} weight="duotone" /> ลบแถวนี้?
            </h3>
            <p>ลบแล้วกู้คืนไม่ได้</p>
            <div className="warn">
              <div>
                จาก <b>{target}</b>
              </div>
              <div className="path">
                {(rowKey ?? [])
                  .map((k) => `${k} = ${cellText(confirmDel[k])}`)
                  .join("  ·  ")}
              </div>
            </div>
            <div className="modal-foot">
              <button className="btn sm" onClick={() => setConfirmDel(null)}>
                ยกเลิก
              </button>
              <button className="btn primary sm danger" onClick={doDelete}>
                ลบแถว
              </button>
            </div>
          </div>
        </div>
      )}

      {updatesOpen && (
        <div className="overlay" onClick={() => pct === null && setUpdatesOpen(false)}>
          <div className="modal wide" onClick={(e) => e.stopPropagation()}>
            <h3>
              <Confetti size={17} weight="duotone" /> อัปเดต
            </h3>
            <p>ตอนนี้ใช้ v{version}</p>

            {pct !== null ? (
              <div className="installing">
                <div className="ring">
                  <svg viewBox="0 0 44 44">
                    <circle cx="22" cy="22" r="19" className="track" />
                    <circle
                      cx="22"
                      cy="22"
                      r="19"
                      className="fill"
                      style={{ strokeDashoffset: 119.4 - (119.4 * pct) / 100 }}
                    />
                  </svg>
                  <b>{pct}%</b>
                </div>
                <div className="istext">
                  <b>{stage}</b>
                  <span>อย่าปิดโปรแกรมระหว่างนี้ — เดี๋ยวเปิดกลับมาเองอัตโนมัติ</span>
                </div>
              </div>
            ) : update ? (
              <div className="newver">
                <div>
                  มีเวอร์ชันใหม่ <b>v{update.version}</b>
                </div>
                <button className="btn primary sm" onClick={installUpdate}>
                  อัปเดตแล้วรีสตาร์ท
                </button>
              </div>
            ) : (
              <div className="uptodate">ใช้เวอร์ชันล่าสุดอยู่แล้ว</div>
            )}

            <div className="side-label" style={{ paddingLeft: 0 }}>
              ประวัติเวอร์ชัน
            </div>
            <div className="cols">
              {relErr && <div className="relrow">โหลดประวัติไม่ได้ — {relErr}</div>}
              {!relErr && rels === null && <div className="relrow">กำลังโหลด…</div>}
              {rels?.length === 0 && <div className="relrow">ยังไม่มี release</div>}
              {rels?.map((r) => (
                <div className="relrow" key={r.tag_name}>
                  <div className="reltop">
                    <b>{r.name || r.tag_name}</b>
                    <span>{r.published_at?.slice(0, 10)}</span>
                    {r.tag_name === `v${version}` && <i>ที่ใช้อยู่</i>}
                  </div>
                  {r.body?.trim() && <div className="relbody">{r.body.trim()}</div>}
                </div>
              ))}
            </div>

            <div className="modal-foot">
              <button className="btn sm" onClick={loadReleases}>
                <ArrowClockwise size={14} weight="bold" /> รีเฟรช
              </button>
              <button
                className="btn primary sm"
                onClick={() => setUpdatesOpen(false)}
                disabled={pct !== null}
              >
                ปิด
              </button>
            </div>
          </div>
        </div>
      )}

      {restoreFile && (
        <div className="overlay" onClick={() => setRestoreFile(null)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <h3>
              <ArrowCounterClockwise size={17} weight="duotone" /> ยืนยันการ restore
            </h3>
            <p>คำสั่งทั้งไฟล์จะถูกรันเข้า database ที่เชื่อมต่ออยู่ตอนนี้</p>
            <div className="warn">
              <div>
                ปลายทาง <b>{conns.find((c) => c.id === activeConn)?.name}</b>
              </div>
              <div className="path">{restoreFile}</div>
              <div>
                ทั้งไฟล์รันใน transaction เดียว — พังกลางทางจะ rollback คืนทั้งหมด แต่ถ้าสำเร็จ
                ข้อมูลที่ไฟล์เขียนทับจะ<b> กู้คืนไม่ได้</b>
              </div>
            </div>
            <div className="modal-foot">
              <button className="btn sm" onClick={() => setRestoreFile(null)}>
                ยกเลิก
              </button>
              <button className="btn primary sm danger" onClick={doRestore}>
                รันไฟล์นี้
              </button>
            </div>
          </div>
        </div>
      )}

      {exportOpen && tab?.res && (
        <div className="overlay" onClick={() => setExportOpen(false)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <h3>
              <DownloadSimple size={17} weight="duotone" /> Export ผลลัพธ์
            </h3>
            <p>
              {tab.res.rows.length.toLocaleString()} แถว · {tab.res.columns.length} คอลัมน์
            </p>
            <button
              className="pick"
              onClick={() => {
                setExportOpen(false);
                exportAs("csv");
              }}
            >
              <DownloadSimple size={20} weight="duotone" />
              <div>
                <b>CSV</b>
                <span>UTF-8 พร้อม BOM — เปิดใน Excel อ่านภาษาไทยได้</span>
              </div>
            </button>
            <button
              className="pick"
              onClick={() => {
                setExportOpen(false);
                exportAs("sql");
              }}
            >
              <DownloadSimple size={20} weight="duotone" />
              <div>
                <b>SQL</b>
                <span>INSERT statements ของ {tab.source ?? tab.title}</span>
              </div>
            </button>
            <button
              className="pick"
              onClick={() => {
                setExportOpen(false);
                exportAs("json");
              }}
            >
              <BracketsCurly size={20} weight="duotone" />
              <div>
                <b>JSON</b>
                <span>array ของ object — คลิกขวาที่แถวเพื่อ export เฉพาะที่เลือก</span>
              </div>
            </button>
            <div className="modal-foot">
              <button className="btn sm" onClick={() => setExportOpen(false)}>
                ยกเลิก
              </button>
            </div>
          </div>
        </div>
      )}

      {props && (
        <div className="overlay" onClick={() => setProps(null)}>
          <div className="modal wide" onClick={(e) => e.stopPropagation()}>
            <h3>
              <TableIcon size={17} weight="duotone" />
              {props.table.schema}.{props.table.name}
            </h3>
            <p>
              {props.data.columns.length} คอลัมน์ · ~
              {props.data.approx_rows.toLocaleString()} แถว (ประมาณ) · {props.data.size}
            </p>
            <div className="cols">
              <div className="colrow head">
                <span>column</span>
                <span>type</span>
                <span>null</span>
                <span>default</span>
              </div>
              {props.data.columns.map((c) => (
                <div className="colrow" key={c.name}>
                  <span>
                    {c.pk && "🔑 "}
                    {c.name}
                  </span>
                  <span className="ctype">{c.data_type}</span>
                  <span className={c.nullable ? "dimtext" : ""}>
                    {c.nullable ? "NULL" : "NOT NULL"}
                  </span>
                  <span className="dimtext" title={c.default}>
                    {c.default}
                  </span>
                </div>
              ))}
            </div>
            <div className="side-label" style={{ paddingLeft: 0 }}>
              ความสัมพันธ์ ({props.data.relations.length})
            </div>
            <div className="cols">
              {!props.data.relations.length && (
                <div className="relrow">ตารางนี้ไม่มี foreign key เชื่อมกับตารางไหน</div>
              )}
              {props.data.relations.map((r) => (
                <button
                  className="fkrow"
                  key={r.dir + r.name}
                  title={`เปิด ${r.other}`}
                  onClick={() => {
                    const [sc, nm] = r.other.replace(/"/g, "").split(".");
                    openTable(
                      nm
                        ? { schema: sc, name: nm, kind: "table" }
                        : { schema: "public", name: sc, kind: "table" },
                    );
                    setProps(null);
                  }}
                >
                  <span className={"fkdir " + r.dir}>{r.dir === "out" ? "→" : "←"}</span>
                  <span>
                    <b>{r.other}</b>
                    <em>{r.def}</em>
                  </span>
                </button>
              ))}
            </div>

            <div className="modal-foot">
              <button
                className="btn sm"
                onClick={() => {
                  navigator.clipboard?.writeText(
                    props.data.columns.map((c) => c.name).join(", "),
                  );
                  say("คัดลอกชื่อคอลัมน์แล้ว");
                }}
              >
                คัดลอกชื่อคอลัมน์
              </button>
              <button
                className="btn primary sm"
                onClick={() => {
                  openTable(props.table);
                  setProps(null);
                }}
              >
                SELECT ตารางนี้
              </button>
            </div>
          </div>
        </div>
      )}

      {toast && (
        <div className="toast">
          <Lightning size={14} weight="fill" />
          {toast}
        </div>
      )}
    </div>
  );
}
