import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import CodeMirror from "@uiw/react-codemirror";
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
  Eye,
  FloppyDisk,
  Lightning,
  MagnifyingGlass,
  PencilSimple,
  Play,
  Plug,
  Plus,
  Spinner,
  Table as TableIcon,
  Trash,
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
  title: string;
  sql: string;
  source?: string;
  pk?: string[];
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
type TableProps = { columns: ColumnInfo[]; approx_rows: number; size: string };

type Conn = {
  id: string;
  name: string;
  host: string;
  port: string;
  user: string;
  pass: string;
  db: string;
  ssl: boolean;
  url?: string; // connection ที่บันทึกไว้แบบเดิม (เป็น URL ล้วน)
};

const CONNS_KEY = "markdb.conns";
const ROW_H = 28;
const BLANK: Conn = {
  id: "",
  name: "",
  host: "localhost",
  port: "5432",
  user: "postgres",
  pass: "",
  db: "postgres",
  ssl: false,
};

const uid = () => Math.random().toString(36).slice(2, 9);
const qname = (t: TableInfo) => `"${t.schema}"."${t.name}"`;

const connUrl = (c: Conn) => {
  if (c.url) return c.url;
  const auth = c.pass
    ? `${encodeURIComponent(c.user)}:${encodeURIComponent(c.pass)}@`
    : c.user
      ? `${encodeURIComponent(c.user)}@`
      : "";
  return `postgres://${auth}${c.host}:${c.port || 5432}/${encodeURIComponent(c.db)}${
    c.ssl ? "?sslmode=require" : ""
  }`;
};

const loadConns = (): Conn[] => {
  try {
    return JSON.parse(localStorage.getItem(CONNS_KEY) || "[]").map((c: Conn) => ({
      ...BLANK,
      ...c,
    }));
  } catch {
    return [];
  }
};

const newTab = (over: Partial<Tab> = {}): Tab => ({
  id: uid(),
  title: "Query",
  sql: "",
  ...over,
});

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
    selection: "#2e2e2e",
    selectionMatch: "#2e2e2e",
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

/* ---------- result grid (memo: พิมพ์ใน editor แล้วตารางไม่ re-render) ---------- */

const Grid = memo(function Grid({
  res,
  pk,
  editable,
  onEdit,
  onCopy,
}: {
  res: QueryResult;
  pk: string[];
  editable: boolean;
  onEdit: (rowIndex: number, column: string, value: string | null) => void;
  onCopy: (text: string) => void;
}) {
  const parent = useRef<HTMLDivElement>(null);
  const [picked, setPicked] = useState("");
  const [editing, setEditing] = useState("");
  const [draft, setDraft] = useState("");

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

  const rv = useVirtualizer({
    count: res.rows.length,
    getScrollElement: () => parent.current,
    estimateSize: () => ROW_H,
    overscan: 12,
  });

  const commit = (index: number, col: string, original: unknown) => {
    setEditing("");
    if (draft === cellText(original)) return;
    onEdit(index, col, draft.toUpperCase() === "NULL" ? null : draft);
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
    <div className="result" ref={parent}>
      <div className="grid-head" style={{ gridTemplateColumns: template }}>
        {res.columns.map((c) => (
          <div key={c} className={pk.includes(c) ? "pk" : ""} title={c}>
            {pk.includes(c) ? `🔑 ${c}` : c}
          </div>
        ))}
      </div>
      <div className="grid-body" style={{ height: rv.getTotalSize() }}>
        {rv.getVirtualItems().map((vi) => {
          const row = res.rows[vi.index];
          return (
            <div
              key={vi.key}
              className="grid-row"
              style={{
                gridTemplateColumns: template,
                height: ROW_H,
                transform: `translateY(${vi.start}px)`,
              }}
            >
              {res.columns.map((c) => {
                const id = `${vi.index}:${c}`;
                if (editing === id)
                  return (
                    <div key={c} className="cell editing">
                      <input
                        autoFocus
                        value={draft}
                        onChange={(e) => setDraft(e.target.value)}
                        onBlur={() => setEditing("")}
                        onKeyDown={(e) => {
                          const save = e.key === "Enter" || ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s");
                          if (save) {
                            e.preventDefault();
                            commit(vi.index, c, row[c]);
                          }
                          if (e.key === "Escape") setEditing("");
                        }}
                      />
                    </div>
                  );
                return (
                  <div
                    key={c}
                    className={cellClass(row[c]) + (picked === id ? " picked" : "")}
                    title={
                      editable
                        ? `${cellText(row[c])}\n\nดับเบิลคลิกเพื่อแก้ (พิมพ์ NULL = ค่าว่าง)`
                        : cellText(row[c])
                    }
                    onDoubleClick={() => {
                      setPicked(id);
                      if (editable) {
                        setDraft(row[c] === null || row[c] === undefined ? "" : cellText(row[c]));
                        setEditing(id);
                      } else {
                        onCopy(cellText(row[c]));
                      }
                    }}
                  >
                    {cellText(row[c])}
                  </div>
                );
              })}
            </div>
          );
        })}
      </div>
    </div>
  );
});

/* ---------- app ---------- */

export default function App() {
  const [conns, setConns] = useState<Conn[]>(loadConns);
  const [activeConn, setActiveConn] = useState<string>("");
  const [tables, setTables] = useState<TableInfo[]>([]);
  const [schema, setSchema] = useState<Record<string, string[]>>({});
  const [filter, setFilter] = useState("");
  const [picked, setPicked] = useState<TableInfo | null>(null);
  const [tabs, setTabs] = useState<Tab[]>([newTab()]);
  const [activeTab, setActiveTab] = useState<string>("");
  const [editorH, setEditorH] = useState(230);
  const [toast, setToast] = useState("");
  const [form, setForm] = useState<Conn | null>(null);
  const [props, setProps] = useState<{ table: TableInfo; data: TableProps } | null>(null);
  const [exportOpen, setExportOpen] = useState(false);
  const [restoreFile, setRestoreFile] = useState<string | null>(null);
  const [test, setTest] = useState<{ ok: boolean; msg: string } | null>(null);
  const [testing, setTesting] = useState(false);
  const [busy, setBusy] = useState(false);

  const tab = tabs.find((t) => t.id === activeTab) ?? tabs[0];
  const connected = !!activeConn;

  /* แก้ค่าได้เฉพาะตอนที่ผลลัพธ์ยังมาจากตารางเดิม + มี pk ครบในผลลัพธ์
     (กันเคสแก้ query ไปชี้ตารางอื่นแล้ว UPDATE ลงผิดที่) */
  const editable = !!(
    tab?.source &&
    tab.pk?.length &&
    tab.res &&
    tab.sql.includes(tab.source) &&
    tab.pk.every((k) => tab.res!.columns.includes(k))
  );

  const editReason =
    editable || !tab?.source || !tab.res
      ? ""
      : tab.pk === undefined
        ? "กำลังตรวจ primary key…"
        : !tab.pk.length
          ? "ตารางนี้ไม่มี primary key — แก้ค่าตรง ๆ ไม่ได้"
          : !tab.sql.includes(tab.source)
            ? "query ไม่ตรงกับตารางต้นทางแล้ว — ปิดการแก้ค่าไว้"
            : "ผลลัพธ์ไม่มีคอลัมน์ primary key ครบ — แก้ค่าไม่ได้";

  useEffect(() => setActiveTab((a) => a || tabs[0].id), [tabs]);
  useEffect(() => localStorage.setItem(CONNS_KEY, JSON.stringify(conns)), [conns]);
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

  const loadMeta = useCallback(async () => {
    const [tbls, cols] = await Promise.all([
      invoke<TableInfo[]>("list_tables"),
      invoke<[string, string, string][]>("list_all_columns"),
    ]);
    setTables(tbls);
    const map: Record<string, string[]> = {};
    for (const [s, t, c] of cols) {
      (map[`${s}.${t}`] ??= []).push(c);
      (map[t] ??= []).push(c);
    }
    setSchema(map);
  }, []);

  const refresh = useCallback(async () => {
    try {
      await loadMeta();
    } catch (e) {
      say(String(e));
    }
  }, [loadMeta, say]);

  const doConnect = useCallback(
    async (c: Conn) => {
      setBusy(true);
      try {
        await invoke<string>("connect", { url: connUrl(c) });
        setActiveConn(c.id);
        await loadMeta();
        say(`เชื่อมต่อ ${c.name} แล้ว`);
      } catch (e) {
        setActiveConn("");
        say(String(e));
      } finally {
        setBusy(false);
      }
    },
    [loadMeta, say],
  );

  const run = useCallback(
    async (id: string, sqlText: string) => {
      if (!connected) return say("ยังไม่ได้เชื่อมต่อ");
      if (!sqlText.trim()) return;
      patch(id, { running: true, err: undefined });
      try {
        const res = await invoke<QueryResult>("run_query", { sql: sqlText });
        patch(id, { res, running: false });
      } catch (e) {
        patch(id, { err: String(e), running: false, res: undefined });
      }
    },
    [connected, patch, say],
  );

  const openTable = useCallback(
    (t: TableInfo) => {
      const src = qname(t);
      const q = `select *\nfrom ${src}\nlimit 500;`;
      const nt = newTab({ title: t.name, sql: q, source: src });
      setTabs((ts) => [...ts, nt]);
      setActiveTab(nt.id);
      run(nt.id, q);
      invoke<string[]>("list_pk", { table: src })
        .then((pk) => patch(nt.id, { pk }))
        .catch(() => patch(nt.id, { pk: [] }));
    },
    [run, patch],
  );

  const showProps = useCallback(
    async (t: TableInfo) => {
      try {
        setProps({ table: t, data: await invoke<TableProps>("table_props", { table: qname(t) }) });
      } catch (e) {
        say(String(e));
      }
    },
    [say],
  );

  const editCell = useCallback(
    async (rowIndex: number, column: string, value: string | null) => {
      if (!tab?.res || !tab.source || !tab.pk) return;
      const row = tab.res.rows[rowIndex];
      const keys = tab.pk.map((k) => ({
        column: k,
        value: row[k] === null || row[k] === undefined ? null : cellText(row[k]),
      }));
      try {
        await invoke("update_cell", { table: tab.source, column, value, keys });
        const rows = tab.res.rows.slice();
        rows[rowIndex] = { ...row, [column]: value };
        patch(tab.id, { res: { ...tab.res, rows } });
        say(`อัปเดต ${column} แล้ว`);
      } catch (e) {
        say(String(e));
      }
    },
    [tab, patch, say],
  );

  const addTab = useCallback(() => {
    const nt = newTab();
    setTabs((ts) => [...ts, nt]);
    setActiveTab(nt.id);
  }, []);

  const closeTab = useCallback((id: string) => {
    setTabs((ts) => {
      const left = ts.filter((t) => t.id !== id);
      const next = left.length ? left : [newTab()];
      setActiveTab((a) => (a === id ? next[next.length - 1].id : a));
      return next;
    });
  }, []);

  const exportAs = useCallback(
    async (kind: "csv" | "sql") => {
      if (!tab?.res?.rows.length) return say("ไม่มีผลลัพธ์ให้ export");
      const path = await save({
        defaultPath: `${tab.title}.${kind}`,
        filters: [{ name: kind.toUpperCase(), extensions: [kind] }],
      });
      if (!path) return;
      try {
        const n = await invoke<number>(kind === "csv" ? "export_csv" : "export_sql", {
          path,
          table: tab.source ?? tab.title,
          columns: tab.res.columns,
          rows: tab.res.rows,
        });
        say(`export ${n} แถว → ${path}`);
      } catch (e) {
        say(String(e));
      }
    },
    [tab, say],
  );

  const testConn = useCallback(async (c: Conn) => {
    setTesting(true);
    setTest(null);
    try {
      const ver = await invoke<string>("test_connection", { url: connUrl(c) });
      setTest({ ok: true, msg: ver.split(" on ")[0] });
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
      say(`backup เสร็จ — ${await invoke<string>("backup_database", { path })}`);
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
        await invoke("import_sql", { path });
        say("รันไฟล์ SQL เรียบร้อย");
        refresh();
      } else if (!picked) {
        say("เลือกตารางปลายทางในแถบซ้ายก่อน");
      } else {
        const n = await invoke<number>("import_csv", { path, table: qname(picked) });
        say(`นำเข้า ${n} แถว → ${picked.name}`);
      }
    } catch (e) {
      say(String(e));
    } finally {
      setBusy(false);
    }
  }, [connected, picked, refresh, say]);

  /* keyboard: Ctrl+Enter รัน, Ctrl+N แท็บใหม่ */
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey)) return;
      // Ctrl+S ถูกจัดการที่ input ของ cell แล้ว — กันไม่ให้ webview เด้ง save page
      if (e.key.toLowerCase() === "s") {
        e.preventDefault();
      } else if (e.key === "Enter") {
        e.preventDefault();
        if (tab) run(tab.id, tab.sql);
      } else if (e.key.toLowerCase() === "n") {
        e.preventDefault();
        addTab();
      }
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [tab, run, addTab]);

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
      completionKeys,
    ];
  }, [schema, ctx.schema, ctx.table]);

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="brand">
          <MarkMark />
          MarkDB
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
              onClick={() => doConnect(c)}
            >
              <Database size={15} weight="duotone" />
              <span>{c.name}</span>
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
              title={`${t.schema}.${t.name} — ดับเบิลคลิก = SELECT, คลิกขวา = properties`}
              onClick={() => setPicked(t)}
              onDoubleClick={() => openTable(t)}
              onContextMenu={(e) => {
                e.preventDefault();
                setPicked(t);
                showProps(t);
              }}
            >
              {t.kind === "view" ? (
                <Eye size={15} weight="duotone" />
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
        </div>

        <div className="toolbar">
          <button
            className="btn primary sm"
            onClick={() => tab && run(tab.id, tab.sql)}
            disabled={!connected || tab?.running}
          >
            <Play size={14} weight="fill" /> Run <span style={{ opacity: 0.55 }}>Ctrl+↵</span>
          </button>
          <div className="spacer" />
          {editable ? (
            <span style={{ color: "var(--dim)", fontSize: 12 }}>
              <PencilSimple size={12} style={{ verticalAlign: -1 }} /> ดับเบิลคลิก cell เพื่อแก้ ·
              Ctrl+S หรือ Enter บันทึก
            </span>
          ) : (
            editReason && <span style={{ color: "var(--dim)", fontSize: 12 }}>{editReason}</span>
          )}
        </div>

        <div className="editor" style={{ height: editorH }}>
          <CodeMirror
            value={tab?.sql ?? ""}
            height={`${editorH}px`}
            theme={blackTheme}
            extensions={cmExt}
            onChange={(v) => tab && patch(tab.id, { sql: v })}
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
            pk={editable ? tab.pk! : []}
            editable={editable}
            onEdit={editCell}
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
            <p>กรอกทีละช่อง หรือวาง connection string ลงช่อง Host ก็ได้</p>

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
                  const c = { ...form, name: form.name.trim() || form.db || "postgres" };
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
