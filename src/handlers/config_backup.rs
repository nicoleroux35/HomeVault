/* =============================================================================
 * File          : config_backup.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Full configuration backup endpoints: export an archive (database snapshot,
 *   settings and generated configs, optionally encrypted), stage a restore, and
 *   report the state so the card can explain what a restore will and will not
 *   change until the server restarts.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::config_backup, crate::db_backup
 *
 * Usage:
 *   Routes under /api/settings/config-backup in src/web/mod.rs; see
 *   docs/guides/settings.md.
 * =============================================================================
 */

//! Full configuration backup endpoints (admin-only), see
//! docs/guides/settings.md.
//!
//! * `GET  /api/settings/config-backup`              — state + capabilities
//! * `POST /api/settings/config-backup/export`       — build an archive (JSON)
//! * `GET  /api/settings/config-backup/archives/{id}`— download it (binary)
//! * `POST /api/settings/config-backup/restore`      — validate + stage a restore

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;

use crate::config_backup::{self, ArchiveError, ExportInput};
use crate::handlers::admin_guard::require_admin;
use crate::utils::ResponseHelper;

use super::AppState;

/// Directory the exported archives are written to (`data/config-backups/`).
fn archives_dir() -> std::path::PathBuf {
    crate::config::paths::data_dir().join("config-backups")
}

/// Only identifiers this server minted can be downloaded: the name is checked
/// against the allow-list of characters and the resolved path is confined to the
/// archive directory, so a crafted id cannot read an arbitrary file.
fn valid_archive_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn archive_path(id: &str) -> Option<std::path::PathBuf> {
    if !valid_archive_id(id) {
        return None;
    }
    let candidate = archives_dir().join(format!("{id}.hvbak"));
    let root = archives_dir().canonicalize().ok()?;
    let resolved = candidate.canonicalize().ok()?;
    resolved.starts_with(&root).then_some(resolved)
}

/// `GET /api/settings/config-backup`
pub async fn handle_config_backup_state(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let pending = config_backup::pending_restore();
    let archives: Vec<serde_json::Value> = std::fs::read_dir(archives_dir())
        .ok()
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let id = name.strip_suffix(".hvbak")?.to_string();
                    let meta = entry.metadata().ok()?;
                    Some(serde_json::json!({
                        "id": id,
                        "bytes": meta.len(),
                        "created_at": meta
                            .modified()
                            .ok()
                            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339()),
                    }))
                })
                .collect()
        })
        .unwrap_or_default();

    ResponseHelper::json_success(serde_json::json!({
        "app_version": env!("CARGO_PKG_VERSION"),
        "format_version": config_backup::FORMAT_VERSION,
        "settings_schema": crate::settings_persistence::CURRENT_SETTINGS_SCHEMA,
        "max_archive_bytes": config_backup::MAX_ARCHIVE_BYTES,
        "config_files": config_backup::CONFIG_FILES
            .iter()
            .map(|(archive, host)| serde_json::json!({ "archive": archive, "host": host }))
            .collect::<Vec<_>>(),
        "archives": archives,
        // A restore is only applied at the next start: the card says so instead
        // of implying a live swap.
        "pending_restore": pending.map(|manifest| serde_json::json!({
            "created_at": manifest.created_at,
            "app_version": manifest.app_version,
            "includes_secrets": manifest.includes_secrets,
            "entries": manifest.entries.len(),
        })),
    }))
}

#[derive(Debug, Deserialize)]
pub struct ExportRequest {
    /// Include credentials (requires a passphrase). Default: false.
    #[serde(default)]
    pub include_secrets: bool,
    #[serde(default)]
    pub passphrase: Option<String>,
}

/// `POST /api/settings/config-backup/export`
///
/// Builds the archive into `data/config-backups/` and answers with its id;
/// the download is a separate GET so the archive never travels inside a JSON
/// string.
pub async fn handle_config_backup_export(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ExportRequest>,
) -> Response {
    let username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    if body.include_secrets && body.passphrase.as_deref().unwrap_or("").trim().is_empty() {
        return ResponseHelper::json_error(
            "An archive that includes secrets must be encrypted: enter a passphrase".to_string(),
        );
    }
    if let Some(passphrase) = body.passphrase.as_deref() {
        if !passphrase.trim().is_empty() && passphrase.trim().len() < 12 {
            return ResponseHelper::json_error(
                "The passphrase must be at least 12 characters".to_string(),
            );
        }
    }

    // A consistent snapshot: `VACUUM INTO` while the server keeps serving.
    let snapshot_dir = crate::config::paths::data_dir().join("config-backups");
    let service = crate::db_backup::DbBackupService::new(state.db.clone());
    let snapshot = service.snapshot_into(&snapshot_dir).await;
    let snapshot = match snapshot {
        Ok(entry) => entry,
        Err(e) => {
            return ResponseHelper::json_error(format!(
                "Could not snapshot the database, so no archive was written: {e}"
            ))
        }
    };
    // Secrets excluded means the *bytes* must be clean, not just the manifest:
    // the snapshot is scrubbed before it is read into the archive.
    if !body.include_secrets {
        if let Err(e) = config_backup::scrub_database(std::path::Path::new(&snapshot.path)).await {
            return ResponseHelper::json_error(e);
        }
    }

    let database = match tokio::fs::read(&snapshot.path).await {
        Ok(bytes) => bytes,
        Err(e) => {
            return ResponseHelper::json_error(format!("The snapshot could not be read: {e}"))
        }
    };

    // Settings: the existing export already masks the SMTP password.
    let settings = match crate::settings_persistence::export_settings() {
        Some(json) => json,
        None => {
            return ResponseHelper::json_error(
                "The settings could not be exported, so no archive was written".to_string(),
            )
        }
    };

    // Configs: allow-listed, and a missing one is reported rather than fatal.
    let mut configs = Vec::new();
    let mut skipped = Vec::new();
    for (archive_path, host_path) in config_backup::CONFIG_FILES {
        match config_backup::read_config(archive_path, host_path) {
            Ok(bytes) => configs.push((archive_path.to_string(), bytes)),
            Err(skipped_entry) => skipped.push(skipped_entry),
        }
    }

    let input = ExportInput {
        database,
        settings,
        configs,
        skipped,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        settings_schema: crate::settings_persistence::CURRENT_SETTINGS_SCHEMA,
    };
    let created_at = chrono::Utc::now().to_rfc3339();
    let bytes = match config_backup::build(
        &input,
        body.include_secrets,
        body.passphrase.as_deref(),
        &created_at,
    ) {
        Ok(bytes) => bytes,
        Err(e) => return ResponseHelper::json_error(e),
    };

    // `2026-10-31T12-00-00Z` — filesystem-safe, sorts chronologically.
    let id = created_at
        .replace(':', "-")
        .replace('+', "Z")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect::<String>();
    let dir = archives_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return ResponseHelper::json_internal_error(format!(
            "The archive directory could not be created: {e}"
        ));
    }
    let path = dir.join(format!("{id}.hvbak"));
    if let Err(e) = crate::storage::AtomicWriter::write_bytes(&path, &bytes) {
        return ResponseHelper::json_internal_error(format!(
            "The archive could not be written: {e}"
        ));
    }

    crate::events::log_event(format!(
        "Admin '{}' exported a configuration backup ({} files, {}, secrets={}, encrypted={})",
        username,
        input.configs.len() + 2,
        crate::share_quota::human_size(bytes.len() as u64),
        body.include_secrets,
        body.passphrase
            .as_deref()
            .is_some_and(|p| !p.trim().is_empty())
    ));

    ResponseHelper::json_success(serde_json::json!({
        "id": id,
        "bytes": bytes.len(),
        "path": path.to_string_lossy(),
        "created_at": created_at,
        "includes_secrets": body.include_secrets,
        "encrypted": body.passphrase.as_deref().is_some_and(|p| !p.trim().is_empty()),
        "skipped": input_skipped_echo(&bytes),
    }))
}

/// The skipped list as the archive recorded it, so the card can name what the
/// restore will not have.
fn input_skipped_echo(bytes: &[u8]) -> serde_json::Value {
    // Reading our own archive back is the cheapest way to echo exactly what it
    // says; a failure here must not fail the export.
    match config_backup::read(bytes, None) {
        Ok(archive) => serde_json::json!(archive.manifest.skipped),
        Err(_) => serde_json::json!([]),
    }
}

/// `GET /api/settings/config-backup/archives/{id}` — the archive itself.
pub async fn handle_config_backup_download(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    // A malformed id is a bad request, not a miss: naming that difference is
    // what tells an admin they mistyped instead of hunting for a missing file.
    if !valid_archive_id(&id) {
        return ResponseHelper::json_error_with_status(
            StatusCode::BAD_REQUEST,
            "That archive identifier is not valid".to_string(),
        );
    }
    let Some(path) = archive_path(&id) else {
        return ResponseHelper::json_error_with_status(
            StatusCode::NOT_FOUND,
            "That archive does not exist".to_string(),
        );
    };
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return ResponseHelper::json_error_with_status(
                StatusCode::NOT_FOUND,
                "That archive does not exist".to_string(),
            )
        }
    };
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"homevault-config-{id}.hvbak\""),
        )
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    response
}

/// `POST /api/settings/config-backup/restore`
///
/// The archive travels as the raw request body (a `.hvbak` file is binary; a
/// JSON envelope would base64-inflate it). The passphrase, when one is needed,
/// comes in the `x-backup-passphrase` header — never in the query string, where
/// it would land in an access log.
pub async fn handle_config_backup_restore(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    if body.len() > config_backup::MAX_ARCHIVE_BYTES {
        return ResponseHelper::json_error(format!(
            "That file is larger than the {} limit for a configuration archive",
            crate::share_quota::human_size(config_backup::MAX_ARCHIVE_BYTES as u64)
        ));
    }
    if !config_backup::looks_like_archive(&body) {
        return ResponseHelper::json_error(ArchiveError::NotAnArchive.message());
    }
    let passphrase = headers
        .get("x-backup-passphrase")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .filter(|p| !p.is_empty());

    let archive = match config_backup::read(&body, passphrase.as_deref()) {
        Ok(archive) => archive,
        Err(e) => return ResponseHelper::json_error(e.message()),
    };
    let manifest = archive.manifest.clone();
    let staged = match config_backup::stage(&archive) {
        Ok(staged) => staged,
        Err(e) => return ResponseHelper::json_error(e),
    };
    if let Err(e) = config_backup::write_staged_manifest(&manifest) {
        return ResponseHelper::json_internal_error(format!(
            "The restore was staged but its manifest could not be written: {e}"
        ));
    }

    crate::events::log_event_warn(format!(
        "Admin '{}' staged a configuration restore from {} ({} entries, secrets={}); it is applied \
         at the next start",
        username,
        manifest.created_at,
        manifest.entries.len(),
        manifest.includes_secrets
    ));

    ResponseHelper::json_success(serde_json::json!({
        "staged": true,
        // Never "restored": the swap happens before the database is opened.
        "requires_restart": true,
        "manifest": {
            "created_at": manifest.created_at,
            "app_version": manifest.app_version,
            "format_version": manifest.format_version,
            "settings_schema": manifest.settings_schema,
            "includes_secrets": manifest.includes_secrets,
            "encrypted": manifest.encrypted,
            "entries": manifest.entries.len(),
            "skipped": manifest.skipped,
        },
        "staged_entries": staged.staged,
        "needs_root": staged.needs_root,
        "message": format!(
            "The archive was validated and staged ({} files). Restart HomeVault to apply it; the \
             current database is kept as homevault.db.pre-restore.",
            staged.staged.len()
        ),
    }))
}

/// `POST /api/settings/config-backup/inspect` — dry run, body is the archive.
pub async fn handle_config_backup_inspect(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let passphrase = headers
        .get("x-backup-passphrase")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .filter(|p| !p.is_empty());
    match config_backup::read(&body, passphrase.as_deref()) {
        Ok(archive) => ResponseHelper::json_success(serde_json::json!({
            "ok": true,
            "manifest": archive.manifest,
        })),
        // The refusal is a 400 with the sentence, so the file picker can show it
        // before the admin commits to a restart.
        Err(e) => ResponseHelper::json_error(e.message()),
    }
}
