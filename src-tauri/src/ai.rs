//! AI เขียน SQL ผ่าน `claude` CLI (Claude Code) ที่ติดตั้งและ login ไว้ในเครื่องผู้ใช้
//! ใช้บัญชี Claude ของผู้ใช้เอง — แอปไม่ต้องถือ API key
//!
//! รัน `claude -p` แบบ stream-json แล้วส่งข้อความทีละก้อนเป็น event "ai-delta" ให้ UI พิมพ์ลง editor
//! ปิด tools / MCP / settings ของผู้ใช้ (hook, plugin) — งานนี้ตอบข้อความอย่างเดียว เร็วกว่าและไม่ไปแตะเครื่อง

use super::{err, R};
use tauri::{AppHandle, Emitter, State};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

/// ตัวที่กำลังรันอยู่ (ทีละตัว) — เก็บไว้ให้ ai_cancel หยุดได้
#[derive(Default)]
pub struct Ai {
    child: Mutex<Option<Child>>,
}

#[derive(Debug, PartialEq)]
enum Line {
    Delta(String),
    Done(Result<String, String>),
    Other,
}

/// แปลงหนึ่งบรรทัดของ `--output-format stream-json`
fn parse(line: &str) -> Line {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return Line::Other;
    };
    match v["type"].as_str() {
        Some("stream_event")
            if v["event"]["type"] == "content_block_delta" && v["event"]["delta"]["type"] == "text_delta" =>
        {
            Line::Delta(v["event"]["delta"]["text"].as_str().unwrap_or("").to_string())
        }
        Some("result") => {
            let text = v["result"].as_str().unwrap_or("").to_string();
            if v["is_error"].as_bool() == Some(true) || v["subtype"] != "success" {
                Line::Done(Err(if text.is_empty() { format!("{}", v["subtype"]) } else { text }))
            } else {
                Line::Done(Ok(text))
            }
        }
        _ => Line::Other,
    }
}

#[tauri::command]
pub async fn ai_sql(
    system: String,
    prompt: String,
    model: String,
    effort: String,
    app: AppHandle,
    ai: State<'_, Ai>,
) -> R<String> {
    let mut cmd = Command::new("claude");
    cmd.args([
        "-p",
        "--model",
        &model,
        "--effort",
        &effort,
        "--output-format",
        "stream-json",
        "--include-partial-messages",
        "--verbose",
        "--tools",
        "",
        "--setting-sources",
        "",
        "--strict-mcp-config",
        "--no-session-persistence",
        "--system-prompt",
        &system,
    ])
    // รันใน temp — ไม่ให้ไปหยิบ CLAUDE.md ของโฟลเดอร์ไหนมาปน
    .current_dir(std::env::temp_dir())
    .stdin(std::process::Stdio::piped())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    .kill_on_drop(true);
    // แอป GUI เรียกโปรแกรม console บน Windows จะมีหน้าต่างดำเด้งขึ้นมา — ซ่อนไว้
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);

    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "ไม่พบคำสั่ง claude — ติดตั้ง Claude Code แล้วพิมพ์ claude ใน terminal เพื่อ login ก่อน".to_string()
        } else {
            err(e)
        }
    })?;
    // prompt ส่งทาง stdin ไม่ใช่ argument — schema ใหญ่ ๆ จะไม่ชนเพดานความยาว command line ของ Windows
    let mut stdin = child.stdin.take().ok_or("เปิด stdin ไม่ได้")?;
    let stdout = child.stdout.take().ok_or("เปิด stdout ไม่ได้")?;
    let mut stderr = child.stderr.take().ok_or("เปิด stderr ไม่ได้")?;
    *ai.child.lock().await = Some(child);

    stdin.write_all(prompt.as_bytes()).await.map_err(err)?;
    drop(stdin);
    // อ่าน stderr คู่ขนาน ไม่งั้นถ้ามันเขียนเยอะจน buffer เต็ม ตัว CLI จะค้างรอ
    let errs = tokio::spawn(async move {
        let mut s = String::new();
        stderr.read_to_string(&mut s).await.ok();
        s
    });

    let mut lines = BufReader::new(stdout).lines();
    let mut done = None;
    while let Some(line) = lines.next_line().await.map_err(err)? {
        match parse(&line) {
            Line::Delta(t) => {
                app.emit("ai-delta", t).ok();
            }
            Line::Done(r) => done = Some(r),
            Line::Other => {}
        }
    }

    // ai_cancel เอา child ไปแล้ว = ผู้ใช้กดหยุด
    let Some(mut child) = ai.child.lock().await.take() else {
        return Err("หยุดแล้ว".into());
    };
    child.wait().await.ok();
    match done {
        Some(r) => r,
        None => {
            let e = errs.await.unwrap_or_default();
            Err(format!("claude CLI จบโดยไม่มีคำตอบ {}", e.trim()))
        }
    }
}

#[tauri::command]
pub async fn ai_cancel(ai: State<'_, Ai>) -> R<()> {
    if let Some(mut c) = ai.child.lock().await.take() {
        c.kill().await.ok();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_json_lines() {
        let delta = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"SELECT"}}}"#;
        assert_eq!(parse(delta), Line::Delta("SELECT".into()));
        let thinking = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}}"#;
        assert_eq!(parse(thinking), Line::Other);
        let ok = r#"{"type":"result","subtype":"success","is_error":false,"result":"SELECT 1;"}"#;
        assert_eq!(parse(ok), Line::Done(Ok("SELECT 1;".into())));
        let bad = r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}"#;
        assert_eq!(parse(bad), Line::Done(Err("Not logged in · Please run /login".into())));
        let maxed = r#"{"type":"result","subtype":"error_max_turns","is_error":true}"#;
        assert!(matches!(parse(maxed), Line::Done(Err(_))));
        assert_eq!(parse(r#"{"type":"system","subtype":"init"}"#), Line::Other);
        assert_eq!(parse("not json"), Line::Other);
    }
}
