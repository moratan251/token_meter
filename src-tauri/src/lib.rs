mod limits;
mod model;
mod pricing;
mod query;
mod settings;
mod sources;
mod store;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;
use tauri::{Emitter, Manager, State};

use pricing::{PriceBook, PriceSpec};
use query::{DashOpts, Filter};
use settings::Settings;
use store::{ScanStats, Store};

struct Inner {
    store: Store,
    settings: Settings,
    prices: PriceBook,
    limits: limits::LimitHistory,
    settings_path: PathBuf,
    data_dir: PathBuf,
}

struct AppState {
    inner: Mutex<Inner>,
    /// 再読み込みが重なっても同じファイルを二重に解析しないよう、スキャンを直列化する。
    scan_lock: Mutex<()>,
}

type Res<T> = Result<T, String>;

fn lock(state: &AppState) -> Res<std::sync::MutexGuard<'_, Inner>> {
    state.inner.lock().map_err(|_| "内部状態のロックに失敗しました".to_string())
}

#[derive(Clone, Serialize)]
struct Progress {
    done: usize,
    total: usize,
}

#[tauri::command]
async fn refresh(app: tauri::AppHandle, state: State<'_, AppState>, force: bool) -> Res<ScanStats> {
    let _scan = state.scan_lock.lock().map_err(|_| "scan lock poisoned".to_string())?;
    let (plan, settings) = {
        let mut g = lock(&state)?;
        let settings = g.settings.clone();
        (g.store.plan_scan(&settings, force), settings)
    };
    let mut plan = plan;
    let jobs = std::mem::take(&mut plan.jobs);
    let total = jobs.len();
    let _ = app.emit("scan-progress", Progress { done: 0, total });
    let results = Store::run_jobs(jobs, |done, total| {
        let _ = app.emit("scan-progress", Progress { done, total });
    });
    let mut g = lock(&state)?;
    let stats = g.store.apply_scan(plan, results, &settings);
    g.limits.refresh(&settings);
    Ok(stats)
}

#[tauri::command]
async fn rebuild_cache(app: tauri::AppHandle, state: State<'_, AppState>) -> Res<ScanStats> {
    {
        let mut g = lock(&state)?;
        g.store.clear_cache();
    }
    refresh(app, state, true).await
}

#[derive(Serialize)]
struct SourceView {
    #[serde(flatten)]
    dir: sources::SourceDir,
    files: usize,
    missing_files: usize,
}

#[derive(Serialize)]
struct Meta {
    sources: Vec<SourceView>,
    models: Vec<query::ModelInfo>,
    projects: Vec<query::ProjectInfo>,
    first_ts: Option<i64>,
    last_ts: Option<i64>,
    /// サービスごとの最古のログ時刻（それ以前のログは残っていない）
    service_first_ts: BTreeMap<String, i64>,
    records: usize,
    files: usize,
    missing_files: usize,
    plan_marks: Vec<(i64, String)>,
    rate_limit: Option<model::RateLimitSnapshot>,
    litellm_fetched_at: Option<String>,
    snapshot_date: &'static str,
    last_scan: ScanStats,
    data_dir: String,
}

#[tauri::command]
async fn get_meta(state: State<'_, AppState>) -> Res<Meta> {
    let g = lock(&state)?;
    let (models, projects) = query::model_and_project_info(&g.store, &g.prices, &g.settings);
    let sources = sources::source_dirs(&g.settings)
        .into_iter()
        .map(|d| {
            let (files, missing_files) = g.store.files_in(&d.path);
            SourceView {
                dir: d,
                files,
                missing_files,
            }
        })
        .collect();
    let (files, missing_files) = g.store.file_count();
    let mut service_first_ts = BTreeMap::new();
    for r in &g.store.recs {
        service_first_ts.entry(r.service.id().to_string()).or_insert(r.ts);
    }
    Ok(Meta {
        sources,
        models,
        projects,
        first_ts: g.store.recs.first().map(|r| r.ts),
        last_ts: g.store.recs.last().map(|r| r.ts),
        service_first_ts,
        records: g.store.recs.len(),
        files,
        missing_files,
        plan_marks: g.store.plan_marks.clone(),
        rate_limit: g.store.rate_limit.clone(),
        litellm_fetched_at: g.prices.latest_fetched_at.clone(),
        snapshot_date: g.prices.snapshot_date(),
        last_scan: g.store.last_scan.clone(),
        data_dir: g.data_dir.to_string_lossy().to_string(),
    })
}

#[tauri::command]
async fn query_dashboard(state: State<'_, AppState>, filter: Filter, opts: DashOpts) -> Res<query::Dashboard> {
    let g = lock(&state)?;
    Ok(query::dashboard(&g.store, &g.prices, &g.settings, &filter, &opts))
}

#[tauri::command]
async fn query_table(state: State<'_, AppState>, filter: Filter, group_by: String) -> Res<Vec<query::Row>> {
    let g = lock(&state)?;
    Ok(query::table(&g.store, &g.prices, &g.settings, &filter, &group_by))
}

#[tauri::command]
async fn subscription_report(state: State<'_, AppState>) -> Res<Vec<query::PlanReport>> {
    let g = lock(&state)?;
    Ok(query::subscription_report(&g.store, &g.prices, &g.settings))
}

#[tauri::command]
async fn limit_report(state: State<'_, AppState>) -> Res<limits::LimitReport> {
    let mut g = lock(&state)?;
    let settings = g.settings.clone();
    g.limits.refresh(&settings);
    Ok(limits::report(&g.store, &g.prices, &g.settings, &g.limits))
}

#[tauri::command]
async fn get_settings(state: State<'_, AppState>) -> Res<Settings> {
    Ok(lock(&state)?.settings.clone())
}

#[tauri::command]
async fn save_settings(state: State<'_, AppState>, settings: Settings) -> Res<Settings> {
    let mut g = lock(&state)?;
    settings.save(&g.settings_path)?;
    g.settings = settings;
    Ok(g.settings.clone())
}

#[derive(Serialize)]
struct ImportResult {
    models: usize,
    fetched_at: String,
}

#[tauri::command]
async fn import_litellm(state: State<'_, AppState>, text: String) -> Res<ImportResult> {
    let models = pricing::import_litellm(&text)?;
    if models.len() < 50 {
        return Err(format!("価格表のモデル数が少なすぎます（{} 件）。取得内容を確認してください", models.len()));
    }
    let n = models.len();
    let fetched_at = chrono::Utc::now().to_rfc3339();
    lock(&state)?.prices.set_latest(models, fetched_at.clone())?;
    Ok(ImportResult { models: n, fetched_at })
}

#[derive(Serialize)]
struct CatalogEntry {
    key: String,
    curated: Option<PriceSpec>,
    litellm: Option<PriceSpec>,
    litellm_latest: bool,
}

/// 既知の全モデルについて、内蔵価格と LiteLLM の価格を並べて返す。
#[tauri::command]
async fn price_catalog(state: State<'_, AppState>) -> Res<Vec<CatalogEntry>> {
    let g = lock(&state)?;
    let p = &g.prices;
    let mut keys: BTreeMap<&String, ()> = BTreeMap::new();
    for k in p.curated.keys().chain(p.latest.keys()).chain(p.snapshot.keys()) {
        keys.insert(k, ());
    }
    Ok(keys
        .into_keys()
        .map(|k| {
            let lit = p.litellm(k);
            CatalogEntry {
                key: k.clone(),
                curated: p.curated.get(k).cloned(),
                litellm: lit.map(|(s, _)| s.clone()),
                litellm_latest: lit.map(|(_, l)| l).unwrap_or(false),
            }
        })
        .collect())
}

#[tauri::command]
async fn write_text_file(path: String, contents: String) -> Res<()> {
    // Excel は BOM 付きでないと UTF-8 の CSV を正しく開けない。
    let mut data = Vec::with_capacity(contents.len() + 3);
    if path.to_lowercase().ends_with(".csv") {
        data.extend_from_slice(b"\xEF\xBB\xBF");
    }
    data.extend_from_slice(contents.as_bytes());
    std::fs::write(&path, data).map_err(|e| e.to_string())
}

#[cfg(test)]
mod real_data {
    use super::*;

    /// `cargo test --release --lib real_data -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn scan_local_logs() {
        let tmp = std::env::temp_dir().join("token-meter-test");
        let _ = std::fs::remove_dir_all(&tmp);
        let settings = Settings::default();
        let book = PriceBook::load(&tmp);
        let mut store = Store::new(&tmp);
        for pass in 0..2 {
            let mut plan = store.plan_scan(&settings, false);
            let jobs = std::mem::take(&mut plan.jobs);
            let results = Store::run_jobs(jobs, |_, _| {});
            let stats = store.apply_scan(plan, results, &settings);
            println!("pass {pass}: {stats:?}");
        }
        let d = query::dashboard(&store, &book, &settings, &Filter::default(), &DashOpts::default());
        println!("total ${:.2} requests {} sessions {}", d.totals.total_cost, d.totals.requests, d.sessions);
        for g in &d.by_service {
            println!("  {:<8} ${:>10.2}  {:>14} tok", g.key, g.agg.total_cost, g.agg.tokens.total());
        }
        for g in &d.by_model {
            println!("  {:<28} ${:>10.2}  {:>14} tok  {:>6} req", g.key, g.agg.total_cost, g.agg.tokens.total(), g.agg.requests);
        }
        println!("unpriced: {:?}", d.unpriced);

        let mut hist = limits::LimitHistory::load(&tmp);
        hist.refresh(&settings);
        let rep = limits::report(&store, &book, &settings, &hist);
        println!("limits: desktop {} samples, codex {} samples", rep.desktop_samples, rep.codex_samples);
        for s in &rep.series {
            println!(
                "  {} {:<10} latest {:?}% per1% {:?} raw {:?} (Δ{:.0}pt + other {:.0}pt / {} intervals, ${:.2}) window_cost {:?} windows {}",
                s.service,
                s.kind,
                s.latest.as_ref().map(|p| p.percent),
                s.usd_per_percent.map(|v| format!("{v:.2} ({})", s.basis)),
                s.raw_usd_per_percent.map(|v| format!("{v:.2}")),
                s.basis_points,
                s.other_points,
                s.basis_intervals,
                s.basis_cost,
                s.current_cost.map(|v| v.round()),
                s.windows.len()
            );
            for w in s.windows.iter().take(4) {
                println!(
                    "      window start {:?} max {:.0}% other {:.0}% cost {:?} per1% {:?}",
                    w.start,
                    w.max_percent,
                    w.other_percent,
                    w.cost.map(|v| v.round()),
                    w.usd_per_percent.map(|v| (v * 100.0).round() / 100.0)
                );
            }
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let config_dir = app.path().app_config_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let settings_path = config_dir.join("settings.json");
            let settings = Settings::load(&settings_path);
            app.manage(AppState {
                inner: Mutex::new(Inner {
                    store: Store::new(&data_dir),
                    prices: PriceBook::load(&data_dir),
                    limits: limits::LimitHistory::load(&data_dir),
                    settings,
                    settings_path,
                    data_dir,
                }),
                scan_lock: Mutex::new(()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            refresh,
            rebuild_cache,
            get_meta,
            query_dashboard,
            query_table,
            subscription_report,
            limit_report,
            get_settings,
            save_settings,
            import_litellm,
            price_catalog,
            write_text_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
