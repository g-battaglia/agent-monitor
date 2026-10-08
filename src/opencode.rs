//! Read-only OpenCode SQLite sessions (legacy and session_v2 schemas).
//! Only session/message/part tables are queried; credentials are never read.
use crate::{
    model::{Conversation, Message, Metadata, Provider},
    paths,
    pi::{self, Cursor},
};
use anyhow::{Result, ensure};
use rusqlite::{Connection, OpenFlags, params};
use serde_json::Value;
use std::{path::Path, time::Duration};

fn open(path: &Path) -> Result<Connection> {
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    db.busy_timeout(Duration::from_millis(200))?;
    db.execute_batch("PRAGMA query_only=ON;")?;
    Ok(db)
}
fn table(db: &Connection) -> Result<&'static str> {
    if db.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='session_v2'",
        [],
        |r| r.get::<_, i64>(0),
    )? > 0
    {
        Ok("session_v2")
    } else {
        ensure!(
            db.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='session'",
                [],
                |r| r.get::<_, i64>(0)
            )? > 0,
            "unsupported OpenCode database schema"
        );
        Ok("session")
    }
}
fn metadata(
    db: &Connection,
    path: &Path,
    offset: usize,
    id: Option<&str>,
) -> Result<Vec<Metadata>> {
    let table = table(db)?;
    let sql = format!(
        "SELECT id,directory,title,parent_id,time_created,time_updated FROM {table} {} ORDER BY id LIMIT 100 OFFSET ?2",
        if id.is_some() {
            "WHERE id=?1"
        } else {
            "WHERE (?1 IS NULL)"
        }
    );
    let mut stmt = db.prepare(&sql)?;
    let result = stmt
        .query_map(params![id, offset as i64], |r| {
            let native_id: String = r.get(0)?;
            Ok(Metadata {
                provider: Provider::Opencode,
                database: Some(paths::canonical(path)),
                file: format!("{}#{}", paths::canonical(path).display(), native_id).into(),
                native_id,
                cwd: r.get(1)?,
                name: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                parent: r.get(3)?,
                created: r.get(4)?,
                updated: r.get(5)?,
                ..Default::default()
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for m in &result {
        ensure!(
            !m.native_id.is_empty() && m.native_id.len() <= 128 && !m.native_id.starts_with('-'),
            "invalid OpenCode session id"
        );
        ensure!(
            Path::new(&m.cwd).is_absolute() && m.cwd.len() <= 4096,
            "invalid OpenCode project directory"
        );
    }
    Ok(result)
}

// Walk bounded message rows; bodies are not retained across indexer calls.
fn visit(db: &Connection, id: &str, tools: bool, mut visitor: impl FnMut(Message)) -> Result<bool> {
    let v2 = table(db)? == "session_v2";
    let sql = if v2 {
        "SELECT id,type,time_created,substr(data,1,8388609) FROM session_message WHERE session_id=?1 ORDER BY seq,id LIMIT 100001"
    } else {
        "SELECT id,json_extract(data,'$.role'),time_created,substr(data,1,8388609) FROM message WHERE session_id=?1 ORDER BY time_created,id LIMIT 100001"
    };
    let mut stmt = db.prepare(sql)?;
    let mut rows = stmt.query([id])?;
    let mut count = 0;
    let mut bytes = 0;
    let mut partial = false;
    while let Some(row) = rows.next()? {
        count += 1;
        let raw: String = row.get(3)?;
        bytes += raw.len();
        if count > 100_000 || bytes > 64 * 1024 * 1024 {
            return Ok(true);
        }
        if raw.len() > 8 * 1024 * 1024 {
            partial = true;
            continue;
        }
        let role: String = row.get(1)?;
        if !matches!(role.as_str(), "user" | "assistant") {
            continue;
        }
        let e: Value = serde_json::from_str(&raw)?;
        let mut text = if v2 {
            if role == "user" {
                pi::text(&e["text"])
            } else {
                pi::text(&e["content"])
            }
        } else {
            let message_id: String = row.get(0)?;
            let mut parts = db.prepare("SELECT substr(data,1,8388609) FROM part WHERE message_id=?1 ORDER BY id LIMIT 10001")?;
            let mut text = String::new();
            for (index, data) in parts
                .query_map([message_id], |r| r.get::<_, String>(0))?
                .enumerate()
            {
                let data = data?;
                bytes += data.len();
                if index >= 10_000 || bytes > 64 * 1024 * 1024 {
                    return Ok(true);
                }
                if data.len() > 8 * 1024 * 1024 {
                    partial = true;
                    continue;
                }
                let p: Value = serde_json::from_str(&data)?;
                if p["type"] == "text" {
                    text.push_str(&pi::text(&p["text"]));
                    text.push('\n');
                }
                if tools && p["type"] == "tool" {
                    text.push_str(&format!(
                        "\n[Tool: {}]\n{}",
                        paths::line(p["tool"].as_str().unwrap_or("")),
                        pi::text(&p["state"]["output"])
                    ));
                }
                if text.len() > 8 * 1024 * 1024 {
                    break;
                }
            }
            text
        };
        if tools
            && v2
            && let Some(parts) = e["content"].as_array()
        {
            for p in parts.iter().filter(|p| p["type"] == "tool") {
                text.push_str(&format!(
                    "\n[Tool: {}]\n{}",
                    paths::line(p["tool"].as_str().unwrap_or("")),
                    pi::text(&p["state"]["output"])
                ));
            }
        }
        if !text.is_empty() {
            visitor(Message {
                role: if role == "user" { "You" } else { "OpenCode" }.into(),
                text,
                time: row.get(2)?,
                tool: false,
            });
        }
    }
    Ok(partial)
}

pub fn batch(path: &Path, offset: usize) -> Result<(Vec<Cursor>, bool)> {
    ensure!(offset < 100_000, "too many OpenCode sessions in source");
    let db = open(path)?;
    let mut result = metadata(&db, path, offset, None)?;
    let more = result.len() == 100;
    for meta in &mut result {
        meta.cwd = paths::canonical(Path::new(&meta.cwd))
            .to_string_lossy()
            .into_owned();
        meta.name = paths::line(&meta.name).chars().take(512).collect();
        let partial = visit(&db, &meta.native_id, false, |msg| {
            if msg.role == "You" && meta.first_message.is_empty() {
                meta.first_message = paths::line(&msg.text).chars().take(160).collect();
            }
            meta.revision =
                pi::hash(format!("{}:{}:{}", meta.revision, msg.role, msg.text).as_bytes());
        })?;
        if partial {
            meta.warning = "Limited preview: message limit reached".into();
        }
    }
    Ok((
        result
            .into_iter()
            .map(|metadata| Cursor {
                metadata,
                ..Default::default()
            })
            .collect(),
        more,
    ))
}

pub fn contains(path: &Path, id: &str) -> Result<bool> {
    Ok(!metadata(&open(path)?, path, 0, Some(id))?.is_empty())
}
pub fn identity(path: &Path, id: &str) -> Result<Metadata> {
    metadata(&open(path)?, path, 0, Some(id))?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("OpenCode session no longer exists"))
}

pub fn conversation(path: &Path, id: &str, tools: bool, page: usize) -> Result<Conversation> {
    let db = open(path)?;
    db.execute_batch("BEGIN;")?;
    ensure!(
        !metadata(&db, path, 0, Some(id))?.is_empty(),
        "OpenCode session no longer exists"
    );
    let mut count = 0usize;
    let partial = visit(&db, id, tools, |_| count += 1)?;
    let pages = count.div_ceil(100).max(1);
    let page = page.min(pages - 1);
    let end = count.saturating_sub(page * 100);
    let start = end.saturating_sub(100);
    let mut messages = vec![];
    let mut i = 0;
    visit(&db, id, tools, |msg| {
        if (start..end).contains(&i) {
            messages.push(msg);
        }
        i += 1;
    })?;
    Ok(Conversation {
        messages,
        page,
        pages,
        partial,
        warning: if partial {
            "Limited preview: message limit reached".into()
        } else {
            String::new()
        },
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_parts_support_pagination_and_tool_filtering() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE session(id TEXT,directory TEXT,title TEXT,parent_id TEXT,time_created INTEGER,time_updated INTEGER); CREATE TABLE message(id TEXT,session_id TEXT,time_created INTEGER,data TEXT); CREATE TABLE part(id TEXT,message_id TEXT,data TEXT); INSERT INTO session VALUES('legacy','/tmp','Legacy session',NULL,1,2);").unwrap();
        for i in 0..205 {
            let id = format!("m{i:04}");
            db.execute(
                "INSERT INTO message VALUES(?1,'legacy',?2,'{\"role\":\"user\"}')",
                params![id, i],
            )
            .unwrap();
            db.execute(
                "INSERT INTO part VALUES(?1,?1,?2)",
                params![
                    id,
                    serde_json::json!({"type":"text","text":format!("Request {i}")}).to_string()
                ],
            )
            .unwrap();
        }
        db.execute("INSERT INTO part VALUES('ztool','m0204','{\"type\":\"tool\",\"tool\":\"bash\",\"state\":{\"output\":\"Tool fixture\"}}')",[]).unwrap();
        drop(db);
        let page = conversation(&path, "legacy", false, 0).unwrap();
        assert_eq!(page.pages, 3);
        assert_eq!(page.messages.len(), 100);
        assert_eq!(page.messages[0].text.trim(), "Request 105");
        assert!(!page.messages[99].text.contains("Tool fixture"));
        assert!(
            conversation(&path, "legacy", true, 0).unwrap().messages[99]
                .text
                .contains("Tool fixture")
        );
        assert_eq!(
            conversation(&path, "legacy", false, 2)
                .unwrap()
                .messages
                .len(),
            5
        );
        assert_eq!(
            batch(&path, 0).unwrap().0[0].metadata.first_message,
            "Request 0"
        );
    }
    #[test]
    fn sqlite_sessions_are_read_only_and_hide_reasoning() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE session_v2(id TEXT PRIMARY KEY,directory TEXT,title TEXT,parent_id TEXT,time_created INTEGER,time_updated INTEGER); CREATE TABLE session_message(id TEXT,session_id TEXT,type TEXT,seq INTEGER,time_created INTEGER,data TEXT); INSERT INTO session_v2 VALUES('ses_one','/tmp','Parser work',NULL,1,2); INSERT INTO session_message VALUES('msg1','ses_one','user',1,1,'{\"text\":\"Fix it\"}'); INSERT INTO session_message VALUES('msg2','ses_one','assistant',2,2,'{\"content\":[{\"type\":\"reasoning\",\"text\":\"SECRET\"},{\"type\":\"text\",\"text\":\"Fixed\"}]}');").unwrap();
        drop(db);
        let before = std::fs::read(&path).unwrap();
        let (rows, more) = batch(&path, 0).unwrap();
        assert!(!more);
        assert_eq!(rows[0].metadata.provider, Provider::Opencode);
        assert_eq!(rows[0].metadata.first_message, "Fix it");
        let page = conversation(&path, "ses_one", false, 0).unwrap();
        assert_eq!(page.messages.len(), 2);
        assert_eq!(page.messages[1].text, "Fixed");
        assert!(identity(&path, "missing").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(
            open(&path)
                .unwrap()
                .execute("DELETE FROM session_v2", [])
                .is_err()
        );
    }
}
