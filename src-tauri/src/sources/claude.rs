//! Claude Code の会話ログ: `<config>/projects/<エンコードされた cwd>/<session>.jsonl`
//! と `<session>/subagents/agent-*.jsonl`。
//!
//! ストリーミングされた各コンテンツブロックが同じ `message.id` / `requestId` を持つ
//! 別々の行として書かれ、`output_tokens` は行が進むごとに増えていく。
//! リクエスト単位で行をまとめて各フィールドの最大値を採るので、最終的な出力数が使われる
//! （最初の行だけを数えると出力を少なく見積もってしまう）。

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::model::*;

#[derive(Deserialize)]
struct Line {
    #[serde(rename = "type")]
    ty: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    uuid: Option<String>,
    cwd: Option<String>,
    #[serde(rename = "isSidechain")]
    is_sidechain: Option<bool>,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    message: Option<Message>,
    #[serde(rename = "customTitle")]
    custom_title: Option<String>,
}

#[derive(Deserialize)]
struct Message {
    id: Option<String>,
    model: Option<String>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Usage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation: Option<CacheCreation>,
    output_tokens_details: Option<OutputDetails>,
    server_tool_use: Option<ServerToolUse>,
    speed: Option<String>,
    inference_geo: Option<String>,
}

#[derive(Deserialize)]
struct CacheCreation {
    ephemeral_5m_input_tokens: Option<u64>,
    ephemeral_1h_input_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct OutputDetails {
    thinking_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct ServerToolUse {
    web_search_requests: Option<u64>,
}

pub fn parse_file(path: &Path) -> std::io::Result<ParsedFile> {
    let data = std::fs::read(path)?;
    Ok(parse_bytes(path, &data))
}

fn parse_bytes(path: &Path, data: &[u8]) -> ParsedFile {
    let mut pf = ParsedFile::default();
    let mut by_key: HashMap<u64, usize> = HashMap::new();
    let in_subagents_dir = path
        .components()
        .any(|c| c.as_os_str().eq_ignore_ascii_case("subagents"));
    let fallback_project = fallback_project(path);
    let fallback_session = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    for raw in data.split(|&b| b == b'\n') {
        let is_assistant = contains(raw, b"\"type\":\"assistant\"");
        let is_title = !is_assistant && contains(raw, b"\"type\":\"custom-title\"");
        if !is_assistant && !is_title {
            continue;
        }
        let Ok(line) = serde_json::from_slice::<Line>(raw) else {
            continue;
        };
        match line.ty.as_deref() {
            Some("custom-title") => {
                if let (Some(sid), Some(t)) = (line.session_id, line.custom_title) {
                    if !t.trim().is_empty() {
                        pf.titles.retain(|(s, _)| s != &sid);
                        pf.titles.push((sid, t));
                    }
                }
            }
            Some("assistant") => {
                let Some(msg) = line.message else { continue };
                let Some(u) = msg.usage else { continue };
                let model = msg.model.unwrap_or_default();
                if model.is_empty() || model.starts_with('<') {
                    continue; // "<synthetic>" はプレースホルダーのメッセージ
                }
                let Some(ts) = line.timestamp.as_deref().and_then(parse_ts) else {
                    continue;
                };

                let cc_total = u.cache_creation_input_tokens.unwrap_or(0);
                let (cw5, cw1) = match &u.cache_creation {
                    Some(c) => {
                        let a = c.ephemeral_5m_input_tokens.unwrap_or(0);
                        let b = c.ephemeral_1h_input_tokens.unwrap_or(0);
                        if a + b == 0 && cc_total > 0 {
                            (cc_total, 0)
                        } else {
                            (a, b)
                        }
                    }
                    None => (cc_total, 0),
                };
                let t = Tokens {
                    input: u.input_tokens.unwrap_or(0),
                    output: u.output_tokens.unwrap_or(0),
                    cache_read: u.cache_read_input_tokens.unwrap_or(0),
                    cache_write_5m: cw5,
                    cache_write_1h: cw1,
                    reasoning: u
                        .output_tokens_details
                        .and_then(|d| d.thinking_tokens)
                        .unwrap_or(0),
                    web_search: u
                        .server_tool_use
                        .and_then(|s| s.web_search_requests)
                        .unwrap_or(0) as u32,
                };

                let mut flags = 0u8;
                if in_subagents_dir || line.is_sidechain.unwrap_or(false) {
                    flags |= F_SUBAGENT;
                }
                if u.speed.as_deref() == Some("fast") {
                    flags |= F_FAST;
                }
                if u.inference_geo.as_deref() == Some("us") {
                    flags |= F_GEO_US;
                }

                let key = match (&msg.id, &line.request_id) {
                    (Some(id), Some(rid)) => stable_hash(&["cc", id, rid]),
                    (Some(id), None) => stable_hash(&["cc", id]),
                    _ => stable_hash(&["cc-line", line.uuid.as_deref().unwrap_or(""), &ts.to_string()]),
                };

                if let Some(&i) = by_key.get(&key) {
                    let r = &mut pf.records[i];
                    r.t.max_with(&t);
                    r.flags |= flags;
                    continue;
                }

                let session = line.session_id.unwrap_or_else(|| fallback_session.clone());
                let project = line.cwd.unwrap_or_else(|| fallback_project.clone());
                let rec = FileRecord {
                    key,
                    ts,
                    model: pf.intern(&model),
                    project: pf.intern(&project),
                    session: pf.intern(&session),
                    flags,
                    t,
                };
                by_key.insert(key, pf.records.len());
                pf.records.push(rec);
            }
            _ => {}
        }
    }
    pf.records.retain(|r| !r.t.is_zero());
    pf
}

/// プロジェクトのフォルダ名は cwd をエンコードしたもの（`C--Users-me-repo`）。
/// どの行にも `cwd` がない場合にだけ使う。
fn fallback_project(path: &Path) -> String {
    let mut p = path.parent();
    while let Some(dir) = p {
        if dir
            .parent()
            .and_then(|pp| pp.file_name())
            .map(|n| n == "projects")
            .unwrap_or(false)
        {
            return dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
        }
        p = dir.parent();
    }
    String::from("(unknown)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_streamed_lines_and_keeps_final_output() {
        let jsonl = br#"{"type":"assistant","timestamp":"2026-10-01T00:00:00Z","sessionId":"s1","cwd":"C:\\repo","requestId":"r1","message":{"id":"m1","model":"claude-opus-5-5","usage":{"input_tokens":2,"output_tokens":7,"cache_read_input_tokens":100,"cache_creation_input_tokens":50,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":50}}}}
{"type":"assistant","timestamp":"2026-10-01T00:00:01Z","sessionId":"s1","cwd":"C:\\repo","requestId":"r1","message":{"id":"m1","model":"claude-opus-5-5","usage":{"input_tokens":2,"output_tokens":597,"cache_read_input_tokens":100,"cache_creation_input_tokens":50,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":50}}}}
{"type":"custom-title","customTitle":"My session","sessionId":"s1"}
{"type":"assistant","timestamp":"2026-10-01T00:00:02Z","sessionId":"s1","message":{"id":"x","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0}}}
"#;
        let pf = parse_bytes(Path::new("projects/C--repo/s1.jsonl"), jsonl);
        assert_eq!(pf.records.len(), 1);
        let r = &pf.records[0];
        assert_eq!(r.t.output, 597);
        assert_eq!(r.t.cache_write_1h, 50);
        assert_eq!(r.t.cache_write_5m, 0);
        assert_eq!(pf.strings[r.project as usize], "C:\\repo");
        assert_eq!(pf.titles, vec![("s1".to_string(), "My session".to_string())]);
    }
}
