/* =============================================================================
 * File          : storage.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Inventory and management of the server's disks and virtual disks: lists
 *   of physical disks, partitions, RAID arrays and USB drives, system-disk
 *   detection, health checks, and the RAID app's create/teardown endpoints
 *   (ext4/xfs via mdadm, native ZFS, native multi-device Btrfs). Also SMART
 *   self-tests, scrub/consistency checks, and discovery/import of pools from
 *   disks moved between machines. Reads are auth-only; every mutation is
 *   admin-only and written to the event log.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - serde_json (>= 1.0)
 *
 * Usage:
 *   Routes /api/disks, /api/raid, /api/virtual-disks and /api/health in
 *   src/web/mod.rs; the heavy lifting is delegated to DiskService,
 *   PoolService and HealthService.
 * =============================================================================
 */

//! Disk, RAID ("virtual disk") and health-check endpoints: inventory of
//! physical disks, partitions and USB drives, creation and teardown of RAID
//! sets (mdadm, ZFS, native Btrfs), SMART self-tests, scrub/consistency
//! checks, and discovery/import of foreign pools.

use crate::disk::DiskService;
use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::handlers::AppState;
use crate::health::HealthService;
use crate::sse::progress::{JobType, ProgressEmitter, PROGRESS_MANAGER};
use crate::utils::ResponseHelper;
use axum::{extract::State, http::HeaderMap, response::Response};
use std::sync::Arc;

/// Lists all physical disks detected on the system.
#[axum_macros::debug_handler]
pub async fn handle_disks_list(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    match DiskService::list_disks().await {
        Ok(disks) => ResponseHelper::json_success(disks),
        Err(e) => ResponseHelper::json_error(format!("Failed to list disks: {}", e)),
    }
}

/// Lists all disk partitions and their mount points.
#[axum_macros::debug_handler]
pub async fn handle_partitions_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    match DiskService::list_partitions().await {
        Ok(partitions) => ResponseHelper::json_success(partitions),
        Err(e) => ResponseHelper::json_error(format!("Failed to list partitions: {}", e)),
    }
}

/// Lists all RAID arrays (mdadm) detected on the system.
#[axum_macros::debug_handler]
pub async fn handle_raid_arrays_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    match DiskService::list_raid_arrays().await {
        Ok(arrays) => ResponseHelper::json_success(arrays),
        Err(e) => ResponseHelper::json_error(format!("Failed to list RAID arrays: {}", e)),
    }
}

/// Lists removable / external USB drives.
#[axum_macros::debug_handler]
pub async fn handle_removable_disks_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    match DiskService::list_removable_disks().await {
        Ok(disks) => ResponseHelper::json_success(disks),
        Err(e) => ResponseHelper::json_error(format!("Failed to list removable disks: {}", e)),
    }
}

/// Detects and returns the system (boot/root) disk device.
#[axum_macros::debug_handler]
pub async fn handle_system_disk(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    match DiskService::get_system_disk().await {
        Ok(device) => ResponseHelper::json_success(serde_json::json!({ "device": device })),
        Err(e) => ResponseHelper::json_error(format!("Failed to detect system disk: {}", e)),
    }
}

/// Runs all system health checks and returns aggregated results.
///
/// `run_all_checks` offloads its subprocess work to `spawn_blocking`
/// internally — see `src/health.rs`. Handler is un-routed today (kept as a
/// trap for future route wiring) but the offload contract binds it all the
/// same.
#[axum_macros::debug_handler]
pub async fn handle_health_all(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let pool = state.db.pool();
    match HealthService::run_all_checks(&pool).await {
        Ok(health) => ResponseHelper::json_success(health),
        Err(e) => ResponseHelper::json_error(format!("Failed to run health checks: {}", e)),
    }
}

/// Runs a single named health check.
#[axum_macros::debug_handler]
pub async fn handle_health_check(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let pool = state.db.pool();
    match HealthService::run_check(&name, &pool).await {
        Ok(check) => ResponseHelper::json_success(check),
        Err(e) => ResponseHelper::json_error(format!("Failed to run health check: {}", e)),
    }
}

/// Body for creating a new RAID array.
#[derive(serde::Deserialize)]
pub struct CreateRaidRequest {
    pub name: String,
    pub level: String,
    pub devices: Vec<String>,
    pub filesystem: String,
    /// Extra devices attached to the array as hot spares (mdadm techs only).
    #[serde(default)]
    pub spares: Option<Vec<String>>,
    /// Create the array when `devices` is one (raid6: two) short of the level
    /// minimum, leaving a `missing` slot to fill with a later disk.
    #[serde(default)]
    pub allow_missing: bool,
}

/// Bridges the creation flow's percent/message milestones into the progress
/// job the request itself created (this struct is the single implementation
/// of `CreateProgress` for the RAID feature).
struct CreateJobProgress {
    job_id: String,
}

impl crate::disk::CreateProgress for CreateJobProgress {
    fn report(&self, percent: u8, message: &str) {
        ProgressEmitter::progress(&self.job_id, percent as f64, message);
    }
}

/// Creates a new "virtual disk" (a RAID set) from the chosen physical
/// disks. This is the RAID app's only job. The `filesystem` picks the
/// technology:
///   - `ext4` / `xfs` → an `mdadm` array formatted with that FS
///   - `zfs`          → a native ZFS pool (striped mirrors for raid10)
///   - `btrfs`        → a **native** multi-device Btrfs filesystem, not
///                      Btrfs-on-mdadm
///
/// Handles all three technologies through `DiskService::create_virtual_disk`
/// (one dispatcher) and runs the work in a **tracked progress job**: the
/// request returns `job_id` immediately, the client follows the milestones
/// on `/api/progress/{job_id}` (SSE) — array creation, formatting, fstab
/// write and initramfs refresh take minutes on real hardware and the old
/// synchronous request could hit gateway timeouts.
///
/// Fast pre-checks (request shape, device gate, name collision) still
/// answer synchronously with a 400 so the wizard fails loud *before* the
/// job starts.
#[axum_macros::debug_handler]
pub async fn handle_raid_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(payload): axum::extract::Json<CreateRaidRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let mut members = payload.devices.clone();
    members.sort();
    members.dedup();
    let spares = payload.spares.clone().unwrap_or_default();
    let mut spares = spares;
    spares.sort();
    spares.dedup();

    let req = crate::disk::VirtualDiskRequest {
        name: payload.name.clone(),
        level: payload.level.clone(),
        members,
        spares,
        allow_missing: payload.allow_missing,
        filesystem: payload.filesystem.clone(),
    };

    // Pure request-shape validation (level/fs/name/count rules) — 400 before
    // anything else runs.
    if let Err(e) = DiskService::validate_virtual_disk_request(&req) {
        return ResponseHelper::json_error(format!("Failed to create virtual disk: {}", e));
    }

    // Server-side safety gate before touching anything: every candidate
    // device must exist and be entirely free (not the system disk, no
    // partitions, no filesystem, not mounted, no md/ZFS/Btrfs superblock).
    // The wizard's own list can't be trusted as the only guard — a
    // hand-built admin request used to reach `mdadm --create --run` with
    // zero device checks.
    let all_devices = req
        .members
        .iter()
        .chain(req.spares.iter())
        .cloned()
        .collect::<Vec<String>>();
    if let Err(e) = DiskService::validate_pool_devices(&all_devices).await {
        return ResponseHelper::json_error(format!("Failed to create virtual disk: {}", e));
    }

    // Duplicate names used to fail only after mdadm ran: catch them upfront.
    if DiskService::array_name_taken(&req.name).await {
        return ResponseHelper::json_error(format!(
            "A virtual disk named '{}' already exists",
            req.name
        ));
    }

    // Tracked job: created now, advanced inside the spawned task below.
    let job_id = PROGRESS_MANAGER.create_job(
        JobType::Custom("raid_create".to_string()),
        &format!("Creating virtual disk '{}'", req.name),
    );
    let reporter = CreateJobProgress {
        job_id: job_id.clone(),
    };

    let name = req.name.clone();
    let level = req.level.clone();
    let filesystem = req.filesystem.clone();
    let username = admin_username;
    let job_id_for_task = job_id.clone();
    tokio::spawn(async move {
        // Single-flight: serializes destructive creates so two concurrent
        // requests settling on the same free disk can't both pass the gate.
        let _guard = crate::disk::CREATE_LOCK.lock().await;
        match DiskService::create_virtual_disk(&req, &reporter).await {
            Ok(mount_point) => {
                crate::events::log_event(format!(
                    "Admin '{}' created virtual disk '{}' ({} / {}) — devices {:?}, mounted at {}",
                    username, name, level, filesystem, req.members, mount_point
                ));
                ProgressEmitter::complete(
                    &job_id_for_task,
                    &format!(
                        "Virtual disk '{}' created and mounted at {}",
                        name, mount_point
                    ),
                );
            }
            Err(e) => {
                crate::events::log_event_warn(format!(
                    "Admin '{}' failed to create virtual disk '{}' ({} / {}): {}",
                    username, name, level, filesystem, e
                ));
                ProgressEmitter::fail(
                    &job_id_for_task,
                    &format!("Failed to create virtual disk: {}", e),
                );
            }
        }
    });

    ResponseHelper::json_success(serde_json::json!({
        "message": "Virtual disk creation started",
        "job_id": job_id,
    }))
}

/// Tears down a RAID array by name.
#[axum_macros::debug_handler]
pub async fn handle_raid_delete(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    match DiskService::delete_raid(&name).await {
        Ok(_) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' deleted RAID array '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success(
                serde_json::json!({ "message": "RAID array deleted successfully" }),
            )
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to delete RAID: {}", e)),
    }
}

/// Returns block devices available for RAID creation (unpartitioned, unused).
#[axum_macros::debug_handler]
pub async fn handle_raid_available_devices(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    match DiskService::get_available_devices().await {
        Ok(devices) => ResponseHelper::json_success(devices),
        Err(e) => ResponseHelper::json_error(format!("Failed to get available devices: {}", e)),
    }
}

/// Body for rebuilding a RAID array. `replacement_device` is only needed when
/// the array is missing a member (e.g. a physically failed disk was removed);
/// without it, a degraded array can't actually be repaired.
#[derive(serde::Deserialize, Default)]
pub struct RebuildRaidRequest {
    #[serde(default)]
    pub replacement_device: Option<String>,
}

/// Triggers a rebuild of a RAID array (e.g. after replacing a failed disk).
#[axum_macros::debug_handler]
pub async fn handle_raid_rebuild(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
    axum::extract::Json(payload): axum::extract::Json<RebuildRaidRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    // The replacement disk must pass the same free-device gate as creation —
    // `mdadm --add` used to accept a partitioned, mounted or system disk
    // silently.
    if let Some(replacement) = payload.replacement_device.as_ref() {
        if let Err(e) = DiskService::validate_pool_devices(std::slice::from_ref(replacement)).await
        {
            return ResponseHelper::json_error(format!("Failed to rebuild RAID: {}", e));
        }
    }

    match DiskService::rebuild_raid(&name, payload.replacement_device.as_deref()).await {
        Ok(_) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' started a rebuild of RAID array '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success(serde_json::json!({ "message": "RAID rebuild started" }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to rebuild RAID: {}", e)),
    }
}

/// Returns SMART capabilities for a disk (supported test types).
#[axum_macros::debug_handler]
pub async fn handle_disk_smart_capabilities(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let caps = DiskService::get_smart_capabilities(&name).await;
    let status = DiskService::get_smart_test_status(&name).await;
    let remaining = if status.in_progress {
        DiskService::get_smart_test_remaining(&name).await
    } else {
        0
    };

    ResponseHelper::json_success(serde_json::json!({
        "capabilities": caps,
        "test_status": status,
        "estimated_remaining_minutes": remaining,
    }))
}

/// Start a SMART self-test on a disk. Admin-only: this runs a real
/// `smartctl -t` on the host device.
#[axum_macros::debug_handler]
pub async fn handle_disk_smart_test_start(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
    axum::Json(payload): axum::Json<serde_json::Value>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let test_type = payload
        .get("test_type")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    match DiskService::start_smart_test(&name, test_type).await {
        Ok(()) => {
            let remaining = DiskService::get_smart_test_remaining(&name).await;
            crate::events::log_event(format!(
                "Admin '{}' started a SMART {} test on disk '{}'",
                admin_username, test_type, name
            ));
            ResponseHelper::json_success(serde_json::json!({
                "message": format!("SMART {} test started", test_type),
                "estimated_remaining_minutes": remaining,
            }))
        }
        Err(e) => ResponseHelper::json_error(e.to_string()),
    }
}

/// Get current SMART test status (progress % and remaining time).
#[axum_macros::debug_handler]
pub async fn handle_disk_smart_test_status(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let status = DiskService::get_smart_test_status(&name).await;
    let remaining = if status.in_progress {
        DiskService::get_smart_test_remaining(&name).await
    } else {
        0
    };

    ResponseHelper::json_success(serde_json::json!({
        "test_status": status,
        "estimated_remaining_minutes": remaining,
    }))
}

// ===========================================================================
// Virtual disks — STORAGE's unified view over every RAID set (mdadm + ZFS +
// Btrfs). The RAID app only creates them; everything else lives here.
// ===========================================================================

/// `PoolService::scan_pools` reports mdadm arrays by their full device path
/// (`/dev/md/storage`); a plain name is what URLs and `delete_raid` want.
fn vd_name(raw: &str) -> String {
    raw.trim_start_matches("/dev/md/")
        .trim_start_matches("/dev/")
        .to_string()
}

/// Lists every virtual disk on the host — mdadm arrays, ZFS pools and Btrfs
/// filesystems together, in one shape (`crate::pools::PoolInfo`).
#[axum_macros::debug_handler]
pub async fn handle_virtual_disks_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    match tokio::task::spawn_blocking(crate::pools::PoolService::scan_pools).await {
        Ok(Ok(mut pools)) => {
            for p in &mut pools {
                p.name = vd_name(&p.name);
            }
            ResponseHelper::json_success(pools)
        }
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to list virtual disks: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Internal error: {}", e)),
    }
}

/// Finds a virtual disk by (normalised) name and returns its technology.
async fn virtual_disk_type(name: &str) -> Option<crate::pools::PoolType> {
    let name = name.to_string();
    tokio::task::spawn_blocking(move || {
        crate::pools::PoolService::scan_pools()
            .ok()?
            .into_iter()
            .find(|p| vd_name(&p.name) == name)
            .map(|p| p.pool_type)
    })
    .await
    .ok()
    .flatten()
}

/// Destroys a virtual disk, dispatching to the right teardown for its
/// technology (mdadm → `DiskService::delete_raid`, which also cleans
/// `mdadm.conf`/`fstab`; ZFS/Btrfs → `PoolService::delete_pool`).
#[axum_macros::debug_handler]
pub async fn handle_virtual_disk_delete(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let kind = match virtual_disk_type(&name).await {
        Some(k) => k,
        None => return ResponseHelper::json_error(format!("Virtual disk '{}' not found", name)),
    };

    let result = match kind {
        crate::pools::PoolType::Mdadm => DiskService::delete_raid(&name)
            .await
            .map_err(|e| e.to_string()),
        crate::pools::PoolType::Zfs | crate::pools::PoolType::Btrfs => {
            let n = name.clone();
            let k = kind.clone();
            tokio::task::spawn_blocking(move || {
                crate::pools::PoolService::delete_pool(&n, k).map_err(|e| e.to_string())
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()))
        }
        crate::pools::PoolType::Unknown => Err("Unknown virtual disk type".to_string()),
    };

    match result {
        Ok(_) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' destroyed virtual disk '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success(serde_json::json!({ "message": "Virtual disk destroyed" }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to destroy virtual disk: {}", e)),
    }
}

/// Starts a data-consistency check on a virtual disk (ZFS/Btrfs `scrub`,
/// mdadm `check` via `sync_action`).
#[axum_macros::debug_handler]
pub async fn handle_virtual_disk_scrub(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let kind = match virtual_disk_type(&name).await {
        Some(k) => k,
        None => return ResponseHelper::json_error(format!("Virtual disk '{}' not found", name)),
    };

    let n = name.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        match kind {
            crate::pools::PoolType::Zfs | crate::pools::PoolType::Btrfs => {
                crate::pools::PoolService::start_scrub(&n, kind).map_err(|e| e.to_string())
            }
            crate::pools::PoolType::Mdadm => {
                // Resolve the array device → mdX, then ask the kernel to check.
                let dev = if n.starts_with("/dev/") {
                    n.clone()
                } else {
                    format!("/dev/md/{}", n)
                };
                let md = std::fs::canonicalize(&dev)
                    .ok()
                    .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
                    .ok_or_else(|| format!("Could not resolve md device for '{}'", n))?;
                let out = std::process::Command::new("sudo")
                    .arg("bash")
                    .arg("-c")
                    .arg(format!("echo check > /sys/block/{}/md/sync_action", md))
                    .output()
                    .map_err(|e| e.to_string())?;
                if !out.status.success() {
                    return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
                }
                Ok(())
            }
            crate::pools::PoolType::Unknown => Err("Unknown virtual disk type".to_string()),
        }
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));

    match result {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' started a consistency check on virtual disk '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success(
                serde_json::json!({ "message": "Consistency check started" }),
            )
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to start check: {}", e)),
    }
}

/// Reports the current scrub / consistency-check status of a virtual disk.
///
/// ZFS returns the `scan:` line from `zpool status`; Btrfs the `Status:` line
/// from `btrfs scrub status`; mdadm reads `/sys/block/mdX/md/sync_action`
/// (`idle`, `check`, `resync`, ...). Auth-only: it's a read.
#[axum_macros::debug_handler]
pub async fn handle_virtual_disk_scrub_status(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let kind = match virtual_disk_type(&name).await {
        Some(k) => k,
        None => return ResponseHelper::json_error(format!("Virtual disk '{}' not found", name)),
    };

    let n = name.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
        match kind {
            crate::pools::PoolType::Zfs | crate::pools::PoolType::Btrfs => {
                crate::pools::PoolService::get_scrub_status(&n, kind).map_err(|e| e.to_string())
            }
            crate::pools::PoolType::Mdadm => {
                let dev = if n.starts_with("/dev/") {
                    n.clone()
                } else {
                    format!("/dev/md/{}", n)
                };
                let md = std::fs::canonicalize(&dev)
                    .ok()
                    .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
                    .ok_or_else(|| format!("Could not resolve md device for '{}'", n))?;
                let path = format!("/sys/block/{}/md/sync_action", md);
                std::fs::read_to_string(&path)
                    .map(|s| s.trim().to_string())
                    .map_err(|e| format!("Could not read {}: {}", path, e))
            }
            crate::pools::PoolType::Unknown => Err("Unknown virtual disk type".to_string()),
        }
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));

    match result {
        Ok(status) => ResponseHelper::json_success(serde_json::json!({
            "name": name,
            "status": status,
            "in_progress": scrub_in_progress(&status),
        })),
        Err(e) => ResponseHelper::json_error(format!("Failed to read scrub status: {}", e)),
    }
}

/// Heuristic: whether a scrub-status string describes work in progress.
///
/// ZFS `scan: scrub in progress since ...`, Btrfs `Status: running` and mdadm
/// `check`/`resync`/`repair` all mean "busy"; `idle`, `finished`, `No scrub in
/// progress` and `Not available` mean "not busy".
fn scrub_in_progress(status: &str) -> bool {
    let lower = status.to_ascii_lowercase();
    // ZFS reports the idle state as the literal phrase "No scrub in progress",
    // which contains the "in progress" substring an active scrub uses — so the
    // negation has to be ruled out first.
    if lower.contains("no scrub in progress") {
        return false;
    }
    if lower.contains("in progress") || lower.contains("running") {
        return true;
    }
    let trimmed = lower.trim();
    matches!(trimmed, "check" | "resync" | "repair" | "reshape")
}

#[derive(serde::Deserialize)]
pub struct GrowVirtualDiskRequest {
    /// The free block device the array grows onto.
    pub device: String,
}

/// Grows an mdadm virtual disk onto an additional device (`mdadm --add`
/// then `--grow --raid-devices N+1`; the reshape runs in the background).
/// Admin-only: destructive-storage mutation. ZFS pools and Btrfs
/// filesystems gain disks natively in their own tooling — mdadm is the only
/// technology this appliance-level `grow` covers.
#[axum_macros::debug_handler]
pub async fn handle_virtual_disk_grow(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
    axum::extract::Json(payload): axum::extract::Json<GrowVirtualDiskRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    // Only mdadm arrays are growable through the appliance.
    match virtual_disk_type(&name).await {
        Some(crate::pools::PoolType::Mdadm) => {}
        _ => {
            return ResponseHelper::json_error(format!(
                "Virtual disk '{}' is not a growable mdadm array",
                name
            ))
        }
    }

    // The new disk must be whole, free and not the system disk — same gate
    // as creation.
    if let Err(e) = DiskService::validate_pool_devices(std::slice::from_ref(&payload.device)).await
    {
        return ResponseHelper::json_error(format!("Failed to grow virtual disk: {}", e));
    }

    match DiskService::grow_raid(&name, &payload.device).await {
        Ok(_) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' grew virtual disk '{}' onto device {} (background reshape in progress)",
                admin_username, name, payload.device
            ));
            ResponseHelper::json_success(serde_json::json!({
                "message": format!("Disk {} added — the array is reshaping in the background", payload.device),
            }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to grow virtual disk: {}", e)),
    }
}

/// Lists pools present on attached disks but not yet imported ("foreign"
/// pools) — the data-recovery path when disks move between machines.
#[axum_macros::debug_handler]
pub async fn handle_importable_pools_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    match tokio::task::spawn_blocking(crate::pools::PoolService::list_importable_pools).await {
        Ok(Ok(pools)) => ResponseHelper::json_success(pools),
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to scan importable pools: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Internal error: {}", e)),
    }
}

#[derive(serde::Deserialize)]
pub struct ImportPoolRequest {
    name: String,
    pool_type: String,
    /// Optional rename applied at import time (ZFS only).
    #[serde(default)]
    new_name: Option<String>,
}

/// Imports a foreign pool (ZFS `zpool import`, Btrfs mount). Admin-only: it
/// brings new storage online under the instance's control.
#[axum_macros::debug_handler]
pub async fn handle_import_pool(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<ImportPoolRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let pool_type = match req.pool_type.to_ascii_lowercase().as_str() {
        "zfs" => crate::pools::PoolType::Zfs,
        "btrfs" => crate::pools::PoolType::Btrfs,
        other => {
            return ResponseHelper::json_error(format!(
                "Unsupported pool type '{}' (expected zfs or btrfs)",
                other
            ))
        }
    };

    let name = req.name.clone();
    let new_name = req.new_name.clone();
    let result = tokio::task::spawn_blocking(move || {
        crate::pools::PoolService::import_pool(&name, pool_type, new_name.as_deref())
            .map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));

    match result {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' imported foreign pool '{}'",
                admin_username, req.name
            ));
            ResponseHelper::json_success(serde_json::json!({ "message": "Pool imported" }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to import pool: {}", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vd_name_normalises_an_mdadm_device_path() {
        // `PoolService::scan_pools` reports mdadm arrays as `/dev/md/<name>`;
        // URLs and `DiskService::delete_raid` want the bare name.
        assert_eq!(vd_name("/dev/md/storage"), "storage");
        assert_eq!(vd_name("/dev/md/data"), "data");
    }

    #[test]
    fn test_vd_name_strips_a_plain_dev_prefix() {
        // A kernel-numbered array with no /dev/md/ symlink.
        assert_eq!(vd_name("/dev/md127"), "md127");
    }

    #[test]
    fn test_vd_name_leaves_a_zfs_or_btrfs_pool_name_untouched() {
        assert_eq!(vd_name("tank"), "tank");
        assert_eq!(vd_name("storage-pool"), "storage-pool");
    }

    #[test]
    fn test_scrub_in_progress_detects_busy_states() {
        assert!(scrub_in_progress(
            "scan: scrub in progress since Sun Jan  1 00:00:00 2026"
        ));
        assert!(scrub_in_progress("Status: running"));
        assert!(scrub_in_progress("check"));
        assert!(scrub_in_progress("resync"));
        assert!(scrub_in_progress("repair"));
    }

    #[test]
    fn test_scrub_in_progress_detects_idle_states() {
        assert!(!scrub_in_progress("scan: scrub repaired 0B in 00:00:01"));
        assert!(!scrub_in_progress("idle"));
        assert!(!scrub_in_progress("No scrub in progress"));
        assert!(!scrub_in_progress("Not available"));
        assert!(!scrub_in_progress("Unsupported"));
    }
}
