use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Service {
    Claude,
    Codex,
}

impl Service {
    pub const ALL: [Service; 2] = [Service::Claude, Service::Codex];

    pub fn id(self) -> &'static str {
        match self {
            Service::Claude => "claude",
            Service::Codex => "codex",
        }
    }

    pub fn from_id(s: &str) -> Option<Service> {
        match s {
            "claude" => Some(Service::Claude),
            "codex" => Some(Service::Codex),
            _ => None,
        }
    }
}

/// メインの会話ではなくサブエージェント（サイドチェーン）として実行されたリクエスト。
pub const F_SUBAGENT: u8 = 1;
/// Claude の fast モード（`usage.speed == "fast"`）。
pub const F_FAST: u8 = 2;
/// Claude の US 限定推論（`usage.inference_geo == "us"`）。
pub const F_GEO_US: u8 = 4;

/// 区分が重ならないよう正規化したトークン数。
/// `input` はキャッシュされていない入力のみで、キャッシュ読込・書込は別に持つ。
/// `reasoning` は参考値で、すでに `output` に含まれている。
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub reasoning: u64,
    pub web_search: u32,
}

impl Tokens {
    pub fn add(&mut self, o: &Tokens) {
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_write_5m += o.cache_write_5m;
        self.cache_write_1h += o.cache_write_1h;
        self.reasoning += o.reasoning;
        self.web_search += o.web_search;
    }

    pub fn max_with(&mut self, o: &Tokens) {
        self.input = self.input.max(o.input);
        self.output = self.output.max(o.output);
        self.cache_read = self.cache_read.max(o.cache_read);
        self.cache_write_5m = self.cache_write_5m.max(o.cache_write_5m);
        self.cache_write_1h = self.cache_write_1h.max(o.cache_write_1h);
        self.reasoning = self.reasoning.max(o.reasoning);
        self.web_search = self.web_search.max(o.web_search);
    }

    pub fn cache_write(&self) -> u64 {
        self.cache_write_5m + self.cache_write_1h
    }

    /// このリクエストのプロンプトサイズ。長文脈料金の判定に使う。
    pub fn prompt(&self) -> u64 {
        self.input + self.cache_read + self.cache_write()
    }

    pub fn total(&self) -> u64 {
        self.prompt() + self.output
    }

    pub fn is_zero(&self) -> bool {
        self.total() == 0 && self.web_search == 0
    }
}

/// ファイル単位のキャッシュに保存する 1 リクエスト分の記録。文字列フィールドは
/// 所属する `ParsedFile::strings` テーブルへのインデックス。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileRecord {
    pub key: u64,
    pub ts: i64,
    pub model: u32,
    pub project: u32,
    pub session: u32,
    pub flags: u8,
    pub t: Tokens,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RateWindow {
    pub used_percent: f64,
    pub window_minutes: Option<u64>,
    pub resets_at: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RateLimitSnapshot {
    pub ts: i64,
    pub plan_type: Option<String>,
    pub primary: Option<RateWindow>,
    pub secondary: Option<RateWindow>,
}

/// ある時点の使用上限の消費率（Codex の `rate_limits` の 1 枠分）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RateSample {
    pub ts: i64,
    pub window_minutes: u64,
    pub used_percent: f64,
    /// リセット時刻（UTC ミリ秒）
    pub resets_at: Option<i64>,
    pub limit_id: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ParsedFile {
    pub strings: Vec<String>,
    pub records: Vec<FileRecord>,
    /// (セッション ID, タイトル)
    pub titles: Vec<(String, String)>,
    /// その時刻に観測されたプラン種別（Codex の `rate_limits.plan_type`）。
    pub plan_marks: Vec<(i64, String)>,
    pub rate_limit: Option<RateLimitSnapshot>,
    /// 使用率が変化した時点の記録（枠ごと）
    pub rate_samples: Vec<RateSample>,
}

impl ParsedFile {
    pub fn intern(&mut self, s: &str) -> u32 {
        if let Some(i) = self.strings.iter().position(|x| x == s) {
            return i as u32;
        }
        self.strings.push(s.to_string());
        (self.strings.len() - 1) as u32
    }
}

/// FNV-1a。Rust のバージョンが変わっても値が変わらないので、キャッシュ済みのキーと比較できる。
pub fn stable_hash(parts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for p in parts {
        for b in p.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub fn parse_ts(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.timestamp_millis())
}

pub fn contains(hay: &[u8], needle: &[u8]) -> bool {
    memchr::memmem::find(hay, needle).is_some()
}
