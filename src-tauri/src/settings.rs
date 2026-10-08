use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::pricing::PriceSpec;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExtraDir {
    pub service: String,
    pub path: String,
}

/// サブスクの契約期間。プラン変更は、前のプランが終わる日から始まる新しい項目として登録する。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub service: String,
    pub name: String,
    /// `currency` 建ての月額。
    pub price: f64,
    pub currency: String,
    /// 最初の請求サイクルの初日（`YYYY-MM-DD`、ローカル時刻）。
    pub start: String,
    /// 終了日（この日は含まない、`YYYY-MM-DD`）。契約中は `None`。
    #[serde(default)]
    pub end: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub extra_dirs: Vec<ExtraDir>,
    pub disabled_dirs: Vec<String>,
    /// 削除されたログファイルの使用量も残す（Claude Code は `cleanupPeriodDays` を
    /// 過ぎた古い会話ログを削除する）。
    pub keep_deleted: bool,
    pub currency: String,
    pub usd_jpy: f64,
    pub usd_jpy_updated: Option<String>,
    /// 1 = 週の始まりが月曜、0 = 日曜。
    pub week_start: u8,
    pub auto_refresh_secs: u32,
    pub plans: Vec<Plan>,
    pub price_overrides: BTreeMap<String, PriceSpec>,
    /// ログに記録されたモデル名 -> 価格を使うモデル名
    pub aliases: BTreeMap<String, String>,
    /// "curated" = 確認済みの内蔵価格を LiteLLM より優先、"litellm" = LiteLLM を優先。
    pub price_priority: String,
    pub auto_update_prices: bool,
    pub geo_us_multiplier: f64,
    /// サーバー側の Web 検索 1 回あたりの USD。
    pub web_search_usd: f64,
    pub theme: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            extra_dirs: Vec::new(),
            disabled_dirs: Vec::new(),
            keep_deleted: true,
            currency: "JPY".into(),
            usd_jpy: 150.0,
            usd_jpy_updated: None,
            week_start: 1,
            auto_refresh_secs: 60,
            plans: Vec::new(),
            price_overrides: BTreeMap::new(),
            aliases: BTreeMap::new(),
            price_priority: "curated".into(),
            auto_update_prices: true,
            geo_us_multiplier: 1.1,
            web_search_usd: 0.01,
            theme: "system".into(),
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Settings {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        write_atomic(path, json.as_bytes())
    }
}

pub fn write_atomic(path: &Path, data: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}
