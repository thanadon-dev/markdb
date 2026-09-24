//! Sync ตารางจาก connection หนึ่งไปอีกอัน (เช่น develop → uat) ให้เหมือนกันทั้งโครงสร้างและข้อมูล
//!
//! ดึงทั้งตารางจากสองฝั่งมาเทียบด้วย primary key ในแอป ได้รายการแถวที่ต้องเพิ่ม / แก้ / ลบ
//! แล้วรันเฉพาะส่วนที่ต่างเข้าปลายทาง ทุกตารางอยู่ใน transaction เดียว
// ponytail: โหลดทั้งตารางทั้งสองฝั่งเข้า memory — หลักแสนแถวไหว ถ้าหลักล้านให้เปลี่ยนเป็น
// เทียบ md5 ต่อแถวก่อน แล้วค่อยดึงเฉพาะแถวที่ต่าง
// เทียบโครงสร้างแค่คอลัมน์ (type / default / not null / generated) กับ primary key
// ไม่เทียบ index, foreign key, trigger, identity — ตารางที่ไม่มี PK และ Redshift ยังไม่รองรับ

use super::{db, err, ident, lit, where_keys, AppState, Engine, KeyVal, R};
use serde::{Deserialize, Serialize};
use sqlx::postgres::{PgConnection, PgPool};
use sqlx::{Executor, Row};
use std::collections::{HashMap, HashSet};
use std::io::Write;

/// ค่าของ session ที่มีผลกับรูปแบบข้อความของค่า — ตั้งให้เหมือนกันทั้งสองฝั่ง
/// ไม่งั้น server ที่ตั้ง timezone ต่างกันจะเห็น timestamptz ทุกแถวว่า "ต่าง"
/// ใช้ SET LOCAL ให้หมดอายุพร้อม transaction ไม่ค้างไปกระทบ query อื่นใน pool
const SESSION: [&str; 5] = [
    "set local timezone = 'UTC'",
    "set local datestyle = 'ISO, YMD'",
    "set local intervalstyle = 'postgres'",
    "set local extra_float_digits = 3",
    "set local bytea_output = 'hex'",
];

/// แถวที่ส่งเข้าปลายทางต่อหนึ่ง statement
const BATCH: usize = 200;
/// ตัวอย่างแถวที่ต่างต่อชนิด (เพิ่ม / แก้ / ลบ) ที่ส่งไปให้ preview
const SAMPLE: usize = 20;

#[derive(Deserialize, Clone)]
pub struct Tbl {
    schema: String,
    name: String,
}

impl Tbl {
    fn qn(&self) -> String {
        format!("{}.{}", ident(&self.schema), ident(&self.name))
    }
    fn id(&self) -> String {
        format!("{}.{}", self.schema, self.name)
    }
}

#[derive(Clone, PartialEq, Debug)]
struct Col {
    name: String,
    ty: String,
    notnull: bool,
    default: String,
    generated: bool,
    identity: bool,
}

/// constraint ของตาราง: (ชื่อ, นิยาม, คอลัมน์)
type Con = (String, String, Vec<String>);

struct Meta {
    cols: Vec<Col>,
    pk: Option<Con>,
    /// unique / check — ใช้ตอนสร้างตารางใหม่เท่านั้น
    others: Vec<Con>,
}

type Rows = Vec<(String, Vec<Option<String>>)>;

#[derive(Serialize, Default)]
pub struct Plan {
    schema: String,
    name: String,
    /// โครงสร้างที่ต่างกัน (ว่าง = ตรงกัน)
    diffs: Vec<String>,
    /// SQL ที่จะรันถ้าเลือกให้ ALTER ปลายทางให้เหมือนต้นทาง
    ddl: Vec<String>,
    /// ตารางนี้ sync ไม่ได้เลย เช่นไม่มี primary key
    error: Option<String>,
    add: usize,
    change: usize,
    del: usize,
    sample: Vec<Sample>,
    /// ตอน apply: ตารางนี้ถูก sync จริง (false = ข้าม)
    done: bool,
}

#[derive(Serialize)]
struct Sample {
    op: char,
    key: String,
    /// (คอลัมน์, ค่าเดิมในปลายทาง, ค่าใหม่จากต้นทาง)
    cells: Vec<(String, Option<String>, Option<String>)>,
}

#[derive(Default)]
struct Built {
    plan: Plan,
    pre: Vec<String>,
    post: Vec<String>,
    dels: Vec<String>,
    ups: Vec<String>,
    ins: Vec<String>,
    /// ข้อมูลเดิมทั้งตารางในปลายทาง สำหรับไฟล์สำรอง
    backup: Vec<String>,
    existed: bool,
}

async fn session(c: &mut PgConnection) -> R<()> {
    for s in SESSION {
        (&mut *c).execute(s).await.map_err(err)?;
    }
    Ok(())
}

async fn meta(c: &mut PgConnection, t: &Tbl) -> R<Option<Meta>> {
    let cols: Vec<(String, String, bool, String, bool, bool)> = sqlx::query_as(
        "select a.attname::text, format_type(a.atttypid, a.atttypmod), a.attnotnull,
                coalesce(pg_get_expr(d.adbin, d.adrelid), ''),
                a.attgenerated::text <> '', a.attidentity::text <> ''
         from pg_attribute a
         join pg_class c on c.oid = a.attrelid
         join pg_namespace n on n.oid = c.relnamespace
         left join pg_attrdef d on d.adrelid = a.attrelid and d.adnum = a.attnum
         where n.nspname = $1 and c.relname = $2 and c.relkind in ('r', 'p')
           and a.attnum > 0 and not a.attisdropped
         order by a.attnum",
    )
    .bind(&t.schema)
    .bind(&t.name)
    .fetch_all(&mut *c)
    .await
    .map_err(err)?;
    if cols.is_empty() {
        return Ok(None);
    }
    let cons: Vec<(String, String, String, Vec<String>)> = sqlx::query_as(
        "select con.conname::text, con.contype::text, pg_get_constraintdef(con.oid),
                array(select a.attname::text
                      from unnest(con.conkey) with ordinality k(num, pos)
                      join pg_attribute a on a.attrelid = con.conrelid and a.attnum = k.num
                      order by k.pos)
         from pg_constraint con
         join pg_class c on c.oid = con.conrelid
         join pg_namespace n on n.oid = c.relnamespace
         where n.nspname = $1 and c.relname = $2 and con.contype in ('p', 'u', 'c')
         order by con.contype, con.conname",
    )
    .bind(&t.schema)
    .bind(&t.name)
    .fetch_all(&mut *c)
    .await
    .map_err(err)?;

    let mut m = Meta {
        cols: cols
            .into_iter()
            .map(|(name, ty, notnull, default, generated, identity)| Col {
                name,
                ty,
                notnull,
                default,
                generated,
                identity,
            })
            .collect(),
        pk: None,
        others: Vec::new(),
    };
    for (name, kind, def, cols) in cons {
        if kind == "p" {
            m.pk = Some((name, def, cols));
        } else {
            m.others.push((name, def, cols));
        }
    }
    Ok(Some(m))
}

/// นิยามคอลัมน์สำหรับ CREATE / ADD COLUMN — ไม่ใส่ NOT NULL (ใส่ทีหลังตอนข้อมูลลงครบแล้ว)
fn col_def(c: &Col) -> String {
    let mut s = format!("{} {}", ident(&c.name), c.ty);
    if c.generated {
        s.push_str(&format!(" generated always as ({}) stored", c.default));
    } else if c.identity {
        s.push_str(" generated by default as identity");
    } else if !c.default.is_empty() {
        s.push_str(&format!(" default {}", c.default));
    }
    s
}

/// ชื่อ sequence ใน default แบบ nextval('ชื่อ'::regclass) — ข้อความข้างในเป็นชื่อที่ SQL อ่านได้อยู่แล้ว
fn seq_of(default: &str) -> Option<&str> {
    default.strip_prefix("nextval('")?.strip_suffix("'::regclass)")
}

#[derive(Default, Debug)]
struct Ddl {
    diffs: Vec<String>,
    /// รันก่อนย้ายข้อมูล
    pre: Vec<String>,
    /// รันหลังย้ายข้อมูล (NOT NULL / PK ที่ต้องรอให้ข้อมูลครบก่อน)
    post: Vec<String>,
}

/// เทียบโครงสร้าง แล้วคืน SQL ที่ทำให้ปลายทางเหมือนต้นทาง
fn ddl_diff(t: &Tbl, src: &Meta, dst: Option<&Meta>) -> Ddl {
    let qn = t.qn();
    let mut d = Ddl::default();
    let alter = |s: String| format!("alter table {} {}", qn, s);
    let set_nn = |c: &Col| alter(format!("alter column {} set not null", ident(&c.name)));
    // default ที่เป็น nextval ต้องมี sequence ในปลายทางก่อน แล้วผูกกับคอลัมน์ให้ setval หาเจอ
    let seq = |d: &mut Ddl, c: &Col| {
        if let Some(s) = seq_of(&c.default) {
            d.pre.insert(0, format!("create sequence if not exists {}", s));
            d.post.push(format!("alter sequence {} owned by {}.{}", s, qn, ident(&c.name)));
        }
    };

    let Some(dst) = dst else {
        d.diffs.push("ไม่มีตารางนี้ในปลายทาง — จะสร้างใหม่".into());
        d.pre.push(format!(
            "create table {} (\n  {}\n)",
            qn,
            src.cols.iter().map(col_def).collect::<Vec<_>>().join(",\n  ")
        ));
        d.post.extend(src.cols.iter().filter(|c| c.notnull).map(set_nn));
        for c in &src.cols {
            seq(&mut d, c);
        }
        d.pre.insert(0, format!("create schema if not exists {}", ident(&t.schema)));
        for (name, def, _) in src.pk.iter().chain(&src.others) {
            d.post.push(alter(format!("add constraint {} {}", ident(name), def)));
        }
        return d;
    };

    for s in &src.cols {
        let col = ident(&s.name);
        match dst.cols.iter().find(|c| c.name == s.name) {
            None => {
                d.diffs.push(format!("ไม่มีคอลัมน์ {} ({})", s.name, s.ty));
                d.pre.push(alter(format!("add column {}", col_def(s))));
                seq(&mut d, s);
                if s.notnull {
                    d.post.push(set_nn(s));
                }
            }
            // generated ต่างกัน ALTER ตรง ๆ ไม่ได้ — ลบแล้วเพิ่มใหม่ ค่าจะถูกคำนวณให้เอง
            Some(o) if o.generated != s.generated || (s.generated && o.default != s.default) => {
                d.diffs.push(format!("{}: generated column ไม่ตรงกัน", s.name));
                d.pre.push(alter(format!("drop column {}", col)));
                d.pre.push(alter(format!("add column {}", col_def(s))));
                if s.notnull {
                    d.post.push(set_nn(s));
                }
            }
            Some(o) => {
                if o.ty != s.ty {
                    d.diffs.push(format!("{}: type {} → {}", s.name, o.ty, s.ty));
                    d.pre.push(alter(format!(
                        "alter column {c} type {} using {c}::text::{}",
                        s.ty,
                        s.ty,
                        c = col
                    )));
                }
                if !s.generated && o.default != s.default {
                    let show = |v: &str| if v.is_empty() { "(ไม่มี)".to_string() } else { v.to_string() };
                    d.diffs.push(format!("{}: default {} → {}", s.name, show(&o.default), show(&s.default)));
                    seq(&mut d, s);
                    d.pre.push(alter(if s.default.is_empty() {
                        format!("alter column {} drop default", col)
                    } else {
                        format!("alter column {} set default {}", col, s.default)
                    }));
                }
                if o.notnull != s.notnull {
                    d.diffs.push(format!(
                        "{}: {}",
                        s.name,
                        if s.notnull { "ต้องเป็น NOT NULL" } else { "ต้องรับ NULL ได้" }
                    ));
                    if s.notnull {
                        d.post.push(set_nn(s));
                    } else {
                        d.pre.push(alter(format!("alter column {} drop not null", col)));
                    }
                }
            }
        }
    }
    for o in &dst.cols {
        if !src.cols.iter().any(|c| c.name == o.name) {
            d.diffs.push(format!("มีคอลัมน์ {} เกินมา — จะถูกลบพร้อมข้อมูลในคอลัมน์นั้น", o.name));
            d.pre.push(alter(format!("drop column {}", ident(&o.name))));
        }
    }

    let pk_cols = |m: &Meta| m.pk.as_ref().map(|p| p.2.clone()).unwrap_or_default();
    if pk_cols(src) != pk_cols(dst) {
        d.diffs.push(format!(
            "primary key ({}) → ({})",
            pk_cols(dst).join(", "),
            pk_cols(src).join(", ")
        ));
        if let Some((name, _, _)) = &dst.pk {
            d.pre.push(alter(format!("drop constraint {}", ident(name))));
        }
        if let Some((name, def, _)) = &src.pk {
            d.post.push(alter(format!("add constraint {} {}", ident(name), def)));
        }
    }
    d
}

/// ทั้งตาราง: (key, ค่าทุกคอลัมน์เป็นข้อความตามลำดับ cols)
/// cast เป็น text ฝั่ง server — ค่าที่ได้เอาไปเป็น literal เขียนกลับได้ทุก type
async fn load(c: &mut PgConnection, qn: &str, cols: &[String], key: &[usize]) -> R<Rows> {
    let sql = format!(
        "select {} from {}",
        cols.iter()
            .map(|c| format!("{}::text", ident(c)))
            .collect::<Vec<_>>()
            .join(", "),
        qn
    );
    let rows = sqlx::query(&sql).fetch_all(&mut *c).await.map_err(err)?;
    rows.iter()
        .map(|r| {
            let vals = (0..cols.len())
                .map(|i| r.try_get::<Option<String>, _>(i))
                .collect::<Result<Vec<_>, _>>()
                .map_err(err)?;
            Ok((key_of(&vals, key), vals))
        })
        .collect()
}

fn key_of(vals: &[Option<String>], key: &[usize]) -> String {
    key.iter()
        .map(|&i| vals[i].as_deref().unwrap_or("NULL"))
        .collect::<Vec<_>>()
        .join(", ")
}

struct Diff<'a> {
    /// แถวใหม่จากต้นทาง
    add: Vec<&'a Vec<Option<String>>>,
    /// (แถวจากต้นทาง, ค่าเดิมในปลายทางเรียงตามคอลัมน์ต้นทาง, index คอลัมน์ที่ต่าง)
    change: Vec<(&'a Vec<Option<String>>, Vec<Option<String>>, Vec<usize>)>,
    /// แถวที่มีแค่ในปลายทาง
    del: Vec<&'a Vec<Option<String>>>,
}

/// proj[i] = ตำแหน่งของคอลัมน์ต้นทาง i ในแถวปลายทาง (None = ปลายทางยังไม่มีคอลัมน์นี้)
fn diff_rows<'a>(src: &'a Rows, dst: &'a Rows, proj: &[Option<usize>]) -> R<Diff<'a>> {
    let mut by_key: HashMap<&str, &Vec<Option<String>>> = HashMap::with_capacity(dst.len());
    for (k, v) in dst {
        if by_key.insert(k, v).is_some() {
            return Err(format!("ปลายทางมีค่า key ซ้ำ ({}) — จับคู่แถวไม่ได้", k));
        }
    }
    let mut d = Diff { add: Vec::new(), change: Vec::new(), del: Vec::new() };
    let mut seen: HashSet<&str> = HashSet::with_capacity(src.len());
    for (k, new) in src {
        seen.insert(k);
        let Some(old) = by_key.get(k.as_str()) else {
            d.add.push(new);
            continue;
        };
        let old: Vec<Option<String>> = proj.iter().map(|j| j.and_then(|j| old[j].clone())).collect();
        // คอลัมน์ที่ปลายทางยังไม่มีนับว่าต่างเสมอ — หลัง ADD COLUMN จะได้ค่า default
        // ซึ่งอาจไม่ตรงกับต้นทาง (เช่นต้นทางเป็น NULL) จึงต้องเขียนทับทุกแถว
        let changed: Vec<usize> = (0..new.len())
            .filter(|&i| proj[i].is_none() || old[i] != new[i])
            .collect();
        if !changed.is_empty() {
            d.change.push((new, old, changed));
        }
    }
    d.del = dst
        .iter()
        .filter(|(k, _)| !seen.contains(k.as_str()))
        .map(|(_, v)| v)
        .collect();
    Ok(d)
}

fn inserts(qn: &str, cols: &[String], rows: &[&Vec<Option<String>>]) -> Vec<String> {
    let list = cols.iter().map(|c| ident(c)).collect::<Vec<_>>().join(", ");
    rows.chunks(BATCH)
        .map(|chunk| {
            format!(
                "insert into {} ({}) overriding system value values\n{}",
                qn,
                list,
                chunk
                    .iter()
                    .map(|r| format!("({})", r.iter().map(lit).collect::<Vec<_>>().join(", ")))
                    .collect::<Vec<_>>()
                    .join(",\n")
            )
        })
        .collect()
}

fn keys_of(cols: &[String], key: &[usize], row: &[Option<String>]) -> Vec<KeyVal> {
    key.iter()
        .map(|&i| KeyVal { column: cols[i].clone(), value: row[i].clone() })
        .collect()
}

async fn build(sc: &mut PgConnection, dc: &mut PgConnection, t: &Tbl) -> R<Built> {
    let qn = t.qn();
    let mut b = Built::default();
    b.plan.schema = t.schema.clone();
    b.plan.name = t.name.clone();

    let Some(sm) = meta(sc, t).await? else {
        b.plan.error = Some("ไม่มีตารางนี้ในต้นทาง".into());
        return Ok(b);
    };
    let Some((_, _, pk)) = sm.pk.clone() else {
        b.plan.error = Some("ต้นทางไม่มี primary key — ยังไม่รองรับ".into());
        return Ok(b);
    };
    let dm = meta(dc, t).await?;
    b.existed = dm.is_some();

    let ddl = ddl_diff(t, &sm, dm.as_ref());
    b.plan.diffs = ddl.diffs;
    b.plan.ddl = ddl.pre.iter().chain(&ddl.post).cloned().collect();
    b.pre = ddl.pre;
    b.post = ddl.post;

    // generated column เขียนค่าเองไม่ได้ ปล่อยให้ database คำนวณ
    let cols: Vec<String> = sm.cols.iter().filter(|c| !c.generated).map(|c| c.name.clone()).collect();
    let pos = |cols: &[String], k: &String| cols.iter().position(|c| c == k);
    let Some(key) = pk.iter().map(|k| pos(&cols, k)).collect::<Option<Vec<_>>>() else {
        b.plan.error = Some("primary key เป็น generated column — ยังไม่รองรับ".into());
        return Ok(b);
    };
    let src = load(sc, &qn, &cols, &key).await?;

    let (dcols, dst) = match &dm {
        None => (Vec::new(), Vec::new()),
        Some(m) => {
            let dcols: Vec<String> = m.cols.iter().filter(|c| !c.generated).map(|c| c.name.clone()).collect();
            let Some(dkey) = pk.iter().map(|k| pos(&dcols, k)).collect::<Option<Vec<_>>>() else {
                b.plan.error = Some(format!(
                    "ปลายทางไม่มีคอลัมน์ primary key ({}) — จับคู่แถวไม่ได้",
                    pk.join(", ")
                ));
                return Ok(b);
            };
            let rows = load(dc, &qn, &dcols, &dkey).await?;
            (dcols, rows)
        }
    };
    let proj: Vec<Option<usize>> = cols.iter().map(|c| pos(&dcols, c)).collect();
    let d = match diff_rows(&src, &dst, &proj) {
        Ok(d) => d,
        Err(e) => {
            b.plan.error = Some(e);
            return Ok(b);
        }
    };

    b.plan.add = d.add.len();
    b.plan.change = d.change.len();
    b.plan.del = d.del.len();
    for r in d.add.iter().take(SAMPLE) {
        b.plan.sample.push(Sample {
            op: '+',
            key: key_of(r, &key),
            cells: cols.iter().zip(r.iter()).map(|(c, v)| (c.clone(), None, v.clone())).collect(),
        });
    }
    for (new, old, changed) in d.change.iter().take(SAMPLE) {
        b.plan.sample.push(Sample {
            op: '~',
            key: key_of(new, &key),
            cells: changed
                .iter()
                .map(|&i| (cols[i].clone(), old[i].clone(), new[i].clone()))
                .collect(),
        });
    }
    let dkey: Vec<usize> = pk.iter().filter_map(|k| pos(&dcols, k)).collect();
    for r in d.del.iter().take(SAMPLE) {
        b.plan.sample.push(Sample { op: '-', key: key_of(r, &dkey), cells: Vec::new() });
    }

    b.ins = inserts(&qn, &cols, &d.add);
    b.ups = d
        .change
        .iter()
        .map(|(new, _, changed)| {
            format!(
                "update {} set {} where {}",
                qn,
                changed
                    .iter()
                    .map(|&i| format!("{} = {}", ident(&cols[i]), lit(&new[i])))
                    .collect::<Vec<_>>()
                    .join(", "),
                where_keys(&keys_of(&cols, &key, new))
            )
        })
        .collect();
    b.dels = d
        .del
        .iter()
        .map(|r| format!("delete from {} where {}", qn, where_keys(&keys_of(&dcols, &dkey, r))))
        .collect();
    if b.existed {
        b.backup = inserts(&qn, &dcols, &dst.iter().map(|(_, v)| v).collect::<Vec<_>>());
    }

    // ตั้ง sequence ให้ต่อจากค่าสูงสุดที่เพิ่งลง ไม่งั้น insert แถวใหม่ในปลายทางจะได้ id ชนของเดิม
    // pg_get_serial_sequence คืน NULL ถ้าคอลัมน์ไม่ได้เป็นเจ้าของ sequence — setval(NULL) ไม่ทำอะไร
    for c in sm.cols.iter().filter(|c| c.identity || c.default.contains("nextval(")) {
        b.post.push(format!(
            "select setval(pg_get_serial_sequence('{}', '{}'), coalesce(max({}), 0) + 1, false) from {}",
            qn.replace('\'', "''"),
            c.name.replace('\'', "''"),
            ident(&c.name),
            qn
        ));
    }
    Ok(b)
}

/// FK ระหว่างตาราง: (ลูก, แม่) เป็นชื่อแบบ schema.table
async fn fk_edges(c: &mut PgConnection) -> R<Vec<(String, String)>> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "select n1.nspname || '.' || c1.relname, n2.nspname || '.' || c2.relname
         from pg_constraint con
         join pg_class c1 on c1.oid = con.conrelid
         join pg_namespace n1 on n1.oid = c1.relnamespace
         join pg_class c2 on c2.oid = con.confrelid
         join pg_namespace n2 on n2.oid = c2.relnamespace
         where con.contype = 'f'",
    )
    .fetch_all(&mut *c)
    .await
    .map_err(err)?;
    Ok(rows)
}

/// เรียงให้ตารางแม่มาก่อนตารางลูก (insert ตามลำดับนี้ ลบย้อนกลับ)
/// FK ที่วนกลับหากันหรือชี้ตัวเองตัดทิ้ง ไม่งั้นเรียงไม่จบ
fn fk_order(ids: &[String], edges: &[(String, String)]) -> Vec<usize> {
    fn visit(i: usize, ids: &[String], edges: &[(String, String)], state: &mut [u8], out: &mut Vec<usize>) {
        if state[i] != 0 {
            return;
        }
        state[i] = 1;
        for (child, parent) in edges {
            if *child == ids[i] && parent != child {
                if let Some(p) = ids.iter().position(|x| x == parent) {
                    visit(p, ids, edges, state, out);
                }
            }
        }
        state[i] = 2;
        out.push(i);
    }
    let mut state = vec![0u8; ids.len()];
    let mut out = Vec::with_capacity(ids.len());
    for i in 0..ids.len() {
        visit(i, ids, edges, &mut state, &mut out);
    }
    out
}

/// อ่านทั้งสองฝั่งแล้วประกอบแผนของทุกตาราง เรียงแม่ก่อนลูก
/// dst ส่งเป็น connection ที่อยู่ใน transaction มา — apply จะเขียนต่อใน transaction เดียวกัน
async fn build_all(sp: &PgPool, dc: &mut PgConnection, tables: &[Tbl]) -> R<Vec<Built>> {
    let mut stx = sp.begin().await.map_err(err)?;
    session(&mut stx).await?;
    let ids: Vec<String> = tables.iter().map(Tbl::id).collect();
    let edges = fk_edges(&mut stx).await?;
    let mut out = Vec::with_capacity(tables.len());
    for i in fk_order(&ids, &edges) {
        out.push(build(&mut stx, dc, &tables[i]).await?);
    }
    stx.rollback().await.map_err(err)?;
    Ok(out)
}

async fn plan_core(sp: &PgPool, dp: &PgPool, tables: &[Tbl]) -> R<Vec<Plan>> {
    let mut dtx = dp.begin().await.map_err(err)?;
    session(&mut dtx).await?;
    let built = build_all(sp, &mut dtx, tables).await?;
    dtx.rollback().await.map_err(err)?;
    Ok(built.into_iter().map(|b| b.plan).collect())
}

async fn run(c: &mut PgConnection, table: &str, stmts: &[String]) -> R<()> {
    // หลาย statement ต่อรอบ ลดจำนวน round-trip — ไม่มี parameter จึงไปทาง simple query
    for chunk in stmts.chunks(50) {
        (&mut *c)
            .execute(chunk.join(";\n").as_str())
            .await
            .map_err(|e| format!("{}: {}", table, e))?;
    }
    Ok(())
}

async fn apply_core(
    sp: &PgPool,
    dp: &PgPool,
    tables: &[Tbl],
    alter: &HashSet<String>,
    delete: bool,
    backup: &str,
) -> R<Vec<Plan>> {
    let mut dtx = dp.begin().await.map_err(err)?;
    session(&mut dtx).await?;
    // อ่านใหม่ใน transaction ที่จะเขียน — ไม่เชื่อ preview ที่อาจเก่าไปแล้ว
    let mut built = build_all(sp, &mut dtx, tables).await?;
    for b in &mut built {
        let id = format!("{}.{}", b.plan.schema, b.plan.name);
        b.plan.done = b.plan.error.is_none() && (b.plan.diffs.is_empty() || alter.contains(&id));
        if !delete {
            b.dels.clear();
        }
    }
    let go: Vec<&Built> = built.iter().filter(|b| b.plan.done).collect();
    if go.is_empty() {
        return Err("ไม่มีตารางที่ sync ได้ — ทุกตารางถูกข้าม".into());
    }

    // สำรองข้อมูลเดิมในปลายทางก่อนแตะอะไร: ลบลูกก่อนแม่ แล้ว insert แม่ก่อนลูก
    let mut f = std::io::BufWriter::new(std::fs::File::create(backup).map_err(err)?);
    let mut w = |s: String| writeln!(f, "{}", s).map_err(err);
    w("-- MarkDB: ข้อมูลในปลายทางก่อน sync — คืนค่าได้ด้วยปุ่ม Restore".into())?;
    w("-- ถ้า sync ได้แก้โครงสร้างไปด้วย (เช่นลบ/เพิ่มคอลัมน์) ต้องแก้โครงสร้างกลับก่อน restore".into())?;
    w("SET client_encoding = 'UTF8';\nSET datestyle = 'ISO, YMD';\nSET intervalstyle = 'postgres';\n".into())?;
    for b in go.iter().filter(|b| !b.plan.ddl.is_empty()) {
        w(format!("-- {}.{} ถูกแก้โครงสร้างด้วย:", b.plan.schema, b.plan.name))?;
        for s in &b.plan.ddl {
            w(format!("--   {}", s.replace('\n', " ")))?;
        }
    }
    for b in go.iter().rev().filter(|b| b.existed) {
        w(format!("\nDELETE FROM {}.{};", ident(&b.plan.schema), ident(&b.plan.name)))?;
    }
    for b in go.iter().filter(|b| b.existed) {
        for s in &b.backup {
            w(format!("{};", s))?;
        }
    }
    drop(w);
    f.flush().map_err(err)?;
    drop(f);

    // โครงสร้างก่อน → ลบ (ลูกก่อนแม่) → แก้และเพิ่ม (แม่ก่อนลูก) → NOT NULL / PK / sequence
    let name = |b: &Built| format!("{}.{}", b.plan.schema, b.plan.name);
    for b in &go {
        run(&mut dtx, &name(b), &b.pre).await?;
    }
    for b in go.iter().rev() {
        run(&mut dtx, &name(b), &b.dels).await?;
    }
    for b in &go {
        run(&mut dtx, &name(b), &b.ups).await?;
        run(&mut dtx, &name(b), &b.ins).await?;
    }
    for b in &go {
        run(&mut dtx, &name(b), &b.post).await?;
    }
    dtx.commit().await.map_err(err)?;
    Ok(built.into_iter().map(|b| b.plan).collect())
}

async fn pools(state: &AppState, src: &str, dst: &str) -> R<(PgPool, PgPool)> {
    if src == dst {
        return Err("ต้นทางกับปลายทางเป็น connection เดียวกัน".into());
    }
    let (s, d) = (db(state, src).await?, db(state, dst).await?);
    if s.engine == Engine::Redshift || d.engine == Engine::Redshift {
        return Err("Sync ยังไม่รองรับ Redshift".into());
    }
    Ok((s.pool, d.pool))
}

/// preview: เทียบโครงสร้างและข้อมูล ไม่เขียนอะไรลงปลายทาง
#[tauri::command]
pub async fn sync_plan(
    src: String,
    dst: String,
    tables: Vec<Tbl>,
    state: tauri::State<'_, AppState>,
) -> R<Vec<Plan>> {
    let (sp, dp) = pools(&state, &src, &dst).await?;
    plan_core(&sp, &dp, &tables).await
}

/// sync จริง — alter = ตาราง (schema.table) ที่โครงสร้างต่างแล้วยอมให้ ALTER ปลายทาง
/// ตารางที่โครงสร้างต่างแต่ไม่อยู่ใน alter จะถูกข้าม · delete = ลบแถวที่มีแค่ในปลายทาง
/// เขียนไฟล์สำรองลง backup ก่อนเสมอ แล้วทุกตารางรันใน transaction เดียว
#[tauri::command]
pub async fn sync_apply(
    src: String,
    dst: String,
    tables: Vec<Tbl>,
    alter: Vec<String>,
    delete: bool,
    backup: String,
    state: tauri::State<'_, AppState>,
) -> R<Vec<Plan>> {
    let (sp, dp) = pools(&state, &src, &dst).await?;
    apply_core(&sp, &dp, &tables, &alter.into_iter().collect(), delete, &backup).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parents_before_children() {
        let ids = s(&["a.child", "a.grand", "a.parent", "a.self"]);
        let edges = vec![
            ("a.child".to_string(), "a.parent".to_string()),
            ("a.grand".to_string(), "a.child".to_string()),
            ("a.self".to_string(), "a.self".to_string()),
            ("a.child".to_string(), "a.not_selected".to_string()),
        ];
        let order: Vec<&str> = fk_order(&ids, &edges).iter().map(|&i| ids[i].as_str()).collect();
        assert_eq!(order, ["a.parent", "a.child", "a.grand", "a.self"]);
        // วนกลับหากันต้องจบได้ ไม่ค้าง
        let cyc = vec![
            ("a.child".to_string(), "a.parent".to_string()),
            ("a.parent".to_string(), "a.child".to_string()),
        ];
        assert_eq!(fk_order(&s(&["a.child", "a.parent"]), &cyc).len(), 2);
    }

    #[test]
    fn row_diff() {
        let row = |v: &[Option<&str>]| v.iter().map(|x| x.map(String::from)).collect::<Vec<_>>();
        let src: Rows = vec![
            ("1".into(), row(&[Some("1"), Some("a")])),
            ("2".into(), row(&[Some("2"), Some("b")])),
            ("3".into(), row(&[Some("3"), None])),
        ];
        let dst: Rows = vec![
            ("1".into(), row(&[Some("1"), Some("a")])),
            ("2".into(), row(&[Some("2"), Some("x")])),
            ("9".into(), row(&[Some("9"), Some("z")])),
        ];
        let d = diff_rows(&src, &dst, &[Some(0), Some(1)]).unwrap();
        assert_eq!(d.add.len(), 1);
        assert_eq!(d.change.len(), 1);
        assert_eq!(d.change[0].2, vec![1]);
        assert_eq!(d.del[0][0].as_deref(), Some("9"));
        // คอลัมน์ที่ปลายทางยังไม่มี → ทุกแถวที่ตรงกันต้องถูกเขียนทับ
        let d = diff_rows(&src, &dst, &[Some(0), None]).unwrap();
        assert_eq!(d.change.len(), 2);
        // key ซ้ำในปลายทางต้องไม่เดา
        let dup: Rows = vec![("1".into(), row(&[Some("1")])), ("1".into(), row(&[Some("1")]))];
        assert!(diff_rows(&src, &dup, &[Some(0), None]).is_err());
    }

    /// รันกับ Postgres จริง: MARKDB_TEST_PG=postgres://user:pass@host:port cargo test -- --ignored
    /// สร้าง database markdb_sync_dev / markdb_sync_uat ใหม่ทุกรอบ
    #[tokio::test]
    #[ignore]
    async fn sync_end_to_end() {
        let base = std::env::var("MARKDB_TEST_PG").expect("ตั้ง MARKDB_TEST_PG ก่อน");
        let admin = PgPool::connect(&format!("{}/postgres", base)).await.unwrap();
        for d in ["markdb_sync_dev", "markdb_sync_uat"] {
            admin.execute(format!("drop database if exists {} with (force)", d).as_str()).await.unwrap();
            admin.execute(format!("create database {}", d).as_str()).await.unwrap();
        }
        let dev = PgPool::connect(&format!("{}/markdb_sync_dev", base)).await.unwrap();
        let uat = PgPool::connect(&format!("{}/markdb_sync_uat", base)).await.unwrap();

        dev.execute(
            r#"
            create table "Meter_Configuration" ("Meter_ID" int primary key, "IsActive" bool not null default false,
                "IsDelete" bool, "CC" numeric(12,2), "Note" text, "Tags" text[], "At" timestamptz);
            insert into "Meter_Configuration" values
                (234, true, false, 3800, 'o''brien', '{a,b}', '2024-01-01 07:00+07'),
                (145, false, true, 30000, null, null, null),
                (314, true, false, 0, 'new row', '{}', now());
            create table parent (id serial primary key, name text not null);
            create table child (id int primary key, parent_id int references parent, v text);
            insert into parent (name) values ('p1'), ('p2');
            insert into child values (1, 1, 'c1'), (2, 2, 'c2');
            create table only_dev (id bigserial primary key, x int unique, y int check (y > 0));
            insert into only_dev (x, y) values (1, 1), (2, 2);
            create table nopk (x int);
            "#,
        )
        .await
        .unwrap();
        uat.execute(
            r#"
            set timezone = 'Asia/Bangkok';
            create table "Meter_Configuration" ("Meter_ID" int primary key, "IsActive" bool,
                "IsDelete" bool, "CC" int, "Old" text, "Tags" text[], "At" timestamptz);
            insert into "Meter_Configuration" values
                (234, false, false, 3800, 'x', '{a,b}', '2024-01-01 00:00+00'),
                (145, false, false, 30000, 'y', null, null),
                (999, false, false, 1, 'gone', null, null);
            create table parent (id serial primary key, name text not null);
            create table child (id int primary key, parent_id int references parent, v text);
            insert into parent (id, name) values (1, 'p1'), (7, 'p7');
            insert into child values (1, 1, 'c1'), (8, 7, 'c8');
            select setval('parent_id_seq', 7);
            create table nopk (x int);
            "#,
        )
        .await
        .unwrap();

        let t = |n: &str| Tbl { schema: "public".into(), name: n.into() };
        let tables = vec![t("child"), t("Meter_Configuration"), t("parent"), t("only_dev"), t("nopk")];
        let plans = plan_core(&dev, &uat, &tables).await.unwrap();
        let p = |n: &str| plans.iter().find(|p| p.name == n).unwrap();

        let m = p("Meter_Configuration");
        assert!(m.error.is_none(), "{:?}", m.error);
        // CC int → numeric, IsActive nullable → not null + default, Note เพิ่ม, Old ลบ
        assert_eq!(m.diffs.len(), 5, "{:#?}", m.diffs);
        // timestamptz เดียวกันคนละ timezone ต้องไม่นับว่าต่าง: 234 ต่างเพราะ IsActive / Note ไม่ใช่ At
        let s234 = m.sample.iter().find(|s| s.key == "234").unwrap();
        assert!(s234.cells.iter().all(|c| c.0 != "At"), "{:?}", s234.cells);
        assert_eq!((m.add, m.change, m.del), (1, 2, 1));
        assert!(p("parent").diffs.is_empty());
        assert_eq!((p("parent").add, p("parent").change, p("parent").del), (1, 0, 1));
        assert_eq!((p("child").add, p("child").del), (1, 1));
        assert!(p("only_dev").diffs[0].contains("ไม่มีตารางนี้"));
        assert!(p("nopk").error.is_some());

        // ไม่ยอม alter Meter_Configuration → ต้องข้าม และไม่แตะข้อมูลของมัน
        let dir = std::env::temp_dir();
        let bk1 = dir.join("markdb_sync_bk1.sql");
        let alter: HashSet<String> = ["public.only_dev".to_string()].into();
        let done = apply_core(&dev, &uat, &tables, &alter, true, bk1.to_str().unwrap()).await.unwrap();
        let d = |n: &str| done.iter().find(|p| p.name == n).unwrap().done;
        assert!(!d("Meter_Configuration") && !d("nopk"));
        assert!(d("parent") && d("child") && d("only_dev"));
        let old: (Option<String>,) = sqlx::query_as(r#"select "Old" from "Meter_Configuration" where "Meter_ID" = 999"#)
            .fetch_one(&uat)
            .await
            .unwrap();
        assert_eq!(old.0.as_deref(), Some("gone"));

        // ยอม alter แล้วรอบนี้ต้องเหมือนกันทั้งหมด
        let bk2 = dir.join("markdb_sync_bk2.sql");
        let alter: HashSet<String> = ["public.Meter_Configuration".to_string()].into();
        apply_core(&dev, &uat, &tables, &alter, true, bk2.to_str().unwrap()).await.unwrap();
        let again = plan_core(&dev, &uat, &tables).await.unwrap();
        for p in again.iter().filter(|p| p.error.is_none()) {
            assert!(p.diffs.is_empty(), "{}: {:?}", p.name, p.diffs);
            assert_eq!((p.add, p.change, p.del), (0, 0, 0), "{}", p.name);
        }

        // sequence ต้องต่อจากค่าสูงสุด — insert แถวใหม่ต้องไม่ชน
        uat.execute("insert into parent (name) values ('after sync')").await.unwrap();
        uat.execute("insert into only_dev (x, y) values (3, 3)").await.unwrap();
        // constraint ของตารางที่สร้างใหม่ต้องมาด้วย
        assert!(uat.execute("insert into only_dev (x, y) values (4, -1)").await.is_err());

        // ไฟล์สำรองรอบแรกต้องมีข้อมูลเดิมของ parent (แถว p7) และ restore กลับได้
        let bk = std::fs::read_to_string(&bk1).unwrap();
        assert!(bk.contains("'p7'"), "{}", bk);
        assert!(!bk.contains("Meter_Configuration"), "ตารางที่ถูกข้ามไม่ต้องสำรอง");
        uat.execute("delete from child; delete from parent").await.unwrap();
        sqlx::raw_sql(&format!("BEGIN;\n{}\nCOMMIT;", bk)).execute(&uat).await.unwrap();
        let n: (i64,) = sqlx::query_as("select count(*) from parent where name = 'p7'")
            .fetch_one(&uat)
            .await
            .unwrap();
        assert_eq!(n.0, 1);
    }
}
