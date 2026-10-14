/* =============================================================================
 * File          : mod.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Module index for Server-Sent Events: the real-time progress channel for
 *   long-running jobs (progress.rs), the live notification bus for the
 *   topbar bell panel (notifications.rs) and the Explorer's filesystem
 *   watcher (file_watcher.rs).
 *
 * Dependencies:
 *   - none (re-exports only)
 *
 * Usage:
 *   Handlers publish through ProgressEmitter / notifications::publish;
 *   browsers subscribe to the /api/progress, /api/events and
 *   /api/explorer/watch SSE endpoints.
 * =============================================================================
 */

//! Server-Sent Events (SSE) Module
//!
//! This module provides SSE functionality for real-time progress updates
//! and event streaming to clients.

pub mod file_watcher;
pub mod notifications;
pub mod progress;

pub use progress::*;
