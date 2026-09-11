use serde::{Deserialize, Serialize};
use sqlx::postgres::{PgPool, PgPoolCopyExt, PgPoolOptions};
use sqlx::{Column, Executor, Row, TypeInfo, ValueRef};
use std::collections::HashMap;
use std::time::Instant;
use tokio::sync::Mutex;

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
struct AppState {
    conns: Mutex<HashMap<String, Db>>,
}

#[derive(Serialize)]
struct QueryResult {
    columns: Vec<String>,
    rows: Vec<serde_json::Value>,
    affected: u64,
    elapsed_ms: u128,
    truncated: bool,
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
    state
        .conns
        .lock()
        .await
        .get(conn)
        .cloned()
        .ok_or_else(|| "connection นี้ยังไม่ได้เชื่อมต่อ".to_string())
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
}

#[tauri::command]
async fn connect(url: String, conn: String,
    state: tauri::State<'_, AppState>) -> R<ConnInfo> {
    let p = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .map_err(err)?;
    let version: String = sqlx::query_scalar("select version()")
        .fetch_one(&p)
        .await
        .map_err(err)?;
    let engine = detect(&version);
    state.conns.lock().await.insert(conn, Db { pool: p, engine });
    Ok(ConnInfo {
        version,
        engine: engine.name(),
    })
}

/// ลองต่อด้วย connection เดี่ยว ๆ แล้วปิดทิ้ง — ไม่แตะ pool ที่ใช้งานอยู่
/// กด test ระหว่างที่ยังต่อ DB อื่นค้างอยู่จึงไม่ทำให้หลุด
#[tauri::command]
async fn test_connection(url: String) -> R<ConnInfo> {
    use sqlx::Connection;
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
    })
}

#[tauri::command]
async fn disconnect(conn: String,
    state: tauri::State<'_, AppState>) -> R<()> {
    if let Some(d) = state.conns.lock().await.remove(&conn) {
        d.pool.close().await;
    }
    Ok(())
}

#[tauri::command]
async fn list_tables(conn: String,
    state: tauri::State<'_, AppState>) -> R<Vec<TableInfo>> {
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
    state: tauri::State<'_, AppState>) -> R<Vec<(String, String, String)>> {
    let p = pool(&state, &conn).await?;
    sqlx::query_as(
        "select table_schema, table_name, column_name from information_schema.columns
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

fn ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn lit(v: &Option<String>) -> String {
    match v {
        None => "NULL".into(),
        // literal ไม่ระบุ type — ให้ Postgres cast ตาม type ของคอลัมน์เอง
        Some(s) => format!("'{}'", s.replace('\'', "''")),
    }
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
        .all(|c| c.is_alphanumeric() || "_$.\"".contains(c))
    {
        return Err("ชื่อตารางไม่ถูกต้อง".into());
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

/// เพิ่มแถวใหม่ — ส่งมาเฉพาะคอลัมน์ที่ผู้ใช้กรอกจริง
/// คอลัมน์ที่ไม่ได้ส่งมาจะได้ DEFAULT ของตาราง (serial/uuid/now() จึงทำงานตามปกติ)
#[tauri::command]
async fn insert_row(
    table: String,
    values: Vec<KeyVal>,
    conn: String,
    state: tauri::State<'_, AppState>,
) -> R<u64> {
    let p = pool(&state, &conn).await?;
    let sql = if values.is_empty() {
        format!("insert into {} default values", table)
    } else {
        format!(
            "insert into {} ({}) values ({})",
            table,
            values
                .iter()
                .map(|v| ident(&v.column))
                .collect::<Vec<_>>()
                .join(", "),
            values
                .iter()
                .map(|v| lit(&v.value))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let mut tx = p.begin().await.map_err(err)?;
    match sqlx::query(&sql).execute(&mut *tx).await {
        Ok(r) => {
            let n = r.rows_affected();
            tx.commit().await.map_err(err)?;
            Ok(n)
        }
        Err(e) => {
            tx.rollback().await.ok();
            Err(err(e))
        }
    }
}

/// ลบแถวผ่าน primary key — เงื่อนไขต้องตรงพอดี 1 แถว ไม่งั้น rollback
#[tauri::command]
async fn delete_row(
    table: String,
    keys: Vec<KeyVal>,
    conn: String,
    state: tauri::State<'_, AppState>,
) -> R<u64> {
    if keys.is_empty() {
        return Err("ตารางนี้ไม่มี primary key จึงลบแถวตรง ๆ ไม่ได้".into());
    }
    let p = pool(&state, &conn).await?;
    let where_sql = keys
        .iter()
        .map(|k| match &k.value {
            None => format!("{} is null", ident(&k.column)),
            v => format!("{} = {}", ident(&k.column), lit(v)),
        })
        .collect::<Vec<_>>()
        .join(" and ");
    let sql = format!("delete from {} where {}", table, where_sql);

    let mut tx = p.begin().await.map_err(err)?;
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
    Ok(n)
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
    let p = pool(&state, &conn).await?;
    let where_sql = keys
        .iter()
        .map(|k| match &k.value {
            None => format!("{} is null", ident(&k.column)),
            v => format!("{} = {}", ident(&k.column), lit(v)),
        })
        .collect::<Vec<_>>()
        .join(" and ");
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
fn wrappable(sql: &str) -> bool {
    let head = sql
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("--"))
        .unwrap_or("")
        .to_ascii_lowercase();
    ["select", "with", "table ", "values"]
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

#[tauri::command]
async fn run_query(sql: String, conn: String,
    state: tauri::State<'_, AppState>) -> R<QueryResult> {
    use futures_util::TryStreamExt;

    let p = pool(&state, &conn).await?;
    let t0 = Instant::now();
    let trimmed = sql.trim().trim_end_matches(';').trim().to_string();
    if trimmed.is_empty() {
        return Err("ไม่มี SQL ให้รัน".into());
    }

    // ดึงเกินมา 1 แถวเพื่อรู้ว่ามีต่ออีกไหม แล้วค่อยตัดทิ้ง
    let capped = wrappable(&trimmed);
    let to_run = if capped {
        format!("select * from ({}) _mdb limit {}", trimmed, MAX_ROWS + 1)
    } else {
        trimmed.clone()
    };

    let mut columns: Vec<String> = Vec::new();
    let mut rows: Vec<serde_json::Value> = Vec::new();
    let mut affected: u64 = 0;
    let mut seen: usize = 0;

    let mut stream = sqlx::raw_sql(&to_run).fetch_many(&p);
    while let Some(item) = stream.try_next().await.map_err(err)? {
        match item {
            sqlx::Either::Left(res) => affected += res.rows_affected(),
            sqlx::Either::Right(row) => {
                if columns.is_empty() {
                    columns = row.columns().iter().map(|c| c.name().to_string()).collect();
                }
                seen += 1;
                if rows.len() < MAX_ROWS {
                    let mut obj = serde_json::Map::with_capacity(columns.len());
                    for (i, c) in columns.iter().enumerate() {
                        obj.insert(c.clone(), cell_json(&row, i)?);
                    }
                    rows.push(serde_json::Value::Object(obj));
                }
            }
        }
    }
    drop(stream);

    // ผลลัพธ์ว่างยังอยากรู้ชื่อคอลัมน์ — ถามจาก describe (พังก็ปล่อยผ่าน)
    if columns.is_empty() && capped {
        if let Ok(desc) = p.describe(&trimmed).await {
            columns = desc.columns().iter().map(|c| c.name().to_string()).collect();
        }
    }

    Ok(QueryResult {
        truncated: seen > MAX_ROWS,
        affected: if columns.is_empty() { affected } else { seen as u64 },
        columns,
        rows,
        elapsed_ms: t0.elapsed().as_millis(),
    })
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
        .manage(AppState {
            conns: Mutex::new(HashMap::new()),
        })
        .invoke_handler(tauri::generate_handler![
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
            delete_row,
            run_query,
            export_csv,
            export_sql,
            export_json,
            import_csv,
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
