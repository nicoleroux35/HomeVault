/* =============================================================================
 * File          : mod.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Module index for authentication and account management: the AuthService
 *   (login, sessions, two-factor, passwords, recovery codes), the session
 *   validation middleware and the model re-exports.
 *
 * Dependencies:
 *   - none (re-exports only)
 *
 * Usage:
 *   `use crate::auth::{AuthService, Session, User};` — the service is built
 *   once at startup and exposed as AppState.auth.
 * =============================================================================
 */

//! Authentication: login, sessions, 2FA, password management, and middleware.

pub mod middleware;
pub mod models;
pub mod service;

#[cfg(test)]
mod tests;

pub use middleware::auth_middleware;
pub use models::{Session, User};
pub use service::{AuthError, AuthService};
