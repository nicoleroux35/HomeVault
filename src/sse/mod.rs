//! Server-Sent Events (SSE) Module
//! 
//! This module provides SSE functionality for real-time progress updates
//! and event streaming to clients.

pub mod file_watcher;
pub mod notifications;
pub mod progress;

pub use progress::*;
