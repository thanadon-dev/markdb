//! SQLite และ MySQL / MariaDB — driver คนละตัวกับ Postgres แต่ทำหน้าที่เดียวกัน
//! รองรับ: ต่อ/ทดสอบ, รายการตาราง/คอลัมน์, primary key, properties, ER, autocomplete ค่าในคอลัมน์,
//! รัน query (หลายคำสั่ง), แก้/เพิ่ม/ลบแถว, truncate/drop, รันไฟล์ .sql
//! ยังไม่รองรับ (backup, import CSV, sync) — db() ฝั่ง Postgres ตอบ error บอกชัด ๆ

use super::{
    cell_text, dedupe, delete_select, err, ident_in, insert_sql_in, into_result, lit_in, where_keys_in, wrappable,
    ColumnInfo, Deleted, Dialect, Edge, Explained, KeyVal, QueryResult, Relation, ResultSet, TableInfo,
    TableProps, KEEP_DELETED, MAX_ROWS, R,
};
use serde_json::Value;
use sqlx::mysql::{MySqlConnectOptions, MySqlPool, MySqlPoolOptions, MySqlRow};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions, SqliteRow};
use sqlx::{Column, Executor, Row, TypeInfo, ValueRef};
use std::str::FromStr;
use std::time::Instant;

#[derive(Clone, Copy)]
pub enum Kind {
    Lite,
    My,
}

/// sqlite:<ไฟล์> / mysql:// / mariadb:// — ที่เหลือเป็น Postgres
pub fn kind(url: &str) -> Option<Kind> {
    let u = url.trim_start().to_ascii_lowercase();
    if u.starts_with("sqlite:") {
        Some(Kind::Lite)
    } else if u.starts_with("mysql://") || u.starts_with("mariadb://") {
        Some(Kind::My)
    } else {
        None
    }
}

#[derive(Clone)]
pub enum Other {
    Lite(SqlitePool),
    My(MySqlPool),
}

/// โค้ดก้อนเดียวรันได้กับทั้งสอง pool — ข้างในใช้ได้เฉพาะของที่ทั้งสอง driver มีเหมือนกัน
macro_rules! on {
    ($o:expr, $p:ident => $body:expr) => {
        match $o {
            Other::Lite($p) => $body,
            Other::My($p) => $body,
        }
    };
}

pub async fn open(kind: Kind, url: &str, ro: bool) -> R<(Other, String)> {
    match kind {
        Kind::Lite => {
            // ไม่สร้างไฟล์ให้เอง — พิมพ์ path ผิดแล้วได้ database ว่างเปล่ามาแทนจะงง
            let opts = SqliteConnectOptions::from_str(url.trim())
                .map_err(err)?
                .create_if_missing(false)
                .read_only(ro);
            let p = SqlitePoolOptions::new().max_connections(4).connect_with(opts).await.map_err(err)?;
            let (v,): (String,) = sqlx::query_as("select sqlite_version()").fetch_one(&p).await.map_err(err)?;
            Ok((Other::Lite(p), format!("SQLite {}", v)))
        }
        Kind::My => {
            let url = format!("mysql://{}", url.trim().split_once("://").map_or("", |(_, r)| r));
            let opts = MySqlConnectOptions::from_str(&url).map_err(err)?;
            let p = MySqlPoolOptions::new()
                .max_connections(4)
                .after_connect(move |c, _| {
                    Box::pin(async move {
                        if ro {
                            c.execute("set session transaction read only").await?;
                        }
                        Ok(())
                    })
                })
                .connect_with(opts)
                .await
                .map_err(err)?;
            let (v,): (String,) = sqlx::query_as("select cast(version() as char)").fetch_one(&p).await.map_err(err)?;
            Ok((Other::My(p), format!("MySQL {}", v)))
        }
    }
}

/// ค่าใน cell เป็น JSON — ใช้กับแถวที่ได้จาก raw SQL (&str) เท่านั้น
pub trait Cells {
    fn cell(&self, i: usize) -> R<Value>;
}

fn hex(b: &[u8]) -> String {
    let mut s = String::from("\\x");
    for x in b {
        s.push_str(&format!("{:02x}", x));
    }
    s
}

impl Cells for SqliteRow {
    // SQLite บอกชนิดตามค่าที่เก็บจริงทีละ cell (INTEGER / REAL / TEXT / BLOB)
    fn cell(&self, i: usize) -> R<Value> {
        let v = self.try_get_raw(i).map_err(err)?;
        if v.is_null() {
            return Ok(Value::Null);
        }
        let ty = v.type_info().name().to_ascii_uppercase();
        Ok(match ty.as_str() {
            "INTEGER" => Value::from(self.try_get_unchecked::<i64, _>(i).map_err(err)?),
            "REAL" => serde_json::Number::from_f64(self.try_get_unchecked::<f64, _>(i).map_err(err)?)
                .map_or(Value::Null, Value::Number),
            "BLOB" => Value::String(hex(&self.try_get_unchecked::<Vec<u8>, _>(i).map_err(err)?)),
            _ => Value::String(self.try_get_unchecked::<String, _>(i).map_err(err)?),
        })
    }
}

impl Cells for MySqlRow {
    // raw SQL ไปทาง text protocol ทุกค่ามาเป็นข้อความ — แปลงเป็นตัวเลขเฉพาะชนิดตัวเลข
    // ponytail: DECIMAL ใหญ่มากแปลงเป็น f64 แล้วเสียหลักท้าย เหมือนฝั่ง Postgres
    fn cell(&self, i: usize) -> R<Value> {
        let v = self.try_get_raw(i).map_err(err)?;
        if v.is_null() {
            return Ok(Value::Null);
        }
        let ty = v.type_info().name().to_ascii_uppercase();
        let text = match self.try_get_unchecked::<String, _>(i) {
            Ok(s) => s,
            // binary / blob ที่ไม่ใช่ UTF-8
            Err(_) => return Ok(Value::String(hex(&self.try_get_unchecked::<Vec<u8>, _>(i).map_err(err)?))),
        };
        let number = ty.ends_with("INT") || ty.contains("INT ") || matches!(ty.as_str(), "DECIMAL" | "FLOAT" | "DOUBLE" | "YEAR");
        Ok(if number {
            serde_json::from_str(&text).unwrap_or(Value::String(text))
        } else if ty == "BOOLEAN" {
            Value::Bool(text == "1")
        } else {
            Value::String(text)
        })
    }
}

/// `a`.`b` / "a"."b" / a.b / b → (schema, ชื่อ) — schema ว่าง = ตัวที่เชื่อมต่ออยู่
fn split(q: &str) -> (String, String) {
    fn unq(s: &str) -> String {
        let s = s.trim();
        for c in ['`', '"'] {
            if s.starts_with(c) && s.ends_with(c) && s.len() >= 2 {
                let one = c.to_string();
                return s[1..s.len() - 1].replace(&format!("{0}{0}", c), &one);
            }
        }
        s.to_string()
    }
    let q = q.trim();
    for sep in ["`.`", "\".\""] {
        if let Some((a, b)) = q.split_once(sep) {
            let c = &sep[..1];
            return (unq(&format!("{}{}", a, c)), unq(&format!("{}{}", c, b)));
        }
    }
    match q.split_once('.') {
        Some((a, b)) if !q.starts_with('`') && !q.starts_with('"') => (unq(a), unq(b)),
        _ => (String::new(), unq(q)),
    }
}

const MY_SCOPE: &str =
    "(table_schema = database() or (database() is null and table_schema not in ('mysql','information_schema','performance_schema','sys')))";

impl Other {
    pub fn name(&self) -> &'static str {
        match self {
            Other::Lite(_) => "sqlite",
            Other::My(_) => "mysql",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Other::Lite(_) => "SQLite",
            Other::My(_) => "MySQL",
        }
    }

    pub fn dialect(&self) -> Dialect {
        match self {
            Other::Lite(_) => Dialect::Std,
            Other::My(_) => Dialect::My,
        }
    }

    pub async fn close(&self) {
        on!(self, p => p.close().await)
    }

    pub async fn list_tables(&self) -> R<Vec<TableInfo>> {
        let rows: Vec<(String, String, String)> = match self {
            Other::Lite(p) => sqlx::query_as(
                "select 'main', name, type from sqlite_master
                 where type in ('table','view') and name not like 'sqlite_%' order by name",
            )
            .fetch_all(p)
            .await,
            Other::My(p) => sqlx::query_as(&format!(
                "select cast(table_schema as char), cast(table_name as char), cast(table_type as char)
                 from information_schema.tables where {} order by 1, 2",
                MY_SCOPE
            ))
            .fetch_all(p)
            .await,
        }
        .map_err(err)?;
        Ok(rows
            .into_iter()
            .map(|(schema, name, t)| TableInfo {
                schema,
                name,
                kind: if t.to_ascii_lowercase().contains("view") { "view" } else { "table" }.into(),
            })
            .collect())
    }

    pub async fn list_all_columns(&self) -> R<Vec<(String, String, String, String)>> {
        match self {
            Other::Lite(p) => sqlx::query_as(
                "select 'main', m.name, c.name, coalesce(c.type, '')
                 from sqlite_master m join pragma_table_info(m.name) c
                 where m.type in ('table','view') and m.name not like 'sqlite_%'
                 order by m.name, c.cid",
            )
            .fetch_all(p)
            .await,
            Other::My(p) => sqlx::query_as(&format!(
                "select cast(table_schema as char), cast(table_name as char), cast(column_name as char),
                        cast(data_type as char)
                 from information_schema.columns where {} order by 1, 2, ordinal_position",
                MY_SCOPE
            ))
            .fetch_all(p)
            .await,
        }
        .map_err(err)
    }

    // shortcut: ใช้แค่ primary key — ตารางที่มีแต่ unique index ยังแก้ค่าตรง ๆ ไม่ได้ (Postgres ทำได้)
    pub async fn list_pk(&self, table: &str) -> R<Vec<String>> {
        let (sch, tbl) = split(table);
        let rows: Vec<(String,)> = match self {
            Other::Lite(p) => sqlx::query_as("select name from pragma_table_info(?) where pk > 0 order by pk")
                .bind(&tbl)
                .fetch_all(p)
                .await,
            Other::My(p) => sqlx::query_as(
                "select cast(column_name as char) from information_schema.key_column_usage
                 where table_schema = coalesce(nullif(?, ''), database()) and table_name = ?
                   and constraint_name = 'PRIMARY'
                 order by ordinal_position",
            )
            .bind(&sch)
            .bind(&tbl)
            .fetch_all(p)
            .await,
        }
        .map_err(err)?;
        Ok(rows.into_iter().map(|(c,)| c).collect())
    }

    pub async fn table_props(&self, table: &str) -> R<TableProps> {
        let (sch, tbl) = split(table);
        let (cols, approx_rows, size, rels): (
            Vec<(String, String, i64, String, i64)>,
            i64,
            String,
            Vec<(String, String, String, String)>,
        ) = match self {
            Other::Lite(p) => {
                let cols = sqlx::query_as(
                    "select name, coalesce(type, ''), \"notnull\" = 0, coalesce(dflt_value, ''), pk > 0
                     from pragma_table_info(?) order by cid",
                )
                .bind(&tbl)
                .fetch_all(p)
                .await
                .map_err(err)?;
                // SQLite ไม่มีสถิติจำนวนแถว — นับจริง (ไฟล์ในเครื่อง เร็วพอ)
                let (n,): (i64,) = sqlx::query_as(&format!("select count(*) from {}", ident_in(Dialect::Std, &tbl)))
                    .fetch_one(p)
                    .await
                    .unwrap_or((0,));
                let rels = sqlx::query_as(
                    "select 'out', 'fk_' || id, 'main.' || \"table\",
                            'FOREIGN KEY (' || group_concat(\"from\", ', ') || ') REFERENCES ' || \"table\" ||
                            ' (' || group_concat(coalesce(\"to\", ''), ', ') || ')'
                     from pragma_foreign_key_list(?1) group by id
                     union all
                     select 'in', 'fk_' || f.id, 'main.' || m.name,
                            'FOREIGN KEY (' || group_concat(f.\"from\", ', ') || ') REFERENCES ' || f.\"table\" ||
                            ' (' || group_concat(coalesce(f.\"to\", ''), ', ') || ')'
                     from sqlite_master m join pragma_foreign_key_list(m.name) f
                     where m.type = 'table' and f.\"table\" = ?1 group by m.name, f.id
                     order by 1, 2",
                )
                .bind(&tbl)
                .fetch_all(p)
                .await
                .unwrap_or_default();
                (cols, n, "-".to_string(), rels)
            }
            Other::My(p) => {
                let cols = sqlx::query_as(
                    "select cast(column_name as char), cast(column_type as char), cast(is_nullable = 'YES' as signed),
                            cast(coalesce(column_default, '') as char), cast(column_key = 'PRI' as signed)
                     from information_schema.columns
                     where table_schema = coalesce(nullif(?, ''), database()) and table_name = ?
                     order by ordinal_position",
                )
                .bind(&sch)
                .bind(&tbl)
                .fetch_all(p)
                .await
                .map_err(err)?;
                let info: Option<(i64, String)> = sqlx::query_as(
                    "select cast(coalesce(table_rows, 0) as signed),
                            cast(concat(round((coalesce(data_length, 0) + coalesce(index_length, 0)) / 1048576, 2), ' MB') as char)
                     from information_schema.tables
                     where table_schema = coalesce(nullif(?, ''), database()) and table_name = ?",
                )
                .bind(&sch)
                .bind(&tbl)
                .fetch_optional(p)
                .await
                .unwrap_or(None);
                let rels = sqlx::query_as(
                    "select cast(case when k.table_schema = x.s and k.table_name = x.t then 'out' else 'in' end as char),
                            cast(k.constraint_name as char),
                            cast(case when k.table_schema = x.s and k.table_name = x.t
                                      then concat(k.referenced_table_schema, '.', k.referenced_table_name)
                                      else concat(k.table_schema, '.', k.table_name) end as char),
                            cast(concat('FOREIGN KEY (', group_concat(k.column_name order by k.ordinal_position separator ', '),
                                        ') REFERENCES ', k.referenced_table_name, ' (',
                                        group_concat(k.referenced_column_name order by k.ordinal_position separator ', '), ')') as char)
                     from information_schema.key_column_usage k,
                          (select coalesce(nullif(?, ''), database()) s, ? t) x
                     where k.referenced_table_name is not null
                       and ((k.table_schema = x.s and k.table_name = x.t)
                            or (k.referenced_table_schema = x.s and k.referenced_table_name = x.t))
                     group by k.table_schema, k.table_name, k.constraint_name,
                              k.referenced_table_schema, k.referenced_table_name, x.s, x.t
                     order by 1, 2",
                )
                .bind(&sch)
                .bind(&tbl)
                .fetch_all(p)
                .await
                .unwrap_or_default();
                let (n, size) = info.unwrap_or((0, "-".into()));
                (cols, n, size, rels)
            }
        };
        Ok(TableProps {
            columns: cols
                .into_iter()
                .map(|(name, data_type, nullable, default, pk)| ColumnInfo {
                    name,
                    data_type,
                    nullable: nullable != 0,
                    default,
                    pk: pk != 0,
                })
                .collect(),
            approx_rows,
            size,
            relations: rels
                .into_iter()
                .map(|(dir, name, other, def)| Relation { dir, name, other, def })
                .collect(),
        })
    }

    pub async fn er_edges(&self) -> R<Vec<Edge>> {
        let rows: Vec<(String, String, String, String)> = match self {
            Other::Lite(p) => sqlx::query_as(
                "select 'main.' || m.name, group_concat(f.\"from\", ', '),
                        'main.' || f.\"table\", group_concat(coalesce(f.\"to\", ''), ', ')
                 from sqlite_master m join pragma_foreign_key_list(m.name) f
                 where m.type = 'table'
                 group by m.name, f.id order by 1, 3",
            )
            .fetch_all(p)
            .await,
            Other::My(p) => sqlx::query_as(&format!(
                "select cast(concat(table_schema, '.', table_name) as char),
                        cast(group_concat(column_name order by ordinal_position separator ', ') as char),
                        cast(concat(referenced_table_schema, '.', referenced_table_name) as char),
                        cast(group_concat(referenced_column_name order by ordinal_position separator ', ') as char)
                 from information_schema.key_column_usage
                 where referenced_table_name is not null and {}
                 group by table_schema, table_name, constraint_name, referenced_table_schema, referenced_table_name
                 order by 1, 3",
                MY_SCOPE
            ))
            .fetch_all(p)
            .await,
        }
        .map_err(err)?;
        Ok(rows
            .into_iter()
            .map(|(src, src_cols, dst, dst_cols)| Edge { src, src_cols, dst, dst_cols })
            .collect())
    }

    /// ค่าจริงในคอลัมน์ไว้ป้อน autocomplete — อ่านใน transaction แล้ว rollback เสมอ
    pub async fn column_values(&self, table: &str, column: &str, prefix: &str) -> R<Vec<String>> {
        let d = self.dialect();
        let c = ident_in(d, column);
        let cast = if d == Dialect::My { "char" } else { "text" };
        // escape ด้วย ! — MySQL ถือ \ ใน string เป็น escape อยู่แล้ว ใช้ \ จะซ้อนกันจนงง
        let pattern = format!("{}%", prefix.replace('!', "!!").replace('%', "!%").replace('_', "!_"));
        let sql = format!(
            "select cast({c} as {cast}) as v from {t}
             where {c} is not null and cast({c} as {cast}) like ? escape '!'
             group by 1 order by 1 limit 200",
            c = c,
            cast = cast,
            t = table
        );
        on!(self, p => {
            let mut tx = p.begin().await.map_err(err)?;
            let rows: Result<Vec<(String,)>, _> = sqlx::query_as(&sql).bind(&pattern).fetch_all(&mut *tx).await;
            tx.rollback().await.ok();
            Ok(rows.map_err(err)?.into_iter().map(|(v,)| v).collect())
        })
    }

    pub async fn run_query(&self, sql: &str) -> R<QueryResult> {
        let t0 = Instant::now();
        let trimmed = sql.trim().trim_end_matches(';').trim().to_string();
        if trimmed.is_empty() {
            return Err("ไม่มี SQL ให้รัน".into());
        }
        if let Some((table, select)) = delete_select(&trimmed) {
            return self.run_delete(&trimmed, table, &select).await;
        }
        let capped = wrappable(&trimmed);
        let to_run = if capped {
            format!("select * from ({}) _mdb limit {}", trimmed, MAX_ROWS + 1)
        } else {
            trimmed.clone()
        };
        let mut sets: Vec<ResultSet> = on!(self, p => result_sets!(sqlx::raw_sql(&to_run).fetch_many(p), Cells::cell));
        // ผลลัพธ์ว่างยังอยากรู้ชื่อคอลัมน์ — ถามจาก describe (พังก็ปล่อยผ่าน)
        if capped && sets.len() <= 1 && sets.first().map_or(true, |s| s.columns.is_empty()) {
            let cols: Option<Vec<String>> = on!(self, p => p
                .describe(&trimmed)
                .await
                .ok()
                .map(|d| d.columns().iter().map(|c| c.name().to_string()).collect()));
            if let Some(mut cols) = cols.filter(|c| !c.is_empty()) {
                dedupe(&mut cols);
                sets = vec![ResultSet { columns: cols, ..Default::default() }];
            }
        }
        Ok(into_result(sets, t0))
    }

    /// ลบใน transaction เดียวกับที่อ่านแถวเก็บไว้ — แถวที่เก็บคือแถวที่ถูกลบจริง
    async fn run_delete(&self, sql: &str, table: String, select: &str) -> R<QueryResult> {
        let t0 = Instant::now();
        let probe = format!("select * from ({}) _d limit {}", select, KEEP_DELETED + 1);
        let (found, affected): (Vec<Value>, u64) = on!(self, p => {
            let mut tx = p.begin().await.map_err(err)?;
            let rows = (&mut *tx).fetch_all(probe.as_str()).await.map_err(err)?;
            let n = (&mut *tx).execute(sql).await.map_err(err)?.rows_affected();
            tx.commit().await.map_err(err)?;
            (rows.iter().map(row_json).collect::<R<Vec<_>>>()?, n)
        });
        let partial = found.len() > KEEP_DELETED;
        Ok(QueryResult {
            columns: Vec::new(),
            rows: Vec::new(),
            affected,
            elapsed_ms: t0.elapsed().as_millis(),
            truncated: false,
            deleted: Some(Deleted { table, rows: if partial { Vec::new() } else { found }, partial }),
            sets: Vec::new(),
        })
    }

    /// คืนแถวที่บันทึกจริงได้เฉพาะ SQLite (มี RETURNING) — MySQL คืน None เหมือน Redshift
    pub async fn insert_one(&self, table: &str, values: &[KeyVal]) -> R<Option<Value>> {
        let sql = insert_sql_in(self.dialect(), table, values);
        match self {
            Other::Lite(p) => Ok(Some(row_json(&p.fetch_one(format!("{} returning *", sql).as_str()).await.map_err(err)?)?)),
            Other::My(p) => {
                p.execute(sql.as_str()).await.map_err(err)?;
                Ok(None)
            }
        }
    }

    pub async fn insert_many(&self, table: &str, rows: &[Vec<KeyVal>]) -> R<Vec<Value>> {
        let d = self.dialect();
        let sqls: Vec<String> = rows.iter().map(|r| insert_sql_in(d, table, r)).collect();
        match self {
            Other::Lite(p) => {
                let mut tx = p.begin().await.map_err(err)?;
                let mut saved = Vec::new();
                for sql in &sqls {
                    let r = (&mut *tx).fetch_one(format!("{} returning *", sql).as_str()).await.map_err(err)?;
                    saved.push(row_json(&r)?);
                }
                tx.commit().await.map_err(err)?;
                Ok(saved)
            }
            Other::My(p) => {
                let mut tx = p.begin().await.map_err(err)?;
                for sql in &sqls {
                    (&mut *tx).execute(sql.as_str()).await.map_err(err)?;
                }
                tx.commit().await.map_err(err)?;
                Ok(Vec::new())
            }
        }
    }

    /// ทุกคำสั่งต้องโดนพอดี 1 แถว ไม่งั้น rollback ทั้งหมด
    pub async fn exact_one_each(&self, sqls: &[String], verb: &str) -> R<u64> {
        if sqls.is_empty() {
            return Ok(0);
        }
        on!(self, p => {
            let mut tx = p.begin().await.map_err(err)?;
            for (i, sql) in sqls.iter().enumerate() {
                let n = (&mut *tx).execute(sql.as_str()).await.map_err(err)?.rows_affected();
                if n != 1 {
                    tx.rollback().await.ok();
                    return Err(format!("รายการที่ {} ตรง {} แถว (ต้องเป็น 1) — ยกเลิกการ{}ทั้งหมด", i + 1, n, verb));
                }
            }
            tx.commit().await.map_err(err)?;
            Ok(sqls.len() as u64)
        })
    }

    pub async fn delete_row(&self, table: &str, keys: &[KeyVal]) -> R<Option<Value>> {
        let w = where_keys_in(self.dialect(), keys);
        let read = format!("select * from {} where {}", table, w);
        let del = format!("delete from {} where {}", table, w);
        on!(self, p => {
            let mut tx = p.begin().await.map_err(err)?;
            let before = match (&mut *tx).fetch_optional(read.as_str()).await {
                Ok(Some(r)) => Some(row_json(&r)?),
                _ => None,
            };
            let n = (&mut *tx).execute(del.as_str()).await.map_err(err)?.rows_affected();
            if n != 1 {
                tx.rollback().await.ok();
                return Err(format!("เงื่อนไขตรง {} แถว (ต้องเป็น 1) — ยกเลิกการลบ", n));
            }
            tx.commit().await.map_err(err)?;
            Ok(before)
        })
    }

    pub async fn update_cell(&self, table: &str, column: &str, value: &Option<String>, keys: &[KeyVal]) -> R<u64> {
        let d = self.dialect();
        let sql = format!("update {} set {} = {} where {}", table, ident_in(d, column), lit_in(d, value), where_keys_in(d, keys));
        self.exact_one_each(&[sql], "แก้ไข").await
    }

    pub async fn table_op(&self, op: &str, table: &str) -> R<u64> {
        let lite = matches!(self, Other::Lite(_));
        let sql = match op {
            // SQLite ไม่มี TRUNCATE / CASCADE
            "truncate" if lite => format!("DELETE FROM {}", table),
            "truncate" => format!("TRUNCATE TABLE {}", table),
            "drop" | "drop_cascade" if lite => format!("DROP TABLE {}", table),
            "drop" => format!("DROP TABLE {}", table),
            "drop_cascade" => format!("DROP TABLE {} CASCADE", table),
            "drop_view" => format!("DROP VIEW {}", table),
            _ => return Err(format!("ไม่รู้จักคำสั่ง {}", op)),
        };
        on!(self, p => Ok(p.execute(sql.as_str()).await.map_err(err)?.rows_affected()))
    }

    /// SQLite: EXPLAIN QUERY PLAN เป็นแถว (id, parent, detail) ให้ UI ต่อเป็นต้นไม้
    /// MySQL: EXPLAIN FORMAT=TREE (MariaDB ไม่มี → FORMAT=JSON), analyze รันจริงแล้ว rollback
    pub async fn explain(&self, q: &str, analyze: bool) -> R<Explained> {
        match self {
            Other::Lite(p) => {
                // แถวของ explain query plan: id, parent, notused, detail
                let json: Vec<Value> = p
                    .fetch_all(format!("explain query plan {}", q).as_str())
                    .await
                    .map_err(err)?
                    .iter()
                    .map(|r| Ok(serde_json::json!({ "id": r.cell(0)?, "parent": r.cell(1)?, "detail": r.cell(3)? })))
                    .collect::<R<Vec<_>>>()?;
                Ok(Explained { format: "sqlite", text: Value::Array(json).to_string() })
            }
            Other::My(p) => {
                let mut tx = p.begin().await.map_err(err)?;
                let first = if analyze { format!("explain analyze {}", q) } else { format!("explain format=tree {}", q) };
                let rows = match (&mut *tx).fetch_all(first.as_str()).await {
                    Ok(r) => r,
                    Err(_) => (&mut *tx).fetch_all(format!("explain format=json {}", q).as_str()).await.map_err(err)?,
                };
                tx.rollback().await.ok();
                let text = rows
                    .iter()
                    .map(|r| r.cell(0).map(|v| cell_text(&v)))
                    .collect::<R<Vec<_>>>()?
                    .join("\n");
                Ok(Explained { format: "text", text })
            }
        }
    }

    /// MySQL: processlist + ใครขวางใครจาก sys.innodb_lock_waits (ถ้ามี) — SQLite ไม่มี server จึงไม่มี session
    pub async fn sessions(&self) -> R<Vec<Value>> {
        let Other::My(p) = self else {
            return Err("SQLite เป็นไฟล์ในเครื่อง ไม่มี session ให้ดู".into());
        };
        let rows = p
            .fetch_all(
                "select id pid, user usr, coalesce(db, '') db, host client, command state, coalesce(state, '') wait,
                        time secs, coalesce(info, '') query
                 from information_schema.processlist
                 where id <> connection_id() and command <> 'Daemon'
                 order by command = 'Sleep', time desc",
            )
            .await
            .map_err(err)?;
        // ตาราง sys มีเฉพาะ MySQL 5.7+ — ไม่มีก็แค่ไม่รู้ว่าใครขวางใคร
        let waits: Vec<(i64, i64)> = sqlx::query_as("select cast(waiting_pid as signed), cast(blocking_pid as signed) from sys.innodb_lock_waits")
            .fetch_all(p)
            .await
            .unwrap_or_default();
        rows.iter()
            .map(|r| {
                let mut v = row_json(r)?;
                let pid = v["pid"].as_i64().unwrap_or(-1);
                let by: Vec<String> = waits.iter().filter(|(w, _)| *w == pid).map(|(_, b)| b.to_string()).collect();
                v["blocked_by"] = Value::String(by.join(","));
                Ok(v)
            })
            .collect()
    }

    pub async fn kill(&self, pid: i64, terminate: bool) -> R<bool> {
        let Other::My(p) = self else {
            return Err("SQLite ไม่มี session".into());
        };
        p.execute(format!("kill {}{}", if terminate { "" } else { "query " }, pid).as_str()).await.map_err(err)?;
        Ok(true)
    }

    /// รันไฟล์ .sql ใน transaction เดียว — MySQL commit เองทุกครั้งที่เจอ DDL จึง rollback ได้แค่ส่วน DML
    pub async fn run_script(&self, text: &str) -> R<u64> {
        on!(self, p => {
            let mut tx = p.begin().await.map_err(err)?;
            // &str (ไม่ใช่ raw_sql) — raw_sql ส่งเข้า transaction แล้วติด lifetime ของ Executor
            // ทั้งสอง driver รันหลายคำสั่งใน string เดียวได้
            let n = (&mut *tx).execute(text).await.map_err(err)?.rows_affected();
            tx.commit().await.map_err(err)?;
            Ok(n)
        })
    }
}

fn row_json<T: Cells + Row>(row: &T) -> R<Value> {
    let mut cols: Vec<String> = row.columns().iter().map(|c| c.name().to_string()).collect();
    dedupe(&mut cols);
    let mut obj = serde_json::Map::with_capacity(cols.len());
    for (i, c) in cols.iter().enumerate() {
        obj.insert(c.clone(), row.cell(i)?);
    }
    Ok(Value::Object(obj))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv(c: &str, v: Option<&str>) -> KeyVal {
        KeyVal { column: c.into(), value: v.map(String::from) }
    }

    /// ไฟล์ SQLite จริงใน temp — ไล่ทุกคำสั่งที่ UI ใช้
    #[tokio::test]
    async fn sqlite_end_to_end() {
        let path = std::env::temp_dir().join(format!("markdb_test_{}.db", std::process::id()));
        std::fs::remove_file(&path).ok();
        let url = format!("sqlite:{}", path.display());
        // ไม่สร้างไฟล์ให้เองถ้ายังไม่มี
        assert!(open(Kind::Lite, &url, false).await.is_err());
        sqlx::sqlite::SqlitePool::connect_with(SqliteConnectOptions::from_str(&url).unwrap().create_if_missing(true))
            .await
            .unwrap()
            .execute(
                "create table customer (id integer primary key, name text not null, note text, score real);
                 create table orders (id integer primary key, customer_id integer references customer(id), total real);
                 create view rich as select * from customer where score > 5;
                 insert into customer values (1, 'กข', 'C:\new', 9.5), (2, 'bob', null, 1), (3, 'ann', 'x', 7);
                 insert into orders values (10, 1, 100), (11, 3, 50);",
            )
            .await
            .unwrap();

        let (o, version) = open(Kind::Lite, &url, false).await.unwrap();
        assert!(version.starts_with("SQLite 3."), "{}", version);
        assert_eq!(kind(&url).map(|_| o.name()), Some("sqlite"));

        let t: Vec<(String, String)> = o.list_tables().await.unwrap().into_iter().map(|t| (t.name, t.kind)).collect();
        assert_eq!(t, [("customer".into(), "table".into()), ("orders".into(), "table".into()), ("rich".into(), "view".into())]);
        let cols = o.list_all_columns().await.unwrap();
        assert!(cols.contains(&("main".into(), "orders".into(), "total".into(), "REAL".into())));

        assert_eq!(o.list_pk("\"main\".\"customer\"").await.unwrap(), ["id"]);
        let props = o.table_props("\"main\".\"customer\"").await.unwrap();
        assert_eq!(props.approx_rows, 3);
        assert!(props.columns.iter().any(|c| c.name == "name" && !c.nullable));
        assert!(props.columns.iter().any(|c| c.name == "id" && c.pk));
        assert_eq!(props.relations.len(), 1);
        assert_eq!(props.relations[0].dir, "in");
        let edges = o.er_edges().await.unwrap();
        assert_eq!((edges[0].src.as_str(), edges[0].dst.as_str()), ("main.orders", "main.customer"));

        assert_eq!(o.column_values("\"main\".\"customer\"", "name", "A").await.unwrap(), ["ann"]);

        // คำสั่งเดียว: ชนิดค่าถูก (ตัวเลข/ข้อความ/NULL) + ภาษาไทย
        let r = o.run_query("select id, name, note, score from customer order by id").await.unwrap();
        assert_eq!(r.columns, ["id", "name", "note", "score"]);
        assert_eq!(r.rows[0], serde_json::json!({"id": 1, "name": "กข", "note": "C:\new", "score": 9.5}));
        assert_eq!(r.rows[1]["note"], Value::Null);
        assert!(r.sets.is_empty());
        // ผลว่างยังได้ชื่อคอลัมน์
        assert_eq!(o.run_query("select id from customer where id < 0").await.unwrap().columns, ["id"]);

        // หลายคำสั่ง: แยกผลทีละคำสั่ง ตัวหลักคือ select ตัวสุดท้าย
        let m = o
            .run_query("select count(*) n from customer; update orders set total = total + 1; select id from orders order by id")
            .await
            .unwrap();
        assert_eq!(m.sets.len(), 3);
        assert_eq!(m.sets[0].rows[0]["n"], 3);
        assert_eq!(m.sets[1].affected, 2);
        assert_eq!(m.columns, ["id"]);
        assert_eq!(m.rows.len(), 2);

        let ex = o.explain("select * from customer where id = 1", false).await.unwrap();
        assert_eq!(ex.format, "sqlite");
        assert!(ex.text.contains("customer"), "{}", ex.text);

        // แก้ / เพิ่ม / ลบ ผ่าน key
        let t = "\"main\".\"customer\"";
        assert_eq!(o.update_cell(t, "note", &Some("it's".into()), &[kv("id", Some("2"))]).await.unwrap(), 1);
        assert!(o.update_cell(t, "note", &None, &[kv("id", Some("99"))]).await.is_err());
        let saved = o.insert_one(t, &[kv("name", Some("new"))]).await.unwrap().unwrap();
        assert_eq!(saved["id"], 4);
        let many = o.insert_many(t, &[vec![kv("name", Some("a"))], vec![kv("name", Some("b"))]]).await.unwrap();
        assert_eq!(many.len(), 2);
        let gone = o.delete_row(t, &[kv("id", Some("4"))]).await.unwrap().unwrap();
        assert_eq!(gone["name"], "new");
        assert!(o.delete_row(t, &[kv("id", Some("4"))]).await.is_err());

        // DELETE ที่พิมพ์เอง: เก็บแถวที่ลบไว้ย้อนกลับ
        let d = o.run_query("delete from customer where id >= 5").await.unwrap();
        assert_eq!(d.affected, 2);
        assert_eq!(d.deleted.unwrap().rows.len(), 2);

        assert_eq!(o.run_script("\u{feff}insert into orders values (12, 2, 1); insert into orders values (13, 2, 2);".trim_start_matches('\u{feff}')).await.unwrap(), 2);
        o.table_op("truncate", "\"main\".\"orders\"").await.unwrap();
        assert_eq!(o.run_query("select * from orders").await.unwrap().rows.len(), 0);

        // อ่านอย่างเดียว: เขียนไม่ได้
        let (ro, _) = open(Kind::Lite, &url, true).await.unwrap();
        assert!(ro.run_query("delete from customer").await.is_err());
        ro.close().await;
        o.close().await;
        std::fs::remove_file(&path).ok();
    }

    /// MySQL จริง: MARKDB_TEST_MYSQL=mysql://root@127.0.0.1:port/markdb cargo test -- --ignored
    #[tokio::test]
    #[ignore]
    async fn mysql_end_to_end() {
        let url = std::env::var("MARKDB_TEST_MYSQL").expect("ตั้ง MARKDB_TEST_MYSQL ก่อน");
        let (o, version) = open(Kind::My, &url, false).await.unwrap();
        assert!(version.starts_with("MySQL "), "{}", version);
        o.run_script(
            "drop table if exists orders; drop table if exists customer; drop view if exists rich;
             create table customer (id int auto_increment primary key, name varchar(50) not null, note text, score double, active boolean, price decimal(10,2));
             create table orders (id int primary key, customer_id int, total double, foreign key (customer_id) references customer(id));
             create view rich as select * from customer where score > 5;
             insert into customer (name, note, score, active, price) values ('กข', 'C:\\new', 9.5, true, 12.30), ('bob', null, 1, false, 0), ('ann', 'x', 7, true, 5);
             insert into orders values (10, 1, 100), (11, 3, 50);",
        )
        .await
        .unwrap();

        // เทสต์อื่นใน database เดียวกันรันคู่ขนานได้ — ดูเฉพาะตารางของเทสต์นี้
        let t: Vec<(String, String)> = o
            .list_tables()
            .await
            .unwrap()
            .into_iter()
            .filter(|t| ["customer", "orders", "rich"].contains(&t.name.as_str()))
            .map(|t| (t.name, t.kind))
            .collect();
        assert_eq!(t, [("customer".into(), "table".into()), ("orders".into(), "table".into()), ("rich".into(), "view".into())]);
        let cols = o.list_all_columns().await.unwrap();
        assert!(cols.iter().any(|c| c.1 == "orders" && c.2 == "total"));
        let t = "`markdb`.`customer`";
        assert_eq!(o.list_pk(t).await.unwrap(), ["id"]);
        assert_eq!(o.list_pk("`customer`").await.unwrap(), ["id"]);
        let props = o.table_props(t).await.unwrap();
        assert!(props.columns.iter().any(|c| c.name == "id" && c.pk && !c.nullable));
        assert_eq!(props.relations.len(), 1);
        assert_eq!(props.relations[0].dir, "in");
        let edges = o.er_edges().await.unwrap();
        assert_eq!((edges[0].src.as_str(), edges[0].dst.as_str()), ("markdb.orders", "markdb.customer"));
        assert_eq!(o.column_values(t, "name", "A").await.unwrap(), ["ann"]);

        let r = o.run_query("select id, name, note, score, active, price from customer order by id").await.unwrap();
        assert_eq!(r.rows[0]["id"], 1);
        assert_eq!(r.rows[0]["name"], "กข");
        assert_eq!(r.rows[0]["note"], "C:\new");
        assert_eq!(r.rows[0]["score"], 9.5);
        assert_eq!(r.rows[0]["active"], true);
        assert_eq!(r.rows[0]["price"], 12.3);
        assert_eq!(r.rows[1]["note"], Value::Null);
        assert_eq!(o.run_query("select id from customer where id < 0").await.unwrap().columns, ["id"]);

        let m = o
            .run_query("select count(*) n from customer; update orders set total = total + 1; select id from orders order by id")
            .await
            .unwrap();
        assert_eq!(m.sets.len(), 3, "{:?}", m.sets.iter().map(|s| (&s.columns, s.affected)).collect::<Vec<_>>());
        assert_eq!(m.sets[1].affected, 2);
        assert_eq!(m.columns, ["id"]);

        let ex = o.explain("select * from customer where name = 'x'", false).await.unwrap();
        assert!(ex.text.contains("customer"), "{}", ex.text);
        // analyze ของ delete ไม่ลบจริง
        o.explain("delete from orders", true).await.ok();
        assert_eq!(o.run_query("select * from orders").await.unwrap().rows.len(), 2);

        // backslash ต้องเข้าไปตรงตัว (MySQL ถือ \ เป็น escape)
        assert_eq!(o.update_cell(t, "note", &Some("D:\tmp it's".into()), &[kv("id", Some("2"))]).await.unwrap(), 1);
        let back = o.run_query("select note from customer where id = 2").await.unwrap();
        assert_eq!(back.rows[0]["note"], "D:\tmp it's");
        assert!(o.update_cell(t, "note", &None, &[kv("id", Some("99"))]).await.is_err());
        assert_eq!(o.insert_one(t, &[kv("name", Some("new"))]).await.unwrap(), None);
        assert_eq!(o.insert_one(t, &[]).await.is_err(), true); // name not null
        o.insert_many(t, &[vec![kv("name", Some("a"))], vec![kv("name", Some("b"))]]).await.unwrap();
        let gone = o.delete_row(t, &[kv("name", Some("new"))]).await.unwrap().unwrap();
        assert_eq!(gone["name"], "new");
        let d = o.run_query("delete from customer where name in ('a', 'b')").await.unwrap();
        assert_eq!(d.affected, 2);
        assert_eq!(d.deleted.unwrap().rows.len(), 2);

        o.table_op("drop_view", "`markdb`.`rich`").await.unwrap();
        o.table_op("truncate", "`markdb`.`orders`").await.unwrap();
        assert_eq!(o.run_query("select * from orders").await.unwrap().rows.len(), 0);

        let (ro, _) = open(Kind::My, &url, true).await.unwrap();
        assert!(ro.run_query("delete from customer").await.is_err());
        ro.close().await;
        o.run_script("drop table orders; drop table customer;").await.unwrap();
        o.close().await;
    }

    /// MySQL จริง: เห็น session ที่โดน lock ขวาง แล้ว kill ตัวที่ขวางได้
    #[tokio::test]
    #[ignore]
    async fn mysql_sessions_show_blocking() {
        let url = std::env::var("MARKDB_TEST_MYSQL").expect("ตั้ง MARKDB_TEST_MYSQL ก่อน");
        let (o, _) = open(Kind::My, &url, false).await.unwrap();
        o.run_script("drop table if exists mdb_lock; create table mdb_lock (id int primary key, v int); insert into mdb_lock values (1, 0);")
            .await
            .unwrap();
        let (holder, _) = open(Kind::My, &url, false).await.unwrap();
        let Other::My(hp) = &holder else { unreachable!() };
        let mut tx = hp.begin().await.unwrap();
        (&mut *tx).execute("update mdb_lock set v = 1 where id = 1").await.unwrap();
        let (holder_pid,): (i64,) = sqlx::query_as("select cast(connection_id() as signed)").fetch_one(&mut *tx).await.unwrap();
        let (waiter, _) = open(Kind::My, &url, false).await.unwrap();
        let wait = tokio::spawn(async move { waiter.run_query("update mdb_lock set v = 2 where id = 1").await.map(|r| r.affected) });
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        let rows = o.sessions().await.unwrap();
        let blocked: Vec<&Value> = rows.iter().filter(|r| r["blocked_by"] != "").collect();
        assert_eq!(blocked.len(), 1, "{:?}", rows);
        assert_eq!(blocked[0]["blocked_by"], holder_pid.to_string());

        assert!(o.kill(holder_pid, true).await.unwrap());
        assert_eq!(wait.await.unwrap().unwrap(), 1);
        drop(tx);
        o.run_script("drop table mdb_lock;").await.unwrap();
        o.close().await;
    }

    #[test]
    fn names_split() {
        assert_eq!(split("`db`.`t`"), ("db".into(), "t".into()));
        assert_eq!(split("\"main\".\"a\"\"b\""), ("main".into(), "a\"b".into()));
        assert_eq!(split("`a.b`"), ("".into(), "a.b".into()));
        assert_eq!(split("x.y"), ("x".into(), "y".into()));
    }
}
