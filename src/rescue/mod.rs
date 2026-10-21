/* =============================================================================
 * File          : mod.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Module index for the rescue toolkit: a safe, validated interface to the
 *   system recovery tools (disk cloning, partition repair, file recovery,
 *   RAID repair) used by the Rescue app.
 *
 * Dependencies:
 *   - none (module declarations)
 *
 * Usage:
 *   handlers/rescue.rs; see docs/guides/rescue.md.
 * =============================================================================
 */

//! Rescue module: disk recovery, partition repair, file recovery, and RAID management
//!
//! This module provides a safe interface to system recovery tools:
//! - ddrescue: Clone failing disks
//! - TestDisk: Analyze and repair partition tables
//! - PhotoRec: Recover deleted files
//! - mdadm: Diagnose and repair RAID arrays

pub mod jobs;
pub mod models;
pub mod service;
pub mod validator;

// Re-export commonly used types
pub use jobs::RescueJobHandle;
pub use models::*;
pub use service::RescueService;
pub use validator::{check_clone_safety, ClonePlan, SecurityValidator};
