//! Read-only Pi JSONL catalog and branch reader.
//!
//! Pi transcripts are append-only JSONL (v3). This module scans them in
//! small bounded chunks, tracks file identity across renames and rewrites,
//! and renders one readable page at a time. Thinking blocks, images, and
//! opaque payloads are skipped by design. Nothing here writes, migrates,
//! or opens Pi's own SessionManager.
use crate::{
    model::{Conversation, Message, Metadata},
    paths,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

/// Bytes scanned per indexer step, so the UI thread never waits on disk.
const CHUNK: u64 = 512 * 1024;
/// Longest single JSONL line accepted; larger rows are reported, not loaded.
const MAX_LINE: usize = 8 * 1024 * 1024;
/// Largest transcript file accepted; larger files are reported, not loaded.
const MAX_FILE: u64 = 512 * 1024 * 1024;

/// Resume position for one transcript: byte offset plus file-identity
/// samples. Inode/device/mtime plus content hashes detect renames,
/// truncation, and full rewrites between scans.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Cursor {
    pub metadata: Metadata,
    pub offset: u64,
    pub inode: u64,
    pub device: u64,
    pub boundary: u64,
    pub prefix: u64,
    pub mtime: i64,
    pub size: u64,
}

/// FNV-1a content hash. Used both for change detection samples and for
/// the per-message revision fingerprint that notices real follow-ups.
pub fn hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |h, b| {
        (h ^ *b as u64).wrapping_mul(0x100000001b3)
    })
}
/// Hash up to `len` bytes at `offset`. Short reads hash what exists;
/// missing regions hash empty, which is enough for rewrite detection.
fn sample(file: &mut File, offset: u64, len: usize) -> Result<u64> {
    file.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0; len];
    let n = file.read(&mut buf)?;
    Ok(hash(&buf[..n]))
}
/// Accept epoch millis or RFC3339 strings; anything else means "unknown".
fn timestamp(value: &Value) -> i64 {
    value
        .as_i64()
        .or_else(|| {
            value
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|t| t.timestamp_millis())
        })
        .unwrap_or(0)
}
/// Missing or non-string fields read as empty, never as an error.
fn str_field<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}
/// Render message content as readable text: plain strings directly,
/// content blocks by keeping only `text` parts. Tool calls, thinking,
/// and images are dropped here; the caller decides about tool results.
pub fn text(value: &Value) -> String {
    if let Some(s) = value.as_str() {
        return paths::clean(s);
    }
    value
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .map(paths::clean)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}
/// Fold one JSONL record into session metadata.
///
/// `session` sets identity (v3 only, absolute cwd required).
/// `session_info` sets the explicit name, including a deliberate clear.
/// `message` updates first-message fallback, newest timestamp, and the
/// revision fingerprint. Everything else is ignored at this level.
fn update(meta: &mut Metadata, e: &Value) -> Result<()> {
    match str_field(e, "type") {
        "session" => {
            ensure!(
                e["version"].as_u64() == Some(3),
                "unsupported session format (expected v3)"
            );
            meta.native_id = str_field(e, "id").to_string();
            ensure!(
                !meta.native_id.is_empty() && meta.native_id.len() <= 128,
                "missing or overlong session id"
            );
            ensure!(
                str_field(e, "cwd").len() <= 4096 && Path::new(str_field(e, "cwd")).is_absolute(),
                "missing or relative session cwd"
            );
            meta.cwd = paths::canonical(Path::new(str_field(e, "cwd")))
                .to_string_lossy()
                .into();
            meta.created = timestamp(&e["timestamp"]);
            meta.parent = e["parentSession"]
                .as_str()
                .filter(|p| p.len() <= 4096)
                .map(str::to_string);
        }
        "session_info" => {
            let name = str_field(e, "name");
            meta.name = paths::line(name).chars().take(512).collect();
            if name.chars().count() > 512 {
                meta.warning = "Long name shortened".into();
            }
        }
        "message" => {
            let msg = &e["message"];
            let role = str_field(msg, "role");
            if role == "user" || role == "assistant" {
                let content = text(&msg["content"]);
                if role == "user" && meta.first_message.is_empty() {
                    meta.first_message = paths::line(&content).chars().take(160).collect();
                }
                meta.updated = meta
                    .updated
                    .max(timestamp(&msg["timestamp"]))
                    .max(timestamp(&e["timestamp"]));
                meta.revision = hash(
                    format!("{}:{}:{}", meta.revision, str_field(e, "id"), content).as_bytes(),
                );
            }
        }
        _ => {}
    }
    if e["type"] != "session" {
        let id = e["id"].as_str();
        ensure!(id.is_none_or(|id| id.len() <= 128), "entry id too long");
        meta.leaf = id.map(str::to_string);
    }
    Ok(())
}

/// At most one bounded chunk per call; the worker yields between chunks.
/// The final incomplete JSONL record is retried on the next scan.
pub fn scan(path: &Path, previous: Option<&Cursor>) -> Result<(Cursor, bool)> {
    let mut file = File::open(path)?;
    let stat = file.metadata()?;
    ensure!(
        stat.is_file() && stat.len() <= MAX_FILE,
        "session is too large or not a regular file"
    );
    let prefix = sample(&mut file, 0, stat.len().min(512) as usize)?;
    let mut cursor = previous.cloned().unwrap_or_default();
    let boundary = sample(
        &mut file,
        cursor.offset.saturating_sub(512),
        cursor.offset.min(512) as usize,
    )?;
    // Prefix length changes until 512 bytes exist. This can safely cause a rescan.
    let rewritten = cursor.inode != stat.ino()
        || cursor.device != stat.dev()
        || stat.len() < cursor.offset
        || cursor.prefix != prefix
        || cursor.boundary != boundary
        || (stat.len() == cursor.size
            && (stat.mtime() * 1_000_000_000 + stat.mtime_nsec()) != cursor.mtime);
    if rewritten {
        cursor = Cursor::default();
    }
    cursor.metadata.file = paths::canonical(path);
    file.seek(SeekFrom::Start(cursor.offset))?;
    let mut reader = BufReader::new(file);
    let start = cursor.offset;
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        let n = reader
            .by_ref()
            .take((MAX_LINE + 1) as u64)
            .read_until(b'\n', &mut buffer)?;
        if n == 0 {
            break;
        }
        ensure!(n <= MAX_LINE, "JSONL line over 8 MiB: preview unavailable");
        if buffer.last() != Some(&b'\n') {
            break;
        }
        let entry: Value = match serde_json::from_slice(&buffer) {
            Ok(v) => v,
            Err(_) => {
                cursor.metadata.warning = "Invalid JSONL line; history is incomplete".into();
                cursor.offset += n as u64;
                continue;
            }
        };
        update(&mut cursor.metadata, &entry)?;
        cursor.offset += n as u64;
        if cursor.offset - start >= CHUNK {
            break;
        }
    }
    ensure!(
        !cursor.metadata.native_id.is_empty(),
        "Pi header is missing or still incomplete"
    );
    cursor.inode = stat.ino();
    cursor.device = stat.dev();
    cursor.prefix = prefix;
    cursor.size = stat.len();
    cursor.mtime = stat.mtime() * 1_000_000_000 + stat.mtime_nsec();
    let mut file = reader.into_inner();
    cursor.boundary = sample(
        &mut file,
        cursor.offset.saturating_sub(512),
        cursor.offset.min(512) as usize,
    )?;
    let pending = cursor.offset < stat.len() && cursor.offset - start >= CHUNK;
    Ok((cursor, pending))
}

pub fn is_session(path: &Path) -> Result<bool> {
    let mut buf = Vec::new();
    BufReader::new(File::open(path)?)
        .take((MAX_LINE + 1) as u64)
        .read_until(b'\n', &mut buf)?;
    ensure!(buf.len() <= MAX_LINE, "header too large");
    Ok(serde_json::from_slice::<Value>(&buf).is_ok_and(|v| v["type"] == "session"))
}

pub fn header(path: &Path) -> Result<Metadata> {
    let mut buf = Vec::new();
    BufReader::new(File::open(path)?)
        .take((MAX_LINE + 1) as u64)
        .read_until(b'\n', &mut buf)?;
    ensure!(buf.len() <= MAX_LINE, "header too large");
    let e: Value = serde_json::from_slice(&buf).context("invalid Pi header")?;
    ensure!(e["type"] == "session", "Pi header is missing");
    let mut meta = Metadata {
        file: paths::canonical(path),
        ..Default::default()
    };
    update(&mut meta, &e)?;
    Ok(meta)
}

pub fn files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut stack = vec![(root.to_path_buf(), 0)];
    let mut files = vec![];
    while let Some((dir, depth)) = stack.pop() {
        ensure!(files.len() < 100_000, "too many files in source");
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let typ = entry.file_type()?;
            if typ.is_dir() && depth < 8 {
                stack.push((entry.path(), depth + 1));
            } else if typ.is_file() && entry.path().extension().is_some_and(|e| e == "jsonl") {
                files.push(paths::canonical(&entry.path()));
            }
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// Only entry offsets and tree links live in memory here; message bodies
/// are re-read from disk per page. Large tool/image payloads are never
/// copied into a transcript cache. Reads are capped and labeled.
pub fn conversation(path: &Path, leaf: Option<&str>, tools: bool) -> Result<Conversation> {
    conversation_page(path, leaf, tools, 0)
}
pub fn conversation_page(
    path: &Path,
    leaf: Option<&str>,
    tools: bool,
    page: usize,
) -> Result<Conversation> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut entries = HashMap::<String, (Option<String>, u64, usize, String)>::new();
    let mut children = HashSet::new();
    let mut order = vec![];
    let mut offset = 0;
    let mut buffer = Vec::new();
    let mut warning = String::new();
    let mut partial = false;
    loop {
        buffer.clear();
        let n = reader
            .by_ref()
            .take((MAX_LINE + 1) as u64)
            .read_until(b'\n', &mut buffer)?;
        if n == 0 {
            break;
        }
        if n > MAX_LINE || offset > MAX_FILE || entries.len() >= 100_000 {
            warning = "Limited preview: file too large".into();
            partial = true;
            break;
        }
        if buffer.last() != Some(&b'\n') {
            break;
        }
        if let Ok(e) = serde_json::from_slice::<Value>(&buffer) {
            if e["type"] != "session"
                && let Some(id) = e["id"].as_str()
            {
                let parent = e["parentId"].as_str().map(str::to_string);
                if id.len() > 128 || parent.as_ref().is_some_and(|p| p.len() > 128) {
                    warning = "Invalid branch id".into();
                    partial = true;
                    offset += n as u64;
                    continue;
                }
                if let Some(p) = &parent {
                    children.insert(p.clone());
                }
                let label = if e["type"] == "message" && e["message"]["role"] == "user" {
                    text(&e["message"]["content"]).chars().take(70).collect()
                } else {
                    String::new()
                };
                entries.insert(id.to_string(), (parent, offset, n, label));
                order.push(id.to_string());
            }
        } else {
            warning = "Incomplete history: invalid JSONL line".into();
            partial = true;
        }
        offset += n as u64;
    }
    let branches = order
        .iter()
        .filter(|id| !children.contains(*id))
        .map(|id| {
            let mut current = Some(id.as_str());
            let mut label = String::new();
            for _ in 0..1000 {
                let Some(entry) = current.and_then(|id| entries.get(id)) else {
                    break;
                };
                if !entry.3.is_empty() {
                    label = entry.3.clone();
                    break;
                }
                current = entry.0.as_deref();
            }
            (
                id.clone(),
                if label.is_empty() {
                    "Alternative without a readable request".into()
                } else {
                    label
                },
            )
        })
        .collect();
    let chosen = leaf.map(str::to_string).or_else(|| order.last().cloned());
    if chosen.as_ref().is_some_and(|id| !entries.contains_key(id)) {
        warning.push_str(" Watched branch is missing from the indexed preview");
        partial = true;
    }
    let mut branch = vec![];
    let mut current = chosen.clone();
    let mut visited = HashSet::new();
    while let Some(id) = current {
        if !visited.insert(id.clone()) {
            warning = "History cycle: branch is incomplete".into();
            partial = true;
            break;
        }
        let Some(entry) = entries.get(&id) else {
            warning = "Incomplete branch: missing parent".into();
            partial = true;
            break;
        };
        branch.push((entry.1, entry.2));
        current = entry.0.clone();
    }
    branch.reverse();
    let mut file = reader.into_inner();
    let mut messages = vec![];
    let pages = branch.len().div_ceil(100).max(1);
    let page = page.min(pages - 1);
    // Pages count backward from the newest entries. Every older page and
    // alternative branch can be read without rendering an entire transcript.
    for (offset, len) in branch
        .into_iter()
        .rev()
        .skip(page * 100)
        .take(100)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        file.seek(SeekFrom::Start(offset))?;
        buffer.resize(len, 0);
        file.read_exact(&mut buffer)?;
        let e: Value = serde_json::from_slice(&buffer)?;
        let time = timestamp(&e["timestamp"]);
        let message = match str_field(&e, "type") {
            "message" => {
                let msg = &e["message"];
                let role = str_field(msg, "role");
                match role {
                    "user" => Some(("You", text(&msg["content"]), false)),
                    "assistant" => {
                        let mut body = text(&msg["content"]);
                        if msg["stopReason"] == "error" {
                            body.push_str(&format!(
                                "\n[Pi error: {}]",
                                paths::line(str_field(msg, "errorMessage"))
                            ));
                        }
                        if msg["stopReason"] == "aborted" {
                            body.push_str("\n[Reply interrupted]");
                        }
                        if tools && let Some(blocks) = msg["content"].as_array() {
                            for call in blocks.iter().filter(|b| b["type"] == "toolCall") {
                                body.push_str(&format!("\n[Tool: {}]", str_field(call, "name")));
                            }
                        }
                        Some(("Pi", body, false))
                    }
                    "toolResult" if tools => Some(("Tool", text(&msg["content"]), true)),
                    _ => None,
                }
            }
            "custom_message" if e["display"] == true => {
                Some(("Extension", text(&e["content"]), false))
            }
            "compaction" => Some((
                "Compacted context",
                str_field(&e, "summary").to_string(),
                false,
            )),
            "branch_summary" => Some((
                "Branch summary",
                str_field(&e, "summary").to_string(),
                false,
            )),
            "context_edit" => Some((
                "Edited context",
                "The original message stays in history.".into(),
                false,
            )),
            _ => None,
        };
        if let Some((role, body, tool)) = message
            && !body.is_empty()
        {
            messages.push(Message {
                role: role.into(),
                text: if body.chars().count() > 4000 {
                    warning =
                        "Long messages shortened in the preview; original stays in the Pi file"
                            .into();
                    format!(
                        "{}\n[… shortened text]",
                        body.chars().take(4000).collect::<String>()
                    )
                } else {
                    body
                },
                time,
                tool,
            });
        }
    }
    Ok(Conversation {
        messages,
        branches,
        leaf: chosen,
        warning,
        page,
        pages,
        partial,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn fixture() -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f,"{}",serde_json::json!({"type":"session","version":3,"id":"native","cwd":"/repo","timestamp":"2026-01-01T00:00:00Z"})).unwrap();
        f
    }
    #[test]
    fn partial_rename_and_rewrite() {
        let mut f = fixture();
        writeln!(
            f,
            "{}",
            serde_json::json!({"type":"session_info","id":"n","parentId":null,"name":"named"})
        )
        .unwrap();
        let (c, _) = scan(f.path(), None).unwrap();
        assert_eq!(c.metadata.name, "named");
        write!(f, "{{\"type\":\"session_info\",").unwrap();
        let (c2, _) = scan(f.path(), Some(&c)).unwrap();
        assert_eq!(c.offset, c2.offset);
        writeln!(f, "\"id\":\"clear\",\"name\":\"\"}}").unwrap();
        let (c3, _) = scan(f.path(), Some(&c2)).unwrap();
        assert_eq!(c3.metadata.name, "");
        f.as_file_mut().set_len(c.offset).unwrap();
        let (c4, _) = scan(f.path(), Some(&c3)).unwrap();
        assert_eq!(c4.metadata.name, "named");
    }
    #[test]
    fn pagination_retains_older_branches_and_skips_hidden_content() {
        let mut f = fixture();
        let mut parent = None;
        for i in 0..250 {
            let id = format!("entry-{i}");
            writeln!(f,"{}",serde_json::json!({"type":"message","id":id,"parentId":parent,"message":{"role":"assistant","content":[{"type":"thinking","thinking":"HIDDEN"},{"type":"text","text":format!("message-{i}")},{"type":"image","data":"HIDDEN"}]}})).unwrap();
            parent = Some(id);
        }
        let recent = conversation_page(f.path(), None, false, 0).unwrap();
        assert_eq!(recent.pages, 3);
        assert_eq!(recent.messages.last().unwrap().text, "message-249");
        assert!(!recent.messages.iter().any(|m| m.text.contains("HIDDEN")));
        let oldest = conversation_page(f.path(), None, false, 2).unwrap();
        assert_eq!(oldest.messages[0].text, "message-0");
        assert_eq!(oldest.messages.len(), 50);
    }
    #[test]
    fn malformed_entries_and_cycles_are_reported() {
        let mut f = fixture();
        writeln!(f, "not-json").unwrap();
        writeln!(f,"{}",serde_json::json!({"type":"message","id":"a","parentId":"b","message":{"role":"user","content":"one"}})).unwrap();
        writeln!(f,"{}",serde_json::json!({"type":"message","id":"b","parentId":"a","message":{"role":"user","content":"two"}})).unwrap();
        let c = conversation(f.path(), None, false).unwrap();
        assert!(c.warning.contains("cycle"));
        assert!(c.messages.len() <= 2);
    }
    #[test]
    fn branch_does_not_mix_conversations() {
        let mut f = fixture();
        for (id, parent, body) in [
            ("a", None, "root"),
            ("b", Some("a"), "old branch"),
            ("c", Some("a"), "new branch"),
        ] {
            writeln!(f,"{}",serde_json::json!({"type":"message","id":id,"parentId":parent,"message":{"role":"user","content":body}})).unwrap();
        }
        let c = conversation(f.path(), Some("b"), false).unwrap();
        assert_eq!(c.messages.len(), 2);
        assert_eq!(c.messages[1].text, "old branch");
        assert_eq!(c.branches.len(), 2);
    }
}
