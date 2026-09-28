//! Configuration: application settings, explorer paths, and path-guard validation.

pub mod explorer_config;
pub mod paths;
pub mod settings;

pub use explorer_config::{ExplorerSettings, PathGuard};
pub use settings::Settings;
