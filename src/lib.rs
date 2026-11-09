/* =============================================================================
 * File          : lib.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Library root: declares every top-level module (auth, config, handlers,
 *   storage, services, middleware, ...) so the single server binary can
 *   pull the whole application from one crate.
 *
 * Dependencies:
 *   - none (module declarations only)
 *
 * Usage:
 *   Referenced by src/bin/server.rs; see docs/BACKEND.md for the map.
 * =============================================================================
 */

pub mod api_contract;
/// Core web framework modules
pub mod auth;
pub mod auto_block;
pub mod catalog_signature;
pub mod config;
pub mod handlers;
pub mod network;
/// Generated OpenAPI 3 description of the JSON API.
pub mod openapi;
pub mod proxy;
pub mod quota_history;
pub mod scrub;
pub mod settings_persistence;
pub mod sse;
pub mod static_routes;
pub mod storage;
pub mod supply_chain;
pub mod syslog_forward;
pub mod system_stability;
pub mod vpn;
pub mod web;
pub mod wol;
pub mod ws_discovery;

/// System modules
pub mod acl;
pub mod acme;
pub mod acme_worker;
pub mod activity_poller;
pub mod antivirus;
pub mod audit;
pub mod antivirus_scheduler;
pub mod app_catalog;
pub mod app_proxy;
pub mod autostart;
pub mod avatars;
pub mod backup_target;
pub mod backup_target_scheduler;
pub mod browser;
pub mod btrfs_maintenance;
pub mod cache;
pub mod cache_scheduler;
pub mod checksum;
pub mod compose;
pub mod config_backup;
pub mod connected_clients;
pub mod content_index;
pub mod db_backup;
pub mod ddns;
pub mod dedup;
pub mod dependencies;
pub mod directory;
pub mod disk;
pub mod disk_health;
pub mod disk_health_scheduler;
pub mod disk_prediction;
pub mod docker;
pub mod docker_update_scheduler;
pub mod email;
pub mod email_oauth;
pub mod encryption;
pub mod events;
pub mod firewall;
pub mod geoip;
pub mod global_search;
pub mod groups;
pub mod health;
pub mod health_report;
pub mod host_accounts;
pub mod i18n;
pub mod identity;
pub mod incus;
pub mod iscsi;
pub mod iscsi_guard;
pub mod iso_mount;
pub mod journal_cleanup_scheduler;
pub mod logging;
pub mod mcp;
pub mod mdns;
pub mod media_ticket;
pub mod metrics;
pub mod middleware;
pub mod notification_engine;
pub mod notification_events;
pub mod oidc;
pub mod outbound;
pub mod passkey;
pub mod permissions;
pub mod pools;
pub mod power;
pub mod quota;
pub mod rag;
pub mod rag_indexers;
pub mod raid_stability;
pub mod ransomware;
pub mod ransomware_watcher;
pub mod reachability;
pub mod release_notes;
pub mod remote_mounts;
pub mod downloader_extract;
pub mod downloader_rss;
pub mod replication;
pub mod replication_scheduler;
pub mod restore_drill;
pub mod replication_witness;
pub mod rescue;
pub mod restic;
pub mod restic_scheduler;
pub mod reverse_proxy;
pub mod rsync_server;
pub mod saml;
pub mod scheduled_tasks_scheduler;
pub mod security_advisor;
pub mod smart;
pub mod sms;
pub mod snapshot_lock_scheduler;
pub mod snapshot_locks;
pub mod snapshot_quiescence;
pub mod snapshot_scheduler;
pub mod transfer_quota;
pub mod trash;
pub mod trash_scheduler;
pub mod upload_session;

pub mod dhcp_server;
pub mod fan_control;
pub mod guided_redundancy;
pub mod local_dns;
pub mod setup;
pub mod ftp;
pub mod sftp;
pub mod share_quota;
pub mod shares;
pub mod snapshots;
pub mod snmp;
pub mod snmp_agent;
pub mod tls;
pub mod traffic_control;
pub mod transfer_history;
pub mod upnp;
pub mod ups;
pub mod ups_server;
pub mod usb_copy;
pub mod utils;
pub mod web_push;
pub mod webdav;
pub mod zfs_cache;
pub mod syncthing_versions;
pub mod update_engine;
pub mod vm_export;
pub mod zvol;
pub mod zfs_volumes;
