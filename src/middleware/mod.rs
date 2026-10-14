/* =============================================================================
 * File          : mod.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 12/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Module index for the HTTP middleware layer. Each submodule is an axum
 *   layer (a function the router runs before/after the requests it guards)
 *   or a small in-memory state type used by those layers and by handlers.
 *   Wiring lives in src/web/mod.rs: contract + CSRF guard the /api routes,
 *   setup_guard protects the first-run bootstrap endpoints, security
 *   headers are applied globally, and the HTTPS redirect runs on the plain
 *   HTTP listener.
 *
 * Dependencies:
 *   - none (re-exports only)
 *
 * Usage:
 *   Handlers and the router import from here, e.g.
 *   `use crate::middleware::csrf_middleware;` — see each submodule's
 *   documentation for details.
 * =============================================================================
 */

//! HTTP middleware: logging, rate limiting, CSRF protection, fail2ban, and security headers.

pub mod contract;
pub mod csrf;
pub mod fail2ban;
pub mod https_redirect;
pub mod logging;
pub mod rate_limit;
pub mod security_headers;
pub mod setup_guard;

pub use contract::contract_middleware;
pub use csrf::csrf_middleware;
pub use fail2ban::Fail2BanState;
pub use https_redirect::{https_redirect_middleware, HttpsRedirect};
pub use logging::logging_middleware;
pub use rate_limit::rate_limit_middleware;
pub use security_headers::{security_headers_middleware, ArrivedOverTls};
pub use setup_guard::setup_guard_middleware;
