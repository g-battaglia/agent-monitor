//! Normalize local Claude and Codex JSONL records into the existing reader.
//! No provider SDKs, migrations, reasoning text, or transcript writes.
use crate::{
    model::{Metadata, Provider},
    paths, pi,
};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{collections::HashMap, fs::File, io::Read, path::Path};

/// Codex's own title index, read as bounded local metadata only.
pub fn codex_names(path: &Path) -> Result<HashMap<String, String>> {
    let mut data = String::new();
    File::open(path)?
        .take(8 * 1024 * 1024 + 1)
        .read_to_string(&mut data)?;
    ensure!(
        data.len() <= 8 * 1024 * 1024,
        "Codex title index is too large"
    );
    let mut names = HashMap::new();
    for line in data.lines() {
        let Ok(e) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let (Some(id), Some(name)) = (e["id"].as_str(), e["thread_name"].as_str())
            && id.len() <= 128
        {
            names.insert(
                id.to_string(),
                paths::line(name).chars().take(512).collect(),
            );
        }
    }
    Ok(names)
}

pub fn identify(meta: &mut Metadata, e: &Value) -> Result<()> {
    let (provider, id, cwd, created) = if e["type"] == "session_meta" {
        let p = &e["payload"];
        (
            Provider::Codex,
            p["id"].as_str().or(p["session_id"].as_str()),
            p["cwd"].as_str(),
            &p["timestamp"],
        )
    } else if let Some(id) = e["sessionId"].as_str() {
        (
            Provider::Claude,
            Some(id),
            e["cwd"].as_str(),
            &e["timestamp"],
        )
    } else {
        return Ok(());
    };
    let id = id.unwrap_or("");
    ensure!(
        !id.is_empty() && id.len() <= 128 && !id.starts_with('-'),
        "invalid provider session id"
    );
    // A Codex fork prepends its own identity to inherited parent records.
    // Only the explicitly linked parent's header may be ignored.
    if provider == Provider::Codex
        && meta.provider == Provider::Codex
        && !meta.native_id.is_empty()
        && meta.native_id != id
        && meta.parent.as_deref() == Some(id)
    {
        return Ok(());
    }
    if provider == Provider::Codex && meta.native_id.is_empty() {
        meta.parent = e["payload"]["forked_from_id"]
            .as_str()
            .or(e["payload"]["parent_thread_id"].as_str())
            .filter(|id| !id.is_empty() && id.len() <= 128)
            .map(str::to_string);
    }
    ensure!(
        meta.native_id.is_empty() || (meta.native_id == id && meta.provider == provider),
        "mixed session identities"
    );
    meta.provider = provider;
    meta.native_id = id.into();
    if let Some(cwd) = cwd {
        ensure!(
            cwd.len() <= 4096 && Path::new(cwd).is_absolute(),
            "missing or relative session cwd"
        );
        // Keep the original project, not a later temporary tool directory.
        if meta.cwd.is_empty() {
            meta.cwd = paths::canonical(Path::new(cwd))
                .to_string_lossy()
                .into_owned();
        }
    }
    let time = pi::timestamp(created);
    if time > 0 && (meta.created == 0 || time < meta.created) {
        meta.created = time;
    }
    let name = match e["type"].as_str() {
        Some("custom-title") => e["customTitle"].as_str(),
        Some("ai-title") if meta.name.is_empty() => e["aiTitle"].as_str(),
        _ => None,
    };
    if let Some(name) = name {
        meta.name = paths::line(name).chars().take(512).collect();
    }
    Ok(())
}

pub fn entry(provider: Provider, e: &Value, offset: u64, previous: Option<&str>) -> Value {
    match provider {
        Provider::Pi => e.clone(),
        Provider::Claude => {
            let Some(id) = e["uuid"].as_str() else {
                return Value::Null;
            };
            let role = e["message"]["role"].as_str().unwrap_or("");
            let content = &e["message"]["content"];
            let only_tools = content
                .as_array()
                .is_some_and(|a| !a.is_empty() && a.iter().all(|b| b["type"] == "tool_result"));
            let content = if only_tools {
                Value::String(
                    content
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|b| pi::text(&b["content"]))
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
            } else {
                content.clone()
            };
            json!({"type": if role.is_empty() {"ignored"} else {"message"},
                "id":id, "parentId":e["parentUuid"], "timestamp":e["timestamp"],
                "message":{"role":if only_tools {"toolResult"} else {role}, "content":content}})
        }
        Provider::Codex => {
            let p = &e["payload"];
            let mut role = "";
            let mut content = Value::Null;
            if e["type"] == "response_item" {
                if p["type"] == "message" {
                    role = p["role"].as_str().unwrap_or("");
                    if matches!(role, "user" | "assistant") {
                        content = if role == "user" {
                            Value::Array(
                                p["content"]
                                    .as_array()
                                    .map(|a| {
                                        a.iter()
                                            .filter(|b| {
                                                let text = b["text"].as_str().unwrap_or("");
                                                !text.starts_with("# AGENTS.md instructions for ")
                                                    && !text.starts_with("<environment_context>")
                                                    && !text.starts_with("<user_instructions>")
                                            })
                                            .cloned()
                                            .collect()
                                    })
                                    .unwrap_or_default(),
                            )
                        } else {
                            p["content"].clone()
                        };
                    } else {
                        role = "";
                    }
                } else if matches!(
                    p["type"].as_str(),
                    Some("function_call_output" | "custom_tool_call_output")
                ) {
                    role = "toolResult";
                    content = p["output"].clone();
                }
            }
            json!({"type":if role.is_empty() {"ignored"} else {"message"}, "id":format!("offset-{offset}"),
                "parentId":previous, "timestamp":e["timestamp"], "message":{"role":role,"content":content}})
        }
        Provider::Opencode => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[test]
    fn claude_metadata_titles_tools_and_hidden_reasoning() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("claude.jsonl");
        fs::write(&file, concat!(
            "{\"type\":\"mode\",\"sessionId\":\"claude-one\"}\n",
            "{\"type\":\"user\",\"sessionId\":\"claude-one\",\"cwd\":\"/tmp\",\"uuid\":\"u\",\"parentUuid\":null,\"timestamp\":\"2026-01-01T00:00:00Z\",\"message\":{\"role\":\"user\",\"content\":\"Fix the parser\"}}\n",
            "{\"type\":\"assistant\",\"sessionId\":\"claude-one\",\"uuid\":\"a\",\"parentUuid\":\"u\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"thinking\",\"thinking\":\"SECRET\"},{\"type\":\"text\",\"text\":\"Fixed\"}]}}\n",
            "{\"type\":\"custom-title\",\"sessionId\":\"claude-one\",\"customTitle\":\"Parser fix\"}\n" )).unwrap();
        let (cursor, more) = pi::scan(&file, None).unwrap();
        assert!(!more);
        assert_eq!(cursor.metadata.provider, Provider::Claude);
        assert_eq!(cursor.metadata.title(), "Parser fix");
        assert_eq!(cursor.metadata.first_message, "Fix the parser");
        assert_eq!(pi::header(&file).unwrap().native_id, "claude-one");
        let page = pi::conversation(&file, None, false).unwrap();
        assert_eq!(page.messages.len(), 2);
        assert_eq!(page.messages[1].role, "Claude");
        assert_eq!(page.messages[1].text, "Fixed");
    }
    #[test]
    fn codex_forks_retain_child_identity_and_reject_unrelated_headers() {
        let mut meta = Metadata::default();
        let child = json!({"type":"session_meta","payload":{"id":"child","session_id":"parent","forked_from_id":"parent","cwd":"/tmp","timestamp":"2026-02-01T00:00:00Z"}});
        identify(&mut meta, &child).unwrap();
        let before = meta.created;
        identify(&mut meta,&json!({"type":"session_meta","payload":{"id":"parent","cwd":"/different","timestamp":"2026-01-01T00:00:00Z"}})).unwrap();
        assert_eq!(meta.native_id, "child");
        assert_eq!(meta.parent.as_deref(), Some("parent"));
        assert_eq!(meta.created, before);
        assert_eq!(
            meta.cwd,
            paths::canonical(Path::new("/tmp")).to_string_lossy()
        );
        assert!(
            identify(
                &mut meta,
                &json!({"type":"session_meta","payload":{"id":"unrelated","cwd":"/tmp"}})
            )
            .is_err()
        );
    }
    #[test]
    fn codex_uses_response_items_not_duplicate_events_or_reasoning() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("codex.jsonl");
        let records = [
            json!({"type":"session_meta","payload":{"id":"codex-one","cwd":"/tmp","timestamp":"2026-01-01T00:00:00Z"}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions for /tmp\nBootstrap"},{"type":"input_text","text":"<environment_context>\nBootstrap"}]}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Fix tests"}]}}),
            json!({"type":"event_msg","payload":{"type":"user_message","message":"Fix tests"}}),
            json!({"type":"response_item","payload":{"type":"reasoning","summary":[{"text":"SECRET"}]}}),
            json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Done"}]}}),
        ];
        fs::write(
            &file,
            records.iter().map(|v| format!("{v}\n")).collect::<String>(),
        )
        .unwrap();
        let (cursor, _) = pi::scan(&file, None).unwrap();
        assert_eq!(cursor.metadata.provider, Provider::Codex);
        assert_eq!(cursor.metadata.first_message, "Fix tests");
        let page = pi::conversation(&file, None, false).unwrap();
        assert_eq!(page.messages.len(), 2);
        assert_eq!(page.messages[1].role, "Codex");
        assert_eq!(page.messages[1].text, "Done");
        assert_eq!(
            pi::scan(&file, Some(&cursor)).unwrap().0.metadata.revision,
            cursor.metadata.revision
        );
    }
}
