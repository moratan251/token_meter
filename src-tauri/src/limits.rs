//! サブスクの使用上限（%）と API 換算額の比較。
//!
//! 使用率の取得元:
//! - Claude: デスクトップアプリが `%APPDATA%\Claude\plan-usage-history.json` に約 15 分ごとに
//!   記録する 5 時間枠（`fh`）と 7 日枠（`sd`）の使用率。元ファイルは古いものから消えていくので、
//!   読み込んだ分をアプリ側の `limit-history.json` に蓄積する。アプリを開いている間しか記録されない。
//! - Claude: Claude Code（端末の CLI。デスクトップアプリの Code タブでは動かない）のステータスラインに設定したスクリプト（`resources/statusline.ps1`）が
//!   `~/.claude/token-meter/rate-limits.jsonl` に書く使用率とリセット時刻。Code を使っている間に記録される。
//! - Codex: ログの `rate_limits`（リセット時刻付き）。
//! - 手動記録: ユーザーが入力・貼り付けした値（モデル別の枠など）。
//!
//! 枠の区切り: リセット時刻が分かればそれを使う。分からなければ、使用率の急な低下・0% への低下・
//! 枠の長さの経過で区切る。0% の記録は「枠が始まっていない」とみなし、次に使用率が上がった枠の
//! 開始時刻の下限に使う（Claude の枠は、リセット後に最初に使った時刻から始まる）。
//!
//! 推定方法: 同じ枠の中で使用率が上がった区間ごとに「増えた使用率」と「その区間の API 換算額」を
//! 集計し、比から 1% あたりの API 換算額を求める。リセット時刻が分からなくても使え、
//! 整数に丸められた使用率でも区間を重ねるほど誤差が小さくなる。
//!
//! 使用率にはチャットや他の端末での利用も含まれる。ログに Code の記録が 1 件もない区間で
//! 使用率が上がった分は「Code 以外の利用」として分母から除く。同じ区間に Code とチャットの
//! 両方がある場合は分けられないので、1% あたりの額はまだ少し低めに出ることがある。
//!
//! 5 時間枠と週間枠は同じ利用を数えるので、両方の増え方の比（5 時間枠の 1% が週間枠の何 % か）は
//! プランで決まる定数になる。この比で互いの推定値を照らし合わせ、大きく食い違えば信頼度の高い方の
//! 推定値から換算する。

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::model::{RateSample, Service};
use crate::pricing::PriceBook;
use crate::query::{date_ts, parse_date, price_table, rec_cost};
use crate::settings::{write_atomic, Settings};
use crate::store::Store;

const MIN: i64 = 60_000;
const DAY: i64 = 86_400_000;
/// 推定に使う期間（上限の仕様が変わることがあるので直近に絞る）
const ESTIMATE_DAYS: i64 = 30;
/// グラフに出す期間
const CHART_DAYS: i64 = 21;
/// 使用率への反映の遅れ。区間の開始よりこれだけ前の Code の利用も、その区間の増加の原因とみなす。
const LAG_BEFORE: i64 = 10 * MIN;
/// ログの記録時刻と使用率の記録時刻のずれの許容幅
const LAG_AFTER: i64 = 2 * MIN;
/// これ以下の使用率は「枠が始まっていない」とみなす（デスクトップアプリの記録は整数に丸められている）
const IDLE_PERCENT: f64 = 0.5;
/// 5 時間枠と週間枠の比を求めるときの、連続した記録の間隔の上限（リセット時刻が分からない場合）
const PAIR_GAP: i64 = 60 * MIN;
/// 推定値どうしがこの倍率以上ずれていたら食い違いとみなす
const MISMATCH: f64 = 2.5;
/// 最後の記録がこれより古ければ注意を出す
const STALE: i64 = 3 * DAY;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DesktopSample {
    pub t: i64,
    #[serde(default)]
    pub org: Option<String>,
    #[serde(default)]
    pub u: BTreeMap<String, f64>,
}

#[derive(Deserialize)]
struct DesktopFile {
    #[serde(default)]
    samples: Vec<DesktopSample>,
}

#[derive(Serialize, Deserialize, Default)]
struct HistoryFile {
    desktop: Vec<DesktopSample>,
}

/// ステータスラインのスクリプトが書く 1 行。`t` は UTC ミリ秒、リセット時刻は Unix 秒。
#[derive(Clone, Debug, Deserialize)]
pub struct StatuslineSample {
    pub t: i64,
    #[serde(default)]
    pub fh: Option<f64>,
    #[serde(default)]
    pub fh_reset: Option<i64>,
    #[serde(default)]
    pub sd: Option<f64>,
    #[serde(default)]
    pub sd_reset: Option<i64>,
}

fn parse_statusline(text: &str) -> Vec<StatuslineSample> {
    let mut v: Vec<StatuslineSample> = text
        .lines()
        .map(|l| l.trim_start_matches('\u{feff}').trim())
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_str::<StatuslineSample>(l).ok())
        .filter(|s| s.t > 0)
        .collect();
    v.sort_by_key(|s| s.t);
    v
}

/// Claude デスクトップアプリの使用率履歴ファイルの候補。
///
/// Microsoft Store 版（MSIX）のデスクトップアプリは、`%APPDATA%` への書き込みが
/// `%LOCALAPPDATA%\Packages\Claude_<発行元ID>\LocalCache\Roaming\` に振り替えられる。
/// 通常起動した Token Meter からは `%APPDATA%\Claude` に見えないので、両方を探す。
pub fn desktop_history_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        if let Ok(rd) = std::fs::read_dir(PathBuf::from(local).join("Packages")) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with("Claude_") {
                    out.push(e.path().join("LocalCache").join("Roaming").join("Claude").join("plan-usage-history.json"));
                }
            }
        }
    }
    if let Some(a) = std::env::var_os("APPDATA") {
        out.push(PathBuf::from(a).join("Claude").join("plan-usage-history.json"));
    }
    out
}

fn mtime_of(path: &Path) -> Option<i64> {
    std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as i64)
}

/// Claude デスクトップアプリとステータスラインの使用率履歴を読み込む。デスクトップアプリの分は
/// アプリ側にも蓄積する（ステータスラインの記録は Token Meter 専用のファイルなので消えない）。
pub struct LimitHistory {
    store_path: PathBuf,
    pub desktop: Vec<DesktopSample>,
    pub statusline: Vec<StatuslineSample>,
    /// 読み込んだ元ファイルとその更新日時
    source_mtimes: HashMap<PathBuf, i64>,
    statusline_mtime: Option<i64>,
}

impl LimitHistory {
    pub fn load(data_dir: &Path) -> LimitHistory {
        let store_path = data_dir.join("limit-history.json");
        let desktop = std::fs::read_to_string(&store_path)
            .ok()
            .and_then(|s| serde_json::from_str::<HistoryFile>(&s).ok())
            .map(|h| h.desktop)
            .unwrap_or_default();
        LimitHistory {
            store_path,
            desktop,
            statusline: Vec::new(),
            source_mtimes: HashMap::new(),
            statusline_mtime: None,
        }
    }

    /// 見つかった元ファイルのうち、更新されたものを取り込む。
    pub fn refresh(&mut self, settings: &Settings) {
        if settings.claude_desktop_usage {
            self.refresh_desktop();
        }
        if let Some(path) = statusline_log_path() {
            let mtime = mtime_of(&path);
            if mtime != self.statusline_mtime {
                self.statusline_mtime = mtime;
                self.statusline = std::fs::read_to_string(&path).map(|s| parse_statusline(&s)).unwrap_or_default();
            }
        }
    }

    fn refresh_desktop(&mut self) {
        let mut incoming = Vec::new();
        for path in desktop_history_paths() {
            let Some(mtime) = mtime_of(&path) else { continue };
            if self.source_mtimes.get(&path) == Some(&mtime) {
                continue;
            }
            self.source_mtimes.insert(path.clone(), mtime);
            if let Some(file) = std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str::<DesktopFile>(&s).ok())
            {
                incoming.extend(file.samples);
            }
        }
        if incoming.is_empty() {
            return;
        }
        let mut by_t: BTreeMap<(i64, Option<String>), DesktopSample> = self
            .desktop
            .drain(..)
            .map(|s| ((s.t, s.org.clone()), s))
            .collect();
        let before = by_t.len();
        for s in incoming {
            by_t.insert((s.t, s.org.clone()), s);
        }
        self.desktop = by_t.into_values().collect();
        if self.desktop.len() != before {
            let json = serde_json::to_vec(&HistoryFile {
                desktop: self.desktop.clone(),
            });
            if let Ok(json) = json {
                let _ = write_atomic(&self.store_path, &json);
            }
        }
    }

    /// 実際に見つかった元ファイル（取り込みを無効にしていても、存在するものは返す）
    pub fn found_paths(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self.source_mtimes.keys().cloned().collect();
        if v.is_empty() {
            v = desktop_history_paths().into_iter().filter(|p| p.is_file()).collect();
        }
        v.sort();
        v
    }
}

// ---------------------------------------------------------------- ステータスライン

/// ステータスラインのスクリプト。Windows PowerShell 5.1 で動く書き方にしてある。
const STATUSLINE_SCRIPT: &str = include_str!("../resources/statusline.ps1");

/// Claude Code の設定フォルダ（`CLAUDE_CONFIG_DIR` の最初の項目、なければ `~/.claude`）
fn claude_dir() -> Option<PathBuf> {
    if let Ok(v) = std::env::var("CLAUDE_CONFIG_DIR") {
        if let Some(p) = v.split([',', ';']).map(str::trim).find(|p| !p.is_empty()) {
            return Some(PathBuf::from(p));
        }
    }
    crate::sources::home().map(|h| h.join(".claude"))
}

fn statusline_dir() -> Option<PathBuf> {
    claude_dir().map(|d| d.join("token-meter"))
}

fn statusline_log_path() -> Option<PathBuf> {
    statusline_dir().map(|d| d.join("rate-limits.jsonl"))
}

fn claude_settings_path() -> Option<PathBuf> {
    claude_dir().map(|d| d.join("settings.json"))
}

fn statusline_command(script: &Path) -> String {
    // Git Bash 経由で実行されることがあるので、パスは `/` で書く（`\` はエスケープとして消える）。
    let p = script.to_string_lossy().replace('\\', "/");
    format!("powershell -NoProfile -ExecutionPolicy Bypass -File \"{p}\"")
}

fn is_our_command(cmd: &str) -> bool {
    cmd.replace('\\', "/").contains("token-meter/statusline.ps1")
}

fn read_json(path: &Path) -> Result<serde_json::Value, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{} を読めません: {e}", path.display()))?;
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    serde_json::from_slice(bytes).map_err(|e| format!("{} を JSON として読めません: {e}", path.display()))
}

#[derive(Debug, Serialize)]
pub struct StatuslineStatus {
    /// Token Meter のスクリプトがステータスラインに設定されている
    pub installed: bool,
    /// Token Meter 以外のステータスラインが設定されている場合、そのコマンド
    pub other_command: Option<String>,
    pub settings_path: Option<String>,
    pub log_path: Option<String>,
}

pub fn statusline_status() -> StatuslineStatus {
    let settings_path = claude_settings_path();
    let line = settings_path
        .as_ref()
        .filter(|p| p.is_file())
        .and_then(|p| read_json(p).ok())
        .and_then(|v| v.get("statusLine").cloned());
    let cmd = line.as_ref().map(|l| l.get("command").and_then(|c| c.as_str()).unwrap_or("").to_string());
    let installed = cmd.as_deref().map(is_our_command).unwrap_or(false);
    StatuslineStatus {
        installed,
        other_command: cmd.filter(|c| !is_our_command(c)),
        settings_path: settings_path.map(|p| p.to_string_lossy().to_string()),
        log_path: statusline_log_path().map(|p| p.to_string_lossy().to_string()),
    }
}

/// スクリプトを置き、Claude Code の `settings.json` の `statusLine` に設定する。
/// 他のステータスラインが設定されていれば上書きしない。元の設定は一度だけ別名で残す。
pub fn install_statusline() -> Result<StatuslineStatus, String> {
    let dir = statusline_dir().ok_or("Claude Code の設定フォルダが見つかりません")?;
    let settings_path = claude_settings_path().ok_or("Claude Code の設定フォルダが見つかりません")?;
    let mut root = if settings_path.is_file() { read_json(&settings_path)? } else { serde_json::json!({}) };
    let obj = root.as_object_mut().ok_or("Claude Code の settings.json の形式が想定と違います")?;
    if let Some(line) = obj.get("statusLine") {
        let cmd = line.get("command").and_then(|c| c.as_str()).unwrap_or("");
        if !is_our_command(cmd) {
            return Err(format!(
                "すでに別のステータスラインが設定されているため、上書きしませんでした（{}）",
                if cmd.is_empty() { "コマンドなし" } else { cmd }
            ));
        }
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let script = dir.join("statusline.ps1");
    // Windows PowerShell 5.1 は BOM がないと UTF-8 のスクリプトを ANSI として読む。
    let mut data = b"\xEF\xBB\xBF".to_vec();
    data.extend_from_slice(STATUSLINE_SCRIPT.as_bytes());
    write_atomic(&script, &data)?;
    if settings_path.is_file() {
        let backup = settings_path.with_file_name("settings.json.token-meter-backup");
        if !backup.exists() {
            std::fs::copy(&settings_path, &backup).map_err(|e| e.to_string())?;
        }
    }
    obj.insert(
        "statusLine".into(),
        serde_json::json!({ "type": "command", "command": statusline_command(&script) }),
    );
    let text = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())? + "\n";
    write_atomic(&settings_path, text.as_bytes())?;
    Ok(statusline_status())
}

/// `statusLine` から Token Meter のスクリプトを外す。記録済みのファイルは残す。
pub fn uninstall_statusline() -> Result<StatuslineStatus, String> {
    let settings_path = claude_settings_path().ok_or("Claude Code の設定フォルダが見つかりません")?;
    if !settings_path.is_file() {
        return Ok(statusline_status());
    }
    let mut root = read_json(&settings_path)?;
    let obj = root.as_object_mut().ok_or("Claude Code の settings.json の形式が想定と違います")?;
    let ours = obj
        .get("statusLine")
        .and_then(|l| l.get("command"))
        .and_then(|c| c.as_str())
        .map(is_our_command)
        .unwrap_or(false);
    if ours {
        obj.remove("statusLine");
        let text = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())? + "\n";
        write_atomic(&settings_path, text.as_bytes())?;
    }
    Ok(statusline_status())
}

// ---------------------------------------------------------------- 系列の組み立て

#[derive(Clone, Debug)]
struct RawPt {
    ts: i64,
    percent: f64,
    resets_at: Option<i64>,
    source: &'static str,
}

struct SeriesBuild {
    service: Service,
    kind: String,
    model: Option<String>,
    window_minutes: u64,
    pts: Vec<RawPt>,
}

fn codex_kind(s: &RateSample) -> String {
    let base = match s.window_minutes {
        300 => "session".to_string(),
        10080 => "weekly".to_string(),
        m => format!("window_{m}"),
    };
    match s.limit_id.as_deref() {
        None | Some("codex") => base,
        Some(id) => format!("{id}:{base}"),
    }
}

/// 複数アカウントの履歴が混ざらないよう、最新サンプルの組織だけを使う。
fn desktop_samples<'a>(history: &'a LimitHistory) -> impl Iterator<Item = &'a DesktopSample> {
    let org = history.desktop.last().and_then(|s| s.org.clone());
    history.desktop.iter().filter(move |s| s.org == org)
}

fn collect(store: &Store, history: &LimitHistory, settings: &Settings) -> Vec<SeriesBuild> {
    let mut map: BTreeMap<(Service, String, Option<String>), SeriesBuild> = BTreeMap::new();
    let mut push = |service: Service, kind: &str, model: Option<String>, window: u64, p: RawPt| {
        map.entry((service, kind.to_string(), model.clone()))
            .or_insert_with(|| SeriesBuild {
                service,
                kind: kind.to_string(),
                model,
                window_minutes: window,
                pts: Vec::new(),
            })
            .pts
            .push(p);
    };

    if settings.claude_desktop_usage {
        for s in desktop_samples(history) {
            for (key, kind, window) in [("fh", "session", 300u64), ("sd", "weekly", 10080)] {
                if let Some(&v) = s.u.get(key) {
                    push(Service::Claude, kind, None, window, RawPt { ts: s.t, percent: v, resets_at: None, source: "desktop" });
                }
            }
        }
    }

    for s in &history.statusline {
        for (v, r, kind, window) in [(s.fh, s.fh_reset, "session", 300u64), (s.sd, s.sd_reset, "weekly", 10080)] {
            if let Some(v) = v {
                push(
                    Service::Claude,
                    kind,
                    None,
                    window,
                    RawPt { ts: s.t, percent: v, resets_at: r.map(|x| x * 1000), source: "statusline" },
                );
            }
        }
    }

    for s in &store.rate_samples {
        push(
            Service::Codex,
            &codex_kind(s),
            None,
            s.window_minutes,
            RawPt { ts: s.ts, percent: s.used_percent, resets_at: s.resets_at, source: "log" },
        );
    }

    for m in &settings.manual_limits {
        let Some(service) = Service::from_id(&m.service) else { continue };
        let (kind, model, window) = match m.kind.as_str() {
            "session" => ("session", None, 300),
            "weekly_model" => (
                "weekly_model",
                m.model.as_ref().map(|x| x.trim().to_lowercase()).filter(|x| !x.is_empty()),
                10080,
            ),
            _ => ("weekly", None, 10080),
        };
        push(service, kind, model, window, RawPt { ts: m.ts, percent: m.percent, resets_at: m.resets_at, source: "manual" });
    }

    map.into_values().collect()
}

/// 今有効なプランの開始時刻。プランが変わると 1% の重みも変わるので、推定はこれより後のデータだけで行う。
fn plan_start(settings: &Settings, service: Service, now: i64) -> Option<i64> {
    settings
        .plans
        .iter()
        .filter(|p| p.service == service.id())
        .filter_map(|p| {
            let start = date_ts(parse_date(&p.start)?);
            let end = p.end.as_deref().and_then(parse_date).map(date_ts).unwrap_or(i64::MAX);
            (start <= now && now < end).then_some(start)
        })
        .max()
}

// ---------------------------------------------------------------- 5 時間枠と週間枠の比

/// 両方の使用率がそろった記録。`resets` は (5 時間枠, 週間枠) のリセット時刻（分かる場合）。
struct Pair {
    ts: i64,
    fh: f64,
    sd: f64,
    resets: Option<(i64, i64)>,
}

/// 連続した記録のうち、どちらの枠もリセットされずに 5 時間枠が増えた区間の、増加量の合計。
fn pair_sums(pairs: &[Pair], since: i64) -> (f64, f64) {
    let (mut dfh, mut dsd) = (0.0, 0.0);
    for w in pairs.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        if a.ts < since {
            continue;
        }
        let same_window = match (a.resets, b.resets) {
            (Some(x), Some(y)) => (x.0 - y.0).abs() < 10 * MIN && (x.1 - y.1).abs() < 10 * MIN,
            _ => b.ts - a.ts <= PAIR_GAP,
        };
        if same_window && b.fh > a.fh && b.sd >= a.sd {
            dfh += b.fh - a.fh;
            dsd += b.sd - a.sd;
        }
    }
    (dfh, dsd)
}

/// Claude の 5 時間枠の 1% が、週間枠の何 % に当たるか。
fn claude_pair_ratio(history: &LimitHistory, settings: &Settings, since: i64) -> Option<f64> {
    let mut sums = (0.0, 0.0);
    if settings.claude_desktop_usage {
        let pairs: Vec<Pair> = desktop_samples(history)
            .filter_map(|s| Some(Pair { ts: s.t, fh: *s.u.get("fh")?, sd: *s.u.get("sd")?, resets: None }))
            .collect();
        let (a, b) = pair_sums(&pairs, since);
        sums = (sums.0 + a, sums.1 + b);
    }
    let pairs: Vec<Pair> = history
        .statusline
        .iter()
        .filter_map(|s| {
            Some(Pair {
                ts: s.t,
                fh: s.fh?,
                sd: s.sd?,
                resets: s.fh_reset.zip(s.sd_reset).map(|(a, b)| (a * 1000, b * 1000)),
            })
        })
        .collect();
    let (a, b) = pair_sums(&pairs, since);
    sums = (sums.0 + a, sums.1 + b);
    // 丸めの誤差が効かないよう、5 時間枠で 30 ポイント以上の増加があるときだけ使う。
    (sums.0 >= 30.0 && sums.1 > 0.0).then(|| sums.1 / sums.0)
}

// ---------------------------------------------------------------- API 換算額の累積

/// 時刻順の累積額。`between(a, b)` は a < ts <= b の記録の合計。
struct CostIndex {
    ts: Vec<i64>,
    cum: Vec<f64>,
}

impl CostIndex {
    fn upto(&self, t: i64) -> f64 {
        let i = self.ts.partition_point(|x| *x <= t);
        if i == 0 {
            0.0
        } else {
            self.cum[i - 1]
        }
    }

    fn between(&self, a: i64, b: i64) -> f64 {
        if b <= a {
            0.0
        } else {
            self.upto(b) - self.upto(a)
        }
    }

    /// a < ts <= b に記録が 1 件でもあるか（価格が未設定で 0 円の記録も数える）
    fn any(&self, a: i64, b: i64) -> bool {
        self.first_after(a, b).is_some()
    }

    /// a < ts <= b の最初の記録の時刻
    fn first_after(&self, a: i64, b: i64) -> Option<i64> {
        let i = self.ts.partition_point(|x| *x <= a);
        self.ts.get(i).copied().filter(|t| *t <= b)
    }
}

fn cost_index(store: &Store, book: &PriceBook, settings: &Settings, service: Service, model: Option<&str>) -> CostIndex {
    let prices = price_table(store, book, settings);
    let model_ok: Vec<bool> = store
        .models
        .names
        .iter()
        .map(|n| model.map(|m| n.to_lowercase().contains(m)).unwrap_or(true))
        .collect();
    let mut ts = Vec::new();
    let mut cum = Vec::new();
    let mut acc = 0.0;
    for r in store.recs.iter().filter(|r| r.service == service && model_ok[r.model as usize]) {
        acc += rec_cost(&prices, r, settings).total();
        ts.push(r.ts);
        cum.push(acc);
    }
    CostIndex { ts, cum }
}

// ---------------------------------------------------------------- 分析

#[derive(Clone, Debug, Serialize)]
pub struct LimitPoint {
    pub ts: i64,
    pub percent: f64,
    /// 枠の開始からこの時点までの API 換算額（開始時刻が分かる場合。0% で枠が始まっていなければ `None`）
    pub window_cost: Option<f64>,
    pub source: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct LimitWindow {
    pub start: Option<i64>,
    pub end: Option<i64>,
    /// 開始時刻がリセット時刻から分かっている（false なら記録やログからの推定）
    pub exact: bool,
    pub max_percent: f64,
    /// 枠の開始から最後の記録までの API 換算額（開始時刻が分かる場合）
    pub cost: Option<f64>,
    /// Code の記録がない区間で増えた使用率（チャットや他の端末での利用の推定）
    pub other_percent: f64,
    /// この枠の実績から求めた、Code 以外の利用を除いた 1% あたりの額（残りが 3% 以上のとき）
    pub usd_per_percent: Option<f64>,
    /// 使用率全体で割った 1% あたりの額（Code 以外の利用を含む）
    pub raw_usd_per_percent: Option<f64>,
    pub last_ts: i64,
}

/// 推定値の信頼度。
#[derive(Clone, Debug, Serialize)]
pub struct Quality {
    /// "good" | "fair" | "poor"
    pub level: &'static str,
    /// 注意点（画面で文に直す）: "no_data" | "few_points" | "mostly_other" | "some_other" | "stale" |
    /// "inconsistent"（同じサービスの別の枠と食い違う）| "from_other"（別の枠の推定から換算した）
    pub notes: Vec<&'static str>,
    /// 使用率の丸めによる誤差の目安（相対値。0.1 なら ±10%）
    pub rounding: Option<f64>,
}

const LEVELS: [&str; 3] = ["poor", "fair", "good"];

fn rank(level: &str) -> usize {
    LEVELS.iter().position(|l| *l == level).unwrap_or(0)
}

fn own_quality(per: Option<f64>, code: f64, other: f64, step: f64, n: usize, latest_ts: Option<i64>, now: i64) -> Quality {
    let mut notes = Vec::new();
    if per.is_none() {
        return Quality { level: "poor", notes: vec!["no_data"], rounding: None };
    }
    let mut level = 2;
    if code < 10.0 {
        notes.push("few_points");
        level = if code < 5.0 { 0 } else { level.min(1) };
    }
    let share = if code + other > 0.0 { other / (code + other) } else { 0.0 };
    if share > 0.5 {
        notes.push("mostly_other");
        level = 0;
    } else if share > 0.25 {
        notes.push("some_other");
        level = level.min(1);
    }
    if latest_ts.map(|t| now - t > STALE).unwrap_or(true) {
        notes.push("stale");
        level = level.min(1);
    }
    Quality {
        level: LEVELS[level],
        notes,
        rounding: (code > 0.0).then(|| step * (n.max(1) as f64).sqrt() / code),
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct LimitSeries {
    pub service: String,
    pub kind: String,
    pub model: Option<String>,
    pub window_minutes: u64,
    pub sources: Vec<&'static str>,
    pub latest: Option<LimitPoint>,
    pub latest_resets_at: Option<i64>,
    /// 1 ポイント（1%）あたりの API 換算額（USD）。Code 以外の利用を除き、`basis` の方法で求めた値。
    pub usd_per_percent: Option<f64>,
    /// 同じ方法で、使用率全体（Code 以外の利用を含む）で割った値
    pub raw_usd_per_percent: Option<f64>,
    /// "current"（今の枠）| "recent_window"（直近の枠）| "average"（期間平均）|
    /// "cross"（同じサービスの別の枠の推定から換算）
    pub basis: &'static str,
    /// この枠の記録だけから求めた値（`basis` が "cross" のときに比べるため）
    pub own_usd_per_percent: Option<f64>,
    pub own_basis: &'static str,
    /// 同じサービスの別の枠（5 時間枠なら週間枠）の推定から換算した値
    pub cross_usd_per_percent: Option<f64>,
    /// この枠の 1% が、別の枠の何 % に当たるか
    pub pair_ratio: Option<f64>,
    pub quality: Quality,
    /// 直近 30 日の、使用率が上がった区間から求めた平均（Code 以外の利用を除く。参考値）
    pub avg_usd_per_percent: Option<f64>,
    /// 同じ期間の、Code 以外の利用を含めた平均
    pub raw_avg_usd_per_percent: Option<f64>,
    /// 推定に使った使用率の増加量の合計（ポイント。Code 以外の利用を除く）
    pub basis_points: f64,
    /// 同じ期間に、Code の記録がない区間で増えた使用率の合計
    pub other_points: f64,
    pub basis_intervals: usize,
    pub basis_cost: f64,
    pub current_start: Option<i64>,
    pub current_end: Option<i64>,
    pub current_cost: Option<f64>,
    /// 最新の記録が今の枠のものか（リセット済み・0% なら false）
    pub current_active: bool,
    /// 最後の記録の後の Code の利用から推定した今の使用率
    pub estimated_percent: Option<f64>,
    /// `estimated_percent` が、最後の記録の後に始まった新しい枠の値
    pub estimated_new_window: bool,
    /// 推定に使うデータの始まり（今のプランの開始。30 日より前なら `None`）
    pub plan_start: Option<i64>,
    pub points: Vec<LimitPoint>,
    pub windows: Vec<LimitWindow>,
}

#[derive(Debug, Serialize)]
pub struct LimitReport {
    pub series: Vec<LimitSeries>,
    pub desktop_path: Option<String>,
    pub desktop_exists: bool,
    pub desktop_samples: usize,
    pub desktop_last_ts: Option<i64>,
    pub statusline: StatuslineStatus,
    pub statusline_samples: usize,
    pub statusline_last_ts: Option<i64>,
    pub codex_samples: usize,
    pub manual_samples: usize,
}

/// 使用率が前の最高値を超えた区間（`ts` は区間の終わり）。`other` はそのうち Code 以外の利用とみなした分。
#[derive(Clone, Copy, Debug)]
struct Rise {
    ts: i64,
    dp: f64,
    other: f64,
}

/// (t0, t1] に終わる細かい区間の、Code 以外の利用の割合。区間がなければ `None`。
fn other_share(fine: &[Rise], t0: i64, t1: i64) -> Option<f64> {
    let a = fine.partition_point(|r| r.ts <= t0);
    let b = fine.partition_point(|r| r.ts <= t1);
    let (dp, other) = fine[a..b].iter().fold((0.0, 0.0), |(d, o), r| (d + r.dp, o + r.other));
    (dp > 0.0).then(|| other / dp)
}

struct Win {
    start: Option<i64>,
    /// 開始時刻がリセット時刻から分かっている
    exact: bool,
    resets_at: Option<i64>,
    high: f64,
    /// 最初の記録の時刻。枠はこれより前に始まっているので、これ + 枠の長さまでには必ず終わる。
    first_ts: i64,
    idx: Vec<usize>,
    closed: bool,
}

/// `fine` は同じサービスの短い枠（5 時間枠）の区間。長い枠は 1% 上がるまでに Code とチャットが
/// 混ざりやすいので、同じ時間帯の短い枠の内訳で按分する。
fn analyze(
    b: SeriesBuild,
    cost: &CostIndex,
    now: i64,
    fine: Option<&[Rise]>,
    plan_from: Option<i64>,
) -> (LimitSeries, Vec<Rise>) {
    let window_ms = b.window_minutes as i64 * MIN;
    let mut pts = b.pts;
    pts.sort_by_key(|p| p.ts);
    pts.dedup_by(|x, y| x.ts == y.ts && x.source == y.source);

    // 枠の区切りを決める。
    let mut wins: Vec<Win> = Vec::new();
    let mut win_of: Vec<Option<usize>> = vec![None; pts.len()];
    for (i, p) in pts.iter().enumerate() {
        let idle = p.resets_at.is_none() && p.percent <= IDLE_PERCENT;
        let join = match wins.last() {
            Some(w) if !w.closed => {
                let last = &pts[*w.idx.last().unwrap()];
                let reset_changed = matches!((w.resets_at, p.resets_at), (Some(a), Some(c)) if (a - c).abs() > 60 * MIN);
                let passed_reset = w.resets_at.map(|r| p.ts >= r).unwrap_or(false);
                // 枠の長さを過ぎたら必ず終わる。開始時刻が推定なら、確実に過ぎた時点（最初の記録 + 長さ）で切る。
                let latest_start = if w.exact { w.start.unwrap_or(w.first_ts) } else { w.first_ts };
                let expired = p.ts >= latest_start + window_ms;
                // 3 ポイント以上の低下は、並行するセッションの古い値ではなくリセットとみなす。
                let dropped = p.percent + 3.0 < w.high || (idle && w.high > IDLE_PERCENT);
                let gap = p.ts - last.ts > window_ms;
                !(reset_changed || passed_reset || expired || dropped || gap)
            }
            _ => false,
        };
        if join {
            let wi = wins.len() - 1;
            let w = &mut wins[wi];
            w.idx.push(i);
            w.high = w.high.max(p.percent);
            win_of[i] = Some(wi);
            if w.resets_at.is_none() && p.resets_at.is_some() {
                // リセット時刻が分かれば、推定した開始時刻より正確なのでそちらを使う。
                w.resets_at = p.resets_at;
                w.start = p.resets_at.map(|r| r - window_ms);
                w.exact = true;
            }
            continue;
        }
        // 前の枠の終わりの推定（新しい枠はこれより後に始まる）
        let prev_end = wins.last().map(|w| w.start.unwrap_or(w.first_ts) + window_ms);
        if let Some(w) = wins.last_mut() {
            w.closed = true;
        }
        if idle {
            // 0% は枠が始まっていない状態。次に使用率が上がったときの開始時刻の下限になる。
            continue;
        }
        let (start, exact) = match p.resets_at {
            Some(r) => (Some(r - window_ms), true),
            None => {
                let start = i.checked_sub(1).map(|j| &pts[j]).map(|q| {
                    let q_idle = q.resets_at.is_none() && q.percent <= IDLE_PERCENT;
                    // 0% の記録の直前の利用は、まだ反映されていなかった可能性がある。
                    let mut lo = if q_idle { q.ts - LAG_BEFORE } else { q.ts };
                    lo = lo.max(p.ts - window_ms);
                    if let Some(e) = prev_end.filter(|e| *e > lo && *e < p.ts - 5 * MIN) {
                        lo = e;
                    }
                    // 枠はリセット後に最初に使った時刻から始まる。Code を使っていればその記録の直前、
                    // なければ（チャットなど）区間の中ほどとする。どちらでも、この枠の API 換算額は変わらない。
                    cost.first_after(lo, p.ts).map(|t| t - 1).unwrap_or((lo + p.ts) / 2)
                });
                (start, false)
            }
        };
        wins.push(Win {
            start,
            exact,
            resets_at: p.resets_at,
            high: p.percent,
            first_ts: p.ts,
            idx: vec![i],
            closed: false,
        });
        win_of[i] = Some(wins.len() - 1);
    }

    // 1% あたりの額: 使用率が前の最高値を超えた区間ごとに、増加量と API 換算額を合計する。
    // 区間内（反映の遅れを含む）に Code の記録が 1 件もなければ、その増加は Code 以外の利用とみなす。
    let plan_from = plan_from.filter(|t| *t > now - ESTIMATE_DAYS * DAY);
    let since = plan_from.unwrap_or(now - ESTIMATE_DAYS * DAY);
    let (mut sum_p, mut sum_c, mut sum_other, mut n) = (0.0, 0.0, 0.0, 0usize);
    let mut win_other = vec![0.0; wins.len()];
    let mut rises: Vec<Rise> = Vec::new();
    for (wi, w) in wins.iter().enumerate() {
        let mut hw: Option<(i64, f64)> = w.start.map(|s| (s, 0.0));
        let mut prev_ts = w.start;
        for &i in &w.idx {
            let p = &pts[i];
            let prev = prev_ts.replace(p.ts);
            match hw {
                None => hw = Some((p.ts, p.percent)),
                Some((t0, p0)) if p.percent > p0 => {
                    let dp = p.percent - p0;
                    let other = match fine.and_then(|f| other_share(f, t0, p.ts)) {
                        Some(r) => dp * r,
                        None => {
                            // 反映の遅れは、増加が見えた直前の記録の少し前までさかのぼって見る。
                            let lo = prev.map(|t| t - LAG_BEFORE).unwrap_or(t0).min(t0);
                            if cost.any(lo, p.ts + LAG_AFTER) {
                                0.0
                            } else {
                                dp
                            }
                        }
                    };
                    rises.push(Rise { ts: p.ts, dp, other });
                    win_other[wi] += other;
                    if t0 >= since && p.ts - t0 <= window_ms {
                        sum_p += dp - other;
                        sum_other += other;
                        sum_c += cost.between(t0, p.ts);
                        if dp > other {
                            n += 1;
                        }
                    }
                    hw = Some((p.ts, p.percent));
                }
                _ => {}
            }
        }
    }
    let avg_usd_per_percent = if sum_p >= 3.0 && sum_c > 0.0 { Some(sum_c / sum_p) } else { None };
    let raw_avg_usd_per_percent = if sum_p + sum_other >= 3.0 && sum_c > 0.0 { Some(sum_c / (sum_p + sum_other)) } else { None };

    let chart_from = now - CHART_DAYS * DAY;
    let points: Vec<LimitPoint> = pts
        .iter()
        .enumerate()
        .filter(|(_, p)| p.ts >= chart_from)
        .map(|(i, p)| LimitPoint {
            ts: p.ts,
            percent: p.percent,
            window_cost: win_of[i].and_then(|wi| wins[wi].start).map(|s| cost.between(s, p.ts)),
            source: p.source,
        })
        .collect();

    let windows: Vec<LimitWindow> = wins
        .iter()
        .enumerate()
        .rev()
        .take(12)
        .map(|(wi, w)| {
            let last_ts = pts[*w.idx.last().unwrap()].ts;
            let cost = w.start.map(|s| cost.between(s, last_ts));
            // 開始時刻を遅めに推定した枠が次の枠と重ならないよう、次の枠の開始で打ち切る。
            let next_start = wins.get(wi + 1).and_then(|n| n.start.or(Some(n.first_ts)));
            let end = w.start.map(|s| s + window_ms).or(w.resets_at);
            let other = win_other[wi].min(w.high);
            let code_percent = w.high - other;
            LimitWindow {
                start: w.start,
                end: match (end, next_start) {
                    (Some(e), Some(n)) => Some(e.min(n)),
                    (e, _) => e,
                },
                exact: w.exact,
                max_percent: w.high,
                cost,
                other_percent: other,
                usd_per_percent: cost.filter(|c| *c > 0.0 && code_percent >= 3.0).map(|c| c / code_percent),
                raw_usd_per_percent: cost.filter(|c| *c > 0.0 && w.high >= 3.0).map(|c| c / w.high),
                last_ts,
            }
        })
        .collect();

    let latest_pt = pts.last().cloned();
    // 最新の記録が 0%（枠が始まっていない）なら、進行中の枠はない。
    let latest_win = pts.len().checked_sub(1).and_then(|i| win_of[i]).map(|wi| &wins[wi]);
    let (current_start, current_end, current_cost, current_active, latest_resets_at) = match latest_win {
        Some(w) => {
            let end = w.start.map(|s| s + window_ms).or(w.resets_at);
            let active = match end {
                Some(e) => now < e,
                None => now < w.first_ts + window_ms,
            };
            let cost_now = if active { w.start.map(|s| cost.between(s, now)) } else { None };
            (w.start, end, cost_now, active, w.resets_at.or(end))
        }
        None => (None, None, None, false, None),
    };

    // 基本の推定値: 今の枠 → 直近の枠 → 期間平均 の順に、使えるものを採る。
    // 過去の枠は、使用率が小さいほど丸め誤差が効くので Code の分が 10% 以上の枠を優先する。
    // プランが変わった後は、変わる前に始まった枠を使わない。
    let in_period = |w: &&LimitWindow| w.last_ts >= since && w.start.unwrap_or(w.last_ts) >= plan_from.unwrap_or(i64::MIN);
    let past = || windows.iter().skip(if current_active { 1 } else { 0 }).filter(|w| w.usd_per_percent.is_some()).filter(in_period);
    let chosen = match (current_active, windows.first()) {
        (true, Some(w)) if w.usd_per_percent.is_some() && in_period(&w) => Some((w, "current")),
        _ => past()
            .find(|w| w.max_percent - w.other_percent >= 10.0)
            .or_else(|| past().next())
            .map(|w| (w, "recent_window")),
    };
    let integer_data = pts.iter().all(|p| p.percent.fract() == 0.0);
    let step = if integer_data { 0.5 } else { 0.05 };
    let (usd_per_percent, raw_usd_per_percent, basis, quality) = match chosen {
        Some((w, basis)) => (
            w.usd_per_percent,
            w.raw_usd_per_percent,
            basis,
            own_quality(w.usd_per_percent, w.max_percent - w.other_percent, w.other_percent, step, 1, latest_pt.as_ref().map(|p| p.ts), now),
        ),
        None => (
            avg_usd_per_percent,
            raw_avg_usd_per_percent,
            "average",
            own_quality(avg_usd_per_percent, sum_p, sum_other, step, n, latest_pt.as_ref().map(|p| p.ts), now),
        ),
    };

    let mut sources: Vec<&'static str> = pts.iter().map(|p| p.source).collect();
    sources.sort();
    sources.dedup();

    let series = LimitSeries {
        service: b.service.id().to_string(),
        kind: b.kind,
        model: b.model,
        window_minutes: b.window_minutes,
        sources,
        latest: latest_pt.map(|p| LimitPoint {
            ts: p.ts,
            percent: p.percent,
            window_cost: latest_win.and(current_start).map(|s| cost.between(s, p.ts)),
            source: p.source,
        }),
        latest_resets_at,
        usd_per_percent,
        raw_usd_per_percent,
        basis,
        own_usd_per_percent: usd_per_percent,
        own_basis: basis,
        cross_usd_per_percent: None,
        pair_ratio: None,
        quality,
        avg_usd_per_percent,
        raw_avg_usd_per_percent,
        basis_points: sum_p,
        other_points: sum_other,
        basis_intervals: n,
        basis_cost: sum_c,
        current_start,
        current_end,
        current_cost,
        current_active,
        estimated_percent: None,
        estimated_new_window: false,
        plan_start: plan_from,
        points,
        windows,
    };
    (series, rises)
}

/// 5 時間枠（`a`）と週間枠（`b`）の推定値を、両者の比 `ratio`（a の 1% が b の何 % か）で照らし合わせる。
/// 食い違うか、片方の推定が弱ければ、信頼度の高い方から換算した値に置き換える。
fn reconcile(series: &mut [LimitSeries], a: usize, b: usize, ratio: f64) {
    series[a].pair_ratio = Some(ratio);
    series[b].pair_ratio = Some(1.0 / ratio);
    series[a].cross_usd_per_percent = series[b].usd_per_percent.map(|v| v * ratio);
    series[b].cross_usd_per_percent = series[a].usd_per_percent.map(|v| v / ratio);
    // 信頼度が同じなら、Code の利用をより長く平均できる週間枠を基準にする。
    let (anchor, other) = if rank(series[b].quality.level) >= rank(series[a].quality.level) { (b, a) } else { (a, b) };
    let anchor_quality = series[anchor].quality.clone();
    if rank(anchor_quality.level) == 0 {
        return;
    }
    let o = &mut series[other];
    let Some(cross) = o.cross_usd_per_percent else { return };
    let mismatch = o.usd_per_percent.map(|v| v / cross > MISMATCH || cross / v > MISMATCH);
    if mismatch == Some(false) && rank(o.quality.level) > 0 {
        return;
    }
    let mut notes = Vec::new();
    if mismatch == Some(true) {
        notes.push("inconsistent");
    }
    notes.push("from_other");
    for n in &anchor_quality.notes {
        if !notes.contains(n) {
            notes.push(n);
        }
    }
    o.usd_per_percent = Some(cross);
    o.raw_usd_per_percent = None;
    o.basis = "cross";
    o.quality = Quality {
        level: anchor_quality.level,
        notes,
        rounding: anchor_quality.rounding,
    };
}

/// 最後の記録の後に使った Code の API 換算額から、今の使用率を推定する（Code 以外の利用は数えられない）。
fn estimate_now(s: &LimitSeries, cost: &CostIndex, now: i64) -> Option<(f64, bool)> {
    let per = s.usd_per_percent.filter(|v| *v > 0.0)?;
    let p = s.latest.as_ref()?;
    if now - p.ts < 10 * MIN {
        return None;
    }
    let window_ms = s.window_minutes as i64 * MIN;
    let (base, from, new_window) = if s.current_active {
        (p.percent, p.ts, false)
    } else {
        // 枠が終わった（または 0% の）後に Code を使っていれば、新しい枠が始まっている。
        let from = p.ts.max(s.current_end.unwrap_or(p.ts));
        let mut start = cost.first_after(from, now)?;
        // その後も枠が入れ替わっていれば、今の枠の始まり（枠が終わった後の最初の利用）まで進める。
        while now - start >= window_ms {
            start = cost.first_after(start + window_ms - 1, now)?;
        }
        (0.0, start - 1, true)
    };
    let added = cost.between(from, now);
    (added > 0.0).then(|| ((base + added / per).min(100.0), new_window))
}

pub fn report(store: &Store, book: &PriceBook, settings: &Settings, history: &LimitHistory) -> LimitReport {
    let now = chrono::Utc::now().timestamp_millis();
    let builds = collect(store, history, settings);
    let mut indexes: HashMap<(Service, Option<String>), CostIndex> = HashMap::new();
    // 5 時間枠を先に分析し、その区間の内訳を同じサービスの長い枠の按分に使う。
    let mut builds = builds;
    builds.sort_by_key(|b| !b.kind.ends_with("session"));
    let mut fine: HashMap<(Service, String), Vec<Rise>> = HashMap::new();
    let mut series = Vec::new();
    let mut keys = Vec::new();
    for b in builds {
        let key = (b.service, b.model.clone());
        let idx = indexes
            .entry(key.clone())
            .or_insert_with(|| cost_index(store, book, settings, b.service, b.model.as_deref()));
        let (group, kind) = b.kind.rsplit_once(':').map(|(g, k)| (g.to_string(), k)).unwrap_or((String::new(), b.kind.as_str()));
        let service = b.service;
        let plan_from = plan_start(settings, service, now);
        if kind == "session" {
            let (s, rises) = analyze(b, idx, now, None, plan_from);
            fine.insert((service, group), rises);
            series.push(s);
        } else {
            series.push(analyze(b, idx, now, fine.get(&(service, group)).map(|v| v.as_slice()), plan_from).0);
        }
        keys.push(key);
    }

    // Claude の 5 時間枠と週間枠を照らし合わせる。
    let find = |k: &str| series.iter().position(|s| s.service == "claude" && s.kind == k && s.model.is_none());
    if let (Some(a), Some(b)) = (find("session"), find("weekly")) {
        let since = series[a].plan_start.unwrap_or(now - ESTIMATE_DAYS * DAY);
        if let Some(r) = claude_pair_ratio(history, settings, since) {
            reconcile(&mut series, a, b, r);
        }
    }

    for (s, key) in series.iter_mut().zip(&keys) {
        if let Some((v, new_window)) = indexes.get(key).and_then(|idx| estimate_now(s, idx, now)) {
            s.estimated_percent = Some(v);
            s.estimated_new_window = new_window;
        }
    }

    let found = history.found_paths();
    LimitReport {
        series,
        desktop_exists: !found.is_empty(),
        desktop_path: found
            .first()
            .or(desktop_history_paths().first())
            .map(|p| p.to_string_lossy().to_string()),
        desktop_samples: history.desktop.len(),
        desktop_last_ts: history.desktop.last().map(|s| s.t),
        statusline: statusline_status(),
        statusline_samples: history.statusline.len(),
        statusline_last_ts: history.statusline.last().map(|s| s.t),
        codex_samples: store.rate_samples.len(),
        manual_samples: settings.manual_limits.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_kind(kind: &str, window_minutes: u64, pts: &[(i64, f64)]) -> SeriesBuild {
        SeriesBuild {
            service: Service::Claude,
            kind: kind.into(),
            model: None,
            window_minutes,
            pts: pts
                .iter()
                .map(|&(ts, percent)| RawPt { ts, percent, resets_at: None, source: "desktop" })
                .collect(),
        }
    }

    fn build(pts: &[(i64, f64)]) -> SeriesBuild {
        build_kind("weekly", 10080, pts)
    }

    #[test]
    fn estimates_cost_per_percent_across_a_reset() {
        // 1 時間ごとに $2 ずつ使い、そのたびに 1% 増える。途中で週次リセットがある。
        let h = 60 * MIN;
        let cost = CostIndex {
            ts: (1..=20).map(|i| i * h - 1).collect(),
            cum: (1..=20).map(|i| i as f64 * 2.0).collect(),
        };
        let mut pts: Vec<(i64, f64)> = (0..10).map(|i| (i * h, 40.0 + i as f64)).collect();
        pts.extend((10..20).map(|i| (i * h, (i - 10) as f64)));
        let s = analyze(build(&pts), &cost, 20 * h, None, None).0;
        let per = s.usd_per_percent.unwrap();
        assert!((per - 2.0).abs() < 0.25, "per={per}");
        // リセット後の枠は 0% の記録の後に始まるので開始時刻が分かる。
        assert_eq!(s.windows.len(), 2);
        assert!(s.current_start.is_some());
    }

    #[test]
    fn ignores_stale_lower_readings() {
        let h = 60 * MIN;
        let cost = CostIndex { ts: vec![h - 1, 2 * h - 1, 3 * h - 1], cum: vec![1.0, 2.0, 3.0] };
        // 並行セッションの古い値（10.5）が混ざっても二重に数えない。
        let s = analyze(build(&[(0, 10.0), (h, 11.0), (h + 1, 10.5), (2 * h, 12.0), (3 * h, 13.0)]), &cost, 4 * h, None, None).0;
        assert!((s.basis_points - 3.0).abs() < 1e-9);
        assert!((s.usd_per_percent.unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn excludes_increases_without_code_usage() {
        // 1〜5 時間目は Code で $2 使うごとに 1% 増え、6 時間目は何もせず、
        // 7〜9 時間目は Code の記録がないまま 2% ずつ増える（チャット）。
        let h = 60 * MIN;
        let cost = CostIndex {
            ts: (1..=5).map(|i| i * h - 1).collect(),
            cum: (1..=5).map(|i| i as f64 * 2.0).collect(),
        };
        let mut pts: Vec<(i64, f64)> = (0..=6).map(|i| (i * h, i.min(5) as f64)).collect();
        pts.extend((7..=9).map(|i| (i * h, 5.0 + (i - 6) as f64 * 2.0)));
        let s = analyze(build(&pts), &cost, 10 * h, None, None).0;
        assert!((s.basis_points - 5.0).abs() < 1e-9, "code={}", s.basis_points);
        assert!((s.other_points - 6.0).abs() < 1e-9, "other={}", s.other_points);
        assert!((s.avg_usd_per_percent.unwrap() - 2.0).abs() < 1e-9);
        assert!((s.raw_avg_usd_per_percent.unwrap() - 10.0 / 11.0).abs() < 1e-9);
        assert!((s.windows[0].other_percent - 6.0).abs() < 1e-9);
    }

    #[test]
    fn splits_long_window_by_short_window_breakdown() {
        // 週間枠が 0% → 2% に上がる間に、5 時間枠では Code 分 1 ポイント、Code 以外 3 ポイント増えた。
        let h = 60 * MIN;
        let cost = CostIndex { ts: vec![h - 1], cum: vec![5.0] };
        let fine = [
            Rise { ts: h, dp: 1.0, other: 0.0 },
            Rise { ts: 2 * h, dp: 3.0, other: 3.0 },
        ];
        let (s, _) = analyze(build(&[(0, 0.0), (2 * h, 2.0)]), &cost, 3 * h, Some(&fine), None);
        assert!((s.other_points - 1.5).abs() < 1e-9, "other={}", s.other_points);
        assert!((s.basis_points - 0.5).abs() < 1e-9);
        assert!((s.windows[0].other_percent - 1.5).abs() < 1e-9);
    }

    #[test]
    fn attributes_delayed_increase_to_code() {
        // Code の利用が使用率に反映されるのが次の記録まで遅れても、Code 以外とはみなさない。
        let q = 15 * MIN;
        let cost = CostIndex { ts: vec![q - MIN, 2 * q - MIN], cum: vec![3.0, 6.0] };
        let s = analyze(build(&[(0, 0.0), (q, 0.0), (2 * q, 1.0), (3 * q, 2.0)]), &cost, 4 * q, None, None).0;
        assert!(s.other_points.abs() < 1e-9, "other={}", s.other_points);
        assert!((s.basis_points - 2.0).abs() < 1e-9);
        // 0% と記録される直前の利用（反映待ち）も、この枠の額に入る。
        assert!((s.windows[0].cost.unwrap() - 6.0).abs() < 1e-9);
    }

    #[test]
    fn window_ends_after_its_length_without_reset_time() {
        // 5 時間枠: 0% の後、30 分に 2% になり、そのまま 2% が続く。5 時間半の時点で 6% に
        // なっていても低下がないので、以前は同じ枠に入り続けていた。
        let h = 60 * MIN;
        let half = 30 * MIN;
        let cost = CostIndex { ts: vec![20 * MIN, 5 * h + 20 * MIN], cum: vec![1.0, 3.0] };
        let mut pts = vec![(0, 0.0)];
        pts.extend((1..=10).map(|i| (i * half, 2.0)));
        pts.push((11 * half, 6.0));
        let s = analyze(build_kind("session", 300, &pts), &cost, 12 * half, None, None).0;
        assert_eq!(s.windows.len(), 2, "windows={:?}", s.windows);
        // 新しい枠は、前の枠が終わった後の最初の利用から始まる。
        let cur = &s.windows[0];
        assert!(cur.start.unwrap() >= 5 * h && cur.start.unwrap() < 5 * h + 20 * MIN, "start={:?}", cur.start);
        assert!((cur.cost.unwrap() - 2.0).abs() < 1e-9);
        assert!(s.current_active);
    }

    #[test]
    fn zero_readings_do_not_start_a_window() {
        // 朝から 0% が続き、夜に使い始めた。枠の開始は使い始めた時刻で、朝の 0% ではない。
        let h = 60 * MIN;
        let cost = CostIndex { ts: vec![11 * h + 5 * MIN], cum: vec![4.0] };
        let mut pts: Vec<(i64, f64)> = (0..=11).map(|i| (i * h, 0.0)).collect();
        pts.push((11 * h + 15 * MIN, 3.0));
        let s = analyze(build_kind("session", 300, &pts), &cost, 12 * h, None, None).0;
        assert_eq!(s.windows.len(), 1);
        let w = &s.windows[0];
        assert!(w.start.unwrap() > 10 * h, "start={:?}", w.start);
        assert!(s.current_active);
        // 0% の記録は枠に属さないので、グラフの額は空ける。
        assert!(s.points[0].window_cost.is_none());
    }

    #[test]
    fn latest_zero_means_no_active_window() {
        let h = 60 * MIN;
        let cost = CostIndex { ts: vec![h - 1], cum: vec![2.0] };
        let s = analyze(build_kind("session", 300, &[(0, 0.0), (h, 5.0), (2 * h, 0.0)]), &cost, 2 * h + MIN, None, None).0;
        assert!(!s.current_active);
        assert!(s.current_start.is_none());
    }

    #[test]
    fn plan_change_limits_the_estimate_period() {
        // プラン変更前は 1% = $1、変更後は 1% = $4。変更後のデータだけで推定する。
        let h = 60 * MIN;
        let ts: Vec<i64> = (1..=20).map(|i| i * h - 1).collect();
        let mut cum = Vec::new();
        let mut acc = 0.0;
        for i in 1..=20 {
            acc += if i <= 10 { 1.0 } else { 4.0 };
            cum.push(acc);
        }
        let cost = CostIndex { ts, cum };
        let pts: Vec<(i64, f64)> = (0..=20).map(|i| (i * h, i as f64)).collect();
        let s = analyze(build(&pts), &cost, 21 * h, None, Some(10 * h)).0;
        assert!((s.avg_usd_per_percent.unwrap() - 4.0).abs() < 1e-9, "avg={:?}", s.avg_usd_per_percent);
    }

    #[test]
    fn pair_ratio_from_simultaneous_readings() {
        let q = 15 * MIN;
        // 5 時間枠が 3 ポイント上がるごとに週間枠が 1 ポイント上がる。途中で 5 時間枠がリセットされる。
        let mut pairs: Vec<Pair> = (0..=20).map(|i| Pair { ts: i * q, fh: 3.0 * i as f64, sd: i as f64, resets: None }).collect();
        pairs.push(Pair { ts: 21 * q, fh: 0.0, sd: 20.0, resets: None });
        pairs.extend((22..=31).map(|i| Pair { ts: i * q, fh: 3.0 * (i - 21) as f64, sd: 20.0 + (i - 21) as f64, resets: None }));
        let (dfh, dsd) = pair_sums(&pairs, 0);
        assert!((dfh - 90.0).abs() < 1e-9 && (dsd - 30.0).abs() < 1e-9, "dfh={dfh} dsd={dsd}");
        // 記録の間隔が空いた区間は、間にリセットがあったかもしれないので使わない。
        let far = [Pair { ts: 0, fh: 1.0, sd: 1.0, resets: None }, Pair { ts: 3 * 60 * MIN, fh: 50.0, sd: 3.0, resets: None }];
        assert_eq!(pair_sums(&far, 0), (0.0, 0.0));
        // リセット時刻が同じなら、間隔が空いても同じ枠と分かる。
        let same = [
            Pair { ts: 0, fh: 1.0, sd: 1.0, resets: Some((1, 2)) },
            Pair { ts: 3 * 60 * MIN, fh: 50.0, sd: 3.0, resets: Some((1, 2)) },
        ];
        assert_eq!(pair_sums(&same, 0), (49.0, 2.0));
    }

    #[test]
    fn reconciles_inconsistent_short_window() {
        let h = 60 * MIN;
        // 週間枠: 0% → 20% の間に $200（1% = $10）。5 時間枠: 同じ時間に 0% → 50% だが、
        // Code の記録は $1 分しかない（チャットの利用がたまたま重なった）。
        let weekly_cost = CostIndex { ts: vec![h - 1], cum: vec![200.0] };
        let session_cost = CostIndex { ts: vec![h - 1], cum: vec![1.0] };
        let now = 2 * h;
        let (s5, _) = analyze(build_kind("session", 300, &[(0, 0.0), (h, 50.0)]), &session_cost, now, None, None);
        let (s7, _) = analyze(build(&[(0, 0.0), (h, 20.0)]), &weekly_cost, now, None, None);
        let mut series = vec![s5, s7];
        // 5 時間枠の 1% = 週間枠の 0.2%
        reconcile(&mut series, 0, 1, 0.2);
        let s5 = &series[0];
        assert_eq!(s5.basis, "cross");
        assert!((s5.usd_per_percent.unwrap() - 2.0).abs() < 1e-9);
        assert!((s5.own_usd_per_percent.unwrap() - 0.02).abs() < 1e-9);
        assert!(s5.quality.notes.contains(&"inconsistent"));
        // 基準にした週間枠はそのまま
        assert_eq!(series[1].basis, "current");
        assert!((series[1].usd_per_percent.unwrap() - 10.0).abs() < 1e-9);
    }

    #[test]
    fn keeps_consistent_estimates() {
        let h = 60 * MIN;
        let weekly_cost = CostIndex { ts: vec![h - 1], cum: vec![200.0] };
        let session_cost = CostIndex { ts: vec![h - 1], cum: vec![110.0] };
        let now = 2 * h;
        let (s5, _) = analyze(build_kind("session", 300, &[(0, 0.0), (h, 50.0)]), &session_cost, now, None, None);
        let (s7, _) = analyze(build(&[(0, 0.0), (h, 20.0)]), &weekly_cost, now, None, None);
        let mut series = vec![s5, s7];
        reconcile(&mut series, 0, 1, 0.2);
        assert_eq!(series[0].basis, "current");
        assert!((series[0].cross_usd_per_percent.unwrap() - 2.0).abs() < 1e-9);
    }

    #[test]
    fn estimates_current_percent_from_later_usage() {
        let h = 60 * MIN;
        // 1% = $2。最後の記録（10%）の後に $20 使った → 約 20%。
        let cost = CostIndex { ts: vec![h - 1, 3 * h], cum: vec![20.0, 40.0] };
        let (s, _) = analyze(build(&[(0, 0.0), (h, 10.0)]), &cost, 4 * h, None, None);
        let (v, new_window) = estimate_now(&s, &cost, 4 * h).unwrap();
        assert!((v - 20.0).abs() < 1e-9, "v={v}");
        assert!(!new_window);
    }

    #[test]
    fn estimates_percent_of_a_window_started_after_the_last_reading() {
        let h = 60 * MIN;
        // 5 時間枠: 1% = $1。最後の記録（10%）の枠が終わった後、7 時間目に使った枠も終わり、
        // 13 時間目から使い始めた今の枠では $20 使った → 約 20%。
        let cost = CostIndex { ts: vec![h - 1, 7 * h, 13 * h, 13 * h + 30 * MIN], cum: vec![10.0, 20.0, 30.0, 40.0] };
        let (s, _) = analyze(build_kind("session", 300, &[(0, 0.0), (h, 10.0)]), &cost, 14 * h, None, None);
        assert!(!s.current_active);
        let (v, new_window) = estimate_now(&s, &cost, 14 * h).unwrap();
        assert!((v - 20.0).abs() < 1e-9, "v={v}");
        assert!(new_window);
    }

    #[test]
    fn installs_and_removes_statusline() {
        // CLAUDE_CONFIG_DIR を一時フォルダに向けて、実際の ~/.claude には触れない。
        let dir = std::env::temp_dir().join(format!("token-meter-statusline-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("CLAUDE_CONFIG_DIR", &dir);
        let settings = dir.join("settings.json");
        std::fs::write(&settings, "{\n  \"language\": \"日本語\",\n  \"hooks\": {}\n}\n").unwrap();

        let st = install_statusline().unwrap();
        assert!(st.installed && st.other_command.is_none());
        let v = read_json(&settings).unwrap();
        // 既存の項目と順序は保つ
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["language", "hooks", "statusLine"]);
        assert_eq!(v["language"], "日本語");
        assert!(dir.join("token-meter").join("statusline.ps1").is_file());
        assert!(dir.join("settings.json.token-meter-backup").is_file());
        // 2 回目も成功する（自分の設定は上書きしてよい）
        assert!(install_statusline().unwrap().installed);

        let st = uninstall_statusline().unwrap();
        assert!(!st.installed);
        assert!(read_json(&settings).unwrap().get("statusLine").is_none());

        // 他のステータスラインは上書きしない
        std::fs::write(&settings, "{\"statusLine\":{\"type\":\"command\",\"command\":\"bash ~/.claude/mine.sh\"}}").unwrap();
        assert!(install_statusline().is_err());
        assert_eq!(statusline_status().other_command.as_deref(), Some("bash ~/.claude/mine.sh"));
        assert!(uninstall_statusline().is_ok());
        assert!(read_json(&settings).unwrap().get("statusLine").is_some());

        std::env::remove_var("CLAUDE_CONFIG_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_statusline_lines() {
        let text = "\u{feff}{\"t\":2000,\"fh\":23.5,\"fh_reset\":1738425600,\"sd\":41,\"sd_reset\":1738857600}\nnot json\n{\"t\":1000,\"fh\":1}\n";
        let v = parse_statusline(text);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].t, 1000);
        assert_eq!(v[1].fh_reset, Some(1738425600));
        assert_eq!(v[1].sd, Some(41.0));
        assert!(is_our_command(&statusline_command(Path::new(r"C:\Users\x\.claude\token-meter\statusline.ps1"))));
        assert!(!is_our_command("bash ~/.claude/statusline.sh"));
    }
}
