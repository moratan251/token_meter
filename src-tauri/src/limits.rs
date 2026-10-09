//! サブスクの使用上限（%）と API 換算額の比較。
//!
//! 使用率の取得元:
//! - Claude: デスクトップアプリが `%APPDATA%\Claude\plan-usage-history.json` に約 15 分ごとに
//!   記録する 5 時間枠（`fh`）と 7 日枠（`sd`）の使用率。元ファイルは古いものから消えていくので、
//!   読み込んだ分をアプリ側の `limit-history.json` に蓄積する。
//! - Codex: ログの `rate_limits`（リセット時刻付き）。
//! - 手動記録: ユーザーが入力・貼り付けした値（モデル別の枠など）。
//!
//! 推定方法: 同じ枠の中で使用率が上がった区間ごとに「増えた使用率」と「その区間の API 換算額」を
//! 集計し、合計の比から 1% あたりの API 換算額を求める。リセット時刻が分からなくても使え、
//! 整数に丸められた使用率でも区間を重ねるほど誤差が小さくなる。

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::model::{RateSample, Service};
use crate::pricing::PriceBook;
use crate::query::{price_table, rec_cost};
use crate::settings::{write_atomic, Settings};
use crate::store::Store;

const MIN: i64 = 60_000;
const DAY: i64 = 86_400_000;
/// 推定に使う期間（上限の仕様が変わることがあるので直近に絞る）
const ESTIMATE_DAYS: i64 = 30;
/// グラフに出す期間
const CHART_DAYS: i64 = 21;

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

pub fn desktop_history_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Claude").join("plan-usage-history.json"))
}

/// Claude デスクトップアプリの使用率履歴を読み込み、アプリ側にも蓄積する。
pub struct LimitHistory {
    store_path: PathBuf,
    pub desktop: Vec<DesktopSample>,
    source_mtime: i64,
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
            source_mtime: 0,
        }
    }

    /// 元ファイルが更新されていれば取り込む。
    pub fn refresh(&mut self, settings: &Settings) {
        if !settings.claude_desktop_usage {
            return;
        }
        let Some(path) = desktop_history_path() else { return };
        let Ok(meta) = std::fs::metadata(&path) else { return };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        if mtime == self.source_mtime {
            return;
        }
        self.source_mtime = mtime;
        let Some(file) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<DesktopFile>(&s).ok())
        else {
            return;
        };
        let mut by_t: BTreeMap<(i64, Option<String>), DesktopSample> = self
            .desktop
            .drain(..)
            .map(|s| ((s.t, s.org.clone()), s))
            .collect();
        let before = by_t.len();
        for s in file.samples {
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

    // 複数アカウントの履歴が混ざらないよう、最新サンプルの組織だけを使う。
    if settings.claude_desktop_usage {
        let org = history.desktop.last().and_then(|s| s.org.clone());
        for s in history.desktop.iter().filter(|s| s.org == org) {
            for (key, kind, window) in [("fh", "session", 300u64), ("sd", "weekly", 10080)] {
                if let Some(&v) = s.u.get(key) {
                    push(Service::Claude, kind, None, window, RawPt { ts: s.t, percent: v, resets_at: None, source: "desktop" });
                }
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
    /// 枠の開始からこの時点までの API 換算額（開始時刻が分かる場合）
    pub window_cost: Option<f64>,
    pub source: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct LimitWindow {
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub max_percent: f64,
    /// 枠の開始から最後の記録までの API 換算額（開始時刻が分かる場合）
    pub cost: Option<f64>,
    /// この枠の実績から求めた 1% あたりの額（使用率 3% 以上のとき）
    pub usd_per_percent: Option<f64>,
    pub last_ts: i64,
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
    /// 1 ポイント（1%）あたりの API 換算額（USD）。`basis` の方法で求めた値。
    pub usd_per_percent: Option<f64>,
    /// "current"（今の枠）| "recent_window"（直近の枠）| "average"（期間平均）
    pub basis: &'static str,
    /// 直近 30 日の、使用率が上がった区間から求めた平均（参考値）
    pub avg_usd_per_percent: Option<f64>,
    /// 推定に使った使用率の増加量の合計（ポイント）
    pub basis_points: f64,
    pub basis_intervals: usize,
    pub basis_cost: f64,
    pub current_start: Option<i64>,
    pub current_end: Option<i64>,
    pub current_cost: Option<f64>,
    /// 最新の記録が今の枠のものか（リセット済みなら false）
    pub current_active: bool,
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
    pub codex_samples: usize,
    pub manual_samples: usize,
}

struct Win {
    start: Option<i64>,
    resets_at: Option<i64>,
    high: f64,
    idx: Vec<usize>,
}

fn analyze(b: SeriesBuild, cost: &CostIndex, now: i64) -> LimitSeries {
    let window_ms = b.window_minutes as i64 * MIN;
    let mut pts = b.pts;
    pts.sort_by_key(|p| p.ts);
    pts.dedup_by(|x, y| x.ts == y.ts && x.source == y.source);

    // 枠の区切りを決める。リセット時刻があればそれを使い、なければ使用率の急な低下や
    // 枠の長さを超える空白から推定する。
    let mut wins: Vec<Win> = Vec::new();
    for (i, p) in pts.iter().enumerate() {
        let new_window = match wins.last() {
            None => true,
            Some(w) => {
                let last = &pts[*w.idx.last().unwrap()];
                let reset_changed = matches!((w.resets_at, p.resets_at), (Some(a), Some(c)) if (a - c).abs() > 60 * MIN);
                let passed_reset = w.resets_at.map(|r| p.ts >= r).unwrap_or(false);
                let dropped = p.percent + 3.0 < w.high;
                let gap = p.ts - last.ts > window_ms;
                reset_changed || passed_reset || dropped || gap
            }
        };
        if new_window {
            let prev = i.checked_sub(1).map(|j| &pts[j]);
            let start = if let Some(r) = p.resets_at {
                Some(r - window_ms)
            } else if let Some(prev) = prev.filter(|q| p.ts - q.ts <= 60 * MIN && p.percent + 3.0 < q.percent) {
                // 低下の直前と直後の記録の間でリセットされたとみなす。
                Some((prev.ts + p.ts) / 2)
            } else if p.percent <= 0.5 {
                Some(p.ts)
            } else {
                None
            };
            wins.push(Win { start, resets_at: p.resets_at, high: p.percent, idx: vec![i] });
        } else {
            let w = wins.last_mut().unwrap();
            w.idx.push(i);
            w.high = w.high.max(p.percent);
            if w.resets_at.is_none() && p.resets_at.is_some() {
                // リセット時刻が分かれば、推定した開始時刻より正確なのでそちらを使う。
                w.resets_at = p.resets_at;
                w.start = p.resets_at.map(|r| r - window_ms);
            }
        }
    }

    // 1% あたりの額: 使用率が前の最高値を超えた区間ごとに、増加量と API 換算額を合計する。
    let since = now - ESTIMATE_DAYS * DAY;
    let (mut sum_p, mut sum_c, mut n) = (0.0, 0.0, 0usize);
    for w in &wins {
        let mut hw: Option<(i64, f64)> = w.start.map(|s| (s, 0.0));
        for &i in &w.idx {
            let p = &pts[i];
            match hw {
                None => hw = Some((p.ts, p.percent)),
                Some((t0, p0)) if p.percent > p0 => {
                    if p.ts >= since && p.ts - t0 <= window_ms {
                        sum_p += p.percent - p0;
                        sum_c += cost.between(t0, p.ts);
                        n += 1;
                    }
                    hw = Some((p.ts, p.percent));
                }
                _ => {}
            }
        }
    }
    let avg_usd_per_percent = if sum_p >= 3.0 && sum_c > 0.0 { Some(sum_c / sum_p) } else { None };

    let window_of: HashMap<usize, usize> = wins
        .iter()
        .enumerate()
        .flat_map(|(wi, w)| w.idx.iter().map(move |&i| (i, wi)))
        .collect();
    let chart_from = now - CHART_DAYS * DAY;
    let points: Vec<LimitPoint> = pts
        .iter()
        .enumerate()
        .filter(|(_, p)| p.ts >= chart_from)
        .map(|(i, p)| LimitPoint {
            ts: p.ts,
            percent: p.percent,
            window_cost: wins[window_of[&i]].start.map(|s| cost.between(s, p.ts)),
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
            let next_start = wins.get(wi + 1).and_then(|n| n.start.or(Some(pts[n.idx[0]].ts)));
            let end = w.start.map(|s| s + window_ms).or(w.resets_at);
            LimitWindow {
                start: w.start,
                end: match (end, next_start) {
                    (Some(e), Some(n)) => Some(e.min(n)),
                    (e, _) => e,
                },
                max_percent: w.high,
                cost,
                usd_per_percent: cost.filter(|c| *c > 0.0 && w.high >= 3.0).map(|c| c / w.high),
                last_ts,
            }
        })
        .collect();

    let latest_pt = pts.last().cloned();
    let (current_start, current_end, current_cost, current_active, latest_resets_at) = match wins.last() {
        Some(w) => {
            let end = w.start.map(|s| s + window_ms).or(w.resets_at);
            let last_ts = latest_pt.as_ref().map(|p| p.ts).unwrap_or(0);
            let active = match end {
                Some(e) => now < e,
                None => now - last_ts < window_ms,
            };
            let cost_now = if active { w.start.map(|s| cost.between(s, now)) } else { None };
            (w.start, end, cost_now, active, w.resets_at.or(end))
        }
        None => (None, None, None, false, None),
    };

    // 基本の推定値: 今の枠 → 直近の枠 → 期間平均 の順に、使えるものを採る。
    // 過去の枠は、使用率が小さいほど丸め誤差が効くので 10% 以上の枠を優先する。
    let past = || windows.iter().skip(if current_active { 1 } else { 0 }).filter(|w| w.usd_per_percent.is_some() && w.last_ts >= since);
    let (usd_per_percent, basis) = match (current_active, windows.first()) {
        (true, Some(w)) if w.usd_per_percent.is_some() => (w.usd_per_percent, "current"),
        _ => match past().find(|w| w.max_percent >= 10.0).or_else(|| past().next()) {
            Some(w) => (w.usd_per_percent, "recent_window"),
            None => (avg_usd_per_percent, "average"),
        },
    };

    let mut sources: Vec<&'static str> = pts.iter().map(|p| p.source).collect();
    sources.sort();
    sources.dedup();

    LimitSeries {
        service: b.service.id().to_string(),
        kind: b.kind,
        model: b.model,
        window_minutes: b.window_minutes,
        sources,
        latest: latest_pt.map(|p| LimitPoint {
            ts: p.ts,
            percent: p.percent,
            window_cost: current_start.map(|s| cost.between(s, p.ts)),
            source: p.source,
        }),
        latest_resets_at,
        usd_per_percent: usd_per_percent.or(avg_usd_per_percent),
        basis: if usd_per_percent.is_some() { basis } else { "average" },
        avg_usd_per_percent,
        basis_points: sum_p,
        basis_intervals: n,
        basis_cost: sum_c,
        current_start,
        current_end,
        current_cost,
        current_active,
        points,
        windows,
    }
}

pub fn report(store: &Store, book: &PriceBook, settings: &Settings, history: &LimitHistory) -> LimitReport {
    let now = chrono::Utc::now().timestamp_millis();
    let builds = collect(store, history, settings);
    let mut indexes: HashMap<(Service, Option<String>), CostIndex> = HashMap::new();
    let mut series = Vec::new();
    for b in builds {
        let key = (b.service, b.model.clone());
        let idx = indexes
            .entry(key)
            .or_insert_with(|| cost_index(store, book, settings, b.service, b.model.as_deref()));
        series.push(analyze(b, idx, now));
    }
    let path = desktop_history_path();
    LimitReport {
        series,
        desktop_exists: path.as_ref().map(|p| p.is_file()).unwrap_or(false),
        desktop_path: path.map(|p| p.to_string_lossy().to_string()),
        desktop_samples: history.desktop.len(),
        desktop_last_ts: history.desktop.last().map(|s| s.t),
        codex_samples: store.rate_samples.len(),
        manual_samples: settings.manual_limits.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(pts: &[(i64, f64)]) -> SeriesBuild {
        SeriesBuild {
            service: Service::Claude,
            kind: "weekly".into(),
            model: None,
            window_minutes: 10080,
            pts: pts
                .iter()
                .map(|&(ts, percent)| RawPt { ts, percent, resets_at: None, source: "desktop" })
                .collect(),
        }
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
        let s = analyze(build(&pts), &cost, 20 * h);
        let per = s.usd_per_percent.unwrap();
        assert!((per - 2.0).abs() < 0.25, "per={per}");
        // リセット後の枠は 0% から始まるので開始時刻が分かる。
        assert_eq!(s.windows.len(), 2);
        assert!(s.current_start.is_some());
    }

    #[test]
    fn ignores_stale_lower_readings() {
        let h = 60 * MIN;
        let cost = CostIndex { ts: vec![h - 1, 2 * h - 1, 3 * h - 1], cum: vec![1.0, 2.0, 3.0] };
        // 並行セッションの古い値（10.5）が混ざっても二重に数えない。
        let s = analyze(build(&[(0, 10.0), (h, 11.0), (h + 1, 10.5), (2 * h, 12.0), (3 * h, 13.0)]), &cost, 4 * h);
        assert!((s.basis_points - 3.0).abs() < 1e-9);
        assert!((s.usd_per_percent.unwrap() - 1.0).abs() < 1e-9);
    }
}
