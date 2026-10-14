/* =============================================================================
 * File          : models.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Re-exports the shared account and session data types (User, Session,
 *   LoginRequest, LoginResponse, CreateUserRequest) defined in the storage
 *   layer, so authentication code can import them from one place.
 *
 * Dependencies:
 *   - none (re-exports only)
 *
 * Usage:
 *   `use crate::auth::models::{Session, User};`
 * =============================================================================
 */

//! Re-exports of auth-related model types from the storage layer.

pub use crate::storage::models::{CreateUserRequest, LoginRequest, LoginResponse, Session, User};
