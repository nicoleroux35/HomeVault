/* =============================================================================
 * File          : iscsi_guard.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Periodic protection against a full volume under an iSCSI LUN. A fileio
 *   backstore grows inside the host filesystem: once that filesystem is full,
 *   the initiator keeps writing into a filesystem that cannot allocate, which
 *   loses data. The guard flips such a backstore to read-only and re-applies
 *   the (now read-only) LUN before the space runs out.
 *
 * Dependencies:
 *   - tokio, crate::iscsi, crate::storage::IscsiRepository
 *
 * Usage:
 *   Spawned once at startup in src/web/mod.rs; see docs/guides/iscsi.md.
 * =============================================================================
 */

//! Read-only guard for fileio iSCSI backstores on a nearly-full filesystem.

use std::sync::Arc;
use std::time::Duration;

use crate::handlers::AppState;
use crate::iscsi::{self, IscsiBackstore};
use crate::storage::IscsiRepository;

const STARTUP_DELAY: Duration = Duration::from_secs(3 * 60);
const CHECK_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// What one guard pass did, for logging and tests.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct GuardReport {
    /// Backstores that were read-only and had enough space.
    pub checked: usize,
    /// Backstores flipped to read-only by this pass.
    pub protected: Vec<String>,
    /// Backstores whose free space could not be measured.
    pub unreadable: Vec<String>,
}

/// Classify one backstore and, when it is nearly full, ask the repository to
/// make it read-only. Split from the loop so the decision is testable without
/// a database.
fn needs_protection(store: &IscsiBackstore, free_bytes: Option<u64>) -> bool {
    match free_bytes {
        Some(free) => iscsi::should_protect_from_full_volume(store, free),
        // No answer from statvfs is not permission to keep writing. Marking a
        // store read-only is reversible; a corrupt backing file is not.
        None => store.kind() == iscsi::BackstoreKind::Fileio && !store.readonly,
    }
}

/// One guard pass. Returns the backstores it protected.
pub async fn check_once(state: &Arc<AppState>) -> Result<GuardReport, String> {
    let repo = IscsiRepository::new(state.db.pool());
    let backstores = repo
        .list_backstores()
        .await
        .map_err(|e| format!("could not read the iSCSI backstores: {e}"))?;

    let mut report = GuardReport::default();
    for store in &backstores {
        if store.kind() != iscsi::BackstoreKind::Fileio || store.readonly {
            continue;
        }
        report.checked += 1;
        let free = iscsi::free_bytes_at(&store.path);
        if free.is_none() {
            report.unreadable.push(store.name.clone());
        }
        if !needs_protection(store, free) {
            continue;
        }
        if let Err(e) = repo.set_backstore_readonly(store.id, true).await {
            tracing::warn!(
                "iSCSI guard: could not mark backstore '{}' read-only: {e}",
                store.name
            );
            continue;
        }
        report.protected.push(store.name.clone());
    }

    if report.protected.is_empty() {
        return Ok(report);
    }

    let message = format!(
        "The backing filesystem is nearly full; these iSCSI disks were made \
         read-only to protect their data: {}. Free space and edit the disk to \
         allow writes again.",
        report.protected.join(", ")
    );
    crate::notification_engine::notify(
        state,
        "health.disk_space",
        "iSCSI disk protected",
        &message,
        "warning",
        "storage",
        "iscsi_space_guard",
    )
    .await;
    crate::events::log_event_warn(format!("iSCSI guard: {message}"));

    // Save is metadata only. Without this apply the running kernel keeps
    // accepting writes, so the protection would exist only on paper.
    let applied = crate::handlers::iscsi::run_apply(state.db.pool()).await;
    if !applied.applied {
        crate::events::log_event_warn(format!(
            "iSCSI guard: the read-only state was saved but could not be applied: {}",
            applied
                .reason
                .unwrap_or_else(|| "unknown reason".to_string())
        ));
    }

    Ok(report)
}

pub async fn run_scheduler_loop(state: Arc<AppState>) {
    tokio::time::sleep(STARTUP_DELAY).await;
    loop {
        match check_once(&state).await {
            Ok(report) if !report.protected.is_empty() => {
                tracing::warn!(
                    "iSCSI guard protected {} backstore(s): {}",
                    report.protected.len(),
                    report.protected.join(", ")
                );
            }
            Ok(report) => {
                tracing::debug!(
                    checked = report.checked,
                    unreadable = report.unreadable.len(),
                    "iSCSI space guard pass"
                );
            }
            Err(e) => tracing::warn!("iSCSI space guard failed: {e}"),
        }
        tokio::time::sleep(CHECK_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(readonly: bool, kind: &str) -> IscsiBackstore {
        IscsiBackstore {
            id: 1,
            name: "data".to_string(),
            kind: kind.to_string(),
            path: "/var/lib/homevault/iscsi/data.img".to_string(),
            size_bytes: 1024 * 1024,
            readonly,
            write_policy: "buffered".to_string(),
            emulate_fua: true,
            unmap: true,
            created_at: "2026-11-01T00:00:00Z".to_string(),
            updated_at: "2026-11-01T00:00:00Z".to_string(),
            lun_count: 1,
        }
    }

    #[test]
    fn a_low_or_unreadable_volume_is_protected() {
        assert!(needs_protection(
            &store(false, "fileio"),
            Some(iscsi::VOLUME_FULL_RESERVE_BYTES - 1)
        ));
        assert!(needs_protection(&store(false, "fileio"), None));
        assert!(!needs_protection(
            &store(false, "fileio"),
            Some(iscsi::VOLUME_FULL_RESERVE_BYTES)
        ));
    }

    #[test]
    fn block_or_already_readonly_stores_are_skipped() {
        assert!(!needs_protection(&store(false, "block"), None));
        assert!(!needs_protection(&store(true, "fileio"), Some(0)));
    }
}
