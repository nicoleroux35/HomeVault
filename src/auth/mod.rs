//! Authentication: login, sessions, 2FA, password management, and middleware.

pub mod middleware;
pub mod models;
pub mod service;

#[cfg(test)]
mod tests;

pub use middleware::auth_middleware;
pub use models::{Session, User};
pub use service::{AuthError, AuthService};
