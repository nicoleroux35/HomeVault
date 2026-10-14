/* =============================================================================
 * File          : mod.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Module index for configuration: global server settings loaded from
 *   environment variables (settings.rs), the centralized filesystem layout
 *   (paths.rs) and the file-explorer root configuration with its
 *   path-traversal guard (explorer_config.rs).
 *
 * Dependencies:
 *   - none (re-exports only)
 *
 * Usage:
 *   `use crate::config::{Settings, PathGuard};` — both are built once at
 *   startup in src/web/mod.rs.
 * =============================================================================
 */

//! Configuration: application settings, explorer paths, and path-guard validation.

pub mod explorer_config;
pub mod paths;
pub mod settings;

pub use explorer_config::{ExplorerSettings, PathGuard};
pub use settings::Settings;
