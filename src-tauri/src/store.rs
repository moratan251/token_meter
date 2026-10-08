//! ログファイルの差分スキャンと、統合・重複排除した記録の集合。
//!
//! 解析結果はファイル単位（サイズ + 更新日時）でキャッシュし、新規・変更されたファイル
//! だけを読み直す。`keep_deleted` が有効なら消えたファイルの記録も残すので、
//! Claude Code が会話ログを削除しても履歴が失われない。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Instant, UNIX_EPOCH};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::model::*;
use crate::settings::{write_atomic, Settings};
use crate::sources::{self, SourceDir};

/// 解析結果が変わる修正をしたら上げる。既存のファイルは再解析され、
/// すでに削除されたファイルの記録はそのまま残る。
const PARSER_VERSION: u32 = 2;
const CACHE_FORMAT: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileEntry {
    pub service: Service,
    pub size: u64,
    pub mtime: i64,
    pub missing: bool,
    pub parser: u32,
    pub parsed: ParsedFile,
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    format: u32,
    files: Vec<(String, FileEntry)>,
}

#[derive(Clone, Copy, Debug)]
pub struct Rec {
    pub ts: i64,
    pub service: Service,
    pub model: u32,
    pub project: u32,
    pub session: u32,
    pub flags: u8,
    pub t: Tokens,
}

#[derive(Default)]
pub struct Interner {
    pub names: Vec<String>,
    map: HashMap<String, u32>,
}

impl Interner {
    pub fn intern(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.map.get(s) {
            return i;
        }
        let i = self.names.len() as u32;
        self.names.push(s.to_string());
        self.map.insert(s.to_string(), i);
        i
    }

    pub fn get(&self, s: &str) -> Option<u32> {
        self.map.get(s).copied()
    }

    pub fn name(&self, i: u32) -> &str {
        &self.names[i as usize]
    }

    fn clear(&mut self) {
        self.names.clear();
        self.map.clear();
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ScanStats {
    pub files_total: usize,
    pub files_parsed: usize,
    pub files_missing: usize,
    pub errors: Vec<String>,
    pub records: usize,
    pub duration_ms: u64,
    pub changed: bool,
    pub scanned_at: i64,
}

pub struct ScanJob {
    pub path: PathBuf,
    pub key: String,
    pub service: Service,
    pub size: u64,
    pub mtime: i64,
}

pub struct ScanPlan {
    pub jobs: Vec<ScanJob>,
    seen: HashSet<String>,
    reappeared: Vec<String>,
    dirs: Vec<SourceDir>,
    total: usize,
    started: Instant,
}

pub type ScanResult = (ScanJob, Result<ParsedFile, String>);

pub struct Store {
    files: HashMap<String, FileEntry>,
    cache_path: PathBuf,
    pub models: Interner,
    pub projects: Interner,
    pub sessions: Interner,
    pub recs: Vec<Rec>,
    pub titles: HashMap<u32, String>,
    pub plan_marks: Vec<(i64, String)>,
    pub rate_limit: Option<RateLimitSnapshot>,
    pub last_scan: ScanStats,
    codex_titles: Vec<(String, String)>,
    loaded: bool,
}

fn mtime_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn under(path: &str, dir: &str) -> bool {
    path.len() > dir.len()
        && path.is_char_boundary(dir.len())
        && path[..dir.len()].eq_ignore_ascii_case(dir)
        && matches!(path.as_bytes()[dir.len()], b'\\' | b'/')
}

impl Store {
    pub fn new(data_dir: &Path) -> Store {
        Store {
            files: HashMap::new(),
            cache_path: data_dir.join("usage-cache.bin"),
            models: Interner::default(),
            projects: Interner::default(),
            sessions: Interner::default(),
            recs: Vec::new(),
            titles: HashMap::new(),
            plan_marks: Vec::new(),
            rate_limit: None,
            last_scan: ScanStats::default(),
            codex_titles: Vec::new(),
            loaded: false,
        }
    }

    fn load_cache(&mut self) {
        self.loaded = true;
        let Ok(bytes) = std::fs::read(&self.cache_path) else { return };
        match postcard::from_bytes::<CacheFile>(&bytes) {
            Ok(c) if c.format == CACHE_FORMAT => {
                self.files = c.files.into_iter().collect();
            }
            _ => {}
        }
    }

    fn save_cache(&self) -> Result<(), String> {
        let mut files: Vec<(String, FileEntry)> =
            self.files.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let bytes = postcard::to_stdvec(&CacheFile {
            format: CACHE_FORMAT,
            files,
        })
        .map_err(|e| e.to_string())?;
        if let Some(dir) = self.cache_path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        write_atomic(&self.cache_path, &bytes)
    }

    pub fn clear_cache(&mut self) {
        self.files.clear();
        let _ = std::fs::remove_file(&self.cache_path);
        self.loaded = true;
    }

    pub fn file_count(&self) -> (usize, usize) {
        let missing = self.files.values().filter(|e| e.missing).count();
        (self.files.len(), missing)
    }

    pub fn files_in(&self, dir: &str) -> (usize, usize) {
        let mut n = 0;
        let mut missing = 0;
        for (k, e) in &self.files {
            if under(k, dir) {
                n += 1;
                if e.missing {
                    missing += 1;
                }
            }
        }
        (n, missing)
    }

    /// 全ログファイルの情報を取得し、解析が必要なものを列挙する。軽い処理なので
    /// 状態のロックを取ったまま実行する。
    pub fn plan_scan(&mut self, settings: &Settings, force: bool) -> ScanPlan {
        if !self.loaded {
            self.load_cache();
        }
        let started = Instant::now();
        let dirs: Vec<SourceDir> = sources::source_dirs(settings);
        let mut jobs = Vec::new();
        let mut seen = HashSet::new();
        let mut reappeared = Vec::new();
        for d in dirs.iter().filter(|d| d.enabled && d.exists) {
            let mut paths = Vec::new();
            sources::list_jsonl(Path::new(&d.path), &mut paths);
            for p in paths {
                let Ok(meta) = std::fs::metadata(&p) else { continue };
                let key = p.to_string_lossy().to_string();
                let size = meta.len();
                let mtime = mtime_ms(&meta);
                if !seen.insert(key.clone()) {
                    continue;
                }
                let fresh = self.files.get(&key).map(|e| {
                    e.size == size && e.mtime == mtime && e.parser == PARSER_VERSION && e.service == d.service
                });
                match fresh {
                    Some(true) if !force => {
                        if self.files.get(&key).map(|e| e.missing).unwrap_or(false) {
                            reappeared.push(key);
                        }
                    }
                    _ => jobs.push(ScanJob {
                        path: p,
                        key,
                        service: d.service,
                        size,
                        mtime,
                    }),
                }
            }
        }
        let total = seen.len();
        ScanPlan {
            jobs,
            seen,
            reappeared,
            dirs,
            total,
            started,
        }
    }

    /// 対象ファイルを並列に解析する。状態のロックは取らずに実行する。
    pub fn run_jobs(jobs: Vec<ScanJob>, progress: impl Fn(usize, usize) + Sync) -> Vec<ScanResult> {
        let total = jobs.len();
        let done = AtomicUsize::new(0);
        jobs.into_par_iter()
            .map(|job| {
                let r = sources::parse(job.service, &job.path).map_err(|e| e.to_string());
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                if n == total || n % 8 == 0 {
                    progress(n, total);
                }
                (job, r)
            })
            .collect()
    }

    pub fn apply_scan(&mut self, plan: ScanPlan, results: Vec<ScanResult>, settings: &Settings) -> ScanStats {
        let mut changed = !plan.reappeared.is_empty() || self.recs.is_empty();
        let mut errors = Vec::new();
        let files_parsed = results.len();
        for key in &plan.reappeared {
            if let Some(e) = self.files.get_mut(key) {
                e.missing = false;
            }
        }
        for (job, r) in results {
            match r {
                Ok(parsed) => {
                    self.files.insert(
                        job.key,
                        FileEntry {
                            service: job.service,
                            size: job.size,
                            mtime: job.mtime,
                            missing: false,
                            parser: PARSER_VERSION,
                            parsed,
                        },
                    );
                    changed = true;
                }
                Err(e) => errors.push(format!("{}: {}", job.key, e)),
            }
        }

        // 有効なフォルダ内にあったが、今は無くなったファイル。
        let enabled: Vec<&SourceDir> = plan.dirs.iter().filter(|d| d.enabled && d.exists).collect();
        let gone: Vec<String> = self
            .files
            .iter()
            .filter(|(k, e)| !e.missing && !plan.seen.contains(*k) && enabled.iter().any(|d| under(k, &d.path)))
            .map(|(k, _)| k.clone())
            .collect();
        for k in gone {
            changed = true;
            if settings.keep_deleted {
                if let Some(e) = self.files.get_mut(&k) {
                    e.missing = true;
                }
            } else {
                self.files.remove(&k);
            }
        }
        if !settings.keep_deleted {
            let before = self.files.len();
            self.files.retain(|_, e| !e.missing);
            changed |= before != self.files.len();
        }

        let titles = read_codex_titles(&plan.dirs);
        if titles != self.codex_titles {
            self.codex_titles = titles;
            changed = true;
        }

        if changed {
            self.rebuild(&plan.dirs);
            if let Err(e) = self.save_cache() {
                errors.push(format!("キャッシュ保存に失敗: {e}"));
            }
        }

        let (_, files_missing) = self.file_count();
        self.last_scan = ScanStats {
            files_total: plan.total,
            files_parsed,
            files_missing,
            errors,
            records: self.recs.len(),
            duration_ms: plan.started.elapsed().as_millis() as u64,
            changed,
            scanned_at: chrono::Utc::now().timestamp_millis(),
        };
        self.last_scan.clone()
    }

    /// キャッシュ済みのファイルを 1 つの記録リストに統合する。複数のファイルに現れる
    /// リクエスト（再開したセッション、移動・アーカイブされたロールアウト）は重複を除く。
    fn rebuild(&mut self, dirs: &[SourceDir]) {
        self.models.clear();
        self.projects.clear();
        self.sessions.clear();
        self.recs.clear();
        self.titles.clear();
        self.plan_marks.clear();
        self.rate_limit = None;

        let active: Vec<&SourceDir> = dirs.iter().filter(|d| d.enabled).collect();
        let mut keys: Vec<&String> = self
            .files
            .keys()
            .filter(|k| active.iter().any(|d| under(k, &d.path)))
            .collect();
        keys.sort();

        let mut by_key: HashMap<u64, usize> = HashMap::new();
        for k in keys {
            let e = &self.files[k];
            let pf = &e.parsed;
            let mut model_map: Vec<Option<u32>> = vec![None; pf.strings.len()];
            let mut project_map: Vec<Option<u32>> = vec![None; pf.strings.len()];
            let mut session_map: Vec<Option<u32>> = vec![None; pf.strings.len()];
            for r in &pf.records {
                if let Some(&i) = by_key.get(&r.key) {
                    self.recs[i].t.max_with(&r.t);
                    continue;
                }
                let model = *model_map[r.model as usize]
                    .get_or_insert_with(|| self.models.intern(&pf.strings[r.model as usize]));
                let project = *project_map[r.project as usize]
                    .get_or_insert_with(|| self.projects.intern(&pf.strings[r.project as usize]));
                let session = *session_map[r.session as usize]
                    .get_or_insert_with(|| self.sessions.intern(&pf.strings[r.session as usize]));
                by_key.insert(r.key, self.recs.len());
                self.recs.push(Rec {
                    ts: r.ts,
                    service: e.service,
                    model,
                    project,
                    session,
                    flags: r.flags,
                    t: r.t,
                });
            }
            for (sid, title) in &pf.titles {
                let i = self.sessions.intern(sid);
                self.titles.insert(i, title.clone());
            }
            self.plan_marks.extend(pf.plan_marks.iter().cloned());
            if let Some(rl) = &pf.rate_limit {
                if self.rate_limit.as_ref().map(|c| rl.ts > c.ts).unwrap_or(true) {
                    self.rate_limit = Some(rl.clone());
                }
            }
        }
        for (id, name) in &self.codex_titles {
            if let Some(i) = self.sessions.get(id) {
                self.titles.entry(i).or_insert_with(|| name.clone());
            }
        }
        self.fill_unknown_models();
        self.recs.sort_by_key(|r| r.ts);
        self.plan_marks.sort();
        self.plan_marks.dedup_by(|b, a| a.1 == b.1);
    }

    /// 古い Codex が派生させたサブエージェントはモデル名を記録しないので、
    /// 同じ（親）セッションで最も使われたモデルの使用量として扱う。
    fn fill_unknown_models(&mut self) {
        let Some(unknown) = self.models.get(sources::codex::UNKNOWN_MODEL) else { return };
        let mut per_session: HashMap<u32, HashMap<u32, u64>> = HashMap::new();
        for r in &self.recs {
            if r.service == Service::Codex && r.model != unknown {
                *per_session.entry(r.session).or_default().entry(r.model).or_default() += r.t.total();
            }
        }
        let main: HashMap<u32, u32> = per_session
            .into_iter()
            .filter_map(|(s, m)| m.into_iter().max_by_key(|x| x.1).map(|(model, _)| (s, model)))
            .collect();
        for r in &mut self.recs {
            if r.model == unknown {
                if let Some(&m) = main.get(&r.session) {
                    r.model = m;
                }
            }
        }
    }

    /// `from <= ts < to` を満たす記録のインデックス範囲。
    pub fn range(&self, from: Option<i64>, to: Option<i64>) -> std::ops::Range<usize> {
        let lo = from.map(|f| self.recs.partition_point(|r| r.ts < f)).unwrap_or(0);
        let hi = to.map(|t| self.recs.partition_point(|r| r.ts < t)).unwrap_or(self.recs.len());
        lo..hi.max(lo)
    }
}

fn read_codex_titles(dirs: &[SourceDir]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for d in dirs.iter().filter(|d| d.enabled && d.service == Service::Codex) {
        let Some(home) = Path::new(&d.path).parent() else { continue };
        let index = home.join("session_index.jsonl");
        if seen.insert(index.clone()) && index.is_file() {
            out.extend(sources::codex::parse_session_index(&index));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::under;

    #[test]
    fn path_prefix() {
        assert!(under(r"C:\Users\a\.claude\projects\x\y.jsonl", r"C:\Users\a\.claude\projects"));
        assert!(under(r"c:\users\a\.claude\projects\x.jsonl", r"C:\Users\a\.claude\projects"));
        assert!(!under(r"C:\Users\a\.claude\projects2\x.jsonl", r"C:\Users\a\.claude\projects"));
    }
}
