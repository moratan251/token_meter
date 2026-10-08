//! モデルの単価（100 万トークンあたりの USD）と API 換算額の計算。
//!
//! 単価の決定順: ユーザー設定 > （確認済みの内蔵価格 | 最新の LiteLLM。
//! `Settings::price_priority` で切り替え）> ビルド時に同梱した LiteLLM のスナップショット。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{Tokens, F_FAST, F_GEO_US};
use crate::settings::{write_atomic, Settings};

const CURATED_JSON: &str = include_str!("../resources/curated_prices.json");
const SNAPSHOT_JSON: &str = include_str!("../resources/litellm_snapshot.json");
const SNAPSHOT_DATE: &str = "2026-10-09";

/// 長文脈料金。プロンプトが `above` を超えたリクエスト全体に適用する。
/// 指定のない単価は、入力単価の比率で基本単価を拡大して求める。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Tier {
    pub above: u64,
    #[serde(default)]
    pub input: Option<f64>,
    #[serde(default)]
    pub output: Option<f64>,
    #[serde(default)]
    pub cache_read: Option<f64>,
    #[serde(default)]
    pub cache_write_5m: Option<f64>,
    #[serde(default)]
    pub cache_write_1h: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PriceSpec {
    pub input: f64,
    pub output: f64,
    #[serde(default)]
    pub cache_read: Option<f64>,
    #[serde(default)]
    pub cache_write_5m: Option<f64>,
    #[serde(default)]
    pub cache_write_1h: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tiers: Vec<Tier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_multiplier: Option<f64>,
}

#[derive(Clone, Copy, Debug)]
struct Rates {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write_5m: f64,
    cache_write_1h: f64,
}

impl PriceSpec {
    fn rates_for(&self, prompt: u64) -> Rates {
        let base = Rates {
            input: self.input,
            output: self.output,
            cache_read: self.cache_read.unwrap_or(self.input * 0.1),
            cache_write_5m: self.cache_write_5m.unwrap_or(self.input * 1.25),
            cache_write_1h: self.cache_write_1h.unwrap_or(self.input * 2.0),
        };
        let Some(t) = self
            .tiers
            .iter()
            .filter(|t| prompt > t.above)
            .max_by_key(|t| t.above)
        else {
            return base;
        };
        let scale = match t.input {
            Some(i) if self.input > 0.0 => i / self.input,
            _ => 1.0,
        };
        Rates {
            input: t.input.unwrap_or(base.input),
            output: t.output.unwrap_or(base.output),
            cache_read: t.cache_read.unwrap_or(base.cache_read * scale),
            cache_write_5m: t.cache_write_5m.unwrap_or(base.cache_write_5m * scale),
            cache_write_1h: t.cache_write_1h.unwrap_or(base.cache_write_1h * scale),
        }
    }
}

/// API 換算額（USD）を課金の区分ごとに分けたもの。
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Cost {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub web: f64,
    /// キャッシュ読込分を通常の入力として払った場合の額から、実際の額を引いたもの。
    /// 参考値で、`total()` には含めない。
    pub cache_savings: f64,
}

impl Cost {
    pub fn total(&self) -> f64 {
        self.input + self.output + self.cache_read + self.cache_write + self.web
    }

    pub fn add(&mut self, o: &Cost) {
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_write += o.cache_write;
        self.web += o.web;
        self.cache_savings += o.cache_savings;
    }
}

pub fn cost_of(spec: &PriceSpec, t: &Tokens, flags: u8, settings: &Settings) -> Cost {
    let r = spec.rates_for(t.prompt());
    let mut m = 1.0;
    if flags & F_FAST != 0 {
        m *= spec.fast_multiplier.unwrap_or(1.0);
    }
    if flags & F_GEO_US != 0 {
        m *= settings.geo_us_multiplier;
    }
    let per = |n: u64, rate: f64| n as f64 * rate * m / 1_000_000.0;
    Cost {
        input: per(t.input, r.input),
        output: per(t.output, r.output),
        cache_read: per(t.cache_read, r.cache_read),
        cache_write: per(t.cache_write_5m, r.cache_write_5m) + per(t.cache_write_1h, r.cache_write_1h),
        web: t.web_search as f64 * settings.web_search_usd,
        cache_savings: per(t.cache_read, (r.input - r.cache_read).max(0.0)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PriceSource {
    User,
    Curated,
    Litellm,
    LitellmSnapshot,
}

#[derive(Clone, Debug, Serialize)]
pub struct Resolved {
    /// 一致した価格表のキー。
    pub key: String,
    pub source: PriceSource,
    pub spec: PriceSpec,
    /// 別モデルの価格を使っている場合に、そのモデル名が入る。
    pub alias: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct LatestFile {
    fetched_at: String,
    models: HashMap<String, PriceSpec>,
}

pub struct PriceBook {
    pub curated: HashMap<String, PriceSpec>,
    pub snapshot: HashMap<String, PriceSpec>,
    pub latest: HashMap<String, PriceSpec>,
    pub latest_fetched_at: Option<String>,
    latest_path: PathBuf,
}

impl PriceBook {
    pub fn load(data_dir: &Path) -> PriceBook {
        #[derive(Deserialize)]
        struct Curated {
            models: HashMap<String, PriceSpec>,
        }
        let curated = serde_json::from_str::<Curated>(CURATED_JSON)
            .map(|c| c.models)
            .unwrap_or_default();
        let snapshot = import_litellm(SNAPSHOT_JSON).unwrap_or_default();
        let latest_path = data_dir.join("litellm_prices.json");
        let latest = std::fs::read_to_string(&latest_path)
            .ok()
            .and_then(|s| serde_json::from_str::<LatestFile>(&s).ok());
        PriceBook {
            curated,
            snapshot,
            latest_fetched_at: latest.as_ref().map(|l| l.fetched_at.clone()),
            latest: latest.map(|l| l.models).unwrap_or_default(),
            latest_path,
        }
    }

    pub fn snapshot_date(&self) -> &'static str {
        SNAPSHOT_DATE
    }

    pub fn set_latest(&mut self, models: HashMap<String, PriceSpec>, fetched_at: String) -> Result<(), String> {
        let file = LatestFile {
            fetched_at: fetched_at.clone(),
            models,
        };
        if let Some(dir) = self.latest_path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_vec(&file).map_err(|e| e.to_string())?;
        write_atomic(&self.latest_path, &json)?;
        self.latest = file.models;
        self.latest_fetched_at = Some(fetched_at);
        Ok(())
    }

    pub fn resolve(&self, model: &str, settings: &Settings) -> Option<Resolved> {
        if let Some(spec) = lookup_ci(&settings.price_overrides, model) {
            return Some(Resolved {
                key: model.to_string(),
                source: PriceSource::User,
                spec: spec.clone(),
                alias: None,
            });
        }
        let alias = lookup_ci(&settings.aliases, model).filter(|a| !a.trim().is_empty());
        let target = alias.map(|s| s.as_str()).unwrap_or(model);
        if alias.is_some() {
            if let Some(spec) = lookup_ci(&settings.price_overrides, target) {
                return Some(Resolved {
                    key: target.to_string(),
                    source: PriceSource::User,
                    spec: spec.clone(),
                    alias: alias.cloned(),
                });
            }
        }

        let latest = (&self.latest, PriceSource::Litellm);
        let curated = (&self.curated, PriceSource::Curated);
        let snapshot = (&self.snapshot, PriceSource::LitellmSnapshot);
        let order = if settings.price_priority == "litellm" {
            [latest, curated, snapshot]
        } else {
            [curated, latest, snapshot]
        };
        let cands = candidates(target);
        for (table, source) in order {
            for c in &cands {
                if let Some(spec) = table.get(c) {
                    return Some(Resolved {
                        key: c.clone(),
                        source,
                        spec: spec.clone(),
                        alias: alias.cloned(),
                    });
                }
            }
        }
        None
    }

    /// 取得済みなら最新の LiteLLM の価格、なければ同梱のスナップショット。
    pub fn litellm(&self, key: &str) -> Option<(&PriceSpec, bool)> {
        self.latest
            .get(key)
            .map(|s| (s, true))
            .or_else(|| self.snapshot.get(key).map(|s| (s, false)))
    }
}

fn lookup_ci<'a, V>(map: &'a std::collections::BTreeMap<String, V>, key: &str) -> Option<&'a V> {
    map.get(key).or_else(|| {
        map.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v)
    })
}

/// ログ上のモデル名から作る検索キー。具体的なものから順に並ぶ。
pub fn candidates(model: &str) -> Vec<String> {
    let mut m = model.trim().to_lowercase();
    if let Some(i) = m.find('[') {
        m.truncate(i); // "claude-opus-4-6[1m]"
    }
    if let Some(i) = m.rfind('/') {
        m = m[i + 1..].to_string();
    }
    if let Some(i) = m.find("anthropic.") {
        m = m[i + "anthropic.".len()..].to_string();
    }
    if let Some(stripped) = m.strip_suffix(":0") {
        if let Some(i) = stripped.rfind("-v") {
            m = stripped[..i].to_string(); // Bedrock "-v1:0"
        }
    }
    let mut out = vec![m.clone()];
    let undated = strip_date(&m);
    if undated != m {
        out.push(undated);
    }
    out
}

fn strip_date(m: &str) -> String {
    if let Some(i) = m.rfind(['-', '@']) {
        let tail = &m[i + 1..];
        let digits = tail.chars().filter(|c| c.is_ascii_digit()).count();
        if (tail.len() == 8 && digits == 8) || (tail.len() == 10 && digits == 8 && tail.matches('-').count() == 2) {
            return m[..i].to_string();
        }
    }
    // "gpt-5-2025-08-07"
    if m.len() > 11 && m.is_char_boundary(m.len() - 11) {
        let (head, tail) = m.split_at(m.len() - 11);
        let b = tail.as_bytes();
        let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
        if b[0] == b'-' && b[5] == b'-' && b[8] == b'-' && digits(1..5) && digits(6..8) && digits(9..11) {
            return head.to_string();
        }
    }
    m.to_string()
}

const PROVIDERS: &[&str] = &["anthropic", "openai", "gemini", "xai", "deepseek", "moonshot"];

/// LiteLLM の `model_prices_and_context_window.json` を価格情報に変換する。
pub fn import_litellm(text: &str) -> Result<HashMap<String, PriceSpec>, String> {
    let root: serde_json::Map<String, Value> =
        serde_json::from_str(text).map_err(|e| format!("価格表の JSON を解析できません: {e}"))?;
    let mut out: HashMap<String, (PriceSpec, bool)> = HashMap::new();
    for (key, entry) in &root {
        let Some(obj) = entry.as_object() else { continue };
        let provider = obj.get("litellm_provider").and_then(Value::as_str).unwrap_or("");
        if !PROVIDERS.contains(&provider) {
            continue;
        }
        if let Some(mode) = obj.get("mode").and_then(Value::as_str) {
            if mode != "chat" && mode != "responses" {
                continue;
            }
        }
        let f = |k: &str| obj.get(k).and_then(Value::as_f64).map(|v| v * 1_000_000.0);
        let Some(input) = f("input_cost_per_token") else { continue };
        let mut spec = PriceSpec {
            input,
            output: f("output_cost_per_token").unwrap_or(0.0),
            cache_read: f("cache_read_input_token_cost"),
            cache_write_5m: f("cache_creation_input_token_cost"),
            cache_write_1h: f("cache_creation_input_token_cost_above_1hr"),
            tiers: Vec::new(),
            fast_multiplier: None,
        };

        let mut tiers: HashMap<u64, Tier> = HashMap::new();
        for (k, v) in obj {
            let Some(v) = v.as_f64() else { continue };
            if ["priority", "flex", "batch", "ultrafast"].iter().any(|x| k.contains(x)) {
                continue;
            }
            let v = Some(v * 1_000_000.0);
            let patterns: [(&str, fn(&mut Tier, Option<f64>)); 5] = [
                ("cache_creation_input_token_cost_above_1hr_above_", |t, v| t.cache_write_1h = v),
                ("cache_creation_input_token_cost_above_", |t, v| t.cache_write_5m = v),
                ("cache_read_input_token_cost_above_", |t, v| t.cache_read = v),
                ("input_cost_per_token_above_", |t, v| t.input = v),
                ("output_cost_per_token_above_", |t, v| t.output = v),
            ];
            for (prefix, set) in patterns {
                if let Some(n) = k
                    .strip_prefix(prefix)
                    .and_then(|r| r.strip_suffix("k_tokens"))
                    .and_then(|n| n.parse::<u64>().ok())
                {
                    let above = n * 1000;
                    let t = tiers.entry(above).or_insert_with(|| Tier {
                        above,
                        ..Default::default()
                    });
                    set(t, v);
                    break;
                }
            }
        }
        let mut tiers: Vec<Tier> = tiers.into_values().collect();
        tiers.sort_by_key(|t| t.above);
        spec.tiers = tiers;

        let prefixed = key.contains('/');
        let name = key.rsplit('/').next().unwrap_or(key).to_lowercase();
        match out.get(&name) {
            Some((_, existing_prefixed)) if !*existing_prefixed || prefixed => {}
            _ => {
                out.insert(name, (spec, prefixed));
            }
        }
    }
    Ok(out.into_iter().map(|(k, (s, _))| (k, s)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_normalisation() {
        assert_eq!(candidates("claude-haiku-4-5-20251001"), vec!["claude-haiku-4-5-20251001", "claude-haiku-4-5"]);
        assert_eq!(candidates("claude-opus-4-6[1m]"), vec!["claude-opus-4-6"]);
        assert_eq!(candidates("us.anthropic.claude-opus-4-6-v1:0"), vec!["claude-opus-4-6"]);
        assert_eq!(candidates("gpt-5-2025-08-07"), vec!["gpt-5-2025-08-07", "gpt-5"]);
        assert_eq!(candidates("gpt-5.6-sol"), vec!["gpt-5.6-sol"]);
    }

    #[test]
    fn tiered_cost() {
        let book = PriceBook::load(Path::new("."));
        let s = Settings::default();
        let haiku = book.resolve("claude-haiku-5-5", &s).unwrap();
        assert_eq!(haiku.source, PriceSource::Curated);
        let small = Tokens { input: 50_000, output: 1_000, ..Default::default() };
        assert!((cost_of(&haiku.spec, &small, 0, &s).total() - (0.05 * 0.1 + 0.001 * 0.5)).abs() < 1e-9);
        let big = Tokens { input: 50_000, cache_read: 60_000, ..Default::default() };
        let c = cost_of(&haiku.spec, &big, 0, &s);
        assert!((c.input - 0.05 * 0.5).abs() < 1e-9);
        assert!((c.cache_read - 0.06 * 0.05).abs() < 1e-9);
    }

    #[test]
    fn snapshot_covers_codex_models() {
        let book = PriceBook::load(Path::new("."));
        let s = Settings::default();
        let r = book.resolve("gpt-5.6-sol", &s).unwrap();
        assert_eq!(r.source, PriceSource::LitellmSnapshot);
        assert!((r.spec.input - 4.0).abs() < 1e-9);
        assert_eq!(r.spec.tiers.first().map(|t| t.above), Some(272_000));
        let sonnet = book.resolve("claude-sonnet-5-5", &s).unwrap();
        assert_eq!(sonnet.spec.cache_read, Some(0.2));
    }
}
