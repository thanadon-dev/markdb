use serde::{Deserialize, Serialize};
use sqlx::postgres::{PgPool, PgPoolCopyExt, PgPoolOptions};
use sqlx::{Column, Executor, Row, TypeInfo, ValueRef};
use std::collections::HashMap;
use std::time::Instant;
use tokio::sync::Mutex;

/// แยกผลของแต่ละคำสั่ง: แถวที่มาก่อน "จบคำสั่ง" (Either::Left) เป็นของคำสั่งนั้น
/// ใช้ได้กับทุก driver — $cell คือฟังก์ชันแปลง cell ของ driver นั้นเป็น JSON
macro_rules! result_sets {
    ($stream:expr, $cell:expr) => {{
        use futures_util::TryStreamExt;
        let mut stream = $stream;
        let mut sets: Vec<ResultSet> = Vec::new();
        let mut cur = ResultSet::default();
        let mut seen = 0usize;
        while let Some(item) = stream.try_next().await.map_err(err)? {
            match item {
                sqlx::Either::Left(res) => {
                    cur.affected = if cur.columns.is_empty() { res.rows_affected() } else { seen as u64 };
                    cur.truncated = seen > MAX_ROWS;
                    sets.push(std::mem::take(&mut cur));
                    seen = 0;
                }
                sqlx::Either::Right(row) => {
                    if cur.columns.is_empty() {
                        cur.columns = row.columns().iter().map(|c| c.name().to_string()).collect();
                        dedupe(&mut cur.columns);
                    }
                    seen += 1;
                    if cur.rows.len() < MAX_ROWS {
                        let mut obj = serde_json::Map::with_capacity(cur.columns.len());
                        for (i, c) in cur.columns.iter().enumerate() {
                            obj.insert(c.clone(), $cell(&row, i)?);
                        }
                        cur.rows.push(serde_json::Value::Object(obj));
                    }
                }
            }
        }
        drop(stream);
        // driver ที่ไม่ส่ง "จบคำสั่ง" ปิดท้ายชุดสุดท้าย
        if seen > 0 {
            cur.affected = seen as u64;
            cur.truncated = seen > MAX_ROWS;
            sets.push(cur);
        }
        sets
    }};
}

mod ai;
mod other;
mod sync;
mod tunnel;

const MAX_ROWS: usize = 5000;

/// Redshift พูด wire protocol เดียวกับ Postgres แต่ไม่มี pg_catalog หลายตัวและไม่มี
/// json_agg — คำสั่งที่ต่างกันจึงแยกตาม engine ที่ตรวจได้ตอน connect
#[derive(Clone, Copy, PartialEq)]
enum Engine {
    Postgres,
    Redshift,
}

impl Engine {
    fn name(self) -> &'static str {
        match self {
            Engine::Postgres => "postgres",
            Engine::Redshift => "redshift",
        }
    }
}

#[derive(Clone)]
struct Db {
    pool: PgPool,
    engine: Engine,
}

/// หลาย connection พร้อมกัน — key คือ id ของ connection ฝั่ง UI
/// Postgres/Redshift อยู่ใน conns, SQLite/MySQL อยู่ใน others (driver คนละตัว)
struct AppState {
    conns: Mutex<HashMap<String, Db>>,
    others: Mutex<HashMap<String, other::Other>>,
    /// SSH tunnel ของ connection ที่ต่อผ่าน bastion — ตัดการเชื่อมต่อแล้ว drop = ปิด tunnel
    tunnels: Mutex<HashMap<String, tunnel::Tunnel>>,
}

#[derive(Serialize)]
struct QueryResult {
    columns: Vec<String>,
    rows: Vec<serde_json::Value>,
    affected: u64,
    elapsed_ms: u128,
    truncated: bool,
    /// DELETE ที่พิมพ์เองจาก editor: แถวที่ถูกลบ เก็บไว้ให้ประวัติย้อนกลับได้
    deleted: Option<Deleted>,
    /// รันหลายคำสั่งพร้อมกัน: ผลของทุกคำสั่งตามลำดับ (คำสั่งเดียว = ว่าง)
    sets: Vec<ResultSet>,
}

/// ผลของคำสั่งเดียว
#[derive(Serialize, Default, Clone)]
struct ResultSet {
    columns: Vec<String>,
    rows: Vec<serde_json::Value>,
    affected: u64,
    truncated: bool,
}

/// ตัวที่โชว์เป็นหลัก = ชุดสุดท้ายที่มีคอลัมน์ (select) ไม่มีก็ชุดสุดท้าย
fn into_result(sets: Vec<ResultSet>, t0: Instant) -> QueryResult {
    let main = sets
        .iter()
        .rposition(|s| !s.columns.is_empty())
        .or(sets.len().checked_sub(1))
        .map(|i| sets[i].clone())
        .unwrap_or_default();
    QueryResult {
        columns: main.columns,
        rows: main.rows,
        affected: main.affected,
        truncated: main.truncated,
        elapsed_ms: t0.elapsed().as_millis(),
        deleted: None,
        sets: if sets.len() > 1 { sets } else { Vec::new() },
    }
}

#[derive(Serialize)]
struct Deleted {
    /// ชื่อตารางตามที่พิมพ์ใน DELETE
    table: String,
    /// ว่าง + partial = ลบเยอะเกินกว่าที่เก็บไว้
    rows: Vec<serde_json::Value>,
    partial: bool,
}

/// แถวที่ลบจาก editor ได้มากสุดเท่านี้ถึงจะเก็บไว้ย้อนกลับ — ประวัติอยู่ใน localStorage ซึ่งจำกัดขนาด
const KEEP_DELETED: usize = 1000;

/// `delete from t [alias] [where …]` คำสั่งเดียว → (ชื่อตารางตามที่พิมพ์, select * ที่ได้แถวชุดเดียวกัน)
/// แบบที่มี using / returning / หลายคำสั่ง ไม่แตะ (ลบตามปกติ แต่ไม่เก็บไว้ย้อน)
fn delete_select(sql: &str) -> Option<(String, String)> {
    let s = sql.trim();
    if s.contains(';') {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    let mut words = lower.split_whitespace();
    if words.next() != Some("delete") || words.next() != Some("from") {
        return None;
    }
    if lower
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|w| w == "using" || w == "returning")
    {
        return None;
    }
    // ตัดคำว่า delete from ทิ้ง (lowercase ไม่เปลี่ยนตำแหน่ง byte จึงใช้ตำแหน่งจากตัวนั้นได้)
    let from = lower.find("from")? + 4;
    let rest = s[from..].trim_start();
    let mut quoted = false;
    let end = rest
        .char_indices()
        .find(|&(_, c)| {
            if c == '"' {
                quoted = !quoted;
            }
            !quoted && c.is_whitespace()
        })
        .map(|(i, _)| i)
        .unwrap_or(rest.len());
    Some((rest[..end].to_string(), format!("select * from {}", rest)))
}

/// ลบใน transaction เดียวกับที่อ่านแถวเก็บไว้ — แถวที่เก็บคือแถวที่ถูกลบจริง
async fn run_delete(p: &PgPool, sql: &str, table: String, select: &str) -> R<QueryResult> {
    let t0 = Instant::now();
    let mut tx = p.begin().await.map_err(err)?;
    // ส่งเป็น &str = simple protocol ค่าทุกคอลัมน์มาเป็น text ให้ row_json อ่านได้
    let found = (&mut *tx)
        .fetch_all(format!("select * from ({}) _d limit {}", select, KEEP_DELETED + 1).as_str())
        .await
        .map_err(err)?;
    let affected = (&mut *tx).execute(sql).await.map_err(err)?.rows_affected();
    tx.commit().await.map_err(err)?;
    let partial = found.len() > KEEP_DELETED;
    let rows = if partial {
        Vec::new()
    } else {
        found.iter().map(row_json).collect::<R<Vec<_>>>()?
    };
    Ok(QueryResult {
        columns: Vec::new(),
        rows: Vec::new(),
        affected,
        elapsed_ms: t0.elapsed().as_millis(),
        truncated: false,
        deleted: Some(Deleted { table, rows, partial }),
        sets: Vec::new(),
    })
}

#[derive(Serialize)]
struct TableInfo {
    schema: String,
    name: String,
    kind: String,
}

type R<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

async fn db(state: &AppState, conn: &str) -> R<Db> {
    if let Some(d) = state.conns.lock().await.get(conn).cloned() {
        return Ok(d);
    }
    Err(match state.others.lock().await.get(conn) {
        Some(o) => format!("{} ยังไม่รองรับคำสั่งนี้ (ตอนนี้ทำได้เฉพาะ Postgres / Redshift)", o.label()),
        None => "connection นี้ยังไม่ได้เชื่อมต่อ".into(),
    })
}

/// SQLite / MySQL — None = Postgres/Redshift หรือยังไม่ได้เชื่อมต่อ
async fn other(state: &AppState, conn: &str) -> Option<other::Other> {
    state.others.lock().await.get(conn).cloned()
}

async fn pool(state: &AppState, conn: &str) -> R<PgPool> {
    Ok(db(state, conn).await?.pool)
}

/// version() ของ Redshift ลงท้ายด้วย "Redshift 1.0.xxxxx" — เชื่อค่าจาก server
/// มากกว่าที่ผู้ใช้เลือกในฟอร์ม เลือกผิดก็ยังใช้งานได้
fn detect(version: &str) -> Engine {
    if version.to_ascii_lowercase().contains("redshift") {
        Engine::Redshift
    } else {
        Engine::Postgres
    }
}

/// แยก `"schema"."table"` (หรือ `schema.table` / `table`) เป็นสองส่วน
/// ใช้กับคำสั่งฝั่ง Redshift ที่ถาม information_schema ซึ่งไม่มี regclass ให้ cast
// ponytail: ไม่รองรับจุดที่อยู่ในชื่อจริง เช่น "a.b" — เจอค่อยไป parse ให้ครบ
fn split_name(q: &str) -> (String, String) {
    let unq = |s: &str| s.trim().trim_matches('"').replace("\"\"", "\"");
    if let Some((a, b)) = q.rsplit_once("\".\"") {
        return (unq(a), unq(b));
    }
    match q.split_once('.') {
        Some((a, b)) => (unq(a), unq(b)),
        None => ("public".into(), unq(q)),
    }
}

#[derive(Serialize)]
struct ConnInfo {
    version: String,
    engine: &'static str,
    /// host key ของ SSH server (ต่อผ่าน tunnel) — UI จำไว้เทียบครั้งต่อไป
    fingerprint: Option<String>,
}

/// ต่อผ่าน SSH ถ้ามี: เปิด tunnel แล้วคืน URL ที่ชี้มาที่ port ในเครื่องแทน
async fn via(url: String, ssh: Option<tunnel::Ssh>) -> R<(String, Option<tunnel::Tunnel>)> {
    match ssh {
        Some(s) => {
            let (u, t) = tunnel::through(&s, &url).await?;
            Ok((u, Some(t)))
        }
        None => Ok((url, None)),
    }
}

#[tauri::command]
async fn connect(url: String, conn: String, readonly: Option<bool>, ssh: Option<tunnel::Ssh>,
    state: tauri::State<'_, AppState>) -> R<ConnInfo> {
    let kind = other::kind(&url);
    // SQLite เป็นไฟล์ในเครื่อง ไม่มีอะไรให้ต่อผ่าน SSH
    let (url, tun) = via(url, ssh.filter(|_| !matches!(kind, Some(other::Kind::Lite)))).await?;
    let fingerprint = tun.as_ref().map(|t| t.fingerprint.clone());
    // tunnel เก็บเมื่อต่อ database สำเร็จเท่านั้น — พังกลางทางแล้ว drop = ปิด tunnel ทิ้ง
    let keep = |tun: Option<tunnel::Tunnel>, conn: String| async {
        let mut ts = state.tunnels.lock().await;
        match tun {
            Some(t) => ts.insert(conn, t),
            None => ts.remove(&conn),
        };
    };
    if let Some(kind) = kind {
        let (o, version) = other::open(kind, &url, readonly.unwrap_or(false)).await?;
        let engine = o.name();
        keep(tun, conn.clone()).await;
        state.conns.lock().await.remove(&conn);
        if let Some(old) = state.others.lock().await.insert(conn, o) {
            old.close().await;
        }
        return Ok(ConnInfo { version, engine, fingerprint });
    }
    let p = pool_options(readonly.unwrap_or(false))
        .connect(&url)
        .await
        .map_err(err)?;
    let version: String = sqlx::query_scalar("select version()")
        .fetch_one(&p)
        .await
        .map_err(err)?;
    let engine = detect(&version);
    keep(tun, conn.clone()).await;
    state.others.lock().await.remove(&conn);
    state.conns.lock().await.insert(conn, Db { pool: p, engine });
    Ok(ConnInfo {
        version,
        engine: engine.name(),
        fingerprint,
    })
}

/// อ่านอย่างเดียว: ให้ DB เองปฏิเสธทุกคำสั่งที่เขียน — ครอบทุกทางที่แอปเขียนได้ในที่เดียว
/// ponytail: Redshift ไม่รองรับคำสั่งนี้ก็ข้ามไป เหลือแค่ด่านฝั่ง UI
fn pool_options(ro: bool) -> PgPoolOptions {
    PgPoolOptions::new().max_connections(4).after_connect(move |c, _| {
        Box::pin(async move {
            if ro {
                let _ = c.execute("set session characteristics as transaction read only").await;
            }
            Ok(())
        })
    })
}

/// ลองต่อด้วย connection เดี่ยว ๆ แล้วปิดทิ้ง — ไม่แตะ pool ที่ใช้งานอยู่
/// กด test ระหว่างที่ยังต่อ DB อื่นค้างอยู่จึงไม่ทำให้หลุด
#[tauri::command]
async fn test_connection(url: String, ssh: Option<tunnel::Ssh>) -> R<ConnInfo> {
    use sqlx::Connection;
    let kind = other::kind(&url);
    // tunnel อยู่แค่ตลอดการทดสอบ จบแล้ว drop ปิดเอง
    let (url, tun) = via(url, ssh.filter(|_| !matches!(kind, Some(other::Kind::Lite)))).await?;
    let fingerprint = tun.as_ref().map(|t| t.fingerprint.clone());
    if let Some(kind) = kind {
        let (o, version) = other::open(kind, &url, true).await?;
        o.close().await;
        return Ok(ConnInfo { version, engine: o.name(), fingerprint });
    }
    let mut c = sqlx::PgConnection::connect(&url).await.map_err(err)?;
    let version: String = sqlx::query_scalar("select version()")
        .fetch_one(&mut c)
        .await
        .map_err(err)?;
    c.close().await.ok();
    let engine = detect(&version);
    Ok(ConnInfo {
        version,
        engine: engine.name(),
        fingerprint,
    })
}

#[tauri::command]
async fn disconnect(conn: String,
    state: tauri::State<'_, AppState>) -> R<()> {
    if let Some(d) = state.conns.lock().await.remove(&conn) {
        d.pool.close().await;
    }
    if let Some(o) = state.others.lock().await.remove(&conn) {
        o.close().await;
    }
    state.tunnels.lock().await.remove(&conn);
    Ok(())
}

#[tauri::command]
async fn list_tables(conn: String,
    state: tauri::State<'_, AppState>) -> R<Vec<TableInfo>> {
    if let Some(o) = other(&state, &conn).await {
        return o.list_tables().await;
    }
    let p = pool(&state, &conn).await?;
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "select table_schema, table_name, table_type
         from information_schema.tables
         where table_schema not in ('pg_catalog','information_schema')
         order by table_schema, table_name",
    )
    .fetch_all(&p)
    .await
    .map_err(err)?;
    Ok(rows
        .into_iter()
        .map(|(schema, name, t)| TableInfo {
            schema,
            name,
            kind: match t.as_str() {
                "VIEW" => "view".into(),
                "FOREIGN" => "foreign".into(),
                _ => "table".into(),
            },
        })
        .collect())
}

/// (schema, table, column) ทั้ง database — ใช้ป้อน autocomplete ฝั่ง editor
#[tauri::command]
async fn list_all_columns(conn: String,
    state: tauri::State<'_, AppState>) -> R<Vec<(String, String, String, String)>> {
    if let Some(o) = other(&state, &conn).await {
        return o.list_all_columns().await;
    }
    let p = pool(&state, &conn).await?;
    // type ไม่ได้ใช้กับ autocomplete แต่ส่งให้ AI รู้ว่าคอลัมน์ไหนเป็น bool/วันที่/ตัวเลข
    sqlx::query_as(
        "select table_schema, table_name, column_name, data_type from information_schema.columns
         where table_schema not in ('pg_catalog','information_schema')
         order by table_schema, table_name, ordinal_position",
    )
    .fetch_all(&p)
    .await
    .map_err(err)
}

/// คอลัมน์ที่ใช้ชี้แถวเดียวได้ — primary key ก่อน ไม่มีก็ใช้ unique index ที่ทุกคอลัมน์
/// เป็น NOT NULL แทน (ตารางที่ลืมประกาศ pk จึงยังแก้ค่าในตารางได้)
/// ว่าง = แก้ค่าตรง ๆ ไม่ได้
#[tauri::command]
async fn list_pk(table: String, conn: String,
    state: tauri::State<'_, AppState>) -> R<Vec<String>> {
    if let Some(o) = other(&state, &conn).await {
        return o.list_pk(&table).await;
    }
    let d = db(&state, &conn).await?;
    if d.engine == Engine::Redshift {
        let (sch, tbl) = split_name(&table);
        // Redshift ไม่บังคับ constraint แต่เก็บ metadata ไว้ให้ query ได้
        let rows: Vec<(String, String)> = sqlx::query_as(
            "select tc.constraint_name, kcu.column_name
             from information_schema.table_constraints tc
             join information_schema.key_column_usage kcu
               on kcu.constraint_name = tc.constraint_name
              and kcu.table_schema = tc.table_schema
              and kcu.table_name = tc.table_name
             where tc.table_schema = $1 and tc.table_name = $2
               and tc.constraint_type in ('PRIMARY KEY', 'UNIQUE')
             order by case tc.constraint_type when 'PRIMARY KEY' then 0 else 1 end,
                      tc.constraint_name, kcu.ordinal_position",
        )
        .bind(&sch)
        .bind(&tbl)
        .fetch_all(&d.pool)
        .await
        .map_err(err)?;
        // เอาเฉพาะ constraint ตัวแรก ไม่ปนคอลัมน์ข้าม constraint
        let first = rows.first().map(|(n, _)| n.clone());
        return Ok(rows
            .into_iter()
            .filter(|(n, _)| Some(n) == first.as_ref())
            .map(|(_, c)| c)
            .collect());
    }

    let rows: Vec<(String,)> = sqlx::query_as(
        "with pick as (
           select i.indexrelid, i.indkey
           from pg_index i
           where i.indrelid = $1::regclass and i.indisunique and i.indpred is null
             and not exists (select 1 from unnest(i.indkey) k where k = 0)
             and not exists (
               select 1 from unnest(i.indkey) k
               join pg_attribute aa on aa.attrelid = i.indrelid and aa.attnum = k
               where not aa.attnotnull)
           order by i.indisprimary desc, array_length(i.indkey, 1), i.indexrelid
           limit 1
         )
         select a.attname
         from pick
         cross join unnest(pick.indkey) with ordinality k(attnum, ord)
         join pg_attribute a on a.attrelid = $1::regclass and a.attnum = k.attnum
         order by k.ord",
    )
    .bind(&table)
    .fetch_all(&d.pool)
    .await
    .map_err(err)?;
    Ok(rows.into_iter().map(|(c,)| c).collect())
}

#[derive(Serialize)]
struct ColumnInfo {
    name: String,
    data_type: String,
    nullable: bool,
    default: String,
    pk: bool,
}

#[derive(Serialize)]
struct Relation {
    dir: String,   // "out" = ตารางนี้ชี้ไปหาคนอื่น, "in" = คนอื่นชี้มาหาตารางนี้
    name: String,
    other: String,
    def: String,
}

#[derive(Serialize)]
struct TableProps {
    columns: Vec<ColumnInfo>,
    approx_rows: i64,
    size: String,
    relations: Vec<Relation>,
}

/// properties ของตาราง — อ่านจาก pg_catalog ล้วน ไม่แตะข้อมูลจริงสักแถว
/// (reltuples เป็นค่าประมาณจาก ANALYZE ล่าสุด จึงไม่ต้อง count ทั้งตาราง)
#[tauri::command]
async fn table_props(table: String, conn: String,
    state: tauri::State<'_, AppState>) -> R<TableProps> {
    if let Some(o) = other(&state, &conn).await {
        return o.table_props(&table).await;
    }
    let d = db(&state, &conn).await?;
    if d.engine == Engine::Redshift {
        return redshift_props(&d.pool, &table).await;
    }
    let p = d.pool;
    let cols: Vec<(String, String, bool, String, bool)> = sqlx::query_as(
        "select a.attname,
                format_type(a.atttypid, a.atttypmod),
                not a.attnotnull,
                coalesce(pg_get_expr(d.adbin, d.adrelid), ''),
                coalesce(bool_or(i.indisprimary), false)
         from pg_attribute a
         left join pg_attrdef d on d.adrelid = a.attrelid and d.adnum = a.attnum
         left join pg_index i on i.indrelid = a.attrelid
              and a.attnum = any(i.indkey) and i.indisprimary
         where a.attrelid = $1::regclass and a.attnum > 0 and not a.attisdropped
         group by a.attname, a.atttypid, a.atttypmod, a.attnotnull, d.adbin, d.adrelid, a.attnum
         order by a.attnum",
    )
    .bind(&table)
    .fetch_all(&p)
    .await
    .map_err(err)?;

    let (approx_rows, size): (i64, String) = sqlx::query_as(
        "select greatest(reltuples, 0)::bigint, pg_size_pretty(pg_total_relation_size($1::regclass))
         from pg_class where oid = $1::regclass",
    )
    .bind(&table)
    .fetch_one(&p)
    .await
    .map_err(err)?;

    let rels: Vec<(String, String, String, String)> = sqlx::query_as(
        "select case when con.conrelid = $1::regclass then 'out' else 'in' end,
                con.conname,
                (case when con.conrelid = $1::regclass then con.confrelid else con.conrelid end)
                    ::regclass::text,
                pg_get_constraintdef(con.oid)
         from pg_constraint con
         where con.contype = 'f'
           and (con.conrelid = $1::regclass or con.confrelid = $1::regclass)
         order by 1, 2",
    )
    .bind(&table)
    .fetch_all(&p)
    .await
    .map_err(err)?;

    Ok(TableProps {
        relations: rels
            .into_iter()
            .map(|(dir, name, other, def)| Relation {
                dir,
                name,
                other,
                def,
            })
            .collect(),
        columns: cols
            .into_iter()
            .map(|(name, data_type, nullable, default, pk)| ColumnInfo {
                name,
                data_type,
                nullable,
                default,
                pk,
            })
            .collect(),
        approx_rows,
        size,
    })
}

/// properties ฝั่ง Redshift — information_schema + svv_table_info แทน pg_catalog
async fn redshift_props(p: &PgPool, table: &str) -> R<TableProps> {
    let (sch, tbl) = split_name(table);
    let cols: Vec<(String, String, bool, String)> = sqlx::query_as(
        "select column_name,
                case when character_maximum_length is not null
                       then data_type || '(' || character_maximum_length::text || ')'
                     when data_type in ('numeric', 'decimal') and numeric_precision is not null
                       then data_type || '(' || numeric_precision::text || ',' ||
                            coalesce(numeric_scale, 0)::text || ')'
                     else data_type end,
                is_nullable = 'YES',
                coalesce(column_default, '')
         from information_schema.columns
         where table_schema = $1 and table_name = $2
         order by ordinal_position",
    )
    .bind(&sch)
    .bind(&tbl)
    .fetch_all(p)
    .await
    .map_err(err)?;

    let keys: Vec<(String,)> = sqlx::query_as(
        "select kcu.column_name
         from information_schema.table_constraints tc
         join information_schema.key_column_usage kcu
           on kcu.constraint_name = tc.constraint_name
          and kcu.table_schema = tc.table_schema
          and kcu.table_name = tc.table_name
         where tc.table_schema = $1 and tc.table_name = $2
           and tc.constraint_type = 'PRIMARY KEY'",
    )
    .bind(&sch)
    .bind(&tbl)
    .fetch_all(p)
    .await
    .map_err(err)?;
    let pks: Vec<String> = keys.into_iter().map(|(c,)| c).collect();

    // svv_table_info มีเฉพาะตารางจริงที่มีข้อมูลแล้ว — view หรือตารางว่างจะไม่เจอ
    let info: Option<(i64, String)> = sqlx::query_as(
        "select coalesce(tbl_rows, 0)::bigint, coalesce(size, 0)::text || ' MB'
         from svv_table_info where \"schema\" = $1 and \"table\" = $2",
    )
    .bind(&sch)
    .bind(&tbl)
    .fetch_optional(p)
    .await
    .unwrap_or(None);

    let rels: Vec<(String, String, String, String)> = sqlx::query_as(
        "select case when tc.table_schema = $1 and tc.table_name = $2 then 'out' else 'in' end,
                tc.constraint_name,
                case when tc.table_schema = $1 and tc.table_name = $2
                     then ccu.table_schema || '.' || ccu.table_name
                     else tc.table_schema || '.' || tc.table_name end,
                'FOREIGN KEY (' || kcu.column_name || ') REFERENCES ' ||
                    ccu.table_schema || '.' || ccu.table_name || ' (' || ccu.column_name || ')'
         from information_schema.table_constraints tc
         join information_schema.key_column_usage kcu
           on kcu.constraint_name = tc.constraint_name
         join information_schema.constraint_column_usage ccu
           on ccu.constraint_name = tc.constraint_name
         where tc.constraint_type = 'FOREIGN KEY'
           and ((tc.table_schema = $1 and tc.table_name = $2)
                or (ccu.table_schema = $1 and ccu.table_name = $2))
         order by 1, 2",
    )
    .bind(&sch)
    .bind(&tbl)
    .fetch_all(p)
    .await
    .unwrap_or_default();

    Ok(TableProps {
        columns: cols
            .into_iter()
            .map(|(name, data_type, nullable, default)| ColumnInfo {
                pk: pks.contains(&name),
                name,
                data_type,
                nullable,
                default,
            })
            .collect(),
        approx_rows: info.as_ref().map(|(r, _)| *r).unwrap_or(0),
        size: info.map(|(_, s)| s).unwrap_or_else(|| "-".into()),
        relations: rels
            .into_iter()
            .map(|(dir, name, other, def)| Relation {
                dir,
                name,
                other,
                def,
            })
            .collect(),
    })
}

#[derive(Serialize)]
struct Edge {
    src: String,
    src_cols: String,
    dst: String,
    dst_cols: String,
}

/// FK ทุกเส้นใน database — ใช้วาด ER diagram
#[tauri::command]
async fn er_edges(conn: String, state: tauri::State<'_, AppState>) -> R<Vec<Edge>> {
    if let Some(o) = other(&state, &conn).await {
        return o.er_edges().await;
    }
    let d = db(&state, &conn).await?;
    let p = d.pool;
    let rows: Vec<(String, String, String, String)> = if d.engine == Engine::Redshift {
        sqlx::query_as(
            "select tc.table_schema || '.' || tc.table_name,
                    kcu.column_name,
                    ccu.table_schema || '.' || ccu.table_name,
                    ccu.column_name
             from information_schema.table_constraints tc
             join information_schema.key_column_usage kcu
               on kcu.constraint_name = tc.constraint_name
             join information_schema.constraint_column_usage ccu
               on ccu.constraint_name = tc.constraint_name
             where tc.constraint_type = 'FOREIGN KEY'
               and tc.table_schema not in ('pg_catalog', 'information_schema')
             order by 1, 3",
        )
        .fetch_all(&p)
        .await
        .map_err(err)?
    } else {
        sqlx::query_as(
        "select con.conrelid::regclass::text,
                (select string_agg(a.attname, ', ' order by k.ord)
                   from unnest(con.conkey) with ordinality k(attnum, ord)
                   join pg_attribute a on a.attrelid = con.conrelid and a.attnum = k.attnum),
                con.confrelid::regclass::text,
                (select string_agg(a.attname, ', ' order by k.ord)
                   from unnest(con.confkey) with ordinality k(attnum, ord)
                   join pg_attribute a on a.attrelid = con.confrelid and a.attnum = k.attnum)
         from pg_constraint con
         join pg_class c on c.oid = con.conrelid
         join pg_namespace n on n.oid = c.relnamespace
         where con.contype = 'f' and n.nspname not in ('pg_catalog','information_schema')
         order by 1, 3",
        )
        .fetch_all(&p)
        .await
        .map_err(err)?
    };
    Ok(rows
        .into_iter()
        .map(|(src, src_cols, dst, dst_cols)| Edge {
            src,
            src_cols,
            dst,
            dst_cols,
        })
        .collect())
}

/// ชนิด SQL ตอนประกอบคำสั่ง — Postgres/Redshift/SQLite quote ชื่อด้วย " ส่วน MySQL ใช้ `
/// และถือ \ ใน string เป็น escape (ไม่ escape ซ้ำ ค่าอย่าง C:\new จะกลายเป็นขึ้นบรรทัดใหม่)
#[derive(Clone, Copy, PartialEq)]
enum Dialect {
    Std,
    My,
}

fn ident_in(d: Dialect, s: &str) -> String {
    match d {
        Dialect::My => format!("`{}`", s.replace('`', "``")),
        Dialect::Std => format!("\"{}\"", s.replace('"', "\"\"")),
    }
}

fn ident(s: &str) -> String {
    ident_in(Dialect::Std, s)
}

fn lit_in(d: Dialect, v: &Option<String>) -> String {
    match v {
        None => "NULL".into(),
        // literal ไม่ระบุ type — ให้ database cast ตาม type ของคอลัมน์เอง
        Some(s) => {
            let s = s.replace('\'', "''");
            format!("'{}'", if d == Dialect::My { s.replace('\\', "\\\\") } else { s })
        }
    }
}

fn lit(v: &Option<String>) -> String {
    lit_in(Dialect::Std, v)
}

/// ค่าจริงในคอลัมน์ ไว้ป้อน autocomplete ตอนพิมพ์ `where col = '`
// ponytail: dedupe ทั้งคอลัมน์ด้วย group by (ไม่ใช่ตัดที่ N แถวแรกแบบเดิม ซึ่งทำให้
// ค่าที่ขึ้นไม่ครบ) แล้วกันค้างด้วย statement_timeout 5 วิแทน
// ตารางใหญ่มากที่ไม่มี index ค่อยไปทำ list ล่วงหน้าตอน connect
#[tauri::command]
async fn column_values(
    table: String,
    column: String,
    prefix: String,
    conn: String,
    state: tauri::State<'_, AppState>,
) -> R<Vec<String>> {
    if !table
        .chars()
        .all(|c| c.is_alphanumeric() || "_$.\"`".contains(c))
    {
        return Err("ชื่อตารางไม่ถูกต้อง".into());
    }
    if let Some(o) = other(&state, &conn).await {
        return o.column_values(&table, &column, &prefix).await;
    }
    let d = db(&state, &conn).await?;
    let p = d.pool;
    let sql = format!(
        "select {c}::text as v from {t}
         where {c} is not null and {c}::text ilike $1
         group by 1 order by 1 limit 200",
        c = ident(&column),
        t = table
    );
    let pattern = format!("{}%", prefix.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));

    // อ่านอย่างเดียว — จบด้วย rollback เสมอ ไม่มีทางเขียนอะไรลง DB
    let mut tx = p.begin().await.map_err(err)?;
    // Redshift ไม่มี SET LOCAL statement_timeout — ข้ามไป rollback ก็ยังคุมความปลอดภัยอยู่
    if d.engine == Engine::Postgres {
        sqlx::query("set local statement_timeout = 5000")
            .execute(&mut *tx)
            .await
            .map_err(err)?;
    }
    let rows: Result<Vec<(String,)>, _> = sqlx::query_as(&sql).bind(&pattern).fetch_all(&mut *tx).await;
    tx.rollback().await.ok();
    Ok(rows.map_err(err)?.into_iter().map(|(v,)| v).collect())
}

#[derive(Deserialize)]
struct KeyVal {
    column: String,
    value: Option<String>,
}

fn where_keys_in(d: Dialect, keys: &[KeyVal]) -> String {
    keys.iter()
        .map(|k| match &k.value {
            None => format!("{} is null", ident_in(d, &k.column)),
            v => format!("{} = {}", ident_in(d, &k.column), lit_in(d, v)),
        })
        .collect::<Vec<_>>()
        .join(" and ")
}

fn where_keys(keys: &[KeyVal]) -> String {
    where_keys_in(Dialect::Std, keys)
}

/// เพิ่มแถวใหม่ — ส่งมาเฉพาะคอลัมน์ที่ผู้ใช้กรอกจริง
/// คอลัมน์ที่ไม่ได้ส่งมาจะได้ DEFAULT ของตาราง (serial/uuid/now() จึงทำงานตามปกติ)
/// คืนทั้งแถวที่ DB บันทึกจริง (id/uuid ที่ DB สร้างให้) — Redshift ไม่มี RETURNING จึงคืน None
#[tauri::command]
async fn insert_row(
    table: String,
    values: Vec<KeyVal>,
    conn: String,
    state: tauri::State<'_, AppState>,
) -> R<Option<serde_json::Value>> {
    if let Some(o) = other(&state, &conn).await {
        return o.insert_one(&table, &values).await;
    }
    insert_one(&db(&state, &conn).await?, &table, &values).await
}

fn insert_sql_in(d: Dialect, table: &str, values: &[KeyVal]) -> String {
    if values.is_empty() {
        // MySQL ไม่มี DEFAULT VALUES
        if d == Dialect::My {
            format!("insert into {} () values ()", table)
        } else {
            format!("insert into {} default values", table)
        }
    } else {
        format!(
            "insert into {} ({}) values ({})",
            table,
            values.iter().map(|v| ident_in(d, &v.column)).collect::<Vec<_>>().join(", "),
            values.iter().map(|v| lit_in(d, &v.value)).collect::<Vec<_>>().join(", ")
        )
    }
}

fn insert_sql(table: &str, values: &[KeyVal]) -> String {
    insert_sql_in(Dialect::Std, table, values)
}

async fn insert_one(d: &Db, table: &str, values: &[KeyVal]) -> R<Option<serde_json::Value>> {
    let sql = insert_sql(table, values);
    // คำสั่งเดียว autocommit ในตัว — พังก็ไม่มีอะไรถูกเขียน
    if d.engine == Engine::Postgres {
        let r = d.pool.fetch_one(format!("{} returning *", sql).as_str()).await.map_err(err)?;
        Ok(Some(row_json(&r)?))
    } else {
        sqlx::query(&sql).execute(&d.pool).await.map_err(err)?;
        Ok(None)
    }
}

/// ใส่หลายแถวใน transaction เดียว — ย้อน DELETE จาก editor และวางหลายแถวจาก Excel
/// แถวไหนใส่ไม่ได้ (เช่น key ซ้ำ) ยกเลิกทั้งหมด
/// Postgres คืนแถวที่บันทึกจริงทุกแถว (id ที่ DB สร้าง) — Redshift คืนว่าง
#[tauri::command]
async fn insert_rows(
    table: String,
    rows: Vec<Vec<KeyVal>>,
    conn: String,
    state: tauri::State<'_, AppState>,
) -> R<Vec<serde_json::Value>> {
    if let Some(o) = other(&state, &conn).await {
        return o.insert_many(&table, &rows).await;
    }
    insert_many(&db(&state, &conn).await?, &table, &rows).await
}

async fn insert_many(d: &Db, table: &str, rows: &[Vec<KeyVal>]) -> R<Vec<serde_json::Value>> {
    let pg = d.engine == Engine::Postgres;
    let sqls: Vec<String> = rows
        .iter()
        .map(|r| insert_sql(table, r) + if pg { " returning *" } else { "" })
        .collect();
    let mut tx = d.pool.begin().await.map_err(err)?;
    let mut saved = Vec::new();
    for sql in &sqls {
        if pg {
            let r = (&mut *tx).fetch_one(sql.as_str()).await.map_err(err)?;
            saved.push(row_json(&r)?);
        } else {
            (&mut *tx).execute(sql.as_str()).await.map_err(err)?;
        }
    }
    tx.commit().await.map_err(err)?;
    Ok(saved)
}

/// ลบหลายแถวผ่าน key ใน transaction เดียว (ย้อนแถวที่วางเพิ่มจาก Excel)
/// ทุกชุด key ต้องโดนพอดี 1 แถว ไม่งั้นยกเลิกทั้งหมด
#[tauri::command]
async fn delete_rows(
    table: String,
    keys: Vec<Vec<KeyVal>>,
    conn: String,
    state: tauri::State<'_, AppState>,
) -> R<u64> {
    let o = other(&state, &conn).await;
    let dl = o.as_ref().map_or(Dialect::Std, |o| o.dialect());
    let sqls: Vec<String> = keys
        .iter()
        .map(|k| format!("delete from {} where {}", table, where_keys_in(dl, k)))
        .collect();
    match o {
        Some(o) => o.exact_one_each(&sqls, "ลบ").await,
        None => exact_one_each(&db(&state, &conn).await?, &sqls, "ลบ").await,
    }
}

#[derive(Deserialize)]
struct CellChange {
    keys: Vec<KeyVal>,
    column: String,
    value: Option<String>,
}

/// แก้หลาย cell ใน transaction เดียว (วางทับจาก Excel) — ทุก cell ต้องโดนพอดี 1 แถว
#[tauri::command]
async fn update_cells(
    table: String,
    changes: Vec<CellChange>,
    conn: String,
    state: tauri::State<'_, AppState>,
) -> R<u64> {
    let o = other(&state, &conn).await;
    let dl = o.as_ref().map_or(Dialect::Std, |o| o.dialect());
    let sqls: Vec<String> = changes
        .iter()
        .map(|c| {
            format!(
                "update {} set {} = {} where {}",
                table,
                ident_in(dl, &c.column),
                lit_in(dl, &c.value),
                where_keys_in(dl, &c.keys)
            )
        })
        .collect();
    match o {
        Some(o) => o.exact_one_each(&sqls, "แก้").await,
        None => exact_one_each(&db(&state, &conn).await?, &sqls, "แก้").await,
    }
}

async fn exact_one_each(d: &Db, sqls: &[String], verb: &str) -> R<u64> {
    if sqls.is_empty() {
        return Ok(0);
    }
    let mut tx = d.pool.begin().await.map_err(err)?;
    for (i, sql) in sqls.iter().enumerate() {
        let n = (&mut *tx).execute(sql.as_str()).await.map_err(err)?.rows_affected();
        if n != 1 {
            tx.rollback().await.ok();
            return Err(format!("รายการที่ {} ตรง {} แถว (ต้องเป็น 1) — ยกเลิกการ{}ทั้งหมด", i + 1, n, verb));
        }
    }
    tx.commit().await.map_err(err)?;
    Ok(sqls.len() as u64)
}

/// ลบแถวผ่าน primary key — เงื่อนไขต้องตรงพอดี 1 แถว ไม่งั้น rollback
/// คืนค่าทั้งแถวที่ลบไป (ทุกคอลัมน์ในตาราง ไม่ใช่เฉพาะที่ query บนจอ select มา)
/// ให้ฝั่ง UI เก็บลงประวัติไว้ insert กลับได้ครบ
#[tauri::command]
async fn delete_row(
    table: String,
    keys: Vec<KeyVal>,
    conn: String,
    state: tauri::State<'_, AppState>,
) -> R<Option<serde_json::Value>> {
    if keys.is_empty() {
        return Err("ตารางนี้ไม่มี primary key จึงลบแถวตรง ๆ ไม่ได้".into());
    }
    if let Some(o) = other(&state, &conn).await {
        return o.delete_row(&table, &keys).await;
    }
    let p = pool(&state, &conn).await?;
    let where_sql = where_keys(&keys);
    let sql = format!("delete from {} where {}", table, where_sql);

    let mut tx = p.begin().await.map_err(err)?;
    // อ่านทั้งแถวไว้ก่อน ใน transaction เดียวกับที่ลบ — ไม่มีช่องว่างให้แถวเปลี่ยน
    // ระหว่างอ่านกับลบ และไม่ใช้ DELETE ... RETURNING เพราะ Redshift ไม่รองรับ
    let before = match (&mut *tx)
        .fetch_optional(format!("select * from {} where {}", table, where_sql).as_str())
        .await
    {
        Ok(Some(r)) => Some(row_json(&r)?),
        // อ่านไม่ได้ก็ยังลบต่อ แค่ประวัติจะเก็บได้ไม่ครบ (ฝั่ง UI ถอยไปใช้ค่าบนจอแทน)
        _ => None,
    };
    let n = sqlx::query(&sql)
        .execute(&mut *tx)
        .await
        .map_err(err)?
        .rows_affected();
    if n != 1 {
        tx.rollback().await.ok();
        return Err(format!("เงื่อนไขตรง {} แถว (ต้องเป็น 1) — ยกเลิกการลบ", n));
    }
    tx.commit().await.map_err(err)?;
    Ok(before)
}

/// แก้ค่า cell เดียวผ่าน primary key — รันใน transaction แล้วยืนยันว่าโดนแค่ 1 แถว
/// ถ้าไม่ใช่ 1 แถว rollback ทิ้งทันที (กันเคส pk ซ้ำ/แถวหาย แล้วเขียนทับข้อมูลคนอื่น)
#[tauri::command]
async fn update_cell(
    table: String,
    column: String,
    value: Option<String>,
    keys: Vec<KeyVal>,
    conn: String,
    state: tauri::State<'_, AppState>,
) -> R<u64> {
    if keys.is_empty() {
        return Err("ตารางนี้ไม่มี primary key จึงแก้ค่าตรง ๆ ไม่ได้".into());
    }
    if let Some(o) = other(&state, &conn).await {
        return o.update_cell(&table, &column, &value, &keys).await;
    }
    let p = pool(&state, &conn).await?;
    let where_sql = where_keys(&keys);
    let sql = format!(
        "update {} set {} = {} where {}",
        table,
        ident(&column),
        lit(&value),
        where_sql
    );

    let mut tx = p.begin().await.map_err(err)?;
    let n = sqlx::query(&sql)
        .execute(&mut *tx)
        .await
        .map_err(err)?
        .rows_affected();
    if n != 1 {
        tx.rollback().await.ok();
        return Err(format!("เงื่อนไขตรง {} แถว (ต้องเป็น 1) — ยกเลิกการแก้ไข", n));
    }
    tx.commit().await.map_err(err)?;
    Ok(n)
}

/// ห่อเป็น subquery เพื่อตัดจำนวนแถวฝั่ง server ได้ไหม
/// (show / explain / คำสั่งเขียน ห่อไม่ได้ ต้องส่งดิบ)
/// คำสั่งระดับตารางที่ทำลายข้อมูล — รับได้เฉพาะ op ในชุดนี้เท่านั้น ไม่รับ SQL อิสระ
/// ชื่อตารางมาจาก list_tables ที่ quote มาแล้ว ฝั่ง UI บังคับพิมพ์ชื่อยืนยันอีกชั้น
#[tauri::command]
async fn table_op(op: String, table: String, conn: String,
    state: tauri::State<'_, AppState>) -> R<u64> {
    if let Some(o) = other(&state, &conn).await {
        return o.table_op(&op, &table).await;
    }
    let sql = op_sql(&op, &table)?;
    let p = pool(&state, &conn).await?;
    Ok(p.execute(sql.as_str()).await.map_err(err)?.rows_affected())
}

fn op_sql(op: &str, table: &str) -> R<String> {
    Ok(match op {
        "truncate" => format!("TRUNCATE TABLE {}", table),
        "drop" => format!("DROP TABLE {}", table),
        "drop_cascade" => format!("DROP TABLE {} CASCADE", table),
        "drop_view" => format!("DROP VIEW {}", table),
        _ => return Err(format!("ไม่รู้จักคำสั่ง {}", op)),
    })
}

fn wrappable(sql: &str) -> bool {
    // หลายคำสั่ง (มี ; ข้างใน) ห่อรวมเป็น subquery เดียวไม่ได้ — รันดิบแล้วแยกผลทีละคำสั่ง
    // ; ที่อยู่ในข้อความก็แค่ไม่ห่อ (ไม่ตัดแถวฝั่ง server) ผลยังถูก
    if sql.contains(';') {
        return false;
    }
    let head = sql
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("--"))
        .unwrap_or("")
        .to_ascii_lowercase();
    if head.starts_with("with") {
        // ห่อเป็น subquery ไม่ได้: Redshift ไม่รับ recursive CTE ใน subquery และ Postgres
        // บังคับให้ CTE ที่มี insert/update/delete อยู่ชั้นนอกสุด — รันดิบแทน (ไม่ตัดแถวฝั่ง server)
        // เจอคำพวกนี้ในชื่อคอลัมน์/ข้อความก็แค่ไม่ห่อ ผลลัพธ์ยังถูก
        let lower = sql.to_ascii_lowercase();
        return !lower
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .any(|w| matches!(w, "recursive" | "insert" | "update" | "delete" | "merge"));
    }
    ["select", "table ", "values"]
        .iter()
        .any(|k| head.starts_with(k))
}

// ponytail: อ่านค่าจาก simple query protocol ซึ่งส่งทุก type มาเป็น text อยู่แล้ว
// จึงไม่ต้อง decode ตาม type และไม่ต้องพึ่ง json_agg (ที่ Redshift ไม่มี)
// map เป็น number/bool เฉพาะ type พื้นฐานพอให้ตารางเรียงลำดับถูก ที่เหลือเป็น string
fn cell_json(row: &sqlx::postgres::PgRow, i: usize) -> R<serde_json::Value> {
    let v = row.try_get_raw(i).map_err(err)?;
    if v.is_null() {
        return Ok(serde_json::Value::Null);
    }
    // ผ่าน sqlx::query (extended protocol) ค่าจะมาเป็น binary — อ่านเป็น text ได้ขยะ ต้องไม่เงียบ
    if v.format() != sqlx::postgres::PgValueFormat::Text {
        return Err("อ่านค่าแบบ binary ไม่ได้ — ต้อง query ผ่าน simple protocol (&str)".into());
    }
    let ty = v.type_info().name().to_ascii_uppercase();
    let text = v.as_str().map_err(err)?.to_string();
    Ok(match ty.as_str() {
        "INT2" | "INT4" | "INT8" | "FLOAT4" | "FLOAT8" | "NUMERIC" => {
            serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text))
        }
        "BOOL" => serde_json::Value::Bool(text == "t" || text == "true"),
        _ => serde_json::Value::String(text),
    })
}

/// ทั้งแถวเป็น JSON object เรียงตามลำดับคอลัมน์จริงในตาราง
fn row_json(row: &sqlx::postgres::PgRow) -> R<serde_json::Value> {
    let mut cols: Vec<String> = row.columns().iter().map(|c| c.name().to_string()).collect();
    dedupe(&mut cols);
    let mut obj = serde_json::Map::with_capacity(cols.len());
    for (i, c) in cols.iter().enumerate() {
        obj.insert(c.clone(), cell_json(row, i)?);
    }
    Ok(serde_json::Value::Object(obj))
}

#[tauri::command]
async fn run_query(sql: String, conn: String,
    state: tauri::State<'_, AppState>) -> R<QueryResult> {
    if let Some(o) = other(&state, &conn).await {
        return o.run_query(&sql).await;
    }

    let p = pool(&state, &conn).await?;
    let t0 = Instant::now();
    let trimmed = sql.trim().trim_end_matches(';').trim().to_string();
    if trimmed.is_empty() {
        return Err("ไม่มี SQL ให้รัน".into());
    }
    if let Some((table, select)) = delete_select(&trimmed) {
        return run_delete(&p, &trimmed, table, &select).await;
    }

    // ดึงเกินมา 1 แถวเพื่อรู้ว่ามีต่ออีกไหม แล้วค่อยตัดทิ้ง
    let capped = wrappable(&trimmed);
    let to_run = if capped {
        format!("select * from ({}) _mdb limit {}", trimmed, MAX_ROWS + 1)
    } else {
        trimmed.clone()
    };

    let mut sets: Vec<ResultSet> = result_sets!(sqlx::raw_sql(&to_run).fetch_many(&p), cell_json);

    // ผลลัพธ์ว่างยังอยากรู้ชื่อคอลัมน์ — ถามจาก describe (พังก็ปล่อยผ่าน)
    if capped && sets.len() <= 1 && sets.first().map_or(true, |s| s.columns.is_empty()) {
        if let Ok(desc) = p.describe(&trimmed).await {
            let mut columns: Vec<String> = desc.columns().iter().map(|c| c.name().to_string()).collect();
            if !columns.is_empty() {
                dedupe(&mut columns);
                sets = vec![ResultSet { columns, ..Default::default() }];
            }
        }
    }
    Ok(into_result(sets, t0))
}

/// แผนการรัน query — format บอกฝั่ง UI ว่าวาดแบบไหน
/// pg = JSON ของ Postgres, sqlite = แถวของ EXPLAIN QUERY PLAN เป็น JSON, text = ข้อความดิบ
#[derive(Serialize)]
struct Explained {
    format: &'static str,
    text: String,
}

/// analyze = รันจริงเพื่อได้เวลา/จำนวนแถวจริง — อยู่ใน transaction แล้ว rollback เสมอ
/// (คำสั่งที่แก้ข้อมูลจึงไม่ค้าง ยกเว้นของที่ rollback ไม่ได้อย่าง sequence)
#[tauri::command]
async fn explain(sql: String, analyze: bool, conn: String,
    state: tauri::State<'_, AppState>) -> R<Explained> {
    let q = sql.trim().trim_end_matches(';').trim().to_string();
    if q.is_empty() {
        return Err("ไม่มี SQL ให้ explain".into());
    }
    if q.contains(';') {
        return Err("Explain ได้ทีละคำสั่ง — วางเคอร์เซอร์ไว้ที่คำสั่งเดียว".into());
    }
    if let Some(o) = other(&state, &conn).await {
        return o.explain(&q, analyze).await;
    }
    let d = db(&state, &conn).await?;
    let mut tx = d.pool.begin().await.map_err(err)?;
    let out = if d.engine == Engine::Redshift {
        // Redshift ไม่มี FORMAT JSON / ANALYZE — ได้เป็นข้อความทีละบรรทัด
        let rows = (&mut *tx).fetch_all(format!("explain {}", q).as_str()).await.map_err(err);
        rows.and_then(|rows| {
            Ok(Explained {
                format: "text",
                text: rows.iter().map(|r| cell_json(r, 0).map(|v| cell_text(&v))).collect::<R<Vec<_>>>()?.join("\n"),
            })
        })
    } else {
        let opts = if analyze { "format json, analyze, buffers" } else { "format json" };
        (&mut *tx)
            .fetch_one(format!("explain ({}) {}", opts, q).as_str())
            .await
            .map_err(err)
            .and_then(|r| Ok(Explained { format: "pg", text: cell_text(&cell_json(&r, 0)?) }))
    };
    tx.rollback().await.ok();
    out
}

/// session ที่ต่อ database นี้อยู่ — ทุกชนิดคืนคีย์ชุดเดียวกัน:
/// pid, usr, db, client, state, wait, secs (รันมากี่วินาที), query, blocked_by (pid ที่ขวางอยู่ คั่นด้วย ,)
const PG_SESSIONS: &str = "select a.pid, coalesce(a.usename, '') usr, coalesce(a.datname, '') db,
        coalesce(host(a.client_addr), '') || case when a.application_name <> '' then ' · ' || a.application_name else '' end client,
        coalesce(a.state, '') state, coalesce(a.wait_event_type || ': ' || a.wait_event, '') wait,
        round(extract(epoch from now() - coalesce(a.query_start, a.backend_start))::numeric, 1) secs,
        coalesce(a.query, '') query, array_to_string(pg_blocking_pids(a.pid), ',') blocked_by
 from pg_stat_activity a
 where a.backend_type = 'client backend' and a.pid <> pg_backend_pid()
 order by a.state = 'active' desc, secs desc";

/// Redshift: เห็นเฉพาะที่กำลังรัน (stv_recents) — ไม่มีข้อมูลว่าใครขวางใคร
const RS_SESSIONS: &str = "select pid, trim(user_name) usr, trim(db_name) db, '' client, trim(status) state, '' wait,
        round(duration / 1000000.0, 1) secs, trim(query) query, '' blocked_by
 from stv_recents where status = 'Running' and pid <> pg_backend_pid() order by duration desc";

async fn pg_sessions(p: &PgPool, engine: Engine) -> R<Vec<serde_json::Value>> {
    let rows = p
        .fetch_all(if engine == Engine::Redshift { RS_SESSIONS } else { PG_SESSIONS })
        .await
        .map_err(err)?;
    rows.iter().map(row_json).collect()
}

#[tauri::command]
async fn sessions(conn: String, state: tauri::State<'_, AppState>) -> R<Vec<serde_json::Value>> {
    if let Some(o) = other(&state, &conn).await {
        return o.sessions().await;
    }
    let d = db(&state, &conn).await?;
    pg_sessions(&d.pool, d.engine).await
}

/// terminate = ตัด session ทิ้งทั้งอัน (transaction ที่ค้างถูก rollback), ไม่งั้นแค่ยกเลิก query ที่รันอยู่
#[tauri::command]
async fn kill_session(conn: String, pid: i64, terminate: bool,
    state: tauri::State<'_, AppState>) -> R<bool> {
    if let Some(o) = other(&state, &conn).await {
        return o.kill(pid, terminate).await;
    }
    let d = db(&state, &conn).await?;
    if d.engine == Engine::Redshift && !terminate {
        d.pool.execute(format!("cancel {}", pid).as_str()).await.map_err(err)?;
        return Ok(true);
    }
    let f = if terminate { "pg_terminate_backend" } else { "pg_cancel_backend" };
    let (ok,): (bool,) = sqlx::query_as(&format!("select {}($1)", f))
        .bind(pid as i32)
        .fetch_one(&d.pool)
        .await
        .map_err(err)?;
    Ok(ok)
}

/// `select * from a join b` ให้ชื่อคอลัมน์ซ้ำได้ ซึ่งพังสองชั้น: key ใน JSON ของแถว
/// ทับกันจนคอลัมน์หลังกลืนค่าคอลัมน์หน้า และฝั่ง UI ที่ใช้ชื่อคอลัมน์เป็น React key
/// ก็ชนกันจน DOM เพี้ยน — เติมเลขต่อท้ายตัวที่ซ้ำตั้งแต่ต้นทางทีเดียวจบทั้งสองอย่าง
fn dedupe(columns: &mut [String]) {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for c in columns.iter_mut() {
        if seen.contains(c.as_str()) {
            // ชื่อ _1 อาจมีอยู่จริงในผลลัพธ์ เลยต้องวนหาเลขที่ยังว่างจริง ๆ
            let mut i = 1;
            while seen.contains(&format!("{}_{}", c, i)) {
                i += 1;
            }
            *c = format!("{}_{}", c, i);
        }
        seen.insert(c.clone());
    }
}

fn cell_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[tauri::command]
fn export_csv(path: String, columns: Vec<String>, rows: Vec<serde_json::Value>) -> R<usize> {
    use std::io::Write;
    let mut f = std::fs::File::create(&path).map_err(err)?;
    // BOM: Rust เขียน UTF-8 อยู่แล้ว แต่ Excel จะเดาเป็น ANSI ถ้าไม่มี BOM แล้วภาษาไทยเพี้ยน
    f.write_all(b"\xEF\xBB\xBF").map_err(err)?;
    let mut w = csv::Writer::from_writer(f);
    w.write_record(&columns).map_err(err)?;
    for row in &rows {
        let record: Vec<String> = columns
            .iter()
            .map(|c| cell_text(row.get(c).unwrap_or(&serde_json::Value::Null)))
            .collect();
        w.write_record(&record).map_err(err)?;
    }
    w.flush().map_err(err)?;
    Ok(rows.len())
}

#[tauri::command]
fn export_json(path: String, rows: Vec<serde_json::Value>) -> R<usize> {
    std::fs::write(&path, serde_json::to_string_pretty(&rows).map_err(err)?).map_err(err)?;
    Ok(rows.len())
}

/// ค่าหนึ่ง cell ลง Excel: ตัวเลข/boolean เป็นชนิดจริง (Excel คำนวณต่อได้) ที่เหลือเป็นข้อความ
/// เลขเต็มที่เกิน 2^53 เก็บเป็นข้อความ — f64 ของ Excel เก็บได้ไม่ครบหลัก (id แบบ bigint จะเพี้ยน)
fn xlsx_cell(ws: &mut rust_xlsxwriter::Worksheet, r: u32, c: u16, v: &serde_json::Value) -> R<()> {
    use serde_json::Value;
    const SAFE: f64 = 9_007_199_254_740_991.0;
    match v {
        Value::Null => {}
        Value::Bool(b) => {
            ws.write_boolean(r, c, *b).map_err(err)?;
        }
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.is_finite() && (n.is_f64() || f.abs() <= SAFE) => {
                ws.write_number(r, c, f).map_err(err)?;
            }
            _ => {
                ws.write_string(r, c, n.to_string()).map_err(err)?;
            }
        },
        other => {
            // Excel รับข้อความยาวสุด 32,767 ตัวอักษรต่อ cell — ยาวกว่านั้นตัดทิ้ง ไม่งั้นทั้งไฟล์ export ไม่ได้
            let t: String = cell_text(other).chars().take(32_767).collect();
            ws.write_string(r, c, t).map_err(err)?;
        }
    }
    Ok(())
}

#[tauri::command]
fn export_xlsx(path: String, columns: Vec<String>, rows: Vec<serde_json::Value>) -> R<usize> {
    let mut wb = rust_xlsxwriter::Workbook::new();
    let ws = wb.add_worksheet();
    let bold = rust_xlsxwriter::Format::new().set_bold();
    for (c, name) in columns.iter().enumerate() {
        ws.write_string_with_format(0, c as u16, name, &bold).map_err(err)?;
    }
    for (r, row) in rows.iter().enumerate() {
        for (c, name) in columns.iter().enumerate() {
            xlsx_cell(ws, r as u32 + 1, c as u16, row.get(name).unwrap_or(&serde_json::Value::Null))?;
        }
    }
    ws.set_freeze_panes(1, 0).map_err(err)?;
    ws.autofit();
    wb.save(&path).map_err(err)?;
    Ok(rows.len())
}

/// อ่านไฟล์ข้อความทั้งไฟล์ — ใช้ตอน import รายการ connection
#[tauri::command]
fn read_text(path: String) -> R<String> {
    std::fs::read_to_string(&path).map_err(err)
}

/// password ของ connection เก็บใน Windows Credential Manager (ผูกกับ Windows user ของเครื่องนี้)
/// ไม่เก็บใน localStorage ที่ใครเปิดไฟล์ profile ของ WebView ก็อ่านได้
fn secret(id: &str) -> R<keyring::Entry> {
    keyring::Entry::new("markdb", id).map_err(err)
}

#[tauri::command]
fn secret_get(id: String) -> R<Option<String>> {
    match secret(&id)?.get_password() {
        Ok(p) => Ok(Some(p)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(err(e)),
    }
}

/// pass ว่าง = ลบทิ้ง (connection ที่ไม่มี password หรือถูกลบไปแล้ว)
#[tauri::command]
fn secret_set(id: String, pass: String) -> R<()> {
    let e = secret(&id)?;
    if pass.is_empty() {
        match e.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(x) => Err(err(x)),
        }
    } else {
        e.set_password(&pass).map_err(err)
    }
}

fn sql_literal(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "NULL".into(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        other => format!("'{}'", cell_text(other).replace('\'', "''")),
    }
}

#[tauri::command]
fn export_sql(
    path: String,
    table: String,
    columns: Vec<String>,
    rows: Vec<serde_json::Value>,
) -> R<usize> {
    let cols = columns
        .iter()
        .map(|c| format!("\"{}\"", c.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(", ");
    let mut out = String::new();
    for row in &rows {
        let vals = columns
            .iter()
            .map(|c| sql_literal(row.get(c).unwrap_or(&serde_json::Value::Null)))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "INSERT INTO {} ({}) VALUES ({});\n",
            table, cols, vals
        ));
    }
    std::fs::write(&path, out).map_err(err)?;
    Ok(rows.len())
}

// ponytail: import ผ่าน COPY ... FROM STDIN ของ Postgres เอง — cast type ให้เอง
// และเร็วกว่า INSERT ทีละแถวหลายสิบเท่า โดยไม่ต้อง parse CSV ฝั่ง Rust
// รองรับ header ธรรมดา ไม่รองรับ comma ในชื่อคอลัมน์ — เจอค่อยใช้ csv crate อ่าน header
#[tauri::command]
async fn import_csv(path: String, table: String, conn: String,
    state: tauri::State<'_, AppState>) -> R<u64> {
    let d = db(&state, &conn).await?;
    if d.engine == Engine::Redshift {
        return import_csv_insert(&d.pool, &path, &table).await;
    }
    let p = d.pool;
    let bytes = std::fs::read(&path).map_err(err)?;
    let header = String::from_utf8_lossy(&bytes)
        .lines()
        .next()
        .ok_or("ไฟล์ CSV ว่าง")?
        .to_string();
    let cols = header
        .split(',')
        .map(|c| format!("\"{}\"", c.trim().trim_matches('"').replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(", ");

    let mut copy = p
        .copy_in_raw(&format!(
            "COPY {} ({}) FROM STDIN WITH (FORMAT csv, HEADER true)",
            table, cols
        ))
        .await
        .map_err(err)?;
    copy.send(bytes.as_slice()).await.map_err(err)?;
    copy.finish().await.map_err(err)
}

/// หัวไฟล์ Excel ติด BOM มากับคอลัมน์แรก — ไม่ตัดทิ้งจะจับคู่ชื่อคอลัมน์ไม่เจอ
fn csv_headers(rdr: &mut csv::Reader<std::fs::File>) -> R<Vec<String>> {
    Ok(rdr
        .headers()
        .map_err(err)?
        .iter()
        .map(|h| h.trim_start_matches('\u{feff}').trim().to_string())
        .collect())
}

/// ช่องว่างหรือคำว่า NULL (แบบที่ SQL Server/Excel export มา) = NULL
fn csv_val(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("null") {
        None
    } else {
        Some(s.to_string())
    }
}

#[derive(Serialize)]
struct CsvHead {
    columns: Vec<String>,
    sample: Vec<Vec<String>>,
    rows: usize,
}

/// หัวคอลัมน์ + ตัวอย่าง 5 แถวแรก + จำนวนแถว ไว้ให้ UI เลือกคอลัมน์และ preview
#[tauri::command]
fn csv_head(path: String) -> R<CsvHead> {
    let mut rdr = csv::Reader::from_path(&path).map_err(err)?;
    let columns = csv_headers(&mut rdr)?;
    let (mut sample, mut rows) = (Vec::new(), 0);
    for rec in rdr.records() {
        let rec = rec.map_err(err)?;
        if sample.len() < 5 {
            sample.push(rec.iter().map(String::from).collect());
        }
        rows += 1;
    }
    Ok(CsvHead { columns, sample, rows })
}

/// อ่าน CSV แล้วประกอบ UPDATE ทีละบรรทัด คืน (บรรทัดในไฟล์, sql, ค่า key ไว้รายงาน)
/// ค่าส่งเป็น literal ไม่ระบุ type ให้ database cast เอง ('1'/'0' เข้าคอลัมน์ boolean ได้ตรง ๆ)
fn csv_updates(
    path: &str,
    table: &str,
    keys: &[String],
    cols: &[String],
) -> R<Vec<(usize, String, String)>> {
    if keys.is_empty() || cols.is_empty() {
        return Err("ต้องเลือกคอลัมน์ key และคอลัมน์ที่จะแก้ อย่างน้อยอย่างละ 1".into());
    }
    let mut rdr = csv::Reader::from_path(path).map_err(err)?;
    let headers = csv_headers(&mut rdr)?;
    let idx = |c: &String| {
        headers
            .iter()
            .position(|h| h == c)
            .ok_or_else(|| format!("ไม่มีคอลัมน์ {} ในไฟล์ CSV", c))
    };
    let ki = keys.iter().map(idx).collect::<R<Vec<_>>>()?;
    let ci = cols.iter().map(idx).collect::<R<Vec<_>>>()?;

    let mut out = Vec::new();
    for (i, rec) in rdr.records().enumerate() {
        let line = i + 2; // นับหัวตารางเป็นบรรทัด 1
        let rec = rec.map_err(|e| format!("บรรทัด {}: {}", line, e))?;
        let get = |j: usize| csv_val(rec.get(j).unwrap_or(""));
        let kv: Vec<KeyVal> = keys
            .iter()
            .zip(&ki)
            .map(|(c, &j)| KeyVal { column: c.clone(), value: get(j) })
            .collect();
        let set = cols
            .iter()
            .zip(&ci)
            .map(|(c, &j)| format!("{} = {}", ident(c), lit(&get(j))))
            .collect::<Vec<_>>()
            .join(", ");
        let label = kv
            .iter()
            .map(|k| k.value.clone().unwrap_or_else(|| "NULL".into()))
            .collect::<Vec<_>>()
            .join(", ");
        out.push((line, format!("update {} set {} where {}", table, set, where_keys(&kv)), label));
    }
    Ok(out)
}

#[derive(Serialize)]
struct CsvUpdate {
    updated: u64,
    missing: Vec<String>,
}

/// แก้แถวที่มีอยู่แล้วจาก CSV — จับคู่แถวด้วยคอลัมน์ key แล้วแก้เฉพาะคอลัมน์ใน cols
/// ทั้งไฟล์อยู่ใน transaction เดียว — แถวไหน error หรือ key ตรงเกิน 1 แถว ยกเลิกทั้งหมด
/// (กันเลือก key ผิด เช่นคอลัมน์ที่ค่าซ้ำกัน แล้วทุกแถวโดนเขียนทับด้วยค่าแถวสุดท้าย)
// ponytail: UPDATE ทีละแถว หลักหมื่นแถวไหว ถ้าหลักแสนให้ COPY เข้า temp table แล้ว UPDATE ... FROM
#[tauri::command]
async fn update_csv(
    path: String,
    table: String,
    keys: Vec<String>,
    cols: Vec<String>,
    conn: String,
    state: tauri::State<'_, AppState>,
) -> R<CsvUpdate> {
    let stmts = csv_updates(&path, &table, &keys, &cols)?;
    let p = pool(&state, &conn).await?;
    // error แล้ว return ออกไปก่อน commit — sqlx rollback ให้เองตอน tx ถูก drop
    let mut tx = p.begin().await.map_err(err)?;
    let mut out = CsvUpdate { updated: 0, missing: Vec::new() };
    for (line, sql, label) in stmts {
        let n = (&mut *tx)
            .execute(sql.as_str())
            .await
            .map_err(|e| format!("บรรทัด {}: {}", line, e))?
            .rows_affected();
        match n {
            0 => out.missing.push(label),
            1 => out.updated += 1,
            _ => {
                return Err(format!(
                    "บรรทัด {} (key {}) ตรงกับ {} แถวในตาราง — key นี้ไม่ unique จึงยกเลิกทั้งหมด ไม่มีอะไรถูกแก้",
                    line, label, n
                ))
            }
        }
    }
    tx.commit().await.map_err(err)?;
    Ok(out)
}

/// Redshift ไม่รับ COPY FROM STDIN (COPY ของมันอ่านจาก S3 เท่านั้น) — ใช้ INSERT
/// ทีละก้อนแทน ช้ากว่ามากแต่ใช้ได้กับไฟล์ระดับหลักหมื่นแถวที่คนกดผ่าน UI จริง ๆ
// ponytail: batch 500 แถว ไฟล์ระดับล้านแถวควรไปทาง COPY จาก S3 แทน
async fn import_csv_insert(p: &PgPool, path: &str, table: &str) -> R<u64> {
    const BATCH: usize = 500;
    let mut rdr = csv::Reader::from_path(path).map_err(err)?;
    let headers = rdr.headers().map_err(err)?.clone();
    let cols = headers
        .iter()
        .map(|c| ident(c.trim()))
        .collect::<Vec<_>>()
        .join(", ");

    let mut done: u64 = 0;
    let mut batch: Vec<String> = Vec::with_capacity(BATCH);
    let flush = |batch: &mut Vec<String>| -> Option<String> {
        if batch.is_empty() {
            return None;
        }
        let sql = format!(
            "insert into {} ({}) values {}",
            table,
            cols,
            batch.join(", ")
        );
        batch.clear();
        Some(sql)
    };

    for rec in rdr.records() {
        let rec = rec.map_err(err)?;
        batch.push(format!(
            "({})",
            rec.iter()
                .map(|v| if v.is_empty() {
                    "NULL".to_string()
                } else {
                    lit(&Some(v.to_string()))
                })
                .collect::<Vec<_>>()
                .join(", ")
        ));
        if batch.len() == BATCH {
            if let Some(sql) = flush(&mut batch) {
                done += sqlx::raw_sql(&sql)
                    .execute(p)
                    .await
                    .map_err(err)?
                    .rows_affected();
            }
        }
    }
    if let Some(sql) = flush(&mut batch) {
        done += sqlx::raw_sql(&sql)
            .execute(p)
            .await
            .map_err(err)?
            .rows_affected();
    }
    Ok(done)
}

// ponytail: เครื่องนี้ไม่มี pg_dump จึง generate dump เอง — DDL อ่านจาก pg_catalog
// (format_type / pg_get_constraintdef / pg_get_viewdef ให้ Postgres ประกอบให้แทนที่จะเดาเอง)
// ส่วน data stream ทีละแถวด้วย row_to_json แล้วเขียนลงไฟล์เลย ไม่กองใน memory
// ไม่ครอบคลุม: trigger, function, extension, grant, partition — ถ้าต้องใช้ให้ลง pg_dump แทน
#[tauri::command]
async fn backup_database(path: String, conn: String,
    state: tauri::State<'_, AppState>) -> R<String> {
    use futures_util::TryStreamExt;
    use std::io::Write;

    const BATCH: usize = 100;
    let d = db(&state, &conn).await?;
    if d.engine == Engine::Redshift {
        return Err("Redshift ยังไม่รองรับ backup ทั้ง database                     (ไม่มี pg_get_constraintdef / pg_indexes) — ใช้ UNLOAD ไป S3 แทน"
            .into());
    }
    let p = d.pool;
    let f = std::fs::File::create(&path).map_err(err)?;
    let mut out = std::io::BufWriter::new(f);

    macro_rules! w {
        ($($t:tt)*) => { writeln!(out, $($t)*).map_err(err)? };
    }

    let db: String = sqlx::query_scalar("select current_database()")
        .fetch_one(&p)
        .await
        .map_err(err)?;
    w!("-- MarkDB backup of database \"{}\"", db);
    w!("SET client_encoding = 'UTF8';");
    w!("SET standard_conforming_strings = on;\n");

    // schemas
    let schemas: Vec<(String,)> = sqlx::query_as(
        "select nspname from pg_namespace
         where nspname not in ('pg_catalog','information_schema','pg_toast')
           and nspname not like 'pg_temp%' and nspname not like 'pg_toast_temp%'
         order by 1",
    )
    .fetch_all(&p)
    .await
    .map_err(err)?;
    for (s,) in &schemas {
        if s != "public" {
            w!("CREATE SCHEMA IF NOT EXISTS {};", ident(s));
        }
    }

    // sequences
    let seqs: Vec<(String, String)> = sqlx::query_as(
        "select sequence_schema, sequence_name from information_schema.sequences order by 1,2",
    )
    .fetch_all(&p)
    .await
    .map_err(err)?;
    if !seqs.is_empty() {
        w!("");
    }
    for (s, n) in &seqs {
        w!("CREATE SEQUENCE IF NOT EXISTS {}.{};", ident(s), ident(n));
    }

    // columns ของทุกตาราง เรียงตาม attnum แล้วค่อยจับกลุ่มฝั่ง Rust
    let cols: Vec<(String, String, String, String, bool, String)> = sqlx::query_as(
        "select n.nspname, c.relname, a.attname, format_type(a.atttypid, a.atttypmod),
                a.attnotnull, coalesce(pg_get_expr(d.adbin, d.adrelid), '')
         from pg_class c
         join pg_namespace n on n.oid = c.relnamespace
         join pg_attribute a on a.attrelid = c.oid and a.attnum > 0 and not a.attisdropped
         left join pg_attrdef d on d.adrelid = c.oid and d.adnum = a.attnum
         where c.relkind = 'r' and n.nspname not in ('pg_catalog','information_schema')
         order by n.nspname, c.relname, a.attnum",
    )
    .fetch_all(&p)
    .await
    .map_err(err)?;

    let mut tables: Vec<(String, String, Vec<(String, String)>)> = Vec::new();
    for (s, t, name, ty, notnull, default) in cols {
        let mut def = format!("  {} {}", ident(&name), ty);
        if !default.is_empty() {
            def.push_str(&format!(" DEFAULT {}", default));
        }
        if notnull {
            def.push_str(" NOT NULL");
        }
        match tables.last_mut() {
            Some((ls, lt, defs)) if *ls == s && *lt == t => defs.push((name, def)),
            _ => tables.push((s, t, vec![(name, def)])),
        }
    }

    let mut total_rows: u64 = 0;
    for (s, t, defs) in &tables {
        let qn = format!("{}.{}", ident(s), ident(t));
        w!(
            "\nCREATE TABLE IF NOT EXISTS {} (\n{}\n);",
            qn,
            defs.iter().map(|(_, d)| d.as_str()).collect::<Vec<_>>().join(",\n")
        );

        let names: Vec<&String> = defs.iter().map(|(n, _)| n).collect();
        let col_list = names.iter().map(|c| ident(c)).collect::<Vec<_>>().join(", ");
        // cast ทุกคอลัมน์เป็น text ก่อน — literal ที่ได้จะ cast กลับเป็น type เดิมตอน INSERT
        // ครอบคลุม array/range/enum ที่ row_to_json ตรง ๆ จะให้รูปแบบที่ INSERT กลับไม่ได้
        let as_text = names
            .iter()
            .map(|c| format!("{i}::text as {i}", i = ident(c)))
            .collect::<Vec<_>>()
            .join(", ");

        let mut batch: Vec<String> = Vec::with_capacity(BATCH);
        let dump_sql = format!(
            "select row_to_json(_t)::text from (select {} from {}) _t",
            as_text, qn
        );
        let mut stream = sqlx::query_scalar::<_, String>(&dump_sql).fetch(&p);
        while let Some(json) = stream.try_next().await.map_err(err)? {
            let row: serde_json::Value = serde_json::from_str(&json).map_err(err)?;
            batch.push(format!(
                "({})",
                names
                    .iter()
                    .map(|c| sql_literal(row.get(c.as_str()).unwrap_or(&serde_json::Value::Null)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            total_rows += 1;
            if batch.len() == BATCH {
                w!("INSERT INTO {} ({}) VALUES\n{};", qn, col_list, batch.join(",\n"));
                batch.clear();
            }
        }
        if !batch.is_empty() {
            w!("INSERT INTO {} ({}) VALUES\n{};", qn, col_list, batch.join(",\n"));
        }
    }

    // constraints ไว้ท้ายสุด ลำดับ p/u/c ก่อน f เพื่อไม่ให้ FK ชี้ไปตารางที่ยังไม่มี
    let cons: Vec<(String, String, String, String)> = sqlx::query_as(
        "select n.nspname, c.relname, con.conname, pg_get_constraintdef(con.oid)
         from pg_constraint con
         join pg_class c on c.oid = con.conrelid
         join pg_namespace n on n.oid = c.relnamespace
         where n.nspname not in ('pg_catalog','information_schema') and c.relkind = 'r'
         order by case con.contype when 'p' then 0 when 'u' then 1 when 'c' then 2 else 3 end,
                  n.nspname, c.relname, con.conname",
    )
    .fetch_all(&p)
    .await
    .map_err(err)?;
    if !cons.is_empty() {
        w!("");
    }
    for (s, t, name, def) in &cons {
        w!(
            "ALTER TABLE {}.{} ADD CONSTRAINT {} {};",
            ident(s),
            ident(t),
            ident(name),
            def
        );
    }

    // index ที่ไม่ได้มาจาก constraint
    let idx: Vec<(String,)> = sqlx::query_as(
        "select indexdef from pg_indexes
         where schemaname not in ('pg_catalog','information_schema')
           and indexname not in (select conname from pg_constraint)
         order by schemaname, tablename, indexname",
    )
    .fetch_all(&p)
    .await
    .map_err(err)?;
    if !idx.is_empty() {
        w!("");
    }
    for (def,) in &idx {
        w!("{};", def.replacen("CREATE INDEX", "CREATE INDEX IF NOT EXISTS", 1));
    }

    // views
    let views: Vec<(String, String, String)> = sqlx::query_as(
        "select n.nspname, c.relname, pg_get_viewdef(c.oid, true)
         from pg_class c join pg_namespace n on n.oid = c.relnamespace
         where c.relkind = 'v' and n.nspname not in ('pg_catalog','information_schema')
         order by 1,2",
    )
    .fetch_all(&p)
    .await
    .map_err(err)?;
    for (s, v, def) in &views {
        w!("\nCREATE OR REPLACE VIEW {}.{} AS\n{}", ident(s), ident(v), def);
    }

    // ตั้งค่า sequence ให้ตรงกับข้อมูลที่เพิ่ง insert
    if !seqs.is_empty() {
        w!("");
    }
    for (s, n) in &seqs {
        let qn = format!("{}.{}", ident(s), ident(n));
        let (last, called): (i64, bool) = sqlx::query_as(&format!(
            "select last_value, is_called from {}",
            qn
        ))
        .fetch_one(&p)
        .await
        .map_err(err)?;
        w!("SELECT setval('{}', {}, {});", qn.replace('\'', "''"), last, called);
    }

    out.flush().map_err(err)?;
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    Ok(format!(
        "{} ตาราง · {} view · {} แถว · {:.1} MB",
        tables.len(),
        views.len(),
        total_rows,
        size as f64 / 1_048_576.0
    ))
}

/// รันไฟล์ .sql ทั้งไฟล์ (ใช้ทั้ง import และ restore)
/// ทุกอย่างอยู่ใน transaction เดียว — Postgres รองรับ DDL ใน transaction จึง rollback
/// ได้ทั้งก้อนถ้าพังกลางทาง ไม่ทิ้ง database ค้างครึ่ง ๆ กลาง ๆ
#[tauri::command]
async fn import_sql(path: String, conn: String,
    state: tauri::State<'_, AppState>) -> R<u64> {
    if let Some(o) = other(&state, &conn).await {
        return o.run_script(std::fs::read_to_string(&path).map_err(err)?.trim_start_matches('\u{feff}')).await;
    }
    let p = pool(&state, &conn).await?;
    let text = std::fs::read_to_string(&path).map_err(err)?;
    // ครอบ BEGIN/COMMIT ในตัว SQL เอง: ถ้ามี statement ไหนพัง Postgres จะ abort ทั้ง
    // transaction แล้ว COMMIT กลายเป็น ROLLBACK ให้เอง — ได้ผลเท่ากับ tx ฝั่ง client
    // แต่ไม่ติดปัญหา lifetime ของ Executor ตอนส่ง &str เข้า transaction
    let wrapped = format!(
        "BEGIN;\n{}\nCOMMIT;",
        text.strip_prefix('\u{feff}').unwrap_or(&text)
    );
    let res = sqlx::raw_sql(&wrapped).execute(&p).await.map_err(err)?;
    Ok(res.rows_affected())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init());

    #[cfg(desktop)]
    {
        builder = builder
            .plugin(tauri_plugin_updater::Builder::new().build())
            .plugin(tauri_plugin_process::init());
    }

    builder
        .manage(ai::Ai::default())
        .manage(AppState {
            conns: Mutex::new(HashMap::new()),
            others: Mutex::new(HashMap::new()),
            tunnels: Mutex::new(HashMap::new()),
        })
        .invoke_handler(tauri::generate_handler![
            sessions,
            kill_session,
            explain,
            export_xlsx,
            read_text,
            secret_get,
            secret_set,
            connect,
            test_connection,
            disconnect,
            list_tables,
            list_all_columns,
            list_pk,
            er_edges,
            column_values,
            table_props,
            update_cell,
            insert_row,
            insert_rows,
            delete_rows,
            update_cells,
            delete_row,
            table_op,
            run_query,
            export_csv,
            export_sql,
            export_json,
            import_csv,
            csv_head,
            update_csv,
            sync::sync_plan,
            ai::ai_sql,
            ai::ai_cancel,
            sync::sync_apply,
            import_sql,
            backup_database
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn read_vs_write() {
        assert!(wrappable("select 1"));
        assert!(wrappable("  SELECT * FROM t"));
        assert!(wrappable("-- comment\nwith x as (select 1) select * from x"));
        assert!(!wrappable("insert into t values (1)"));
        assert!(!wrappable("create table t (id int)"));
        assert!(!wrappable("explain select 1"));
        assert!(!wrappable("WITH RECURSIVE t(n) AS (select 1 union all select n+1 from t where n < 5) select * from t"));
        assert!(!wrappable("with gone as (delete from t returning *) select * from gone"));
        assert!(wrappable("with x as (select updated_at from t) select * from x"));
    }

    #[test]
    fn deletes_become_selects() {
        let d = |s: &str| delete_select(s);
        assert_eq!(
            d("DELETE FROM \"public\".\"Meter Config\" WHERE \"Meter_ID\" = 999"),
            Some((
                "\"public\".\"Meter Config\"".into(),
                "select * from \"public\".\"Meter Config\" WHERE \"Meter_ID\" = 999".into()
            ))
        );
        assert_eq!(d("delete from t"), Some(("t".into(), "select * from t".into())));
        assert_eq!(d("delete from t x where x.id = 1").unwrap().1, "select * from t x where x.id = 1");
        assert_eq!(d("delete from t using u where t.id = u.id"), None);
        assert_eq!(d("delete from t where id = 1 returning *"), None);
        assert_eq!(d("delete from t; delete from u"), None);
        assert_eq!(d("with x as (select 1) delete from t"), None);
        assert_eq!(d("select * from t"), None);
    }

    /// MARKDB_TEST_PG=postgres://user:pass@host:port cargo test -- --ignored
    #[tokio::test]
    #[ignore]
    async fn delete_keeps_rows_for_undo() {
        let base = std::env::var("MARKDB_TEST_PG").expect("ตั้ง MARKDB_TEST_PG ก่อน");
        let p = PgPool::connect(&format!("{}/postgres", base)).await.unwrap();
        p.execute("drop table if exists markdb_del_t; create table markdb_del_t (id int primary key, note text, ok bool)")
            .await
            .unwrap();
        p.execute("insert into markdb_del_t values (1, 'a', true), (2, null, false), (3, 'o''b', null)")
            .await
            .unwrap();
        let sql = "delete from markdb_del_t where id > 1";
        let (table, select) = delete_select(sql).unwrap();
        let r = run_delete(&p, sql, table, &select).await.unwrap();
        assert_eq!(r.affected, 2);
        let d = r.deleted.unwrap();
        assert_eq!(d.table, "markdb_del_t");
        assert!(!d.partial);
        assert_eq!(d.rows.len(), 2);
        assert_eq!(d.rows[1]["note"], serde_json::json!("o'b"));
        assert_eq!(d.rows[0]["id"], serde_json::json!(2));
        assert_eq!(d.rows[0]["ok"], serde_json::json!(false));
        let left: (i64,) = sqlx::query_as("select count(*) from markdb_del_t").fetch_one(&p).await.unwrap();
        assert_eq!(left.0, 1);
        p.execute("drop table markdb_del_t").await.unwrap();
    }

    /// แถวใหม่ท้ายตาราง: ได้ค่าที่ DB สร้างเอง (serial, default) กลับมา ไม่ใช่แค่ที่พิมพ์
    #[tokio::test]
    #[ignore]
    async fn insert_returns_saved_row() {
        let base = std::env::var("MARKDB_TEST_PG").expect("ตั้ง MARKDB_TEST_PG ก่อน");
        let p = PgPool::connect(&format!("{}/postgres", base)).await.unwrap();
        p.execute("drop table if exists markdb_ins_t; create table markdb_ins_t (id serial primary key, uid uuid default gen_random_uuid(), name text not null, ok bool default true, at timestamp default now())")
            .await
            .unwrap();
        let d = Db { pool: p.clone(), engine: Engine::Postgres };
        let kv = |c: &str, v: Option<&str>| KeyVal { column: c.into(), value: v.map(|s| s.into()) };
        let r = insert_one(&d, "markdb_ins_t", &[kv("name", Some("o'b"))]).await.unwrap().unwrap();
        assert_eq!(r["id"], serde_json::json!(1));
        assert_eq!(r["name"], serde_json::json!("o'b"));
        assert_eq!(r["ok"], serde_json::json!(true));
        assert_eq!(r["uid"].as_str().unwrap().len(), 36);
        assert!(!r["at"].is_null());
        // ติด NOT NULL → error และไม่มีแถวค้าง
        assert!(insert_one(&d, "markdb_ins_t", &[kv("name", None)]).await.is_err());
        let n: (i64,) = sqlx::query_as("select count(*) from markdb_ins_t").fetch_one(&p).await.unwrap();
        assert_eq!(n.0, 1);
        p.execute("drop table markdb_ins_t").await.unwrap();
    }

    /// วางจาก Excel: insert หลายแถวได้แถวจริงคืน, แก้/ลบหลายแถวต้องโดนแถวละ 1 ไม่งั้นยกเลิกทั้งชุด
    #[tokio::test]
    #[ignore]
    async fn bulk_insert_update_delete() {
        let base = std::env::var("MARKDB_TEST_PG").expect("ตั้ง MARKDB_TEST_PG ก่อน");
        let p = PgPool::connect(&format!("{}/postgres", base)).await.unwrap();
        p.execute("drop table if exists markdb_bulk_t; create table markdb_bulk_t (id serial primary key, name text not null)")
            .await
            .unwrap();
        let d = Db { pool: p.clone(), engine: Engine::Postgres };
        let kv = |c: &str, v: Option<&str>| KeyVal { column: c.into(), value: v.map(|s| s.into()) };
        let saved = insert_many(&d, "markdb_bulk_t", &[vec![kv("name", Some("a"))], vec![kv("name", Some("b"))]])
            .await
            .unwrap();
        assert_eq!(saved.len(), 2);
        assert_eq!(saved[1]["id"], serde_json::json!(2));
        // แถวที่ 2 พัง → แถวแรกต้องไม่ถูกใส่
        assert!(insert_many(&d, "markdb_bulk_t", &[vec![kv("name", Some("c"))], vec![kv("name", None)]]).await.is_err());
        let count = || async { sqlx::query_as::<_, (i64,)>("select count(*) from markdb_bulk_t").fetch_one(&p).await.unwrap().0 };
        assert_eq!(count().await, 2);

        let upd = |id: &str, v: &str| format!("update markdb_bulk_t set name = '{}' where id = {}", v, id);
        assert_eq!(exact_one_each(&d, &[upd("1", "x"), upd("2", "y")], "แก้").await.unwrap(), 2);
        // id 99 ไม่มีจริง → rollback ทั้งชุด id 1 ต้องยังเป็น x
        assert!(exact_one_each(&d, &[upd("1", "z"), upd("99", "q")], "แก้").await.is_err());
        let n: (String,) = sqlx::query_as("select name from markdb_bulk_t where id = 1").fetch_one(&p).await.unwrap();
        assert_eq!(n.0, "x");
        p.execute("drop table markdb_bulk_t").await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn readonly_blocks_writes() {
        let base = std::env::var("MARKDB_TEST_PG").expect("ตั้ง MARKDB_TEST_PG ก่อน");
        let url = format!("{}/postgres", base);
        let rw = PgPool::connect(&url).await.unwrap();
        rw.execute("drop table if exists markdb_ro_t; create table markdb_ro_t (id int primary key)").await.unwrap();
        let ro = pool_options(true).connect(&url).await.unwrap();
        let e = ro.execute("insert into markdb_ro_t values (1)").await.unwrap_err().to_string();
        assert!(e.contains("read-only"), "{}", e);
        // ใน transaction ก็โดนเหมือนกัน (ทุกคำสั่งเขียนของแอปวิ่งผ่าน tx)
        let mut tx = ro.begin().await.unwrap();
        assert!((&mut *tx).execute("delete from markdb_ro_t").await.is_err());
        drop(tx);
        let n: (i64,) = sqlx::query_as("select count(*) from markdb_ro_t").fetch_one(&ro).await.unwrap();
        assert_eq!(n.0, 0);
        rw.execute("drop table markdb_ro_t").await.unwrap();
    }

    /// host เดียวกันคนละ database เปิดค้างพร้อมกันได้ และแต่ละ pool เห็นตารางของ database ตัวเอง
    #[tokio::test]
    #[ignore]
    async fn two_databases_same_host() {
        let base = std::env::var("MARKDB_TEST_PG").expect("ตั้ง MARKDB_TEST_PG ก่อน");
        let admin = PgPool::connect(&format!("{}/postgres", base)).await.unwrap();
        for db in ["markdb_two_a", "markdb_two_b"] {
            admin.execute(format!("drop database if exists {db} with (force)").as_str()).await.unwrap();
            admin.execute(format!("create database {db}").as_str()).await.unwrap();
        }
        let a = pool_options(false).connect(&format!("{}/markdb_two_a", base)).await.unwrap();
        let b = pool_options(false).connect(&format!("{}/markdb_two_b", base)).await.unwrap();
        a.execute("create table only_in_a (id int)").await.unwrap();
        b.execute("create table only_in_b (id int)").await.unwrap();
        let q = "select table_name::text from information_schema.tables where table_schema = 'public'";
        let ta: Vec<(String,)> = sqlx::query_as(q).fetch_all(&a).await.unwrap();
        let tb: Vec<(String,)> = sqlx::query_as(q).fetch_all(&b).await.unwrap();
        assert_eq!(ta, [("only_in_a".to_string(),)]);
        assert_eq!(tb, [("only_in_b".to_string(),)]);
        a.close().await;
        b.close().await;
        for db in ["markdb_two_a", "markdb_two_b"] {
            admin.execute(format!("drop database {db} with (force)").as_str()).await.unwrap();
        }
    }

    #[test]
    fn xlsx_keeps_types() {
        let path = std::env::temp_dir().join("markdb_test.xlsx");
        let rows = vec![serde_json::json!({"id": 9007199254740993u64, "n": 1.5, "ok": true, "name": "ไทย", "x": null})];
        let cols = ["id", "n", "ok", "name", "x"].map(String::from).to_vec();
        assert_eq!(export_xlsx(path.to_string_lossy().into(), cols, rows).unwrap(), 1);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..2], b"PK"); // xlsx = zip
        std::fs::remove_file(path).ok();
    }

    /// เขียน-อ่าน-ลบ ใน Credential Manager จริง (ignored: แตะ credential store ของเครื่อง)
    #[test]
    #[ignore]
    fn secret_roundtrip() {
        let id = "markdb-test-secret".to_string();
        secret_set(id.clone(), "p@ss ไทย".into()).unwrap();
        assert_eq!(secret_get(id.clone()).unwrap().as_deref(), Some("p@ss ไทย"));
        secret_set(id.clone(), "".into()).unwrap();
        assert_eq!(secret_get(id.clone()).unwrap(), None);
        secret_set(id, "".into()).unwrap(); // ลบซ้ำไม่ error
    }

    /// รันหลายคำสั่งใน Postgres จริง: ได้ผลแยกทีละคำสั่ง ตัวหลักคือ select ตัวสุดท้าย
    #[tokio::test]
    #[ignore]
    async fn pg_multi_result_sets() -> R<()> {
        let base = std::env::var("MARKDB_TEST_PG").expect("ตั้ง MARKDB_TEST_PG ก่อน");
        let p = PgPool::connect(&format!("{}/postgres", base)).await.unwrap();
        let sets: Vec<ResultSet> = result_sets!(
            sqlx::raw_sql("select 1 as a; create temp table mdb_x (i int); insert into mdb_x values (1), (2); select i, 'ข' t from mdb_x order by i")
                .fetch_many(&p),
            cell_json
        );
        assert_eq!(sets.len(), 4);
        assert_eq!(sets[0].rows, [json!({"a": 1})]);
        assert_eq!(sets[2].affected, 2);
        let r = into_result(sets, Instant::now());
        assert_eq!(r.columns, ["i", "t"]);
        assert_eq!(r.rows[1], json!({"i": 2, "t": "ข"}));
        assert_eq!(r.sets.len(), 4);
        Ok(())
    }

    /// explain ใน Postgres จริง: ได้ JSON และ analyze ของ DELETE ไม่ลบจริง
    #[tokio::test]
    #[ignore]
    async fn pg_explain_rolls_back() {
        let base = std::env::var("MARKDB_TEST_PG").expect("ตั้ง MARKDB_TEST_PG ก่อน");
        let p = PgPool::connect(&format!("{}/postgres", base)).await.unwrap();
        p.execute("drop table if exists mdb_ex; create table mdb_ex (id int primary key); insert into mdb_ex select generate_series(1, 50)")
            .await
            .unwrap();
        let mut tx = p.begin().await.unwrap();
        let r = (&mut *tx).fetch_one("explain (format json, analyze, buffers) delete from mdb_ex").await.unwrap();
        tx.rollback().await.unwrap();
        let plan: serde_json::Value = serde_json::from_str(&cell_text(&cell_json(&r, 0).unwrap())).unwrap();
        assert_eq!(plan[0]["Plan"]["Node Type"], "ModifyTable");
        assert!(plan[0]["Execution Time"].is_number());
        let (n,): (i64,) = sqlx::query_as("select count(*) from mdb_ex").fetch_one(&p).await.unwrap();
        assert_eq!(n, 50);
        p.execute("drop table mdb_ex").await.unwrap();
    }

    /// session ใน Postgres จริง: เห็นคนที่โดน lock ขวาง และ kill ตัวที่ขวางแล้วอีกฝั่งไปต่อได้
    #[tokio::test]
    #[ignore]
    async fn pg_sessions_show_blocking() {
        let base = std::env::var("MARKDB_TEST_PG").expect("ตั้ง MARKDB_TEST_PG ก่อน");
        let url = format!("{}/postgres", base);
        let admin = PgPool::connect(&url).await.unwrap();
        admin.execute("drop table if exists mdb_lock; create table mdb_lock (id int primary key, v int); insert into mdb_lock values (1, 0)")
            .await
            .unwrap();
        // ตัวขวาง: ถือ lock แถว 1 ไว้ใน transaction ที่ยังไม่ commit
        let holder = PgPool::connect(&url).await.unwrap();
        let mut tx = holder.begin().await.unwrap();
        (&mut *tx).execute("update mdb_lock set v = 1 where id = 1").await.unwrap();
        let (holder_pid,): (i32,) = sqlx::query_as("select pg_backend_pid()").fetch_one(&mut *tx).await.unwrap();
        // ตัวที่รอ
        let waiter = PgPool::connect(&url).await.unwrap();
        let wait = tokio::spawn(async move { waiter.execute("update mdb_lock set v = 2 where id = 1").await.map(|r| r.rows_affected()) });
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        let rows = pg_sessions(&admin, Engine::Postgres).await.unwrap();
        let blocked: Vec<&serde_json::Value> = rows.iter().filter(|r| r["blocked_by"] != "").collect();
        assert_eq!(blocked.len(), 1, "{:?}", rows);
        assert_eq!(blocked[0]["blocked_by"], holder_pid.to_string());
        assert!(blocked[0]["query"].as_str().unwrap().contains("set v = 2"));

        let (ok,): (bool,) = sqlx::query_as("select pg_terminate_backend($1)").bind(holder_pid).fetch_one(&admin).await.unwrap();
        assert!(ok);
        assert_eq!(wait.await.unwrap().unwrap(), 1);
        drop(tx);
        admin.execute("drop table mdb_lock").await.unwrap();
    }

    #[test]
    fn several_statements_are_not_wrapped() {
        assert!(wrappable("select 1"));
        assert!(!wrappable("select 1; select 2"));
    }

    #[test]
    fn duplicate_columns_get_suffixed() {
        let mut c: Vec<String> = ["id", "name", "id", "id"].iter().map(|s| s.to_string()).collect();
        dedupe(&mut c);
        assert_eq!(c, ["id", "name", "id_1", "id_2"]);

        // ชื่อ id_1 มีอยู่จริงอยู่แล้ว ตัวที่ซ้ำต้องข้ามไปเลขถัดไป ไม่ไปทับของจริง
        let mut c: Vec<String> = ["id", "id_1", "id"].iter().map(|s| s.to_string()).collect();
        dedupe(&mut c);
        assert_eq!(c, ["id", "id_1", "id_2"]);

        // ไม่ซ้ำต้องไม่ถูกแตะ
        let mut c: Vec<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();
        dedupe(&mut c);
        assert_eq!(c, ["a", "b"]);
    }

    #[test]
    fn table_op_is_a_closed_set() {
        assert_eq!(op_sql("truncate", "\"s\".\"t\"").unwrap(), "TRUNCATE TABLE \"s\".\"t\"");
        assert_eq!(op_sql("drop_cascade", "t").unwrap(), "DROP TABLE t CASCADE");
        assert_eq!(op_sql("drop_view", "v").unwrap(), "DROP VIEW v");
        // op ที่ไม่รู้จักต้องไม่หลุดเป็น SQL — กันไม่ให้ UI ส่งคำสั่งอิสระเข้ามา
        assert!(op_sql("drop database markdb", "t").is_err());
        assert!(op_sql("", "t").is_err());
    }

    #[test]
    fn qualified_names_split() {
        assert_eq!(split_name("\"public\".\"users\""), ("public".into(), "users".into()));
        assert_eq!(split_name("sales.orders"), ("sales".into(), "orders".into()));
        assert_eq!(split_name("users"), ("public".into(), "users".into()));
        assert_eq!(split_name("\"users\""), ("public".into(), "users".into()));
    }

    #[test]
    fn redshift_detected_from_version() {
        assert!(detect("PostgreSQL 8.0.2 on i686-pc-linux-gnu, Redshift 1.0.63590") == Engine::Redshift);
        assert!(detect("PostgreSQL 17.6 on x86_64-pc-linux-gnu") == Engine::Postgres);
    }

    #[test]
    fn literals_escape() {
        assert_eq!(sql_literal(&json!(null)), "NULL");
        assert_eq!(sql_literal(&json!(42)), "42");
        assert_eq!(sql_literal(&json!(true)), "true");
        assert_eq!(sql_literal(&json!("o'brien")), "'o''brien'");
        assert_eq!(sql_literal(&json!({"a":1})), "'{\"a\":1}'");
    }

    #[test]
    fn csv_nulls_and_keys() {
        assert_eq!(csv_val(""), None);
        assert_eq!(csv_val(" NULL "), None);
        assert_eq!(csv_val("null"), None);
        assert_eq!(csv_val("0"), Some("0".into()));
        let k = |c: &str, v: Option<&str>| KeyVal { column: c.into(), value: v.map(Into::into) };
        assert_eq!(where_keys(&[k("Meter_ID", Some("234"))]), "\"Meter_ID\" = '234'");
        assert_eq!(
            where_keys(&[k("a", Some("o'b")), k("b", None)]),
            "\"a\" = 'o''b' and \"b\" is null"
        );
    }

    #[test]
    fn csv_to_updates() {
        let path = std::env::temp_dir().join("markdb_csv_updates_test.csv");
        std::fs::write(
            &path,
            "\u{feff}Meter_ID,Is_Active,IsActive,IsDelete\n234,0,0,1\n145,1,1,NULL\n",
        )
        .unwrap();
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let got = csv_updates(
            path.to_str().unwrap(),
            "\"public\".\"t\"",
            &s(&["Meter_ID"]),
            &s(&["IsActive", "IsDelete"]),
        )
        .unwrap();
        assert_eq!(
            got,
            vec![
                (2, "update \"public\".\"t\" set \"IsActive\" = '0', \"IsDelete\" = '1' where \"Meter_ID\" = '234'".into(), "234".into()),
                (3, "update \"public\".\"t\" set \"IsActive\" = '1', \"IsDelete\" = NULL where \"Meter_ID\" = '145'".into(), "145".into()),
            ]
        );
        let bad = csv_updates(path.to_str().unwrap(), "t", &s(&["Nope"]), &s(&["IsActive"]));
        assert!(bad.unwrap_err().contains("Nope"));
        assert!(csv_updates(path.to_str().unwrap(), "t", &s(&["Meter_ID"]), &[]).is_err());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn identifiers_and_literals_escape() {
        assert_eq!(ident("ok"), "\"ok\"");
        assert_eq!(ident("we\"ird"), "\"we\"\"ird\"");
        assert_eq!(lit(&None), "NULL");
        assert_eq!(lit(&Some("o'brien".into())), "'o''brien'");
    }

    #[test]
    fn cells_flatten() {
        assert_eq!(cell_text(&json!(null)), "");
        assert_eq!(cell_text(&json!("hi")), "hi");
        assert_eq!(cell_text(&json!([1, 2])), "[1,2]");
    }
}
