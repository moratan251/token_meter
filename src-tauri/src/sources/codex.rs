//! Codex のロールアウト: `<codex home>/sessions/YYYY/MM/DD/rollout-*.jsonl` と
//! `<codex home>/archived_sessions/*.jsonl`。
//!
//! 新しい Codex は API レスポンスごとに `token_usage_record`（一意な `response_id` 付き）を
//! 書き出す。これには従来の `token_count` イベントに出ない圧縮処理などの呼び出しも含まれる。
//! あればこちらを使い、それより前に書かれた部分は `token_count` の差分で補う。
//!
//! OpenAI の定義では `input_tokens` は `cached_input_tokens` を含み、
//! `output_tokens` は `reasoning_output_tokens` を含む。

use std::path::Path;

use serde::Deserialize;
use serde_json::value::RawValue;

use crate::model::*;

#[derive(Deserialize)]
struct Line<'a> {
    timestamp: Option<String>,
    #[serde(rename = "type")]
    ty: String,
    #[serde(borrow)]
    payload: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct SessionMeta {
    id: Option<String>,
    session_id: Option<String>,
    parent_thread_id: Option<String>,
    cwd: Option<String>,
    source: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct TurnContext {
    model: Option<String>,
    cwd: Option<String>,
}

#[derive(Deserialize)]
struct ThreadSettingsApplied {
    thread_settings: Option<TurnContext>,
}

/// モデルが判明する前に記録された使用量の仮のモデル名。古い Codex が派生させた
/// サブエージェントはモデル名を記録しないため、store が親セッションから補う。
pub const UNKNOWN_MODEL: &str = "unknown";

#[derive(Deserialize, Clone, Copy, Default, PartialEq)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cached_input_tokens: u64,
    #[serde(default)]
    cache_write_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    reasoning_output_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

impl Usage {
    fn sub(&self, o: &Usage) -> Option<Usage> {
        Some(Usage {
            input_tokens: self.input_tokens.checked_sub(o.input_tokens)?,
            cached_input_tokens: self.cached_input_tokens.checked_sub(o.cached_input_tokens)?,
            cache_write_input_tokens: self
                .cache_write_input_tokens
                .checked_sub(o.cache_write_input_tokens)?,
            output_tokens: self.output_tokens.checked_sub(o.output_tokens)?,
            reasoning_output_tokens: self
                .reasoning_output_tokens
                .checked_sub(o.reasoning_output_tokens)?,
            total_tokens: self.total_tokens.checked_sub(o.total_tokens)?,
        })
    }

    fn to_tokens(self) -> Tokens {
        let cached = self.cached_input_tokens.min(self.input_tokens);
        let write = self
            .cache_write_input_tokens
            .min(self.input_tokens - cached);
        Tokens {
            input: self.input_tokens - cached - write,
            output: self.output_tokens,
            cache_read: cached,
            cache_write_5m: write,
            cache_write_1h: 0,
            reasoning: self.reasoning_output_tokens,
            web_search: 0,
        }
    }
}

#[derive(Deserialize)]
struct TokenCount {
    info: Option<TokenInfo>,
    rate_limits: Option<RateLimits>,
}

#[derive(Deserialize)]
struct TokenInfo {
    total_token_usage: Option<Usage>,
    last_token_usage: Option<Usage>,
}

#[derive(Deserialize)]
struct RateLimits {
    plan_type: Option<String>,
    primary: Option<RateWin>,
    secondary: Option<RateWin>,
}

#[derive(Deserialize)]
struct RateWin {
    used_percent: Option<f64>,
    window_minutes: Option<u64>,
    resets_at: Option<i64>,
}

impl RateWin {
    fn to_window(&self) -> RateWindow {
        RateWindow {
            used_percent: self.used_percent.unwrap_or(0.0),
            window_minutes: self.window_minutes,
            resets_at: self.resets_at,
        }
    }
}

#[derive(Deserialize)]
struct UsageRecord {
    response_id: Option<String>,
    usage: Option<Usage>,
}

struct Pending {
    ts: i64,
    model: String,
    project: String,
    usage: Usage,
    key: u64,
}

pub fn parse_file(path: &Path) -> std::io::Result<ParsedFile> {
    let data = std::fs::read(path)?;
    Ok(parse_bytes(path, &data))
}

fn parse_bytes(path: &Path, data: &[u8]) -> ParsedFile {
    let mut pf = ParsedFile::default();
    let file_id = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut thread_id = file_id.clone();
    let mut session = file_id.clone();
    let mut is_sub = false;
    let mut cwd = String::from("(unknown)");
    let mut model = String::from(UNKNOWN_MODEL);
    let mut seen_meta = false;

    let mut from_counts: Vec<Pending> = Vec::new();
    let mut from_records: Vec<Pending> = Vec::new();
    let mut prev_total: Option<Usage> = None;
    let mut last_plan: Option<String> = None;

    for raw in data.split(|&b| b == b'\n') {
        let kind = if contains(raw, b"\"type\":\"token_count\"") {
            1
        } else if contains(raw, b"\"type\":\"token_usage_record\"") {
            2
        } else if contains(raw, b"\"type\":\"turn_context\"") {
            3
        } else if !seen_meta && contains(raw, b"\"type\":\"session_meta\"") {
            4
        } else if contains(raw, b"\"type\":\"thread_settings_applied\"") {
            5
        } else {
            continue;
        };
        let Ok(line) = serde_json::from_slice::<Line>(raw) else {
            continue;
        };
        let Some(payload) = line.payload else { continue };
        let ts = line.timestamp.as_deref().and_then(parse_ts);

        match (kind, line.ty.as_str()) {
            (4, "session_meta") => {
                let Ok(m) = serde_json::from_str::<SessionMeta>(payload.get()) else {
                    continue;
                };
                seen_meta = true;
                if let Some(id) = m.id {
                    thread_id = id;
                }
                let spawned_by_parent = m
                    .source
                    .as_ref()
                    .map(|s| s.get("subagent").is_some())
                    .unwrap_or(false);
                is_sub = spawned_by_parent || m.parent_thread_id.is_some();
                session = m
                    .session_id
                    .or(m.parent_thread_id)
                    .unwrap_or_else(|| thread_id.clone());
                if let Some(c) = m.cwd {
                    cwd = c;
                }
            }
            (3, "turn_context") => {
                if let Ok(t) = serde_json::from_str::<TurnContext>(payload.get()) {
                    if let Some(m) = t.model {
                        model = m;
                    }
                    if let Some(c) = t.cwd {
                        cwd = c;
                    }
                }
            }
            (5, "event_msg") => {
                if let Ok(ts) = serde_json::from_str::<ThreadSettingsApplied>(payload.get()) {
                    if let Some(m) = ts.thread_settings.and_then(|s| s.model) {
                        model = m;
                    }
                }
            }
            (1, "event_msg") => {
                let Ok(tc) = serde_json::from_str::<TokenCount>(payload.get()) else {
                    continue;
                };
                let Some(ts) = ts else { continue };
                if let Some(rl) = tc.rate_limits {
                    if let Some(p) = &rl.plan_type {
                        if last_plan.as_ref() != Some(p) {
                            pf.plan_marks.push((ts, p.clone()));
                            last_plan = Some(p.clone());
                        }
                    }
                    pf.rate_limit = Some(RateLimitSnapshot {
                        ts,
                        plan_type: rl.plan_type,
                        primary: rl.primary.as_ref().map(RateWin::to_window),
                        secondary: rl.secondary.as_ref().map(RateWin::to_window),
                    });
                }
                let Some(info) = tc.info else { continue };
                let Some(total) = info.total_token_usage else { continue };
                if prev_total == Some(total) {
                    continue; // 同じイベントの繰り返しで、新しい使用量はない
                }
                // 累計の差分を優先する。累計が減った場合（コンテキストのリセット）は
                // この呼び出し自身の使用量を使う。
                let delta = prev_total
                    .and_then(|p| total.sub(&p))
                    .or(if prev_total.is_none() { Some(total) } else { None })
                    .or(info.last_token_usage)
                    .unwrap_or_default();
                prev_total = Some(total);
                from_counts.push(Pending {
                    ts,
                    model: model.clone(),
                    project: cwd.clone(),
                    usage: delta,
                    key: stable_hash(&[
                        "cx-tc",
                        &thread_id,
                        &ts.to_string(),
                        &total.total_tokens.to_string(),
                    ]),
                });
            }
            (2, "token_usage_record") => {
                let Ok(r) = serde_json::from_str::<UsageRecord>(payload.get()) else {
                    continue;
                };
                let (Some(ts), Some(usage)) = (ts, r.usage) else { continue };
                let key = match &r.response_id {
                    Some(id) => stable_hash(&["cx-resp", id]),
                    None => stable_hash(&["cx-rec", &thread_id, &ts.to_string()]),
                };
                from_records.push(Pending {
                    ts,
                    model: model.clone(),
                    project: cwd.clone(),
                    usage,
                    key,
                });
            }
            _ => {}
        }
    }

    let first_record_ts = from_records.iter().map(|p| p.ts).min();
    let flags = if is_sub { F_SUBAGENT } else { 0 };
    let all = from_counts
        .into_iter()
        .filter(|p| first_record_ts.map(|t0| p.ts < t0).unwrap_or(true))
        .chain(from_records);
    for p in all {
        let t = p.usage.to_tokens();
        if t.is_zero() {
            continue;
        }
        let rec = FileRecord {
            key: p.key,
            ts: p.ts,
            model: pf.intern(&p.model),
            project: pf.intern(&p.project),
            session: pf.intern(&session),
            flags,
            t,
        };
        pf.records.push(rec);
    }
    pf.records.sort_by_key(|r| r.ts);
    pf
}

/// `session_index.jsonl` には Codex の画面に表示されるスレッド名が入っている。
pub fn parse_session_index(path: &Path) -> Vec<(String, String)> {
    #[derive(Deserialize)]
    struct Entry {
        id: Option<String>,
        thread_name: Option<String>,
    }
    let Ok(data) = std::fs::read(path) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = Vec::new();
    for raw in data.split(|&b| b == b'\n') {
        if let Ok(e) = serde_json::from_slice::<Entry>(raw) {
            if let (Some(id), Some(name)) = (e.id, e.thread_name) {
                if !name.trim().is_empty() {
                    out.push((id, name));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_usage_records_and_uses_deltas_before_them() {
        let jsonl = br#"{"timestamp":"2026-10-01T00:00:00Z","type":"session_meta","payload":{"id":"t1","cwd":"C:\\w","source":"vscode"}}
{"timestamp":"2026-10-01T00:00:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol","cwd":"C:\\w"}}
{"timestamp":"2026-10-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":10,"reasoning_output_tokens":4,"total_tokens":110},"last_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":10,"reasoning_output_tokens":4,"total_tokens":110}},"rate_limits":{"plan_type":"plus","primary":{"used_percent":12.5,"window_minutes":300,"resets_at":1790000000}}}}
{"timestamp":"2026-10-01T00:00:03Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":10,"reasoning_output_tokens":4,"total_tokens":110},"last_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":10,"reasoning_output_tokens":4,"total_tokens":110}}}}
{"timestamp":"2026-10-01T00:00:04Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":300,"cached_input_tokens":140,"output_tokens":30,"reasoning_output_tokens":4,"total_tokens":330},"last_token_usage":{"input_tokens":200,"cached_input_tokens":100,"output_tokens":20,"reasoning_output_tokens":0,"total_tokens":220}}}}
{"timestamp":"2026-10-01T00:00:05Z","type":"token_usage_record","payload":{"response_id":"resp_a","usage":{"input_tokens":500,"cached_input_tokens":400,"output_tokens":50,"reasoning_output_tokens":10,"total_tokens":550}}}
{"timestamp":"2026-10-01T00:00:06Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":800,"cached_input_tokens":540,"output_tokens":80,"reasoning_output_tokens":14,"total_tokens":880}}}}
"#;
        let pf = parse_bytes(Path::new("rollout-x.jsonl"), jsonl);
        assert_eq!(pf.records.len(), 3);
        assert_eq!(pf.records[0].t.input, 60);
        assert_eq!(pf.records[0].t.cache_read, 40);
        assert_eq!(pf.records[1].t.input, 100);
        assert_eq!(pf.records[1].t.output, 20);
        assert_eq!(pf.records[2].t.cache_read, 400);
        assert_eq!(pf.strings[pf.records[2].model as usize], "gpt-5.6-sol");
        assert_eq!(pf.plan_marks, vec![(1790812802000, "plus".to_string())]);
        assert!(pf.rate_limit.is_some());
    }
}
