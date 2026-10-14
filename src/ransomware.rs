/* =============================================================================
 * File          : ransomware.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Anti-ransomware protection logic: honeypot decoy files placed on
 *   protected paths, settings/paths/events management, and the
 *   auto-response executed when the watcher flags suspicious activity.
 *   Heuristic and best-effort by design — a mitigation layer, not a
 *   guarantee (scope notes in CLAUDE.md).
 *
 * Dependencies:
 *   - serde (>= 1.0)
 *
 * Usage:
 *   handlers/ransomware.rs (API surface), ransomware_watcher.rs (detection
 *   triggers the response here).
 * =============================================================================
 */

//! Anti-ransomware protection: honeypot decoy files + the auto-response
//! executed when the watcher (`src/ransomware_watcher.rs`) detects suspicious
//! activity. Heuristic/best-effort, not a guarantee — see the scope notes in
//! `docs/CLAUDE.md`.

use crate::handlers::AppState;
use crate::shares::{ShareType, SharesService};
use crate::storage::{NotificationRepository, RansomwareRepository};
use std::path::Path;
use std::sync::Arc;

/// Plausible decoy filenames — chosen to look like something a ransomware
/// scanner would target for encryption, not an obvious trap.
const HONEYPOT_NAMES: &[&str] = &[
    "Family_Photos_Backup.zip",
    "Tax_Documents_2024_DO_NOT_DELETE.pdf",
    "Passwords_Backup.xlsx",
    "Important_Contracts.docx",
];

/// Create a honeypot file in `dir` with harmless placeholder content.
/// Returns the chosen filename (persisted so the watcher and
/// `remove_honeypot` know exactly which file to watch/clean up).
pub async fn place_honeypot(dir: &Path) -> Result<String, String> {
    let dir = dir.to_path_buf();
    let filename = HONEYPOT_NAMES[rand::random_range(0..HONEYPOT_NAMES.len())].to_string();
    let path = dir.join(&filename);
    let name = filename.clone();
    tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create '{}': {}", dir.display(), e))?;
        std::fs::write(&path, b"placeholder")
            .map_err(|e| format!("Failed to write honeypot: {}", e))
    })
    .await
    .map_err(|e| format!("place_honeypot task panicked: {}", e))??;
    Ok(name)
}

/// Remove a previously placed honeypot file. Best-effort — a missing file
/// (already deleted, e.g. by the ransomware activity that triggered the
/// alert) is not an error.
pub async fn remove_honeypot(dir: &Path, filename: &str) {
    let path = dir.join(filename);
    let _ = tokio::task::spawn_blocking(move || std::fs::remove_file(&path)).await;
}

/// Record a detected event, notify, and — unless the configured
/// `auto_response` is `"alert_only"` — execute the response. Falls back to
/// `alert_only` (recorded as such) if the response can't be determined
/// (e.g. no share info for `disable_share`, no resolvable username for
/// `lock_account`) rather than guessing.
pub async fn record_event(
    state: &Arc<AppState>,
    protected_path_id: Option<&str>,
    event_type: &str,
    detail: Option<&str>,
    suspected_username: Option<&str>,
) -> Result<(), String> {
    let repo = RansomwareRepository::new(state.db.pool());
    let settings = repo.get_settings().await.map_err(|e| e.to_string())?;

    let response_taken = if settings.auto_response == "alert_only" {
        "alert_only".to_string()
    } else {
        execute_response(
            state,
            &repo,
            protected_path_id,
            &settings.auto_response,
            suspected_username,
        )
        .await
    };

    repo.record_event(
        protected_path_id,
        event_type,
        detail,
        suspected_username,
        &response_taken,
    )
    .await
    .map_err(|e| e.to_string())?;

    let notif_repo = NotificationRepository::new(state.db.pool());
    let title = match event_type {
        "honeypot_touched" => "Ransomware guard: honeypot file touched",
        "mass_modify" => "Ransomware guard: mass file modification detected",
        "mass_rename" => "Ransomware guard: mass file rename detected",
        "mass_delete" => "Ransomware guard: mass file deletion detected",
        _ => "Ransomware guard: suspicious activity detected",
    };
    let message = detail
        .map(|d| d.to_string())
        .unwrap_or_else(|| "Suspicious file activity detected.".to_string());
    let _ = notif_repo
        .create(
            title,
            &message,
            "critical",
            "ransomware",
            "ransomware_watcher",
        )
        .await;

    Ok(())
}

async fn execute_response(
    state: &Arc<AppState>,
    repo: &RansomwareRepository,
    protected_path_id: Option<&str>,
    auto_response: &str,
    suspected_username: Option<&str>,
) -> String {
    match auto_response {
        "disable_share" => {
            let Some(path_id) = protected_path_id else {
                return "alert_only".to_string();
            };
            let Ok(Some(protected)) = repo.get_protected_path(path_id).await else {
                return "alert_only".to_string();
            };
            let (Some(share_name), Some(share_type_str)) =
                (protected.share_name, protected.share_type)
            else {
                return "alert_only".to_string();
            };
            let Some(share_type) = ShareType::parse(&share_type_str) else {
                return "alert_only".to_string();
            };
            match SharesService::set_share_enabled(&share_name, share_type, false) {
                Ok(()) => "disable_share".to_string(),
                Err(e) => {
                    tracing::warn!(
                        "Ransomware guard: failed to disable share '{}': {}",
                        share_name,
                        e
                    );
                    "alert_only".to_string()
                }
            }
        }
        "lock_account" => {
            let Some(username) = suspected_username else {
                return "alert_only".to_string();
            };
            match state.auth.set_user_locked(username, true).await {
                Ok(()) => "lock_account".to_string(),
                Err(e) => {
                    tracing::warn!(
                        "Ransomware guard: failed to lock account '{}': {}",
                        username,
                        e
                    );
                    "alert_only".to_string()
                }
            }
        }
        _ => "alert_only".to_string(),
    }
}

/// Best-effort UID → username resolution via `/etc/passwd`, used to
/// attribute a detected event to a local account for the `lock_account`
/// auto-response. Returns `None` (falls back to `alert_only`, see
/// `execute_response`) for LDAP/NSS-only accounts not in this file, or if
/// the UID doesn't map to a HomeVault-managed user at all.
pub fn resolve_username_from_uid(uid: u32) -> Option<String> {
    let content = std::fs::read_to_string("/etc/passwd").ok()?;
    for line in content.lines() {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() >= 3 {
            if let Ok(entry_uid) = fields[2].parse::<u32>() {
                if entry_uid == uid {
                    return Some(fields[0].to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    //! Pure-logic tests. `place_honeypot` / `remove_honeypot` (real
    //! filesystem) and `record_event` / `execute_response` (need `AppState`,
    //! DB, a real watcher) are covered by
    //! `tests/security_integration.rs` / `tests/e2e/security/security.spec.ts`.

    use super::*;

    #[test]
    fn test_honeypot_names_look_like_real_targets_not_obvious_traps() {
        assert!(!HONEYPOT_NAMES.is_empty());
        for name in HONEYPOT_NAMES {
            // Every decoy carries a plausible document/archive extension so a
            // ransomware scanner treats it as a target worth encrypting.
            assert!(
                name.ends_with(".zip")
                    || name.ends_with(".pdf")
                    || name.ends_with(".xlsx")
                    || name.ends_with(".docx"),
                "{name} does not look like a real user file"
            );
            assert!(!name.to_lowercase().contains("honeypot"));
            assert!(!name.to_lowercase().contains("decoy"));
        }
    }

    #[test]
    fn test_resolve_username_from_uid_resolves_root() {
        // uid 0 is `root` on every Linux box `/etc/passwd` describes.
        assert_eq!(resolve_username_from_uid(0).as_deref(), Some("root"));
    }

    #[test]
    fn test_resolve_username_from_uid_unknown_uid_is_none() {
        // A uid far above any real account must not resolve to a name — the
        // auto-response then correctly falls back to alert_only.
        assert_eq!(resolve_username_from_uid(4_000_000_000), None);
    }
}
