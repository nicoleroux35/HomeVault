/* =============================================================================
 * File          : mod.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   HTTP router and server startup: assembles every API route with its
 *   auth model, wires the middleware layers (contract, CSRF, setup guard,
 *   security headers, CORS, body limits), spawns the background
 *   schedulers/watchers, and brings up the HTTP/HTTPS listeners plus the
 *   static SPA and per-app proxies.
 *
 * Dependencies:
 *   - axum (>= 0.8), tower-http, tokio (>= 1.52)
 *
 * Usage:
 *   Launched by src/bin/server.rs; see docs/API.md for the route map.
 * =============================================================================
 */

//! HTTP router and server startup — assembles all API routes and middleware.

use axum::{
    extract::{ConnectInfo, State},
    routing::{delete, get, post, put},
    Json, Router,
};
use http::header::{HeaderMap, HeaderName};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use crate::handlers::chat::{
    handle_chat_model_get, handle_chat_model_set, handle_chat_models_list, handle_chat_models_pull,
    handle_chat_models_recommend, handle_chat_send, handle_chat_send_stream,
};
use crate::handlers::rag::{
    handle_rag_documents, handle_rag_index, handle_rag_search, handle_rag_status,
};
use crate::handlers::{
    antivirus::{
        handle_antivirus_scan_cancel, handle_antivirus_scan_start, handle_antivirus_scans_list,
        handle_antivirus_settings_get, handle_antivirus_settings_update, handle_antivirus_status,
        handle_antivirus_threat_resolve, handle_antivirus_threats_list,
    },
    app_auth::handle_app_auth,
    auth::{
        handle_2fa_setup, handle_2fa_verify, handle_check_username, handle_generate_recovery_code,
        handle_get_current_user, handle_login, handle_logout, handle_reset_password_recovery,
        handle_setup, handle_setup_progress_get, handle_setup_progress_put,
        handle_setup_rebaseline, handle_setup_status, handle_setup_verify_totp,
        handle_verify_recovery_code,
    },
    avatars::{handle_avatar_preview, handle_update_my_avatar, handle_user_avatar},
    cache::{
        handle_cache_mover_run, handle_cache_mover_runs, handle_cache_pools_get,
        handle_cache_settings_get, handle_cache_settings_update, handle_cache_share_create,
        handle_cache_share_delete, handle_cache_share_update, handle_cache_shares_list,
    },
    compose::{
        handle_compose_available, handle_compose_create, handle_compose_delete, handle_compose_get,
        handle_compose_get_config, handle_compose_list, handle_compose_pull,
        handle_compose_restart, handle_compose_services, handle_compose_start, handle_compose_stop,
        handle_compose_update, handle_compose_validate,
    },
    dashboard::{
        handle_add_widget, handle_cpu_temperature, handle_dashboard_data, handle_disk_io_data,
        handle_get_all_widgets, handle_get_widget_preferences, handle_health_data,
        handle_network_traffic_data, handle_remove_widget, handle_reorder_widgets,
        handle_server_details, handle_storage_data, handle_storage_volumes,
        handle_update_widget_preference, handle_widget_data,
    },
    docker::{
        handle_docker_available, handle_docker_container_create, handle_docker_container_exec,
        handle_docker_container_inspect, handle_docker_container_kill,
        handle_docker_container_logs, handle_docker_container_logs_stream,
        handle_docker_container_logs_ticket, handle_docker_container_pause,
        handle_docker_container_recreate, handle_docker_container_remove,
        handle_docker_container_rename, handle_docker_container_restart,
        handle_docker_container_start, handle_docker_container_stop,
        handle_docker_container_unpause, handle_docker_container_update,
        handle_docker_containers_list, handle_docker_containers_running,
        handle_docker_daemon_config_get, handle_docker_daemon_config_put,
        handle_docker_daemon_logs, handle_docker_daemon_start, handle_docker_hub_search,
        handle_docker_image_build, handle_docker_image_pull, handle_docker_image_remove,
        handle_docker_images_list, handle_docker_info, handle_docker_network_connect,
        handle_docker_network_create, handle_docker_network_disconnect, handle_docker_network_get,
        handle_docker_network_prune, handle_docker_network_remove, handle_docker_networks_list,
        handle_docker_port_check, handle_docker_prune, handle_docker_registry_list,
        handle_docker_registry_login, handle_docker_registry_logout, handle_docker_system_df,
        handle_docker_volume_create, handle_docker_volume_remove, handle_docker_volumes_list,
    },
    docker_catalog::{
        handle_catalog_gallery, handle_catalog_icon, handle_catalog_install,
        handle_catalog_install_review, handle_catalog_list, handle_catalog_logs,
        handle_catalog_proxy_config, handle_catalog_uninstall, handle_catalog_update,
        handle_catalog_update_all, handle_catalog_updates, handle_store_add, handle_store_delete,
        handle_store_sync, handle_stores_list,
    },
    downloader::{
        handle_downloader_add, handle_downloader_global_stat, handle_downloader_pause,
        handle_downloader_remove, handle_downloader_start, handle_downloader_status,
        handle_downloader_stop, handle_downloader_task, handle_downloader_tasks,
        handle_downloader_unpause,
    },
    encryption::{
        handle_encryption_available_devices, handle_encryption_change_passphrase,
        handle_encryption_create, handle_encryption_delete, handle_encryption_list,
        handle_encryption_lock, handle_encryption_unlock,
    },
    explorer::{
        handle_explorer_compress, handle_explorer_copy, handle_explorer_delete,
        handle_explorer_disks, handle_explorer_download, handle_explorer_download_ticket,
        handle_explorer_eject, handle_explorer_extract, handle_explorer_favorites_add,
        handle_explorer_favorites_list, handle_explorer_favorites_remove, handle_explorer_format,
        handle_explorer_history, handle_explorer_list, handle_explorer_mkdir, handle_explorer_move,
        handle_explorer_network_discovery, handle_explorer_permissions, handle_explorer_preview,
        handle_explorer_recents_clear, handle_explorer_recents_list, handle_explorer_recents_touch,
        handle_explorer_rename, handle_explorer_roots, handle_explorer_save,
        handle_explorer_search, handle_explorer_snapshots, handle_explorer_thumbnail,
        handle_explorer_thumbnail_ticket, handle_explorer_trash, handle_explorer_trash_empty,
        handle_explorer_trash_list, handle_explorer_trash_purge, handle_explorer_trash_restore,
        handle_explorer_upload, handle_explorer_upload_chunk, handle_explorer_upload_complete,
        handle_explorer_upload_session_abort, handle_explorer_upload_session_start,
        handle_explorer_watch, handle_explorer_watch_ticket,
    },
    firewall::{
        handle_firewall_default_policy, handle_firewall_disable, handle_firewall_enable,
        handle_firewall_enable_preview, handle_firewall_history, handle_firewall_rule_add,
        handle_firewall_rule_delete, handle_firewall_status,
    },
    groups::{
        handle_group_add_member, handle_group_create, handle_group_delete, handle_group_get,
        handle_group_remove_member, handle_group_update, handle_groups_list,
        handle_groups_list_users, handle_user_groups,
    },
    journal::{
        handle_journal_cleanup_settings_get, handle_journal_cleanup_settings_update,
        handle_journal_clear, handle_journal_entries, handle_journal_sources,
    },
    mcp::{
        handle_mcp_create_token, handle_mcp_revoke_token, handle_mcp_rpc, handle_mcp_set_enabled,
        handle_mcp_set_permissions, handle_mcp_status,
    },
    mdns::handle_mdns_status,
    meta::handle_meta,
    metrics::{handle_metrics_history, handle_metrics_latest},
    monitor_layout::{handle_get_monitor_layout, handle_set_monitor_layout},
    notifications::{
        handle_notification_delete, handle_notification_events_config_get,
        handle_notification_events_config_update, handle_notification_mark_read,
        handle_notifications_events, handle_notifications_events_ticket, handle_notifications_list,
        handle_notifications_mark_all_read, handle_notifications_unread_count,
    },
    permissions::{
        handle_share_permission_grant, handle_share_permission_revoke,
        handle_share_permissions_list, handle_share_permissions_sync, handle_user_permissions,
    },
    power::{
        handle_power_action, handle_power_governor_set, handle_power_spindown_apply,
        handle_power_spindown_get, handle_power_spindown_set, handle_power_status,
    },
    progress::{handle_cancel_job, handle_get_job_status, handle_list_jobs, handle_progress_sse},
    ransomware::{
        handle_ransomware_events_list, handle_ransomware_protected_path_add,
        handle_ransomware_protected_path_delete, handle_ransomware_protected_paths_list,
        handle_ransomware_settings_get, handle_ransomware_settings_update,
        handle_ransomware_status,
    },
    rclone::{
        handle_add_remote, handle_copy, handle_create_sync_job, handle_delete_remote,
        handle_delete_sync_job, handle_get_config, handle_list_mounts, handle_list_providers,
        handle_list_remotes, handle_list_sync_jobs, handle_mount_start, handle_mount_stop,
        handle_rclone_status, handle_run_sync_job, handle_save_config, handle_sync,
    },
    resource_shares::{
        handle_grant_access, handle_grant_group_access, handle_list_access,
        handle_list_group_access, handle_my_access, handle_revoke_access,
        handle_revoke_group_access,
    },
    restic::{
        handle_restic_available, handle_restic_backup_create, handle_restic_backup_jobs,
        handle_restic_check, handle_restic_forget, handle_restic_prune, handle_restic_reminders,
        handle_restic_repo_create, handle_restic_repo_delete, handle_restic_repo_get,
        handle_restic_repo_update, handle_restic_repos_list, handle_restic_restore,
        handle_restic_restore_plan, handle_restic_schedule_create, handle_restic_schedule_delete,
        handle_restic_schedule_set_enabled, handle_restic_schedule_update,
        handle_restic_schedules_list, handle_restic_snapshot_delete, handle_restic_snapshots_list,
        handle_restic_stats, handle_restic_unlock, handle_restic_version,
        handle_retention_policies_list, handle_retention_policy_create,
        handle_retention_policy_delete, handle_retention_policy_update,
    },
    scheduled_tasks::{
        handle_task_create, handle_task_delete, handle_task_get, handle_task_run_now,
        handle_task_runs, handle_task_set_enabled, handle_task_update, handle_tasks_list,
    },
    security::{
        handle_get_active_sessions_count, handle_get_login_history, handle_get_my_sessions,
        handle_get_user_login_history, handle_revoke_all_sessions, handle_revoke_sessions,
    },
    settings::{
        handle_all_settings_get, handle_appearance_settings_get, handle_appearance_settings_update,
        handle_autostart_get, handle_autostart_update, handle_check_version_now,
        handle_email_settings_get, handle_email_settings_update, handle_email_test,
        handle_general_settings_get, handle_general_settings_update, handle_get_language,
        handle_get_translations, handle_notification_settings_get,
        handle_notification_settings_update, handle_public_settings_get, handle_set_language,
        handle_settings_export, handle_settings_import, handle_settings_reset,
        handle_timezones_get, handle_update_settings_get, handle_update_settings_update,
        handle_version_info,
    },
    share_links::{
        handle_share_link_create, handle_share_link_delete, handle_share_link_download,
        handle_share_links_list,
    },
    shares::{
        handle_ftp_config_get, handle_ftp_config_update, handle_nfs_config_get,
        handle_nfs_config_update, handle_sftp_config_get, handle_sftp_config_update,
        handle_sftp_grantees_list, handle_sftp_key_set, handle_sftp_rebuild, handle_share_create,
        handle_share_delete, handle_share_get, handle_share_service_restart,
        handle_share_service_start, handle_share_service_stop, handle_share_toggle,
        handle_share_update, handle_shares_list, handle_smb_config_get, handle_smb_config_update,
        handle_webdav_config_get, handle_webdav_config_update,
    },
    shortcuts::handle_fetch_metadata,
    snapshot_schedules::{
        handle_snapshot_pool_list, handle_snapshot_schedule_create,
        handle_snapshot_schedule_delete, handle_snapshot_schedule_run_now,
        handle_snapshot_schedule_update, handle_snapshot_schedules_list,
    },
    snmp::{
        handle_snmp_availability, handle_snmp_info, handle_snmp_interfaces,
        handle_snmp_settings_get, handle_snmp_settings_update, handle_snmp_storage,
    },
    spa::serve_assets_file,
    spa::serve_dist_file,
    spa::serve_fonts_file,
    spa::serve_spa,
    spa::serve_sprite_unified,
    spa::serve_static_file,
    spa::serve_wallpapers_file,
    storage::{
        handle_disk_smart_capabilities, handle_disk_smart_test_start,
        handle_disk_smart_test_status, handle_disks_list, handle_import_pool,
        handle_importable_pools_list, handle_partitions_list, handle_raid_arrays_list,
        handle_raid_available_devices, handle_raid_create, handle_raid_rebuild,
        handle_removable_disks_list, handle_virtual_disk_delete, handle_virtual_disk_grow,
        handle_virtual_disk_scrub, handle_virtual_disk_scrub_status, handle_virtual_disks_list,
    },
    syncthing::{
        handle_syncthing_config, handle_syncthing_connections, handle_syncthing_devices,
        handle_syncthing_errors, handle_syncthing_folders, handle_syncthing_pause_device,
        handle_syncthing_restart, handle_syncthing_resume_device, handle_syncthing_scan_folder,
        handle_syncthing_status,
    },
    system::{handle_server_restart, handle_server_shutdown},
    transfer_quotas::{
        handle_transfer_quota_delete, handle_transfer_quota_get, handle_transfer_quota_set,
        handle_transfer_quotas_list,
    },
    ups::{
        handle_ups_availability, handle_ups_config_delete, handle_ups_config_list,
        handle_ups_config_save, handle_ups_list, handle_ups_status, handle_ups_variables,
    },
    user_homes::{
        handle_available_home_disks, handle_user_home_delete, handle_user_home_get,
        handle_user_home_put,
    },
    users::{
        handle_admin_disable_2fa, handle_admin_revoke_user_sessions, handle_admin_user_sessions,
        handle_user_count, handle_user_create, handle_user_delete, handle_user_exists,
        handle_user_get, handle_user_set_expiry, handle_user_set_locked, handle_user_update,
        handle_user_update_full, handle_user_update_groups, handle_user_update_password,
        handle_user_update_role, handle_users_list,
    },
    weather_location::{handle_get_weather_location, handle_set_weather_location},
    whats_new::{handle_whats_new, handle_whats_new_seen},
    widgets::{
        handle_widget_cpu_temp, handle_widget_cpu_usage, handle_widget_network_traffic,
        handle_widget_ram_usage, handle_widget_update_info,
    },
    AppState, AppStateArc,
};

use crate::handlers::rescue::create_rescue_router;
use crate::health::{HealthService, HealthStatus, SystemHealth};

/// Run all health checks and return CPU + memory usage.
///
/// `pub` so `tests/health_integration.rs` can mount it on a minimal router
/// (`tower::ServiceExt::oneshot`) — behaviour is unchanged.
pub async fn handle_health_endpoint(
    State(state): State<AppStateArc>,
    headers: axum::http::HeaderMap,
) -> Json<serde_json::Value> {
    // This runs real `smartctl`/`systemctl`/`df` checks and reveals real host
    // details (disk names/SMART status, service status, memory usage, thermal
    // readings) — same sensitivity class as the Dashboard endpoints, which
    // were found and fixed to require auth for the same reason. This handler
    // had no check at all before this fix.
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if session_token.is_empty()
        || state
            .auth
            .get_username_from_token(session_token)
            .await
            .is_none()
    {
        return Json(json!({
            "success": false,
            "error": "No valid session token provided"
        }));
    }

    let pool = state.db.pool();
    let health = HealthService::run_all_checks(&pool).await;

    // Get CPU and memory usage from /proc
    let cpu_usage = get_cpu_usage();
    let memory_usage = get_memory_usage();

    match health {
        Ok(SystemHealth {
            overall_status,
            checks,
            timestamp,
            ..
        }) => {
            let status_str = match overall_status {
                HealthStatus::Healthy => "healthy",
                HealthStatus::Warning => "warning",
                HealthStatus::Unhealthy => "unhealthy",
                HealthStatus::Unknown => "unknown",
            };
            Json(json!({
                "success": true,
                "data": {
                    "status": status_str,
                    "overall_status": status_str,
                    "cpu_usage": cpu_usage,
                    "memory_usage": memory_usage,
                    "timestamp": timestamp.to_rfc3339(),
                    "checks": checks.iter().map(|c| {
                        json!({
                            "name": c.name,
                            "status": match c.status {
                                crate::health::HealthStatus::Healthy => "healthy",
                                crate::health::HealthStatus::Warning => "warning",
                                crate::health::HealthStatus::Unhealthy => "unhealthy",
                                crate::health::HealthStatus::Unknown => "unknown",
                            },
                            "message": c.message,
                            "details": c.details
                        })
                    }).collect::<Vec<_>>()
                }
            }))
        }
        Err(e) => Json(json!({
            "success": false,
            "error": e.to_string()
        })),
    }
}

/// Parse `/proc/stat` and return the overall CPU usage percentage.
fn get_cpu_usage() -> f64 {
    if let Ok(content) = std::fs::read_to_string("/proc/stat") {
        if let Some(first_line) = content.lines().next() {
            let parts: Vec<&str> = first_line.split_whitespace().collect();
            if parts.len() > 4 && parts[0].starts_with("cpu") {
                let user: u64 = parts[1].parse().unwrap_or(0);
                let nice: u64 = parts[2].parse().unwrap_or(0);
                let system: u64 = parts[3].parse().unwrap_or(0);
                let idle: u64 = parts[4].parse().unwrap_or(0);

                let total = user + nice + system + idle;
                let used = user + nice + system;

                if total > 0 {
                    return (used as f64 / total as f64) * 100.0;
                }
            }
        }
    }
    0.0
}

/// Parse `/proc/meminfo` and return the memory usage percentage.
fn get_memory_usage() -> f64 {
    if let Ok(content) = std::fs::read_to_string("/proc/meminfo") {
        let mut mem_total = 0u64;
        let mut mem_available = 0u64;

        for line in content.lines() {
            if line.starts_with("MemTotal:") {
                mem_total = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
            } else if line.starts_with("MemAvailable:") {
                mem_available = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
            }
        }

        if mem_total > 0 {
            let used = mem_total - mem_available;
            return (used as f64 / mem_total as f64) * 100.0;
        }
    }
    0.0
}

async fn server_info(ConnectInfo(addr): ConnectInfo<SocketAddr>) -> Json<serde_json::Value> {
    let settings = crate::config::Settings::load();
    let port = settings.port;

    let host = addr.ip().to_string();
    let display_ip = if host.contains(':') {
        host.clone()
    } else {
        format!("{}:{}", host, port)
    };

    Json(json!({
        "ip": host,
        "port": port,
        "display_address": display_ip
    }))
}

/// Build the axum [`Router`] with all API routes and middleware layers.
pub fn create_app(state: AppStateArc) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(tower_http::cors::Any)
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any)
        .expose_headers([HeaderName::from_static("x-session-token")]);

    let api = Router::new()
        // Auth
        .route("/api/auth/login", post(handle_login))
        .route("/api/auth/logout", post(handle_logout))
        .route("/api/auth/me", get(handle_get_current_user))
        .route("/api/auth/2fa/verify", post(handle_2fa_verify))
        .route("/api/auth/2fa/setup", post(handle_2fa_setup))
        .route(
            "/api/auth/verify-recovery-code",
            post(handle_verify_recovery_code),
        )
        .route(
            "/api/auth/reset-password-recovery",
            post(handle_reset_password_recovery),
        )
        .route(
            "/api/auth/recovery-code",
            post(handle_generate_recovery_code),
        )
        .route("/api/auth/setup-status", get(handle_setup_status))
        // The public first-run bootstrap routes (`setup`, `setup/verify-totp`,
        // `check-username`) are registered separately below, behind the
        // local-network guard. Re-baselining after a hardware change is
        // admin-authenticated, so it needs no such guard.
        .route("/api/auth/setup/rebaseline", post(handle_setup_rebaseline))
        // Avatars (offline generation via bundled dicebear crates)
        .route("/api/avatars/preview", get(handle_avatar_preview))
        .route("/api/auth/me/avatar", post(handle_update_my_avatar))
        // System & Widgets
        .route("/api/widgets/cpu", get(handle_widget_cpu_usage))
        .route("/api/widgets/ram", get(handle_widget_ram_usage))
        .route(
            "/api/widgets/network-traffic",
            get(handle_widget_network_traffic),
        )
        .route("/api/widgets/cpu-temp", get(handle_widget_cpu_temp))
        .route("/api/widgets/update-info", get(handle_widget_update_info))
        // Server control
        .route("/api/server/restart", post(handle_server_restart))
        .route("/api/server/shutdown", post(handle_server_shutdown))
        .route("/api/server/info", get(server_info))
        // Settings - aggregated & public
        .route("/api/settings", get(handle_all_settings_get))
        .route("/api/settings/public", get(handle_public_settings_get))
        // Settings - general
        .route("/api/settings/general", get(handle_general_settings_get))
        .route("/api/settings/general", put(handle_general_settings_update))
        .route("/api/settings/timezones", get(handle_timezones_get))
        // Settings - start on boot (systemd autostart for homevault.service)
        .route("/api/settings/autostart", get(handle_autostart_get))
        .route("/api/settings/autostart", put(handle_autostart_update))
        // Settings - language
        .route("/api/settings/language", get(handle_get_language))
        .route("/api/settings/language", post(handle_set_language))
        // Translations - served to the SPA
        .route("/api/translations", get(handle_get_translations))
        // Settings - email
        .route("/api/settings/email", get(handle_email_settings_get))
        .route("/api/settings/email", put(handle_email_settings_update))
        .route("/api/settings/email/test", post(handle_email_test))
        // Settings - notifications
        .route(
            "/api/settings/notifications",
            get(handle_notification_settings_get),
        )
        .route(
            "/api/settings/notifications",
            put(handle_notification_settings_update),
        )
        // Settings - appearance
        .route(
            "/api/settings/appearance",
            get(handle_appearance_settings_get),
        )
        .route(
            "/api/settings/appearance",
            put(handle_appearance_settings_update),
        )
        // Settings - update checks
        .route("/api/settings/update", get(handle_update_settings_get))
        .route("/api/settings/update", put(handle_update_settings_update))
        // Settings - version info. The GET never contacts the update endpoint
        // unless the admin's saved frequency says a check is due; the POST is
        // the explicit "Check now" action (admin only).
        .route("/api/settings/version", get(handle_version_info))
        .route(
            "/api/settings/version/check",
            axum::routing::post(handle_check_version_now),
        )
        // Settings - built-in HTTPS/TLS (admin-only; applies after a restart)
        .route(
            "/api/settings/tls",
            get(crate::handlers::tls::handle_tls_settings_get)
                .put(crate::handlers::tls::handle_tls_settings_update),
        )
        // Settings - snapshots of HomeVault's own database (admin-only). The
        // database runs in WAL mode, so the `.db` file alone is not a backup;
        // these produce and list `VACUUM INTO` snapshots.
        .route(
            "/api/settings/backup/db",
            get(crate::handlers::db_backup::handle_db_backup_status)
                .put(crate::handlers::db_backup::handle_db_backup_settings_update),
        )
        .route(
            "/api/settings/backup/db/now",
            post(crate::handlers::db_backup::handle_db_backup_now),
        )
        .route(
            "/api/settings/tls/regenerate",
            post(crate::handlers::tls::handle_tls_regenerate),
        )
        // Settings - export/import/reset
        .route("/api/settings/export", get(handle_settings_export))
        .route("/api/settings/import", post(handle_settings_import))
        .route("/api/settings/reset", post(handle_settings_reset))
        // Security - sessions and login history
        .route("/api/security/sessions", get(handle_get_my_sessions))
        .route(
            "/api/security/sessions/revoke",
            post(handle_revoke_sessions),
        )
        .route(
            "/api/security/sessions/revoke-all",
            post(handle_revoke_all_sessions),
        )
        .route("/api/security/login-history", get(handle_get_login_history))
        .route(
            "/api/security/login-history/{username}",
            get(handle_get_user_login_history),
        )
        .route(
            "/api/security/active-sessions-count",
            get(handle_get_active_sessions_count),
        )
        .route(
            "/api/monitor-layout",
            get(handle_get_monitor_layout).post(handle_set_monitor_layout),
        )
        .route("/api/mdns/status", get(handle_mdns_status))
        .route(
            "/api/weather-location",
            get(handle_get_weather_location).post(handle_set_weather_location),
        )
        // Per-member resource access (apps / folders / shares). The listing is
        // admin-scoped; `mine` is the server-side filter the frontend reads.
        .route(
            "/api/shares/access",
            get(handle_list_access)
                .post(handle_grant_access)
                .delete(handle_revoke_access),
        )
        .route("/api/shares/access/mine", get(handle_my_access))
        // Group-level grants (migration 051): "everyone in `family` may use
        // Jellyfin". Resolved as a union with the per-member rows at read
        // time from live OS group membership — nothing is copied over.
        .route(
            "/api/shares/access/groups",
            get(handle_list_group_access)
                .post(handle_grant_group_access)
                .delete(handle_revoke_group_access),
        )
        // Web shortcuts: fetch a page's title/description/favicon (SSRF-guarded).
        .route("/api/shortcuts/fetch-metadata", post(handle_fetch_metadata))
        // Antivirus (ClamAV) - status/settings/scans/threats
        .route("/api/antivirus/status", get(handle_antivirus_status))
        .route(
            "/api/antivirus/settings",
            get(handle_antivirus_settings_get).put(handle_antivirus_settings_update),
        )
        .route("/api/antivirus/scan", post(handle_antivirus_scan_start))
        .route(
            "/api/antivirus/scan/cancel",
            post(handle_antivirus_scan_cancel),
        )
        .route("/api/antivirus/scans", get(handle_antivirus_scans_list))
        .route("/api/antivirus/threats", get(handle_antivirus_threats_list))
        .route(
            "/api/antivirus/threats/{id}/resolve",
            post(handle_antivirus_threat_resolve),
        )
        // Firewall (ufw)
        .route("/api/firewall/status", get(handle_firewall_status))
        .route(
            "/api/firewall/enable/preview",
            get(handle_firewall_enable_preview),
        )
        .route("/api/firewall/enable", post(handle_firewall_enable))
        .route("/api/firewall/disable", post(handle_firewall_disable))
        .route(
            "/api/firewall/default-policy",
            post(handle_firewall_default_policy),
        )
        .route("/api/firewall/history", get(handle_firewall_history))
        .route(
            "/api/firewall/rules",
            post(handle_firewall_rule_add).delete(handle_firewall_rule_delete),
        )
        // Anti-ransomware protection (honeypots + mass file-op rate detection)
        .route("/api/ransomware/status", get(handle_ransomware_status))
        .route(
            "/api/ransomware/settings",
            get(handle_ransomware_settings_get).put(handle_ransomware_settings_update),
        )
        .route(
            "/api/ransomware/protected-paths",
            get(handle_ransomware_protected_paths_list).post(handle_ransomware_protected_path_add),
        )
        .route(
            "/api/ransomware/protected-paths/{id}",
            axum::routing::delete(handle_ransomware_protected_path_delete),
        )
        .route("/api/ransomware/events", get(handle_ransomware_events_list))
        // Dashboard
        .route("/api/dashboard/data", get(handle_dashboard_data))
        .route("/api/dashboard/widget/{id}", get(handle_widget_data))
        .route("/api/dashboard/widget", post(handle_add_widget))
        .route(
            "/api/dashboard/widget/{id}",
            axum::routing::delete(handle_remove_widget),
        )
        .route(
            "/api/dashboard/widgets/reorder",
            post(handle_reorder_widgets),
        )
        .route("/api/dashboard/widgets", get(handle_get_all_widgets))
        .route(
            "/api/dashboard/network-traffic",
            get(handle_network_traffic_data),
        )
        .route(
            "/api/dashboard/cpu-temperature",
            get(handle_cpu_temperature),
        )
        .route(
            "/api/dashboard/storage-volumes",
            get(handle_storage_volumes),
        )
        .route(
            "/api/dashboard/widget-preferences",
            get(handle_get_widget_preferences).post(handle_update_widget_preference),
        )
        .route("/api/dashboard/disk-io", get(handle_disk_io_data))
        .route("/api/dashboard/storage", get(handle_storage_data))
        .route("/api/dashboard/health", get(handle_health_data))
        .route("/api/dashboard/server-details", get(handle_server_details))
        .route("/api/health", get(handle_health_endpoint))
        // Public liveness/readiness probes for external monitoring. Deliberately
        // unauthenticated and deliberately terse: no hostname, no disk names,
        // no version, no machine id. `/api/health` above stays authenticated
        // and detailed — it answers 200 even when the DB is unusable, which is
        // exactly why a monitor cannot be built on it.
        .route(
            "/api/health/live",
            get(crate::handlers::health_probe::handle_health_live),
        )
        .route(
            "/api/health/ready",
            get(crate::handlers::health_probe::handle_health_ready),
        )
        // Docker
        .route("/api/docker/info", get(handle_docker_info))
        .route("/api/docker/containers", get(handle_docker_containers_list))
        .route(
            "/api/docker/containers/running",
            get(handle_docker_containers_running),
        )
        .route("/api/docker/images", get(handle_docker_images_list))
        .route(
            "/api/docker/containers/{name}/start",
            axum::routing::put(handle_docker_container_start),
        )
        .route(
            "/api/docker/containers/{name}/stop",
            axum::routing::put(handle_docker_container_stop),
        )
        .route(
            "/api/docker/containers/{name}/restart",
            axum::routing::put(handle_docker_container_restart),
        )
        .route(
            "/api/docker/containers/{name}/logs",
            get(handle_docker_container_logs),
        )
        .route(
            "/api/docker/containers",
            post(handle_docker_container_create),
        )
        .route(
            "/api/docker/containers/{name}",
            delete(handle_docker_container_remove),
        )
        .route("/api/docker/images/pull", post(handle_docker_image_pull))
        .route(
            "/api/docker/images/{image_id}",
            delete(handle_docker_image_remove),
        )
        .route("/api/docker/available", get(handle_docker_available))
        .route("/api/docker/daemon/start", post(handle_docker_daemon_start))
        .route("/api/docker/daemon/logs", get(handle_docker_daemon_logs))
        .route(
            "/api/docker/daemon/config",
            get(handle_docker_daemon_config_get).put(handle_docker_daemon_config_put),
        )
        .route("/api/docker/port-check", get(handle_docker_port_check))
        .route("/api/docker/hub/search", get(handle_docker_hub_search))
        // Container detail, extra lifecycle actions, exec, live logs
        .route(
            "/api/docker/containers/{name}/inspect",
            get(handle_docker_container_inspect),
        )
        .route(
            "/api/docker/containers/{name}/pause",
            axum::routing::put(handle_docker_container_pause),
        )
        .route(
            "/api/docker/containers/{name}/unpause",
            axum::routing::put(handle_docker_container_unpause),
        )
        .route(
            "/api/docker/containers/{name}/kill",
            post(handle_docker_container_kill),
        )
        .route(
            "/api/docker/containers/{name}/rename",
            axum::routing::put(handle_docker_container_rename),
        )
        .route(
            "/api/docker/containers/{name}/update",
            post(handle_docker_container_update),
        )
        .route(
            "/api/docker/containers/{name}/recreate",
            post(handle_docker_container_recreate),
        )
        .route(
            "/api/docker/containers/{name}/exec",
            post(handle_docker_container_exec),
        )
        .route(
            "/api/docker/containers/{name}/logs/ticket",
            post(handle_docker_container_logs_ticket),
        )
        .route(
            "/api/docker/containers/{name}/logs/stream",
            get(handle_docker_container_logs_stream),
        )
        // Volumes, disk usage, prune, builds, registries
        .route(
            "/api/docker/volumes",
            get(handle_docker_volumes_list).post(handle_docker_volume_create),
        )
        .route(
            "/api/docker/volumes/{name}",
            delete(handle_docker_volume_remove),
        )
        .route("/api/docker/system/df", get(handle_docker_system_df))
        .route("/api/docker/prune", post(handle_docker_prune))
        .route("/api/docker/images/build", post(handle_docker_image_build))
        .route("/api/docker/registries", get(handle_docker_registry_list))
        .route(
            "/api/docker/registries/login",
            post(handle_docker_registry_login),
        )
        .route(
            "/api/docker/registries/logout",
            post(handle_docker_registry_logout),
        )
        // Docker Networks
        .route("/api/docker/networks", get(handle_docker_networks_list))
        .route(
            "/api/docker/networks/prune",
            post(handle_docker_network_prune),
        )
        .route(
            "/api/docker/networks/{name}",
            get(handle_docker_network_get),
        )
        .route(
            "/api/docker/networks/{name}",
            delete(handle_docker_network_remove),
        )
        .route("/api/docker/networks", post(handle_docker_network_create))
        .route(
            "/api/docker/networks/connect",
            post(handle_docker_network_connect),
        )
        .route(
            "/api/docker/networks/disconnect",
            post(handle_docker_network_disconnect),
        )
        // Docker Compose
        .route("/api/docker/compose/projects", get(handle_compose_list))
        .route("/api/docker/compose/projects", post(handle_compose_create))
        .route(
            "/api/docker/compose/projects/{name}",
            get(handle_compose_get),
        )
        .route(
            "/api/docker/compose/projects/{name}/config",
            get(handle_compose_get_config),
        )
        .route(
            "/api/docker/compose/projects/{name}",
            put(handle_compose_update),
        )
        .route(
            "/api/docker/compose/projects/{name}",
            delete(handle_compose_delete),
        )
        .route(
            "/api/docker/compose/projects/{name}/start",
            post(handle_compose_start),
        )
        .route(
            "/api/docker/compose/projects/{name}/stop",
            post(handle_compose_stop),
        )
        .route(
            "/api/docker/compose/projects/{name}/restart",
            post(handle_compose_restart),
        )
        .route(
            "/api/docker/compose/projects/{name}/services",
            get(handle_compose_services),
        )
        .route(
            "/api/docker/compose/projects/{name}/update",
            post(handle_compose_pull),
        )
        .route(
            "/api/docker/compose/available",
            get(handle_compose_available),
        )
        .route(
            "/api/docker/compose/validate",
            post(handle_compose_validate),
        )
        // Docker app catalog — manifest-driven one-click Compose templates
        .route("/api/docker/catalog", get(handle_catalog_list))
        .route("/api/docker/catalog/updates", get(handle_catalog_updates))
        .route(
            "/api/docker/catalog/update-all",
            post(handle_catalog_update_all),
        )
        .route(
            "/api/docker/catalog/{id}/update",
            post(handle_catalog_update),
        )
        .route(
            "/api/docker/catalog/{id}/proxy",
            post(handle_catalog_proxy_config),
        )
        .route("/api/docker/catalog/{id}/icon", get(handle_catalog_icon))
        .route(
            "/api/docker/catalog/{id}/gallery/{n}",
            get(handle_catalog_gallery),
        )
        .route(
            "/api/docker/catalog/{id}/install",
            post(handle_catalog_install),
        )
        .route(
            "/api/docker/catalog/{id}/install-review",
            get(handle_catalog_install_review),
        )
        .route("/api/docker/catalog/{id}/logs", get(handle_catalog_logs))
        .route("/api/docker/catalog/{id}", delete(handle_catalog_uninstall))
        // Community app stores (extra git-repo catalog sources)
        .route("/api/docker/stores", get(handle_stores_list))
        .route("/api/docker/stores", post(handle_store_add))
        .route("/api/docker/stores/{id}/sync", post(handle_store_sync))
        .route("/api/docker/stores/{id}", delete(handle_store_delete))
        // Downloader (aria2) — admin-gated handlers
        .route("/api/downloader/status", get(handle_downloader_status))
        .route("/api/downloader/start", post(handle_downloader_start))
        .route("/api/downloader/stop", post(handle_downloader_stop))
        .route(
            "/api/downloader/global-stat",
            get(handle_downloader_global_stat),
        )
        .route("/api/downloader/tasks", get(handle_downloader_tasks))
        .route("/api/downloader/add", post(handle_downloader_add))
        .route("/api/downloader/pause", post(handle_downloader_pause))
        .route("/api/downloader/unpause", post(handle_downloader_unpause))
        .route("/api/downloader/remove", post(handle_downloader_remove))
        .route("/api/downloader/task/{gid}", get(handle_downloader_task))
        // Storage
        .route("/api/storage/disks", get(handle_disks_list))
        .route(
            "/api/storage/disks/{name}/smart/capabilities",
            get(handle_disk_smart_capabilities),
        )
        .route(
            "/api/storage/disks/{name}/smart/test",
            get(handle_disk_smart_test_status),
        )
        .route(
            "/api/storage/disks/{name}/smart/test",
            post(handle_disk_smart_test_start),
        )
        .route("/api/storage/removable", get(handle_removable_disks_list))
        .route("/api/storage/partitions", get(handle_partitions_list))
        .route("/api/storage/raid", get(handle_raid_arrays_list))
        .route("/api/storage/raid", post(handle_raid_create))
        .route(
            "/api/storage/raid/devices",
            get(handle_raid_available_devices),
        )
        .route(
            "/api/storage/raid/{name}/rebuild",
            post(handle_raid_rebuild),
        )
        // Unified virtual-disk management (STORAGE): mdadm + ZFS + Btrfs
        .route("/api/storage/virtual-disks", get(handle_virtual_disks_list))
        .route(
            "/api/storage/virtual-disks/{name}",
            delete(handle_virtual_disk_delete),
        )
        .route(
            "/api/storage/virtual-disks/{name}/scrub",
            post(handle_virtual_disk_scrub),
        )
        .route(
            "/api/storage/virtual-disks/{name}/scrub",
            get(handle_virtual_disk_scrub_status),
        )
        // Grow an mdadm array onto a new disk (mdadm --add + --grow).
        .route(
            "/api/storage/virtual-disks/{name}/grow",
            post(handle_virtual_disk_grow),
        )
        // Foreign (importable) pools discovered on attached disks
        .route(
            "/api/storage/importable-pools",
            get(handle_importable_pools_list),
        )
        .route("/api/storage/import-pool", post(handle_import_pool))
        // Cache tier (HomeVault write caching)
        .route("/api/cache/settings", get(handle_cache_settings_get))
        .route("/api/cache/settings", put(handle_cache_settings_update))
        .route("/api/cache/pools", get(handle_cache_pools_get))
        .route("/api/cache/shares", get(handle_cache_shares_list))
        .route("/api/cache/shares", post(handle_cache_share_create))
        .route("/api/cache/shares/{name}", put(handle_cache_share_update))
        .route(
            "/api/cache/shares/{name}",
            axum::routing::delete(handle_cache_share_delete),
        )
        .route("/api/cache/mover/run", post(handle_cache_mover_run))
        .route("/api/cache/mover/runs", get(handle_cache_mover_runs))
        // Disk encryption (LUKS2, manual unlock only)
        .route("/api/encryption/volumes", get(handle_encryption_list))
        .route("/api/encryption/volumes", post(handle_encryption_create))
        .route(
            "/api/encryption/available-devices",
            get(handle_encryption_available_devices),
        )
        .route(
            "/api/encryption/volumes/{id}/unlock",
            post(handle_encryption_unlock),
        )
        .route(
            "/api/encryption/volumes/{id}/lock",
            post(handle_encryption_lock),
        )
        .route(
            "/api/encryption/volumes/{id}/passphrase",
            post(handle_encryption_change_passphrase),
        )
        .route(
            "/api/encryption/volumes/{id}",
            delete(handle_encryption_delete),
        )
        // Notifications
        .route("/api/notifications", get(handle_notifications_list))
        .route(
            "/api/notifications/unread-count",
            get(handle_notifications_unread_count),
        )
        .route(
            "/api/notifications/read-all",
            post(handle_notifications_mark_all_read),
        )
        .route(
            "/api/notifications/{id}/read",
            post(handle_notification_mark_read),
        )
        .route(
            "/api/notifications/{id}",
            delete(handle_notification_delete),
        )
        .route(
            "/api/notifications/events/ticket",
            post(handle_notifications_events_ticket),
        )
        .route("/api/events", get(handle_notifications_events))
        // Per-event notification toggles + webhook sink format (admin).
        .route(
            "/api/notifications/events/config",
            get(handle_notification_events_config_get),
        )
        .route(
            "/api/notifications/events/config",
            axum::routing::put(handle_notification_events_config_update),
        )
        // Scheduled tasks (generic cron engine: commands, snapshots, SMART
        // self-tests, host power actions) — admin-only, see
        // docs/guides/scheduled-tasks.md.
        .route("/api/scheduled-tasks", get(handle_tasks_list))
        .route("/api/scheduled-tasks", post(handle_task_create))
        .route(
            "/api/scheduled-tasks/{id}",
            get(handle_task_get)
                .put(handle_task_update)
                .delete(handle_task_delete),
        )
        // "Run now" answers 202 + a run id; the execution continues on its
        // own (ROADMAP 12.4), and the task row / `{id}/runs` report the
        // outcome.
        .route("/api/scheduled-tasks/{id}/run", post(handle_task_run_now))
        .route(
            "/api/scheduled-tasks/{id}/enabled",
            post(handle_task_set_enabled),
        )
        .route("/api/scheduled-tasks/{id}/runs", get(handle_task_runs))
        // Snapshot schedules: cron-driven pool snapshots with GFS retention
        // (admin-only; see docs/guides/snapshots.md).
        .route(
            "/api/snapshot-schedules",
            get(handle_snapshot_schedules_list),
        )
        .route(
            "/api/snapshot-schedules",
            post(handle_snapshot_schedule_create),
        )
        .route(
            "/api/snapshot-schedules/snapshots",
            get(handle_snapshot_pool_list),
        )
        .route(
            "/api/snapshot-schedules/{id}",
            axum::routing::put(handle_snapshot_schedule_update),
        )
        .route(
            "/api/snapshot-schedules/{id}",
            axum::routing::delete(handle_snapshot_schedule_delete),
        )
        .route(
            "/api/snapshot-schedules/{id}/run",
            post(handle_snapshot_schedule_run_now),
        )
        // Historical metrics (see docs/guides/metrics.md) — read-only for
        // authed users; samples are written only by the collector loop.
        .route("/api/metrics/history", get(handle_metrics_history))
        .route("/api/metrics/latest", get(handle_metrics_latest))
        // Host power management + disk spindown (admin-only; see
        // docs/guides/power.md).
        .route("/api/power/status", get(handle_power_status))
        .route(
            "/api/power/governor",
            axum::routing::put(handle_power_governor_set),
        )
        .route("/api/power/{action}", post(handle_power_action))
        .route("/api/power/spindown", get(handle_power_spindown_get))
        .route(
            "/api/power/spindown",
            axum::routing::put(handle_power_spindown_set),
        )
        .route(
            "/api/power/spindown/apply",
            post(handle_power_spindown_apply),
        )
        // App-proxy auth entry (browser navigation, cookie-authenticated):
        // mints the app-scoped session and redirects to the app origin.
        .route("/app-auth", get(handle_app_auth))
        // Versioned client/server contract handshake (unauthenticated).
        .route("/api/meta", get(handle_meta))
        // What's New (release highlights, per-user acknowledgement)
        .route("/api/whats-new", get(handle_whats_new))
        .route("/api/whats-new/seen", post(handle_whats_new_seen))
        // Explorer
        .route("/api/explorer/list", get(handle_explorer_list))
        .route("/api/explorer/search", get(handle_explorer_search))
        .route("/api/explorer/disks", get(handle_explorer_disks))
        .route("/api/explorer/roots", get(handle_explorer_roots))
        .route("/api/explorer/save", post(handle_explorer_save))
        .route("/api/explorer/eject", post(handle_explorer_eject))
        .route("/api/explorer/snapshots", get(handle_explorer_snapshots))
        .route("/api/explorer/mkdir", post(handle_explorer_mkdir))
        .route("/api/explorer/delete", post(handle_explorer_delete))
        .route("/api/explorer/rename", post(handle_explorer_rename))
        .route("/api/explorer/copy", post(handle_explorer_copy))
        .route("/api/explorer/move", post(handle_explorer_move))
        .route(
            "/api/explorer/permissions",
            get(handle_explorer_permissions),
        )
        .route(
            "/api/explorer/permissions",
            post(handle_explorer_permissions),
        )
        .route("/api/explorer/preview", get(handle_explorer_preview))
        .route("/api/explorer/extract", post(handle_explorer_extract))
        .route("/api/explorer/download", get(handle_explorer_download))
        .route(
            "/api/explorer/download-ticket",
            post(handle_explorer_download_ticket),
        )
        .route("/api/explorer/thumbnail", get(handle_explorer_thumbnail))
        .route(
            "/api/explorer/thumbnail-ticket",
            post(handle_explorer_thumbnail_ticket),
        )
        .route("/api/explorer/upload", post(handle_explorer_upload))
        .route(
            "/api/explorer/upload/session",
            post(handle_explorer_upload_session_start),
        )
        .route(
            "/api/explorer/upload/session",
            delete(handle_explorer_upload_session_abort),
        )
        .route(
            "/api/explorer/upload/chunk",
            post(handle_explorer_upload_chunk),
        )
        .route(
            "/api/explorer/upload/complete",
            post(handle_explorer_upload_complete),
        )
        .route("/api/explorer/compress", post(handle_explorer_compress))
        .route("/api/explorer/trash", post(handle_explorer_trash))
        .route("/api/explorer/trash", get(handle_explorer_trash_list))
        .route(
            "/api/explorer/trash",
            axum::routing::delete(handle_explorer_trash_empty),
        )
        .route(
            "/api/explorer/trash/restore",
            post(handle_explorer_trash_restore),
        )
        .route(
            "/api/explorer/trash/{id}",
            axum::routing::delete(handle_explorer_trash_purge),
        )
        .route(
            "/api/explorer/favorites",
            get(handle_explorer_favorites_list),
        )
        .route(
            "/api/explorer/favorites",
            post(handle_explorer_favorites_add),
        )
        .route(
            "/api/explorer/favorites",
            delete(handle_explorer_favorites_remove),
        )
        .route("/api/explorer/shares", get(handle_share_links_list))
        .route("/api/explorer/shares", post(handle_share_link_create))
        .route(
            "/api/explorer/shares/{token}",
            axum::routing::delete(handle_share_link_delete),
        )
        .route("/api/explorer/watch", get(handle_explorer_watch))
        .route(
            "/api/explorer/watch/ticket",
            post(handle_explorer_watch_ticket),
        )
        .route(
            "/api/explorer/recents",
            get(handle_explorer_recents_list)
                .post(handle_explorer_recents_touch)
                .delete(handle_explorer_recents_clear),
        )
        .route("/api/explorer/history", get(handle_explorer_history))
        .route(
            "/api/explorer/network-discovery",
            get(handle_explorer_network_discovery),
        )
        .route("/api/explorer/format", post(handle_explorer_format))
        // Shares
        .route("/api/shares", get(handle_shares_list))
        .route("/api/shares", post(handle_share_create))
        .route("/api/shares/{name}/{share_type}", get(handle_share_get))
        .route("/api/shares/{name}/{share_type}", put(handle_share_update))
        .route(
            "/api/shares/{name}/{share_type}/toggle/{enabled}",
            put(handle_share_toggle),
        )
        .route(
            "/api/shares/{name}/{share_type}",
            axum::routing::delete(handle_share_delete),
        )
        .route("/api/shares/ftp/config", get(handle_ftp_config_get))
        .route("/api/shares/ftp/config", put(handle_ftp_config_update))
        .route("/api/shares/webdav/config", get(handle_webdav_config_get))
        .route(
            "/api/shares/webdav/config",
            put(handle_webdav_config_update),
        )
        .route("/api/shares/smb/config", get(handle_smb_config_get))
        .route("/api/shares/smb/config", put(handle_smb_config_update))
        .route("/api/shares/nfs/config", get(handle_nfs_config_get))
        .route("/api/shares/nfs/config", put(handle_nfs_config_update))
        .route("/api/shares/sftp/config", get(handle_sftp_config_get))
        .route("/api/shares/sftp/config", put(handle_sftp_config_update))
        .route("/api/shares/sftp/users", get(handle_sftp_grantees_list))
        .route("/api/shares/sftp/users/key", put(handle_sftp_key_set))
        .route("/api/shares/sftp/rebuild", post(handle_sftp_rebuild))
        .route(
            "/api/shares/{share_type}/start",
            post(handle_share_service_start),
        )
        .route(
            "/api/shares/{share_type}/stop",
            post(handle_share_service_stop),
        )
        .route(
            "/api/shares/{share_type}/restart",
            post(handle_share_service_restart),
        )
        // UPS monitoring (NUT)
        .route("/api/ups/availability", get(handle_ups_availability))
        .route("/api/ups/list", get(handle_ups_list))
        .route("/api/ups/status/{name}", get(handle_ups_status))
        .route("/api/ups/variables/{name}", get(handle_ups_variables))
        .route(
            "/api/ups/config",
            get(handle_ups_config_list).post(handle_ups_config_save),
        )
        .route(
            "/api/ups/config/{name}",
            axum::routing::delete(handle_ups_config_delete),
        )
        // SNMP monitoring
        .route("/api/snmp/availability", get(handle_snmp_availability))
        .route("/api/snmp/info", get(handle_snmp_info))
        .route("/api/snmp/interfaces", get(handle_snmp_interfaces))
        .route("/api/snmp/storage", get(handle_snmp_storage))
        .route(
            "/api/snmp/settings",
            get(handle_snmp_settings_get).put(handle_snmp_settings_update),
        )
        // Share Permissions
        .route(
            "/api/shares/{name}/{share_type}/permissions",
            get(handle_share_permissions_list),
        )
        .route(
            "/api/shares/{name}/{share_type}/permissions",
            post(handle_share_permission_grant),
        )
        .route(
            "/api/shares/{name}/{share_type}/permissions",
            axum::routing::delete(handle_share_permission_revoke),
        )
        .route(
            "/api/shares/{name}/{share_type}/permissions/sync",
            post(handle_share_permissions_sync),
        )
        .route(
            "/api/users/{username}/permissions",
            get(handle_user_permissions),
        )
        // Users
        .route("/api/users", get(handle_users_list))
        .route("/api/users", post(handle_user_create))
        .route("/api/users/{username}", get(handle_user_get))
        .route(
            "/api/users/{username}",
            axum::routing::put(handle_user_update),
        )
        .route("/api/users/{username}/avatar.svg", get(handle_user_avatar))
        .route(
            "/api/users/{username}/full",
            axum::routing::put(handle_user_update_full),
        )
        .route(
            "/api/users/{username}",
            axum::routing::delete(handle_user_delete),
        )
        .route(
            "/api/users/{username}/password",
            axum::routing::put(handle_user_update_password),
        )
        .route(
            // Admin per-user session management: audit (masked tokens) +
            // force logout everywhere for a user whose device is lost or
            // whose session is suspected stolen.
            "/api/users/{username}/sessions",
            axum::routing::get(handle_admin_user_sessions)
                .delete(handle_admin_revoke_user_sessions),
        )
        .route(
            "/api/users/{username}/role",
            axum::routing::put(handle_user_update_role),
        )
        .route(
            "/api/users/{username}/locked",
            axum::routing::put(handle_user_set_locked),
        )
        .route(
            // Account expiry (migration 050): profile saves must never move
            // this horizon, only this dedicated admin endpoint does.
            "/api/users/{username}/expiry",
            axum::routing::put(handle_user_set_expiry),
        )
        .route(
            "/api/users/{username}/2fa/disable",
            axum::routing::post(handle_admin_disable_2fa),
        )
        .route(
            "/api/users/{username}/groups",
            axum::routing::put(handle_user_update_groups),
        )
        .route(
            "/api/users/{username}/home",
            axum::routing::get(handle_user_home_get),
        )
        .route(
            "/api/users/{username}/home",
            axum::routing::put(handle_user_home_put),
        )
        .route(
            "/api/users/{username}/home",
            axum::routing::delete(handle_user_home_delete),
        )
        .route(
            "/api/storage/available-home-disks",
            axum::routing::get(handle_available_home_disks),
        )
        .route("/api/users/count", axum::routing::get(handle_user_count))
        .route(
            "/api/users/{username}/exists",
            axum::routing::get(handle_user_exists),
        )
        // Groups
        .route("/api/groups/users", get(handle_groups_list_users))
        .route("/api/groups", get(handle_groups_list))
        .route("/api/groups", post(handle_group_create))
        .route(
            "/api/groups/{name}",
            // Rename + description (2026-10-20): a rename migrates the
            // display-name references (mirror, group grants, quotas, ACLs).
            get(handle_group_get).put(handle_group_update),
        )
        .route(
            "/api/groups/{name}",
            axum::routing::delete(handle_group_delete),
        )
        .route(
            "/api/groups/{name}/members",
            axum::routing::post(handle_group_add_member),
        )
        .route(
            "/api/groups/{name}/members/{username}",
            axum::routing::delete(handle_group_remove_member),
        )
        .route(
            "/api/users/{username}/groups",
            axum::routing::get(handle_user_groups),
        )
        // Progress SSE
        .route("/api/progress/{job_id}", get(handle_progress_sse))
        .route("/api/progress/jobs", get(handle_list_jobs))
        .route("/api/progress/{job_id}/status", get(handle_get_job_status))
        .route(
            "/api/progress/{job_id}",
            axum::routing::delete(handle_cancel_job),
        )
        // Unified background-activity feed (desktop taskbar)
        .route(
            "/api/activity",
            get(crate::handlers::activity::handle_activity),
        )
        // Rclone
        .route("/api/rclone/status", get(handle_rclone_status))
        .route("/api/rclone/remotes", get(handle_list_remotes))
        .route("/api/rclone/remotes", post(handle_add_remote))
        .route(
            "/api/rclone/remotes/{name}",
            axum::routing::delete(handle_delete_remote),
        )
        .route("/api/rclone/providers", get(handle_list_providers))
        .route("/api/rclone/mounts", get(handle_list_mounts))
        .route("/api/rclone/mount/start", post(handle_mount_start))
        .route("/api/rclone/mount/stop", post(handle_mount_stop))
        .route("/api/rclone/sync", post(handle_sync))
        .route("/api/rclone/copy", post(handle_copy))
        .route("/api/rclone/config", get(handle_get_config))
        .route("/api/rclone/config", post(handle_save_config))
        // Sync jobs — persisted server-side (formerly localStorage in the UI)
        .route("/api/rclone/jobs", get(handle_list_sync_jobs))
        .route("/api/rclone/jobs", post(handle_create_sync_job))
        .route(
            "/api/rclone/jobs/{id}",
            axum::routing::delete(handle_delete_sync_job),
        )
        .route("/api/rclone/jobs/{id}/run", post(handle_run_sync_job))
        // Syncthing
        .route("/api/syncthing/status", get(handle_syncthing_status))
        .route("/api/syncthing/config", get(handle_syncthing_config))
        .route("/api/syncthing/devices", get(handle_syncthing_devices))
        .route("/api/syncthing/folders", get(handle_syncthing_folders))
        .route(
            "/api/syncthing/connections",
            get(handle_syncthing_connections),
        )
        .route("/api/syncthing/restart", post(handle_syncthing_restart))
        .route("/api/syncthing/pause", post(handle_syncthing_pause_device))
        .route(
            "/api/syncthing/resume",
            post(handle_syncthing_resume_device),
        )
        .route("/api/syncthing/scan", post(handle_syncthing_scan_folder))
        .route("/api/syncthing/errors", get(handle_syncthing_errors))
        // Restic
        .route("/api/restic/available", get(handle_restic_available))
        .route("/api/restic/version", get(handle_restic_version))
        .route("/api/restic/repos", get(handle_restic_repos_list))
        .route("/api/restic/repos", post(handle_restic_repo_create))
        .route("/api/restic/repos/{id}", get(handle_restic_repo_get))
        .route("/api/restic/repos/{id}", put(handle_restic_repo_update))
        .route("/api/restic/repos/{id}", delete(handle_restic_repo_delete))
        .route(
            "/api/restic/repos/{id}/snapshots",
            get(handle_restic_snapshots_list),
        )
        .route("/api/restic/repos/{id}/stats", get(handle_restic_stats))
        .route(
            "/api/restic/repos/{id}/jobs",
            get(handle_restic_backup_jobs),
        )
        .route("/api/restic/repos/{id}/unlock", post(handle_restic_unlock))
        .route("/api/restic/repos/{id}/prune", post(handle_restic_prune))
        .route("/api/restic/backup", post(handle_restic_backup_create))
        .route("/api/restic/restore", post(handle_restic_restore))
        .route("/api/restic/restore-plan", get(handle_restic_restore_plan))
        .route("/api/restic/reminders", get(handle_restic_reminders))
        .route("/api/restic/check", post(handle_restic_check))
        .route("/api/restic/forget", post(handle_restic_forget))
        .route(
            "/api/restic/snapshots/delete",
            post(handle_restic_snapshot_delete),
        )
        .route("/api/restic/schedules", get(handle_restic_schedules_list))
        .route("/api/restic/schedules", post(handle_restic_schedule_create))
        .route(
            "/api/restic/schedules/{id}",
            put(handle_restic_schedule_update),
        )
        .route(
            "/api/restic/schedules/{id}",
            delete(handle_restic_schedule_delete),
        )
        .route(
            "/api/restic/schedules/{id}/enabled",
            put(handle_restic_schedule_set_enabled),
        )
        .route(
            "/api/restic/retention-policies",
            get(handle_retention_policies_list),
        )
        .route(
            "/api/restic/retention-policies",
            post(handle_retention_policy_create),
        )
        .route(
            "/api/restic/retention-policies/{id}",
            put(handle_retention_policy_update),
        )
        .route(
            "/api/restic/retention-policies/{id}",
            delete(handle_retention_policy_delete),
        )
        // Chat AI Assistant
        .route("/api/chat/send", post(handle_chat_send))
        .route("/api/chat/send/stream", post(handle_chat_send_stream))
        .route("/api/chat/models", get(handle_chat_models_list))
        .route("/api/chat/models/pull", post(handle_chat_models_pull))
        .route(
            "/api/chat/models/recommend",
            get(handle_chat_models_recommend),
        )
        .route("/api/chat/model", get(handle_chat_model_get))
        .route("/api/chat/model", put(handle_chat_model_set))
        // RAG Knowledge Base
        .route("/api/rag/status", get(handle_rag_status))
        .route("/api/rag/index", post(handle_rag_index))
        .route("/api/rag/search", post(handle_rag_search))
        .route("/api/rag/documents", get(handle_rag_documents))
        // External AI agents (MCP, Streamable HTTP) — management is
        // admin-session; POST /mcp is Bearer-token (see handlers::mcp).
        .route("/api/mcp/status", get(handle_mcp_status))
        .route("/api/mcp/status", put(handle_mcp_set_enabled))
        .route("/api/mcp/permissions", put(handle_mcp_set_permissions))
        .route("/api/mcp/tokens", post(handle_mcp_create_token))
        .route("/api/mcp/tokens/{id}", delete(handle_mcp_revoke_token))
        .route("/mcp", post(handle_mcp_rpc))
        // System Journal
        .route("/api/journal/sources", get(handle_journal_sources))
        .route("/api/journal/entries", get(handle_journal_entries))
        .route("/api/journal/clear", delete(handle_journal_clear))
        .route(
            "/api/journal/cleanup-settings",
            get(handle_journal_cleanup_settings_get),
        )
        .route(
            "/api/journal/cleanup-settings",
            put(handle_journal_cleanup_settings_update),
        )
        // Upload/download transfer quotas (Users/Groups pages)
        .route(
            "/api/transfer-quotas/{subject_type}",
            get(handle_transfer_quotas_list),
        )
        .route(
            "/api/transfer-quotas/{subject_type}/{subject_name}",
            get(handle_transfer_quota_get),
        )
        .route(
            "/api/transfer-quotas/{subject_type}/{subject_name}",
            put(handle_transfer_quota_set),
        )
        .route(
            "/api/transfer-quotas/{subject_type}/{subject_name}",
            delete(handle_transfer_quota_delete),
        )
        // Virtual Machines (Incus) — every route is admin-only (see src/handlers/vm.rs)
        .route(
            "/api/vm/available",
            get(crate::handlers::vm::handle_vm_available),
        )
        .route(
            "/api/vm/os-options",
            get(crate::handlers::vm::handle_vm_os_options),
        )
        .route("/api/vm/list", get(crate::handlers::vm::handle_vm_list))
        .route(
            "/api/vm/create",
            post(crate::handlers::vm::handle_vm_create),
        )
        .route(
            "/api/vm/console/ticket",
            post(crate::handlers::vm::handle_console_ticket),
        )
        .route("/api/vm/isos", get(crate::handlers::vm::handle_vm_isos))
        .route(
            "/api/vm/isos/download",
            post(crate::handlers::vm::handle_vm_iso_download),
        )
        .route(
            "/api/vm/isos/virtio",
            post(crate::handlers::vm::handle_vm_iso_virtio),
        )
        .route(
            "/api/vm/isos/{name}",
            delete(crate::handlers::vm::handle_vm_iso_delete),
        )
        .route(
            "/api/vm/{name}/screen/ticket",
            post(crate::handlers::vm::handle_screen_ticket),
        )
        .route("/api/vm/{name}", get(crate::handlers::vm::handle_vm_get))
        .route(
            "/api/vm/{name}",
            delete(crate::handlers::vm::handle_vm_delete),
        )
        .route(
            "/api/vm/{name}/start",
            put(crate::handlers::vm::handle_vm_start),
        )
        .route(
            "/api/vm/{name}/stop",
            put(crate::handlers::vm::handle_vm_stop),
        )
        .route(
            "/api/vm/{name}/restart",
            put(crate::handlers::vm::handle_vm_restart),
        )
        .route(
            "/api/vm/{name}/config",
            put(crate::handlers::vm::handle_vm_config),
        )
        .route(
            "/api/vm/{name}/snapshots",
            get(crate::handlers::vm::handle_vm_snapshots),
        )
        .route(
            "/api/vm/{name}/snapshots",
            post(crate::handlers::vm::handle_vm_snapshot_create),
        )
        .route(
            "/api/vm/{name}/snapshots/{snap}/restore",
            post(crate::handlers::vm::handle_vm_snapshot_restore),
        )
        .route(
            "/api/vm/{name}/snapshots/{snap}",
            delete(crate::handlers::vm::handle_vm_snapshot_delete),
        )
        .with_state(state.clone());

    // First-run bootstrap endpoints. They are public by necessity (no admin
    // exists yet), so `setup_guard_middleware` restricts them to loopback /
    // private networks — otherwise the first client to reach a fresh appliance
    // on a shared network could claim its admin account.
    let setup_bootstrap = Router::new()
        .route("/api/auth/setup", post(handle_setup))
        .route(
            "/api/auth/setup/verify-totp",
            post(handle_setup_verify_totp),
        )
        .route("/api/auth/check-username", get(handle_check_username))
        .route(
            "/api/auth/setup/progress",
            get(handle_setup_progress_get).put(handle_setup_progress_put),
        )
        .route_layer(axum::middleware::from_fn(
            crate::middleware::setup_guard_middleware,
        ))
        .with_state(state.clone());
    let api = api.merge(setup_bootstrap);

    // Network sub-router: interface listing, WiFi, diagnostics, etc.
    let network_router = crate::handlers::network::create_network_router(state.clone());
    let api = api.nest("/api/network", network_router);

    // VPN sub-router: WireGuard, OpenVPN management
    let vpn_router = crate::handlers::vpn::create_vpn_router(state.clone());
    let api = api.nest("/api/vpn", vpn_router);

    // Rescue sub-router: ddrescue, TestDisk, PhotoRec, mdadm
    let rescue_router = create_rescue_router(state.clone());
    let api = api.nest("/api/rescue", rescue_router);

    // SPA fallback: serve index.html for all non-API routes.
    let spa = Router::new()
        .route("/sprite-unified.svg", get(serve_sprite_unified))
        .route(
            "/manifest.json",
            get(|headers: HeaderMap| serve_dist_file("manifest.json", headers)),
        )
        .route(
            "/favicon.svg",
            get(|headers: HeaderMap| serve_dist_file("favicon.svg", headers)),
        )
        .route(
            "/sw.js",
            get(|headers: HeaderMap| serve_dist_file("sw.js", headers)),
        )
        // Bundled desktop wallpapers (embedded from `frontend/public/wallpapers/`).
        .route("/wallpapers/{*path}", get(serve_wallpapers_file))
        // Vite emits content-hashed filenames under /assets and /fonts (e.g.
        // `index-DMTSp6tp.js`) — the content behind a given hashed name never
        // changes, so `spa` marks it `immutable` for a year. Every one of
        // these is served out of the frontend embedded in the binary at
        // compile time (see `spa::STATIC_DIR`), which is what lets a bare
        // copy of the binary serve the UI without also shipping
        // `static/dist/` alongside it.
        //
        // These routes must stay in this router rather than being merged in
        // from a separately-layered one: the main router below applies
        // `CompressionLayer` and the security headers to everything, and the
        // per-file caching lives in `spa`'s own handlers so a route can never
        // be cached for a year by accident.
        .route("/assets/{*path}", get(serve_assets_file))
        .route("/fonts/{*path}", get(serve_fonts_file))
        .fallback(serve_spa);

    // Public share-link downloads (`/share/{token}`) live outside `/api` and
    // are GET-only, so they are exempt from the CSRF layer (which only wraps
    // the API router before the merge below). Built as its own stateful
    // router so the public route can reach `AppState`.
    let public_share = Router::new()
        .route("/share/{token}", get(handle_share_link_download))
        .with_state(state.clone());

    // Terminal WebSocket route: native PTY, no external process or proxy hop.
    let ws_api = crate::handlers::terminal_pty::create_terminal_router(state.clone());

    // Nest terminal routes under /api/terminal.
    let api_with_terminal = api.nest("/api/terminal", ws_api);

    // VM console WebSocket route: xterm <-> Incus interactive shell relay.
    let vm_ws_api = crate::handlers::vm::create_vm_router(state.clone());

    // Nest the VM console WebSocket under /api/vm (just /console/ws).
    let api_with_terminal = api_with_terminal.nest("/api/vm", vm_ws_api);

    // CSRF protection, scoped to /api/* only (not the SPA fallback or static
    // asset serving, which are GET-only anyway) — applied here, before the
    // merge below, so it doesn't also wrap the SPA/static routers. See
    // `middleware::csrf::csrf_middleware`'s own doc comment for the exact
    // enforcement rule (session-tied token, only checked when a session
    // token is actually present on a state-changing request).
    let api_with_terminal = api_with_terminal.layer(axum::middleware::from_fn_with_state(
        state.clone(),
        crate::middleware::csrf_middleware,
    ));

    // Versioned client/server contract: reject an explicit contract mismatch
    // on `/api/*` with a 426 so a stale cached bundle fails loudly instead of
    // silently misparsing. `/api/meta` is exempt (it is the discovery
    // endpoint). Missing headers stay allowed for legacy clients.
    let api_with_terminal = api_with_terminal.layer(axum::middleware::from_fn(
        crate::middleware::contract_middleware,
    ));

    // First-sign-in password change enforcement: a session still holding an
    // admin-chosen password (`users.must_change_password`, migration 049) may
    // only reach /auth/me, /auth/logout and its own password change — the
    // UI gate shows the change screen, but this layer is the boundary the
    // UI cannot bypass. See `middleware::password_change_guard`.
    let api_with_terminal = api_with_terminal.layer(axum::middleware::from_fn_with_state(
        state.clone(),
        crate::middleware::password_change_guard_middleware,
    ));

    // Cache lifetimes are set per file by the `spa` handlers themselves (see
    // `spa::asset_response` + `CACHE_IMMUTABLE` / `CACHE_REVALIDATE`), not by
    // a blanket layer here: a layer applied after the merges below would also
    // pin API responses and `index.html` for a year, which is exactly the
    // stale-client bug the per-path policy avoids.
    api_with_terminal
        .merge(spa)
        .merge(public_share)
        .route("/static/{*path}", get(serve_static_file))
        .layer(cors)
        .layer(RequestBodyLimitLayer::new(10 * 1024 * 1024 * 1024))
        .layer(CompressionLayer::new().br(true).gzip(true))
        .layer(TraceLayer::new_for_http())
        // Outermost: inject CSP / HSTS / X-Frame-Options / nosniff /
        // Referrer-Policy / Permissions-Policy on every response (SPA, API
        // and static alike). Defined in `middleware::security_headers`;
        // wired here so it can never be bypassed by a route.
        .layer(axum::middleware::from_fn(
            crate::middleware::security_headers_middleware,
        ))
}

/// Initialize the database, auth service, and HTTP server, then start listening.
pub async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use crate::config::Settings;
    use crate::storage::database::DbError;
    use crate::storage::Database;
    use tracing_subscriber::layer::{Layer, SubscriberExt};
    use tracing_subscriber::util::SubscriberInitExt;

    // Initialize tracing: terminal (stderr, WARN+ only — no user info leaked
    // to the console) plus a daily-rotating file under our own logs_dir()
    // (INFO+, full detail), read by the System Journal app's "HomeVault
    // Application Log" source — the file keeps the full audit trail without
    // exposing usernames/actions on the terminal.
    let logs_dir = crate::config::paths::logs_dir();
    std::fs::create_dir_all(&logs_dir)?;
    let file_appender = tracing_appender::rolling::daily(&logs_dir, "homevault.log");
    let (file_writer, log_guard) = tracing_appender::non_blocking(file_appender);
    // Kept alive for the whole process lifetime so buffered log lines are
    // still flushed to disk on graceful shutdown; `run()` only returns once
    // `axum::serve(...)` below does.
    let _log_guard = log_guard;

    let file_filter = tracing_subscriber::EnvFilter::new("info");

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(
            tracing_subscriber::filter::FilterFn::new(|metadata| {
                metadata.level() <= &tracing::Level::WARN
            }),
        ))
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(file_writer)
                .with_ansi(false)
                .with_filter(file_filter),
        )
        .init();

    tracing::info!("HomeVault v{} starting up", env!("CARGO_PKG_VERSION"));

    // Initialize the internationalisation (i18n) system.
    crate::i18n::init_i18n(None);

    // Refuse to start on a platform the installer itself wouldn't have
    // allowed (Debian/Ubuntu on x86_64/aarch64/armv7 only) — previously
    // these checks existed in Settings but were never called anywhere,
    // so a binary copied onto an unsupported host would start anyway.
    if !Settings::is_supported_os() {
        tracing::error!("Unsupported OS: HomeVault requires Debian or Ubuntu. Refusing to start.");
        return Err("Unsupported OS".into());
    }
    if !Settings::is_supported_architecture() {
        tracing::error!(
            "Unsupported CPU architecture '{}': HomeVault requires x86_64, aarch64, or armv7. Refusing to start.",
            std::env::consts::ARCH
        );
        return Err("Unsupported architecture".into());
    }
    tracing::info!("Platform check passed ({})", std::env::consts::ARCH);

    let settings = Settings::load();

    let data_dir = crate::config::paths::data_dir();
    std::fs::create_dir_all(&data_dir)?;
    let db_path = crate::config::paths::database_path();
    let db_url = format!("sqlite:{}?mode=rwc", db_path.display());

    tracing::info!("Opening database at {}", db_path.display());
    // An existing file means there may be something to migrate — and therefore
    // something worth snapshotting before we touch it. A brand-new database has
    // nothing to lose, so it gets no pre-migration snapshot.
    let database_existed = db_path.exists();
    let db = Database::new(&db_url).await?;

    // Snapshot before applying anything. An upgrade that goes wrong then always
    // has a point to come back to, and it is the only moment the pre-upgrade
    // state is still reachable. Best-effort: a snapshot failure must not stop a
    // migration the operator may be depending on to boot.
    if database_existed {
        let pending = db.pending_migrations().await;
        if !pending.is_empty() {
            tracing::info!(
                count = pending.len(),
                "Pending migrations — taking a pre-migration snapshot"
            );
            match crate::db_backup::snapshot_before_migration(
                &db,
                &crate::config::paths::db_backup_dir(),
            )
            .await
            {
                Ok(path) => {
                    tracing::info!(path = %path.display(), "Pre-migration snapshot written")
                }
                Err(e) => tracing::warn!(error = %e, "Pre-migration snapshot failed"),
            }
        }
    }

    // Point 2 of the production-readiness pass: an older binary must not run
    // against a newer schema (migrations are append-only and never reverted).
    // `run_migrations` refuses in that case; surface it as a clear, fatal
    // startup error instead of a mystery on the first query.
    match db.run_migrations().await {
        Ok(()) => {}
        Err(DbError::SchemaTooNew {
            db_version,
            binary_version,
        }) => {
            tracing::error!(
                db_schema = db_version,
                binary_schema = binary_version,
                "Refusing to start: the database was written by a newer HomeVault. \
                 Reinstall the newer version, or restore a snapshot from \
                 data/backups/ taken before the upgrade."
            );
            return Err("Database schema is newer than this HomeVault build".into());
        }
        Err(e) => return Err(e.into()),
    }
    db.migrate_roles_005().await?;
    // Record what this binary brought the file to, so a later downgrade is
    // detected instead of corrupting data quietly.
    if let Err(e) = db.record_schema_version().await {
        tracing::warn!(error = %e, "Could not record the schema version");
    }

    // Point 7: a fast structural check at boot. A damaged file is logged and
    // surfaced through `/api/health/ready`, but never blocks startup — a
    // degraded appliance that can still export its data beats one that refuses
    // to boot.
    match db.quick_check().await {
        Ok(result) => crate::handlers::health_probe::record_integrity_result(&result),
        Err(e) => tracing::warn!(error = %e, "Database integrity check could not run"),
    }

    // Point 1: snapshot the database at startup when the last one is older than
    // the interval. Best-effort — the appliance must still boot if the backup
    // directory is unwritable.
    if database_existed {
        let backup_service = crate::db_backup::DbBackupService::new(db.clone());
        backup_service.backup_if_due().await;
    }

    let auth_service = crate::auth::AuthService::new(db.clone());
    auth_service.init().await?;

    // A first-run setup that was interrupted (OOM kill, systemd restart, power
    // cut) leaves `.setup_lock` behind; without this the wizard would keep
    // showing while every submission answered "Setup is already in progress".
    // `mark_setup_in_progress` also reclaims a stale lock on demand, so this is
    // the fast path at boot, not the only line of defence.
    crate::setup::SetupService::cleanup_stale_setup_lock();

    // Settings persistence must be initialised before the first load.
    // It writes to `{base}/data/settings.json`, so we pass the base directory.
    crate::settings_persistence::init_persistence(settings.data_directory.clone());

    // Core in-memory services — infallible constructors, so there's nothing
    // useful to report per-service beyond "this ran"; one summary line
    // replaces what used to be a "Creating X.../ X created" pair for each.
    let widgets = crate::storage::WidgetRepository::new(db.pool());
    let path_guard = crate::config::PathGuard::default();
    // Admin rate limiter: max 10 operations/min per admin.
    let admin_rate_limiter = Arc::new(crate::middleware::rate_limit::RateLimitState::new(10, 60));
    let pty_manager = Arc::new(crate::handlers::terminal_pty::PtyManager::new());
    let downloader = Arc::new(tokio::sync::Mutex::new(
        crate::handlers::downloader::Aria2Process::new(),
    ));
    let (ransomware_watch_tx, ransomware_watch_rx) =
        tokio::sync::mpsc::unbounded_channel::<crate::ransomware_watcher::WatchCommand>();
    let state = Arc::new(AppState {
        auth: auth_service,
        widgets,
        db: db.clone(),
        path_guard,
        admin_rate_limiter,
        pty_manager,
        downloader,
        ransomware_watch_tx,
    });
    tracing::info!(
        "Core services ready (widgets, path guard, rate limiter, terminal manager, downloader)"
    );

    // Background schedulers — a `tokio::spawn` call itself can't fail, so
    // (unlike the checks above) there's no meaningful per-task success/error
    // to log here; a real failure inside one of these loops is already
    // logged from within it. One line instead of a pair per scheduler.
    tokio::spawn(crate::ransomware_watcher::run_watcher_loop(
        state.clone(),
        ransomware_watch_rx,
    ));
    tokio::spawn(crate::restic_scheduler::run_scheduler_loop(state.clone()));
    // Snapshots of HomeVault's own database (WAL mode: the `.db` file alone is
    // not restorable) plus the periodic WAL checkpoint that keeps `-wal` bounded.
    tokio::spawn(crate::db_backup::DbBackupService::new(db.clone()).run_scheduler_loop());
    tokio::spawn(crate::cache_scheduler::run_scheduler_loop(state.clone()));
    tokio::spawn(crate::antivirus_scheduler::run_scheduler_loop(
        state.clone(),
    ));
    tokio::spawn(crate::journal_cleanup_scheduler::run_scheduler_loop(
        state.clone(),
    ));
    tokio::spawn(crate::scheduled_tasks_scheduler::run_scheduler_loop(
        state.clone(),
    ));
    tokio::spawn(crate::snapshot_scheduler::run_scheduler_loop(state.clone()));
    tokio::spawn(crate::metrics::run_collector_loop(state.db.pool()));
    // Transfer quotas (ROADMAP 23.2): re-opens the counters of every subject
    // whose counting window elapsed, so the Quotas/Users screens show a reset
    // that really happened. Enforcement does not depend on it — a stale row is
    // settled at check time by `transfer_quota::load_applicable`.
    tokio::spawn(crate::transfer_quota::run_reset_loop(state.db.pool()));
    // Account-expiry deactivation path (2026-10-20, migration 050): a live
    // session of an account past its `users.expires_at` horizon dies at the
    // next sweep. Sign-in refusal is immediate (`AuthService::authenticate`
    // re-checks `expires_at` on every attempt), this loop only bounds how
    // long an already-held session can outlive the horizon. One cheap SQL
    // DELETE every 10 minutes; no scheduler module of its own.
    {
        let auth = state.auth.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(600));
            loop {
                interval.tick().await;
                if let Err(e) = auth.revoke_expired_account_sessions().await {
                    tracing::warn!("Expired-account session sweep failed: {}", e);
                }
            }
        });
    }
    // Re-apply persisted disk spindown settings after boot — hdparm's -S
    // timer is per-drive volatile state.
    {
        let pool = state.db.pool();
        tokio::spawn(async move {
            let repo = crate::storage::DiskPowerRepository::new(pool);
            if let Ok(settings) = repo.list().await {
                let _ = tokio::task::spawn_blocking(move || {
                    crate::power::apply_all_spindown(&settings)
                })
                .await;
            }
        });
    }
    // Quota accounting is volatile kernel state: a reboot clears it while the
    // stored per-user limits stay in the database. `quotaon -aug` re-arms every
    // mount `/etc/fstab` declares with `usrquota`/`grpquota`, so a home limit
    // cannot silently stop being enforced until the next save. Not fatal when
    // there is nothing to arm (host without quota-tools, no quota mount):
    // `QuotaService::inspect` tells the UI the truth per user.
    tokio::spawn(async {
        crate::quota::QuotaService::activate_all().await;
    });
    tracing::info!("Background schedulers started (ransomware watcher, backups, cache-tier mover, antivirus, journal cleanup, scheduled tasks, metrics collector)");

    if crate::firewall::ufw_available() {
        // Every SSH port sshd may listen on, not just the first: a `Port 2222`
        // in an `sshd_config.d/*.conf` drop-in used to be missed, so the boot
        // re-assert left the admin's actual SSH port closed.
        let ssh_ports = crate::firewall::detect_ssh_ports();
        let app_port = crate::firewall::detect_app_port();
        tokio::spawn(async move {
            if let Err(e) = crate::firewall::ensure_mandatory_rules(&ssh_ports, app_port).await {
                tracing::warn!(
                    "Failed to (re)assert mandatory firewall rules at startup: {}",
                    e
                );
            }
        });
    }

    // Share services (SMB/NFS/FTP/SFTP) shouldn't need an admin to notice
    // they're down and restart them by hand — ensure each is actually
    // running on every boot, same "idempotent ensure" pattern as the
    // firewall rules above (`systemctl start` on an already-running unit
    // is a harmless no-op success). SFTP's dedicated sshd instance is
    // provisioned on demand, so its config/systemd unit may not exist yet
    // on a box where SFTP was never configured — `apply_config`
    // (re)generates both before we try to start it; on a fresh box with no
    // SSH host keys yet this just fails harmlessly and gets logged.
    if let Ok(sftp_config) = crate::shares::SharesService::get_sftp_config() {
        if let Err(e) = crate::sftp::SftpJailService::apply_config(&sftp_config) {
            tracing::warn!("Failed to apply SFTP config at startup: {}", e);
        }
    }
    for share_type in [
        crate::shares::ShareType::Smb,
        crate::shares::ShareType::Nfs,
        crate::shares::ShareType::Ftp,
        crate::shares::ShareType::Sftp,
    ] {
        tokio::task::spawn_blocking(move || {
            if let Err(e) = crate::shares::SharesService::start_service(share_type.clone()) {
                tracing::warn!(
                    "Failed to start '{}' share service at startup: {}",
                    share_type,
                    e
                );
            }
        });
    }
    {
        let sftp_state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::sftp::SftpJailService::rebuild_jails(sftp_state.db.pool()).await
            {
                tracing::warn!("SFTP: jail rebuild failed at startup: {}", e);
            }
        });
    }

    // mDNS / Zeroconf: publish `homevault.local` + the running share
    // services on the LAN, and keep the advertisement in sync as shares are
    // enabled/disabled. Link-local only, no-op when avahi-daemon is absent.
    tokio::spawn(crate::mdns::run_publisher_loop(settings.port));

    tokio::spawn(crate::notification_engine::run_notification_loop(
        state.clone(),
    ));

    // Per-app reverse proxy: initialise with the pieces its tasks need, then
    // bring back a proxy for every catalog app that had one before restart.
    crate::app_proxy::APP_PROXY.init(settings.port);
    tokio::spawn(async {
        crate::app_proxy::restore_all(
            crate::handlers::docker_catalog::proxy_restore_entries().await,
        )
        .await;
    });

    // Built-in WebDAV server (in-process, `crate::webdav`): drop the legacy
    // generated Python/systemd service on an upgraded host, then start it when
    // at least one enabled WebDAV share exists. A failure to bind is logged
    // and never stops the rest of HomeVault.
    crate::webdav::cleanup_legacy_service();
    {
        let webdav_state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::webdav::WEBDAV.start(&webdav_state).await {
                tracing::warn!("WebDAV: startup failed: {e}");
            }
        });
    }

    // Progress job map (compress/extract/copy/move, restic, rescue) is only
    // ever pruned on explicit cancellation — without this, completed/failed
    // jobs accumulate in memory for the lifetime of the process. Sweep hourly
    // and drop anything finished for more than a day; `cleanup_old_jobs`
    // already never removes a job that's still `Running`.
    tokio::spawn(async {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
        loop {
            interval.tick().await;
            crate::sse::progress::PROGRESS_MANAGER
                .cleanup_old_jobs(std::time::Duration::from_secs(24 * 3600));
        }
    });
    tracing::info!("Notification engine and progress-job cleanup started");

    let app = create_app(state.clone());

    // ---- Built-in HTTPS/TLS: optional second listener -----------------------
    // The plain-HTTP listener below is unchanged. When the admin has enabled
    // HTTPS (Settings > Security > HTTPS/TLS, persisted in data/tls.json), we
    // *additionally* serve the same router over TLS on `HTTPS_PORT` (8443) via a
    // small `axum_server` task. A missing/broken certificate is logged and the
    // instance stays HTTP-only — TLS trouble must never stop the server.
    let tls_settings = crate::tls::load_settings();
    let mut https_bound: Option<u16> = None;
    if tls_settings.enabled {
        match crate::tls::build_rustls_config(&tls_settings).await {
            Ok(rustls_config) => {
                let https_port = tls_settings.effective_https_port();
                let https_addr: SocketAddr = ([0, 0, 0, 0], https_port).into();
                // Tag every request on this listener so `security_headers`
                // emits HSTS only for connections that really arrived over TLS.
                let https_app = app.clone().layer(axum::middleware::from_fn(
                    |mut req: axum::extract::Request, next: axum::middleware::Next| async move {
                        req.extensions_mut()
                            .insert(crate::middleware::ArrivedOverTls);
                        next.run(req).await
                    },
                ));
                let handle = axum_server::Handle::new();
                let shutdown_handle = handle.clone();
                tokio::spawn(async move {
                    let _ = tokio::signal::ctrl_c().await;
                    shutdown_handle.graceful_shutdown(Some(std::time::Duration::from_secs(10)));
                });
                tokio::spawn(async move {
                    if let Err(e) = axum_server::bind_rustls(https_addr, rustls_config)
                        .handle(handle)
                        .serve(https_app.into_make_service_with_connect_info::<SocketAddr>())
                        .await
                    {
                        tracing::error!("HTTPS listener on :{https_port} stopped: {e}");
                    }
                });
                crate::tls::set_tls_active(true);
                https_bound = Some(https_port);
            }
            Err(e) => {
                tracing::warn!(
                    "HTTPS is enabled but the certificate could not be loaded — serving HTTP only: {e}"
                );
            }
        }
    }

    // When asked, redirect non-loopback plain-HTTP requests to HTTPS. Applied
    // to the HTTP router only; loopback (installer health check, app-proxy) is
    // exempt inside the middleware.
    let app = match (https_bound, tls_settings.redirect) {
        (Some(https_port), true) => app.layer(axum::middleware::from_fn_with_state(
            crate::middleware::HttpsRedirect { https_port },
            crate::middleware::https_redirect_middleware,
        )),
        _ => app,
    };

    let addr = format!("0.0.0.0:{}", settings.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    // A single, glanceable summary instead of scattered log lines — this is
    // the one thing an admin watching the terminal actually wants to know:
    // is it up, where do I reach it, and what did it find on this host.
    let host = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "localhost".to_string());

    // Full external-package dependency check (see `crate::dependencies`) —
    // runs every check concurrently via `spawn_blocking` (sequentially this
    // took 25+ seconds for ~24 entries when first measured live). Being
    // *installed* isn't the whole picture for daemon-backed dependencies
    // (docker, smbd, vsftpd, nfs-server, ufw, snmpd, the dedicated
    // homevault-sftp instance) — the package can be present but the
    // service itself stopped, which looks identical to "not installed"
    // from the affected page's point of view, so that's checked and
    // reported separately from plain presence.
    let dependency_statuses = crate::dependencies::check_all().await;
    for dep in dependency_statuses
        .iter()
        .filter(|d| d.required && !d.installed)
    {
        tracing::warn!(
            "Required package '{}' ({}) not found — {} will not work. See installer/install.sh.",
            dep.package,
            dep.name,
            dep.used_by
        );
    }
    for dep in dependency_statuses
        .iter()
        .filter(|d| d.installed && d.running == Some(false))
    {
        tracing::warn!(
            "'{}' is installed but its service isn't running — {} will not work until it's started.",
            dep.name, dep.used_by
        );
    }
    let optional_ready: Vec<&str> = dependency_statuses
        .iter()
        .filter(|d| !d.required && d.installed && d.running != Some(false))
        .map(|d| d.name)
        .collect();

    tracing::info!("----------------------------------------");
    tracing::info!("HomeVault v{} is ready", env!("CARGO_PKG_VERSION"));
    tracing::info!(
        "  Web UI:   http://{}:{}  (or http://localhost:{})",
        host,
        settings.port,
        settings.port
    );
    if let Some(https_port) = https_bound {
        let ci = crate::tls::certificate_info(&tls_settings);
        tracing::info!(
            "  HTTPS:    https://{}:{}  ({} certificate{}){}",
            host,
            https_port,
            if ci.self_signed {
                "self-signed"
            } else {
                "provided"
            },
            ci.not_after
                .as_deref()
                .map(|d| format!(", expires {d}"))
                .unwrap_or_default(),
            if tls_settings.redirect {
                "  [HTTP redirects here]"
            } else {
                ""
            },
        );
    }
    if crate::mdns::avahi_available() {
        tracing::info!(
            "  mDNS:     http://{}.local:{}",
            crate::mdns::mdns_hostname(),
            settings.port
        );
    }
    tracing::info!(
        "  Optional integrations detected: {}",
        if optional_ready.is_empty() {
            "none".to_string()
        } else {
            optional_ready.join(", ")
        }
    );
    tracing::info!("----------------------------------------");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal(
        state.pty_manager.clone(),
        state.downloader.clone(),
    ))
    .await?;

    Ok(())
}

/// Waits for Ctrl+C (or the equivalent shutdown signal from systemd), then kills
/// every live terminal session and the aria2 daemon it started, so neither
/// outlives the process that's supposed to own them (ROADMAP 17.3).
async fn shutdown_signal(
    pty_manager: Arc<crate::handlers::terminal_pty::PtyManager>,
    downloader: crate::handlers::downloader::Aria2State,
) {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("Shutdown signal received, cleaning up terminal sessions and the downloader...");
    pty_manager.cleanup_all().await;

    let mut proc = downloader.lock().await;
    if let Some(mut child) = proc.child.take() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
}
