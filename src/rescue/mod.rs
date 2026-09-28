//! Rescue module: disk recovery, partition repair, file recovery, and RAID management
//!
//! This module provides a safe interface to system recovery tools:
//! - ddrescue: Clone failing disks
//! - TestDisk: Analyze and repair partition tables
//! - PhotoRec: Recover deleted files
//! - mdadm: Diagnose and repair RAID arrays

pub mod models;
pub mod service;
pub mod validator;

// Re-export commonly used types
pub use models::*;
pub use service::RescueService;
pub use validator::SecurityValidator;
