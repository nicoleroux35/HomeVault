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
pub mod config;
pub mod handlers;
pub mod network;
pub mod settings_persistence;
pub mod sse;
pub mod storage;
pub mod vpn;
pub mod web;

/// System modules
pub mod acl;
pub mod antivirus;
pub mod antivirus_scheduler;
pub mod app_catalog;
pub mod app_proxy;
pub mod autostart;
pub mod avatars;
pub mod browser;
pub mod cache;
pub mod cache_scheduler;
pub mod compose;
pub mod db_backup;
pub mod dependencies;
pub mod disk;
pub mod docker;
pub mod email;
pub mod encryption;
pub mod events;
pub mod firewall;
pub mod groups;
pub mod health;
pub mod i18n;
pub mod incus;
pub mod journal_cleanup_scheduler;
pub mod logging;
pub mod mcp;
pub mod mdns;
pub mod media_ticket;
pub mod metrics;
pub mod middleware;
pub mod notification_engine;
pub mod notification_events;
pub mod permissions;
pub mod pools;
pub mod power;
pub mod quota;
pub mod rag;
pub mod rag_indexers;
pub mod ransomware;
pub mod ransomware_watcher;
pub mod release_notes;
pub mod rescue;
pub mod restic;
pub mod restic_scheduler;
pub mod scheduled_tasks_scheduler;
pub mod snapshot_scheduler;
pub mod transfer_quota;

pub mod setup;
pub mod sftp;
pub mod shares;
pub mod snapshots;
pub mod snmp;
pub mod tls;
pub mod ups;
pub mod utils;
pub mod webdav;
