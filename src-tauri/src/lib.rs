use serde::{Deserialize, Serialize};
use sqlx::postgres::{PgPool, PgPoolCopyExt, PgPoolOptions};
use sqlx::{Column, Executor};
use std::time::Instant;
use tokio::sync::Mutex;

const MAX_ROWS: usize = 5000;

struct AppState {
    pool: Mutex<Option<PgPool>>,
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

async fn pool(state: &AppState) -> R<PgPool> {
    state
        .pool
        .lock()
        .await
        .clone()
        .ok_or_else(|| "ยังไม่ได้เชื่อมต่อฐานข้อมูล".to_string())
}

#[tauri::command]
async fn connect(url: String, state: tauri::State<'_, AppState>) -> R<String> {
    let p = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .map_err(err)?;
    let ver: String = sqlx::query_scalar("select version()")
        .fetch_one(&p)
        .await
        .map_err(err)?;
    *state.pool.lock().await = Some(p);
    Ok(ver)
}

/// ลองต่อด้วย connection เดี่ยว ๆ แล้วปิดทิ้ง — ไม่แตะ pool ที่ใช้งานอยู่
/// กด test ระหว่างที่ยังต่อ DB อื่นค้างอยู่จึงไม่ทำให้หลุด
#[tauri::command]
async fn test_connection(url: String) -> R<String> {
    use sqlx::Connection;
    let mut c = sqlx::PgConnection::connect(&url).await.map_err(err)?;
    let ver: String = sqlx::query_scalar("select version()")
        .fetch_one(&mut c)
        .await
        .map_err(err)?;
    c.close().await.ok();
    Ok(ver)
}

#[tauri::command]
async fn disconnect(state: tauri::State<'_, AppState>) -> R<()> {
    if let Some(p) = state.pool.lock().await.take() {
        p.close().await;
    }
    Ok(())
}

#[tauri::command]
async fn list_tables(state: tauri::State<'_, AppState>) -> R<Vec<TableInfo>> {
    let p = pool(&state).await?;
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
            kind: if t == "VIEW" { "view".into() } else { "table".into() },
        })
        .collect())
}

/// (schema, table, column) ทั้ง database — ใช้ป้อน autocomplete ฝั่ง editor
#[tauri::command]
async fn list_all_columns(state: tauri::State<'_, AppState>) -> R<Vec<(String, String, String)>> {
    let p = pool(&state).await?;
    sqlx::query_as(
        "select table_schema, table_name, column_name from information_schema.columns
         where table_schema not in ('pg_catalog','information_schema')
         order by table_schema, table_name, ordinal_position",
    )
    .fetch_all(&p)
    .await
    .map_err(err)
}

/// คอลัมน์ที่เป็น primary key ของตาราง — ว่าง = แก้ค่าในตารางไม่ได้
#[tauri::command]
async fn list_pk(table: String, state: tauri::State<'_, AppState>) -> R<Vec<String>> {
    let p = pool(&state).await?;
    let rows: Vec<(String,)> = sqlx::query_as(
        "select a.attname from pg_index i
         join pg_attribute a on a.attrelid = i.indrelid and a.attnum = any(i.indkey)
         where i.indrelid = $1::regclass and i.indisprimary",
    )
    .bind(&table)
    .fetch_all(&p)
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
struct TableProps {
    columns: Vec<ColumnInfo>,
    approx_rows: i64,
    size: String,
}

/// properties ของตาราง — อ่านจาก pg_catalog ล้วน ไม่แตะข้อมูลจริงสักแถว
/// (reltuples เป็นค่าประมาณจาก ANALYZE ล่าสุด จึงไม่ต้อง count ทั้งตาราง)
#[tauri::command]
async fn table_props(table: String, state: tauri::State<'_, AppState>) -> R<TableProps> {
    let p = pool(&state).await?;
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

    Ok(TableProps {
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
    state: tauri::State<'_, AppState>,
) -> R<Vec<String>> {
    if !table
        .chars()
        .all(|c| c.is_alphanumeric() || "_$.\"".contains(c))
    {
        return Err("ชื่อตารางไม่ถูกต้อง".into());
    }
    let p = pool(&state).await?;
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
    sqlx::query("set local statement_timeout = 5000")
        .execute(&mut *tx)
        .await
        .map_err(err)?;
    let rows: Result<Vec<(String,)>, _> = sqlx::query_as(&sql).bind(&pattern).fetch_all(&mut *tx).await;
    tx.rollback().await.ok();
    Ok(rows.map_err(err)?.into_iter().map(|(v,)| v).collect())
}

#[derive(Deserialize)]
struct KeyVal {
    column: String,
    value: Option<String>,
}

/// แก้ค่า cell เดียวผ่าน primary key — รันใน transaction แล้วยืนยันว่าโดนแค่ 1 แถว
/// ถ้าไม่ใช่ 1 แถว rollback ทิ้งทันที (กันเคส pk ซ้ำ/แถวหาย แล้วเขียนทับข้อมูลคนอื่น)
#[tauri::command]
async fn update_cell(
    table: String,
    column: String,
    value: Option<String>,
    keys: Vec<KeyVal>,
    state: tauri::State<'_, AppState>,
) -> R<u64> {
    if keys.is_empty() {
        return Err("ตารางนี้ไม่มี primary key จึงแก้ค่าตรง ๆ ไม่ได้".into());
    }
    let p = pool(&state).await?;
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

fn is_read_query(sql: &str) -> bool {
    let head = sql
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("--"))
        .unwrap_or("")
        .to_ascii_lowercase();
    ["select", "with", "table ", "values", "show", "explain"]
        .iter()
        .any(|k| head.starts_with(k))
}

// ponytail: ให้ Postgres serialize ผลลัพธ์เป็น JSON เอง (json_agg) แทน decode ทีละ type
// ฝั่ง Rust — ครอบคลุมทุก type รวม array/jsonb/range โดยไม่ต้องเขียน mapping เลย
// ราคาที่จ่ายคือทั้งชุดอยู่ใน memory ครั้งเดียว จึงตัดที่ MAX_ROWS
// อยาก stream ระดับล้านแถวค่อยเปลี่ยนไป decode raw + server-side cursor
#[tauri::command]
async fn run_query(sql: String, state: tauri::State<'_, AppState>) -> R<QueryResult> {
    let p = pool(&state).await?;
    let t0 = Instant::now();
    let trimmed = sql.trim().trim_end_matches(';').trim().to_string();
    if trimmed.is_empty() {
        return Err("ไม่มี SQL ให้รัน".into());
    }

    if !is_read_query(&trimmed) {
        let res = sqlx::raw_sql(&trimmed).execute(&p).await.map_err(err)?;
        return Ok(QueryResult {
            columns: vec![],
            rows: vec![],
            affected: res.rows_affected(),
            elapsed_ms: t0.elapsed().as_millis(),
            truncated: false,
        });
    }

    let described = p.describe(&trimmed).await.map_err(err)?;
    let columns: Vec<String> = described
        .columns()
        .iter()
        .map(|c| c.name().to_string())
        .collect();

    let wrapped = format!(
        "select coalesce(json_agg(_t), '[]'::json)::text from (select * from ({}) _q limit {}) _t",
        trimmed, MAX_ROWS
    );
    let raw: String = sqlx::query_scalar(&wrapped)
        .fetch_one(&p)
        .await
        .map_err(err)?;
    let rows: Vec<serde_json::Value> = serde_json::from_str(&raw).map_err(err)?;

    Ok(QueryResult {
        truncated: rows.len() >= MAX_ROWS,
        affected: rows.len() as u64,
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
async fn import_csv(path: String, table: String, state: tauri::State<'_, AppState>) -> R<u64> {
    let p = pool(&state).await?;
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

// ponytail: เครื่องนี้ไม่มี pg_dump จึง generate dump เอง — DDL อ่านจาก pg_catalog
// (format_type / pg_get_constraintdef / pg_get_viewdef ให้ Postgres ประกอบให้แทนที่จะเดาเอง)
// ส่วน data stream ทีละแถวด้วย row_to_json แล้วเขียนลงไฟล์เลย ไม่กองใน memory
// ไม่ครอบคลุม: trigger, function, extension, grant, partition — ถ้าต้องใช้ให้ลง pg_dump แทน
#[tauri::command]
async fn backup_database(path: String, state: tauri::State<'_, AppState>) -> R<String> {
    use futures_util::TryStreamExt;
    use std::io::Write;

    const BATCH: usize = 100;
    let p = pool(&state).await?;
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
async fn import_sql(path: String, state: tauri::State<'_, AppState>) -> R<u64> {
    let p = pool(&state).await?;
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
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            pool: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            connect,
            test_connection,
            disconnect,
            list_tables,
            list_all_columns,
            list_pk,
            column_values,
            table_props,
            update_cell,
            run_query,
            export_csv,
            export_sql,
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
        assert!(is_read_query("select 1"));
        assert!(is_read_query("  SELECT * FROM t"));
        assert!(is_read_query("-- comment\nwith x as (select 1) select * from x"));
        assert!(!is_read_query("insert into t values (1)"));
        assert!(!is_read_query("create table t (id int)"));
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
