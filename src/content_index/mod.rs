/* =============================================================================
 * File          : mod.rs
 * Author        : Nicolas Le Roux
 * Created       : 06/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Full-text content index (phase D1): one appliance-wide FTS5 index over the
 *   text files of administrator-chosen roots, queried with the caller's own
 *   Explorer roots as the ACL filter.
 *
 *   The design note is docs/analysis/d1-content-index-design.md. The rules it
 *   keeps, in code:
 *   - off by default, enabling requires an explicit confirmation;
 *   - a root is chosen from the appliance's Explorer roots, never typed blind;
 *   - only text files on an allowlist are read (1 MiB, UTF-8, no NUL), so the
 *     worker can never turn into a binary parser;
 *   - a query returns a path only if it is under one of the caller's roots;
 *   - the index is bounded (50k docs / 256 MiB), re-read only on mtime/size
 *     change, pruned per root, and aged out by `retention_days`;
 *   - "delete my index" wipes every document and turns the feature off.
 *
 * Dependencies:
 *   - sqlx (>= 0.9), chrono, crate::config::paths, crate::network
 *
 * Usage:
 *   handlers/content_index.rs (the API); src/web/mod.rs starts the periodic
 *   worker. See docs/guides/content-index.md.
 * ============================================================================= */

//! The content index: configuration, walker, FTS5 worker and query policy.

pub mod repo;

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::config::paths;
use crate::handlers::AppState;

/// How often an enabled index refreshes itself.
const REFRESH_INTERVAL: Duration = Duration::from_secs(30 * 60);

pub const MAX_ROOTS: usize = 16;
pub const MAX_DOCS: i64 = 50_000;
pub const MAX_TOTAL_BYTES: i64 = 256 * 1024 * 1024;
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_DEPTH: usize = 32;
const MAX_EXCLUDES: usize = 64;
const MAX_QUERY_TOKENS: usize = 8;
const MAX_SNIPPET_RESULTS: i64 = 50;

/// Only these extensions are ever read. Anything else (media, archives,
/// images, PDF, Office) is skipped before a byte is opened.
const TEXT_EXTENSIONS: &[&str] = &[
    "txt",
    "text",
    "md",
    "markdown",
    "rst",
    "org",
    "log",
    "csv",
    "tsv",
    "json",
    "jsonl",
    "yaml",
    "yml",
    "toml",
    "ini",
    "conf",
    "cfg",
    "env",
    "html",
    "htm",
    "xml",
    "css",
    "scss",
    "less",
    "js",
    "mjs",
    "cjs",
    "ts",
    "tsx",
    "jsx",
    "svelte",
    "vue",
    "astro",
    "rs",
    "py",
    "rb",
    "php",
    "pl",
    "lua",
    "go",
    "java",
    "kt",
    "c",
    "h",
    "cc",
    "cpp",
    "hpp",
    "cs",
    "swift",
    "sh",
    "bash",
    "zsh",
    "fish",
    "ps1",
    "bat",
    "sql",
    "tex",
    "bib",
    "properties",
    "gradle",
    "dockerfile",
    "makefile",
];

/// Configuration document, persisted as JSON like the other appliance cards.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContentIndexConfig {
    pub enabled: bool,
    pub roots: Vec<String>,
    /// Names or simple globs (`*.bak`) excluded from every root.
    pub exclude_names: Vec<String>,
    /// Extensions excluded on top of the allowlist.
    pub exclude_extensions: Vec<String>,
    /// Documents not seen for this many days are dropped.
    pub retention_days: u32,
    /// Required to turn the feature on.
    pub confirmed: bool,
}

impl Default for ContentIndexConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            roots: Vec::new(),
            exclude_names: vec![
                ".git".to_string(),
                "node_modules".to_string(),
                ".cache".to_string(),
                "#recycle".to_string(),
                "@eaDir".to_string(),
                "$RECYCLE.BIN".to_string(),
                "lost+found".to_string(),
            ],
            exclude_extensions: [
                "iso", "img", "mp4", "mkv", "avi", "mov", "zip", "7z", "rar", "gz", "xz", "tar",
                "jpg", "jpeg", "png", "gif", "webp", "heic", "pdf", "doc", "docx", "xls", "xlsx",
                "ppt", "pptx",
            ]
            .iter()
            .map(|ext| ext.to_string())
            .collect(),
            retention_days: 30,
            confirmed: false,
        }
    }
}

/// Validate the document's own shape. Whether a root is a real, allowed folder
/// is checked by the handler against the candidate list.
pub fn validate_config(mut config: ContentIndexConfig) -> Result<ContentIndexConfig, String> {
    if config.roots.len() > MAX_ROOTS {
        return Err(format!("At most {MAX_ROOTS} roots can be indexed."));
    }
    if !(1..=3650).contains(&config.retention_days) {
        return Err("Retention must be between 1 and 3650 days.".to_string());
    }
    if config.exclude_names.len() > MAX_EXCLUDES || config.exclude_extensions.len() > MAX_EXCLUDES {
        return Err(format!(
            "At most {MAX_EXCLUDES} exclusions of each kind are supported."
        ));
    }

    let mut roots = Vec::with_capacity(config.roots.len());
    for raw in &config.roots {
        let root = raw.trim().trim_end_matches('/');
        if root.is_empty() || root == "/" || !root.starts_with('/') {
            return Err("Every root must be an absolute folder, not '/'.".to_string());
        }
        if root.len() > 4096
            || root.contains('\0')
            || root
                .split('/')
                .any(|segment| segment == ".." || segment == ".")
        {
            return Err("A root path is not a normalised absolute folder.".to_string());
        }
        if !roots.contains(&root.to_string()) {
            roots.push(root.to_string());
        }
    }
    config.roots = roots;

    config.exclude_names = normalize_patterns(&config.exclude_names)?;
    config.exclude_extensions = normalize_extensions(&config.exclude_extensions)?;

    if config.enabled {
        if !config.confirmed {
            return Err(
                "Turning the content index on must be confirmed: it reads the chosen folders."
                    .to_string(),
            );
        }
        if config.roots.is_empty() {
            return Err("Choose at least one folder to index.".to_string());
        }
    }
    Ok(config)
}

fn normalize_patterns(raw: &[String]) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for value in raw {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        if value.len() > 64
            || value.contains('\0')
            || value.contains('/')
            || value.contains('\\')
            || value.chars().any(char::is_control)
        {
            return Err(format!("'{value}' is not a valid exclusion name."));
        }
        if !out.contains(&value.to_string()) {
            out.push(value.to_string());
        }
    }
    Ok(out)
}

fn normalize_extensions(raw: &[String]) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for value in raw {
        let value = value.trim().trim_start_matches('.').to_ascii_lowercase();
        if value.is_empty() {
            continue;
        }
        if value.len() > 16 || !value.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(format!("'{value}' is not a valid file extension."));
        }
        if !out.contains(&value) {
            out.push(value);
        }
    }
    Ok(out)
}

/// A root is allowed when it is one of the candidate roots or a folder below
/// one. `/` can never be a candidate.
pub fn root_is_allowed(root: &str, candidates: &[String]) -> bool {
    candidates
        .iter()
        .any(|candidate| path_is_under(root, candidate) || candidate == root)
}

/// Boundary-aware prefix test: `/srv/share` contains `/srv/share/a`, never
/// `/srv/share2`.
pub fn path_is_under(path: &str, root: &str) -> bool {
    let root = root.trim_end_matches('/');
    if root.is_empty() {
        return false;
    }
    if path == root {
        return true;
    }
    path.strip_prefix(root)
        .is_some_and(|rest| rest.starts_with('/'))
}

fn config_path() -> PathBuf {
    paths::base_dir().join("data/content_index.json")
}

pub async fn load_persisted() -> Result<ContentIndexConfig, String> {
    let path = config_path();
    if !path.exists() {
        return Ok(ContentIndexConfig::default());
    }
    let content = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| format!("failed to read {}: {}", path.display(), e))?;
    serde_json::from_str(&content).map_err(|e| format!("failed to parse {}: {}", path.display(), e))
}

pub async fn save_persisted(config: &ContentIndexConfig) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("failed to create {}: {}", parent.display(), e))?;
    }
    let content = serde_json::to_string_pretty(config)
        .map_err(|e| format!("failed to serialise the content index settings: {e}"))?;
    tokio::fs::write(&path, content)
        .await
        .map_err(|e| format!("failed to write {}: {}", path.display(), e))
}

/// The appliance's own Explorer roots, canonicalised, plus mounted pools. The
/// admin chooses from this list; the handler refuses anything else.
pub async fn candidate_roots(state: &Arc<AppState>) -> Vec<String> {
    let mut raw: Vec<String> = state
        .path_guard
        .allowed_roots()
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect();

    if let Ok(Ok(pools)) = tokio::task::spawn_blocking(crate::pools::PoolService::scan_pools).await
    {
        for pool in pools {
            if let Some(mount) = pool.mount_point {
                if !mount.is_empty() {
                    raw.push(mount);
                }
            }
        }
    }

    tokio::task::spawn_blocking(move || {
        let mut roots: Vec<String> = Vec::new();
        for path in raw {
            let Ok(canonical) = std::fs::canonicalize(&path) else {
                continue;
            };
            if !canonical.is_dir() {
                continue;
            }
            let text = canonical
                .to_string_lossy()
                .trim_end_matches('/')
                .to_string();
            if !text.is_empty() && !roots.contains(&text) {
                roots.push(text);
            }
        }
        roots.sort();
        roots
    })
    .await
    .unwrap_or_default()
}

/// One file the walker kept (metadata only; the body is read later).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub path: String,
    pub name: String,
    pub size_bytes: i64,
    pub modified: i64,
}

/// What a walk produced. `complete` is false when reading the root failed, so
/// the caller never prunes an index it could not refresh.
#[derive(Debug, Default)]
pub struct WalkOutcome {
    pub files: Vec<FileEntry>,
    pub errors: Vec<String>,
    pub complete: bool,
    pub truncated: bool,
}

/// Walk one root synchronously (call from `spawn_blocking`).
pub fn collect_files(config: &ContentIndexConfig, root: &str) -> WalkOutcome {
    let mut outcome = WalkOutcome::default();
    let canonical_root = match std::fs::canonicalize(root) {
        Ok(path) if path.is_dir() => path,
        Ok(_) => {
            outcome.errors.push(format!("{root} is not a folder"));
            return outcome;
        }
        Err(e) => {
            outcome
                .errors
                .push(format!("{root} could not be opened: {e}"));
            return outcome;
        }
    };

    let mut stack = vec![(canonical_root.clone(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            outcome.truncated = true;
            continue;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) => {
                outcome
                    .errors
                    .push(format!("{} could not be read: {e}", dir.display()));
                continue;
            }
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            if is_excluded_name(config, &name) {
                continue;
            }
            let path = entry.path();
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                stack.push((path, depth + 1));
                continue;
            }
            if !metadata.is_file() {
                continue;
            }
            let size = metadata.len();
            if size == 0 || size > MAX_FILE_BYTES {
                continue;
            }
            if !is_text_file(config, &path) {
                continue;
            }
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs() as i64)
                .unwrap_or(0);
            outcome.files.push(FileEntry {
                path: path.to_string_lossy().to_string(),
                name,
                size_bytes: size as i64,
                modified,
            });
            if outcome.files.len() as i64 >= MAX_DOCS {
                outcome.truncated = true;
                break;
            }
        }
        if outcome.truncated {
            break;
        }
    }

    outcome.files.sort_by(|a, b| a.path.cmp(&b.path));
    outcome.complete = true;
    outcome
}

fn extension_of(path: &Path) -> String {
    path.extension()
        .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

fn is_text_file(config: &ContentIndexConfig, path: &Path) -> bool {
    let ext = extension_of(path);
    !ext.is_empty()
        && TEXT_EXTENSIONS.contains(&ext.as_str())
        && !config
            .exclude_extensions
            .iter()
            .any(|excluded| excluded == &ext)
}

fn is_excluded_name(config: &ContentIndexConfig, name: &str) -> bool {
    config
        .exclude_names
        .iter()
        .any(|pattern| glob_match(pattern, name))
}

/// `*` matches any run of characters, everything else is literal. Enough for
/// `*.bak` without pulling in a glob crate.
fn glob_match(pattern: &str, value: &str) -> bool {
    if !pattern.contains('*') {
        return pattern.eq_ignore_ascii_case(value);
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut rest = value;
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if index == 0 {
            if !rest
                .to_ascii_lowercase()
                .starts_with(&part.to_ascii_lowercase())
            {
                return false;
            }
            rest = &rest[part.len()..];
        } else if index == parts.len() - 1 {
            return rest
                .to_ascii_lowercase()
                .ends_with(&part.to_ascii_lowercase());
        } else if let Some(position) = rest.to_ascii_lowercase().find(&part.to_ascii_lowercase()) {
            rest = &rest[position + part.len()..];
        } else {
            return false;
        }
    }
    true
}

/// Read a file as UTF-8 text, bounded and binary-safe.
pub fn read_text(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut buffer = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut buffer)
        .ok()?;
    if buffer.len() as u64 > MAX_FILE_BYTES || buffer.is_empty() {
        return None;
    }
    if buffer.contains(&0) {
        return None;
    }
    let text = String::from_utf8(buffer).ok()?;
    if text.trim().is_empty() {
        return None;
    }
    Some(text)
}

/// Build a safe FTS5 expression: at most eight quoted literals, ANDed. FTS5
/// operators in the user's input never reach the parser.
pub fn search_expression(query: &str) -> Option<String> {
    let tokens: Vec<String> = query
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .filter(|token| !token.is_empty() && token.chars().any(char::is_alphanumeric))
        .take(MAX_QUERY_TOKENS)
        .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
        .collect();
    if tokens.is_empty() {
        None
    } else {
        Some(tokens.join(" AND "))
    }
}

/// The ACL gate for a hit.
pub fn hit_is_visible(path: &str, caller_roots: &[String]) -> bool {
    caller_roots.iter().any(|root| path_is_under(path, root))
}

/// Snapshot of the worker, for the status line.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RunState {
    pub running: bool,
    pub last_run: Option<String>,
    pub last_error: Option<String>,
    pub truncated: bool,
}

static RUNNING: AtomicBool = AtomicBool::new(false);
static RUN_STATE: Lazy<RwLock<RunState>> = Lazy::new(|| RwLock::new(RunState::default()));

pub fn state() -> RunState {
    RUN_STATE.read().unwrap_or_else(|e| e.into_inner()).clone()
}

fn set_state(update: impl FnOnce(&mut RunState)) {
    let mut state = RUN_STATE.write().unwrap_or_else(|e| e.into_inner());
    update(&mut state);
}

/// The result of one worker pass.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RunReport {
    pub indexed: usize,
    pub unchanged: usize,
    pub skipped: usize,
    pub deleted: u64,
    pub truncated: bool,
    pub errors: Vec<String>,
    pub elapsed_ms: u64,
}

/// Run one pass. Returns an error only when the database itself failed; a
/// single unreadable folder is reported in `errors` and never aborts the run.
pub async fn run(pool: &SqlitePool, config: &ContentIndexConfig) -> Result<RunReport, String> {
    let started = std::time::Instant::now();
    let mut report = RunReport::default();
    if !config.enabled || config.roots.is_empty() {
        return Ok(report);
    }

    let seen_at = chrono::Utc::now().to_rfc3339();
    let unchanged = repo::unchanged(pool)
        .await
        .map_err(|e| format!("the index could not be read: {e}"))?;
    let (mut total_docs, mut total_bytes) = repo::stats(pool)
        .await
        .map_err(|e| format!("the index size could not be read: {e}"))?;

    for root in &config.roots {
        if report.truncated {
            break;
        }
        let outcome = {
            let config = config.clone();
            let root = root.clone();
            match tokio::task::spawn_blocking(move || collect_files(&config, &root)).await {
                Ok(outcome) => outcome,
                Err(e) => {
                    report.errors.push(format!("the walker stopped: {e}"));
                    continue;
                }
            }
        };
        report.errors.extend(outcome.errors.iter().cloned());
        report.truncated |= outcome.truncated;

        for entry in outcome.files {
            if total_docs >= MAX_DOCS || total_bytes >= MAX_TOTAL_BYTES {
                report.truncated = true;
                break;
            }
            if let Some((modified, size)) = unchanged.get(&entry.path) {
                if *modified == entry.modified && *size == entry.size_bytes {
                    if let Err(e) = repo::touch(pool, &entry.path, &seen_at).await {
                        report.errors.push(format!("{}: {e}", entry.path));
                    }
                    report.unchanged += 1;
                    continue;
                }
            }

            let path = PathBuf::from(&entry.path);
            let text = match tokio::task::spawn_blocking(move || read_text(&path)).await {
                Ok(Some(text)) => text,
                Ok(None) => {
                    report.skipped += 1;
                    continue;
                }
                Err(e) => {
                    report.errors.push(format!("{}: {e}", entry.path));
                    report.skipped += 1;
                    continue;
                }
            };
            let doc = repo::ContentDoc {
                path: entry.path.clone(),
                root: root.clone(),
                name: entry.name.clone(),
                size_bytes: entry.size_bytes,
                modified: entry.modified,
            };
            let body_len = text.len() as i64;
            if let Err(e) = repo::upsert(pool, &doc, &text, &seen_at).await {
                report.errors.push(format!("{}: {e}", entry.path));
                continue;
            }
            total_docs += 1;
            total_bytes += body_len;
            report.indexed += 1;
        }

        if outcome.complete {
            match repo::prune_root(pool, root, &seen_at).await {
                Ok(deleted) => report.deleted += deleted,
                Err(e) => report
                    .errors
                    .push(format!("{root} could not be pruned: {e}")),
            }
            if let Ok((docs, bytes)) = repo::stats(pool).await {
                total_docs = docs;
                total_bytes = bytes;
            }
        }
    }

    let cutoff = (chrono::Utc::now()
        - chrono::Duration::days(i64::from(config.retention_days.max(1))))
    .to_rfc3339();
    if let Ok(deleted) = repo::prune_older_than(pool, &cutoff).await {
        report.deleted += deleted;
    }

    report.elapsed_ms = started.elapsed().as_millis() as u64;
    Ok(report)
}

/// Start a pass in the background unless one is already running. Returns
/// `true` when this call started it.
pub fn trigger(state: Arc<AppState>) -> bool {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return false;
    }
    set_state(|state| {
        state.running = true;
        state.last_error = None;
    });
    let pool = state.db.pool();
    tokio::spawn(async move {
        let config = load_persisted().await.unwrap_or_default();
        let result = run(&pool, &config).await;
        set_state(|state| {
            state.running = false;
            state.last_run = Some(chrono::Utc::now().to_rfc3339());
            match result {
                Ok(report) => {
                    state.truncated = report.truncated;
                    if !report.errors.is_empty() {
                        state.last_error = Some(report.errors.join("; "));
                    }
                }
                Err(e) => state.last_error = Some(e),
            }
        });
        RUNNING.store(false, Ordering::SeqCst);
    });
    true
}

/// Refresh an enabled index every 30 minutes; the first tick runs at startup.
pub async fn run_periodic_loop(state: Arc<AppState>) {
    let mut interval = tokio::time::interval(REFRESH_INTERVAL);
    loop {
        interval.tick().await;
        let config = match load_persisted().await {
            Ok(config) => config,
            Err(e) => {
                tracing::warn!("Content index: {e}");
                continue;
            }
        };
        if config.enabled {
            trigger(state.clone());
        }
    }
}

/// The maximum number of hits the search endpoint returns regardless of the
/// caller's `limit`.
pub fn clamp_limit(limit: Option<u32>) -> usize {
    limit.unwrap_or(10).clamp(1, MAX_SNIPPET_RESULTS as u32) as usize
}

/// Whether the current search should offer content hits at all.
pub fn content_search_available(config: &ContentIndexConfig) -> bool {
    config.enabled && !config.roots.is_empty()
}

/// Keep only the first `limit` hits; the caller has already filtered by ACL.
pub fn take_hits(mut hits: Vec<repo::ContentHit>, limit: usize) -> (Vec<repo::ContentHit>, bool) {
    let truncated = hits.len() > limit;
    hits.truncate(limit);
    (hits, truncated)
}

/// Deduplicate caller roots for the ACL filter.
pub fn unique_roots(roots: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    roots
        .into_iter()
        .filter(|root| seen.insert(root.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> ContentIndexConfig {
        ContentIndexConfig::default()
    }

    #[test]
    fn test_disabled_default_is_safe() {
        let config = default_config();
        assert!(!config.enabled);
        assert!(!config.confirmed);
        assert!(config.roots.is_empty());
        assert!(validate_config(config).is_ok());
    }

    #[test]
    fn test_enabling_requires_confirmation_and_a_root() {
        let mut config = default_config();
        config.enabled = true;
        assert!(validate_config(config.clone())
            .unwrap_err()
            .contains("confirmed"));
        config.confirmed = true;
        assert!(validate_config(config)
            .unwrap_err()
            .contains("at least one"));
    }

    #[test]
    fn test_roots_must_be_absolute_and_normalised() {
        let mut config = default_config();
        config.enabled = true;
        config.confirmed = true;
        for bad in ["", "/", "relative/path", "/srv/../etc", "/srv/./x"] {
            config.roots = vec![bad.to_string()];
            assert!(validate_config(config.clone()).is_err(), "{bad}");
        }
        config.roots = vec!["/srv/share/".to_string()];
        assert_eq!(validate_config(config).unwrap().roots, vec!["/srv/share"]);
    }

    #[test]
    fn test_root_count_and_retention_are_bounded() {
        let mut config = default_config();
        config.roots = (0..MAX_ROOTS + 1).map(|i| format!("/srv/{i}")).collect();
        assert!(validate_config(config.clone()).is_err());
        config.roots = vec!["/srv/a".to_string()];
        config.retention_days = 0;
        assert!(validate_config(config.clone()).is_err());
        config.retention_days = 3651;
        assert!(validate_config(config).is_err());
    }

    #[test]
    fn test_exclusions_are_normalised_and_validated() {
        let mut config = default_config();
        config.exclude_extensions = vec![".TXT".to_string(), "iso".to_string()];
        let validated = validate_config(config).unwrap();
        assert_eq!(validated.exclude_extensions, vec!["txt", "iso"]);

        let mut bad = default_config();
        bad.exclude_names = vec!["a/b".to_string()];
        assert!(validate_config(bad.clone()).is_err());
        bad.exclude_names = vec!["a\0b".to_string()];
        assert!(validate_config(bad).is_err());
    }

    fn config_with(names: &[&str], extensions: &[&str]) -> ContentIndexConfig {
        ContentIndexConfig {
            exclude_names: names.iter().map(|s| s.to_string()).collect(),
            exclude_extensions: extensions.iter().map(|s| s.to_string()).collect(),
            ..ContentIndexConfig::default()
        }
    }

    #[test]
    fn test_glob_exclusion() {
        assert!(glob_match("node_modules", "node_modules"));
        assert!(glob_match("*.bak", "notes.bak"));
        assert!(glob_match("*.BAK", "notes.bak"));
        assert!(!glob_match("*.bak", "notes.txt"));
        assert!(!glob_match("node_modules", "node_modules_old"));
    }

    #[test]
    fn test_hidden_and_excluded_names_are_skipped() {
        let config = config_with(&["secret"], &[]);
        assert!(is_excluded_name(&config, "secret"));
        // Hidden entries are skipped by the walker before this check; the
        // helper itself only sees the configured patterns.
        assert!(!is_excluded_name(&config, ".ssh"));
    }

    #[test]
    fn test_only_text_allowlisted_extensions_are_read() {
        let config = config_with(&[], &[]);
        for name in ["a.txt", "b.md", "c.RS", "d.json"] {
            assert!(is_text_file(&config, Path::new(name)), "{name}");
        }
        for name in ["a.mp4", "b.iso", "c.pdf", "d.bin", "e", "f.docx"] {
            assert!(!is_text_file(&config, Path::new(name)), "{name}");
        }
        let config = config_with(&[], &["txt"]);
        assert!(!is_text_file(&config, Path::new("a.txt")));
    }

    #[test]
    fn test_search_expression_quotes_tokens_and_drops_operators() {
        assert_eq!(
            search_expression("hello world").unwrap(),
            "\"hello\" AND \"world\""
        );
        assert_eq!(
            search_expression("NEAR(foo) OR bar*").unwrap(),
            "\"NEAR\" AND \"foo\" AND \"OR\" AND \"bar\""
        );
        assert!(search_expression("  --  ").is_none());
        assert!(search_expression("!!!").is_none());
    }

    #[test]
    fn test_search_expression_is_bounded() {
        let expression = search_expression(&["a"; 20].join(" ")).unwrap();
        assert_eq!(expression.matches(" AND ").count(), MAX_QUERY_TOKENS - 1);
    }

    #[test]
    fn test_acl_prefix_is_boundary_aware() {
        assert!(hit_is_visible(
            "/srv/share/a.txt",
            &["/srv/share".to_string()]
        ));
        assert!(hit_is_visible("/srv/share", &["/srv/share".to_string()]));
        assert!(!hit_is_visible(
            "/srv/share2/a.txt",
            &["/srv/share".to_string()]
        ));
        assert!(!hit_is_visible(
            "/home/bob/a.txt",
            &["/home/alice".to_string()]
        ));
        assert!(!hit_is_visible("/srv/share/a.txt", &[]));
    }

    #[test]
    fn test_root_must_be_a_candidate_or_below() {
        let candidates = vec!["/srv/share".to_string(), "/mnt/pool".to_string()];
        assert!(root_is_allowed("/srv/share", &candidates));
        assert!(root_is_allowed("/srv/share/docs", &candidates));
        assert!(!root_is_allowed("/srv/share2", &candidates));
        assert!(!root_is_allowed("/etc", &candidates));
    }

    #[test]
    fn test_limit_is_clamped() {
        assert_eq!(clamp_limit(None), 10);
        assert_eq!(clamp_limit(Some(0)), 1);
        assert_eq!(clamp_limit(Some(9999)), MAX_SNIPPET_RESULTS as usize);
    }

    #[test]
    fn test_take_hits_reports_truncation() {
        let hit = repo::ContentHit {
            path: "/x".to_string(),
            root: "/".to_string(),
            name: "x".to_string(),
            size_bytes: 1,
            modified: 0,
            snippet: "x".to_string(),
        };
        let (hits, truncated) = take_hits(vec![hit.clone(), hit.clone(), hit], 2);
        assert_eq!(hits.len(), 2);
        assert!(truncated);
    }
}
