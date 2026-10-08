pub mod claude;
pub mod codex;

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::model::{ParsedFile, Service};
use crate::settings::Settings;

/// JSONL ログを探すフォルダ。
#[derive(Clone, Debug, Serialize)]
pub struct SourceDir {
    pub service: Service,
    pub path: String,
    pub auto: bool,
    pub exists: bool,
    pub enabled: bool,
}

fn home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn push_unique(out: &mut Vec<SourceDir>, service: Service, path: PathBuf, auto: bool) {
    let s = path.to_string_lossy().to_string();
    if out
        .iter()
        .any(|d| d.service == service && d.path.eq_ignore_ascii_case(&s))
    {
        return;
    }
    out.push(SourceDir {
        service,
        exists: path.is_dir(),
        path: s,
        auto,
        enabled: true,
    });
}

/// 既定のログの場所と、ユーザーが追加したフォルダ。
pub fn source_dirs(settings: &Settings) -> Vec<SourceDir> {
    let mut out = Vec::new();
    let h = home();

    if let Ok(v) = std::env::var("CLAUDE_CONFIG_DIR") {
        for p in v.split([',', ';']).filter(|p| !p.trim().is_empty()) {
            push_unique(&mut out, Service::Claude, Path::new(p.trim()).join("projects"), true);
        }
    }
    if let Some(h) = &h {
        push_unique(&mut out, Service::Claude, h.join(".claude").join("projects"), true);
        push_unique(&mut out, Service::Claude, h.join(".config").join("claude").join("projects"), true);
    }

    let codex_home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| h.as_ref().map(|h| h.join(".codex")));
    if let Some(c) = codex_home {
        push_unique(&mut out, Service::Codex, c.join("sessions"), true);
        push_unique(&mut out, Service::Codex, c.join("archived_sessions"), true);
    }

    for extra in &settings.extra_dirs {
        if let Some(svc) = Service::from_id(&extra.service) {
            push_unique(&mut out, svc, PathBuf::from(&extra.path), false);
        }
    }

    for d in &mut out {
        d.enabled = !settings
            .disabled_dirs
            .iter()
            .any(|x| x.eq_ignore_ascii_case(&d.path));
    }
    out
}

/// `*.jsonl` を再帰的に列挙する（Codex の `session_index.jsonl` はロールアウトではないので除く）。
pub fn list_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        let p = e.path();
        if ft.is_dir() {
            list_jsonl(&p, out);
        } else if ft.is_file()
            && p.extension().map(|x| x.eq_ignore_ascii_case("jsonl")).unwrap_or(false)
            && p.file_name().map(|n| n != "session_index.jsonl").unwrap_or(true)
        {
            out.push(p);
        }
    }
}

pub fn parse(service: Service, path: &Path) -> std::io::Result<ParsedFile> {
    match service {
        Service::Claude => claude::parse_file(path),
        Service::Codex => codex::parse_file(path),
    }
}
