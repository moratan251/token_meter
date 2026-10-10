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

/// 手動で記録した使用上限の消費率。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ManualLimit {
    pub id: String,
    pub service: String,
    /// "session"（5時間枠）| "weekly"（週間・すべて）| "weekly_model"（週間・モデル別）
    pub kind: String,
    /// `weekly_model` の対象モデル名（例: "fable"）。モデル名にこの文字列を含む使用量を対象にする。
    #[serde(default)]
    pub model: Option<String>,
    pub percent: f64,
    /// 記録した時刻（UTC ミリ秒）
    pub ts: i64,
    #[serde(default)]
    pub resets_at: Option<i64>,
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
    pub manual_limits: Vec<ManualLimit>,
    /// Claude デスクトップアプリが記録する使用率の履歴を読み込む
    pub claude_desktop_usage: bool,
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
            manual_limits: Vec::new(),
            claude_desktop_usage: true,
        }
    }
}

impl Settings {
    /// 設定を読み込む。読めない・壊れている場合は既定値を返し、元のファイルを別名で残して
    /// 警告を返す（そのまま既定値で保存すると、プランや手動記録が消えてしまうため）。
    pub fn load(path: &Path) -> (Settings, Option<String>) {
        let text = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Settings::default(), None),
            Err(e) => {
                let backup = backup_broken(path);
                return (Settings::default(), Some(load_warning(&e.to_string(), backup.as_deref())));
            }
        };
        let text = text.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&text);
        match serde_json::from_slice(text) {
            Ok(s) => (s, None),
            Err(e) => {
                let backup = backup_broken(path);
                (Settings::default(), Some(load_warning(&e.to_string(), backup.as_deref())))
            }
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        write_atomic(path, json.as_bytes())
    }
}

/// 壊れた設定ファイルを `settings.broken-<日時>.json` という名前でコピーして残す。
fn backup_broken(path: &Path) -> Option<String> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let stem = path.file_stem()?.to_string_lossy().to_string();
    let backup = path.with_file_name(format!("{stem}.broken-{stamp}.json"));
    std::fs::copy(path, &backup).ok()?;
    Some(backup.to_string_lossy().to_string())
}

fn load_warning(err: &str, backup: Option<&str>) -> String {
    match backup {
        Some(b) => format!("設定ファイルを読み込めなかったため、既定の設定で起動しました（{err}）。元のファイルは {b} に残しています。"),
        None => format!("設定ファイルを読み込めなかったため、既定の設定で起動しました（{err}）。"),
    }
}

/// 一時ファイルに書いてディスクに反映してから置き換える。一時ファイル名は書き込みごとに変え、
/// 同時に保存しても互いの一時ファイルを壊さないようにする。
pub fn write_atomic(path: &Path, data: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let tmp = path.with_file_name(format!(
        ".{name}.{}-{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broken_settings_are_kept_aside() {
        let dir = std::env::temp_dir().join(format!("token-meter-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        // ファイルがなければ警告なしで既定値
        let (s, w) = Settings::load(&path);
        assert!(w.is_none());
        assert!(s.plans.is_empty());

        // 保存した内容は読み戻せる（BOM 付きでも読める）
        let mut s = Settings::default();
        s.currency = "USD".into();
        s.save(&path).unwrap();
        let mut bom = b"\xEF\xBB\xBF".to_vec();
        bom.extend(std::fs::read(&path).unwrap());
        std::fs::write(&path, bom).unwrap();
        let (s, w) = Settings::load(&path);
        assert!(w.is_none());
        assert_eq!(s.currency, "USD");

        // 壊れていれば既定値で起動し、元のファイルを残す
        std::fs::write(&path, b"{ \"plans\": [ broken").unwrap();
        let (s, w) = Settings::load(&path);
        assert!(w.is_some());
        assert_eq!(s.currency, "JPY");
        let kept = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("settings.broken-"));
        assert!(kept);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
