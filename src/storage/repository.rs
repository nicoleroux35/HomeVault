/* =============================================================================
 * File          : repository.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.1
 *
 * Description:
 *   Common trait for all SQLite repositories: standardises pool access
 *   and transaction handling so every *_repo.rs module shares one shape.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 *
 * Usage:
 *   Parent of all repositories in this directory.
 * =============================================================================
 */

use crate::storage::database::DbError;
use sqlx::{Sqlite, SqlitePool, Transaction};
use std::future::Future;
use std::sync::Arc;

/// Common trait for all repositories, standardising pool access and transactions.
pub trait Repository: Clone + Send + Sync {
    fn pool(&self) -> Arc<SqlitePool>;

    /// Begin a new database transaction.
    fn begin_transaction(
        &self,
    ) -> impl Future<Output = Result<Transaction<'static, Sqlite>, DbError>> + Send {
        async move { Ok(self.pool().begin().await?) }
    }
}

/// Derives the `Repository` trait for a struct containing `pool: Arc<SqlitePool>`.
#[macro_export]
macro_rules! impl_repository {
    ($repo:ty) => {
        impl $crate::storage::repository::Repository for $repo {
            fn pool(&self) -> std::sync::Arc<sqlx::SqlitePool> {
                std::sync::Arc::clone(&self.pool)
            }
        }
    };
}
