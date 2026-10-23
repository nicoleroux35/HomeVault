/* =============================================================================
 * File          : widget_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 23/10/2026 by Nicolas Le Roux
 * Version       : 2.0.0
 *
 * Description:
 *   Dashboard widget tiles, scoped to their owner. Every read and every
 *   mutation takes the caller's username, because a tile belongs to exactly
 *   one account (`widgets` is keyed by `(username, id)`): there is no
 *   cross-account "list all", no unscoped delete, and no reorder that can
 *   touch a row the caller does not own.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 *
 * Usage:
 *   `state.widgets.find_by_username(&username)`, `delete_for(&username, &id)`,
 *   `reorder_for(&username, &ids)` — the handler passes the username that
 *   `require_auth` resolved, never a client-supplied one.
 * =============================================================================
 */

//! Owner-scoped dashboard widget tiles.
//!
//! Every method takes the caller's username. There is deliberately no
//! unscoped `find_all`/`delete`/`reorder` any more (ROADMAP 15.1): a global
//! accessor is how "any household user can delete or reorder every widget"
//! happened in the first place.

use crate::impl_repository;
use crate::storage::database::DbError;
use crate::storage::models::{Widget, WidgetRow};
use sqlx::SqlitePool;
use std::sync::Arc;

/// Repository for dashboard widget tiles, scoped to their owner.
#[derive(Clone)]
pub struct WidgetRepository {
    pool: Arc<SqlitePool>,
}

impl_repository!(WidgetRepository);

impl WidgetRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// Look up one of the caller's widgets by its id.
    ///
    /// Scoped by owner so a probe for another account's id is indistinguishable
    /// from a missing id (`None`), which is what the handler turns into 404.
    pub async fn find_by_id(&self, username: &str, id: &str) -> Result<Option<Widget>, DbError> {
        let widget = sqlx::query_as::<_, WidgetRow>(
            "SELECT id, wtype, title, size, row_pos, col_pos, row_span, col_span, username
             FROM widgets WHERE username = ? AND id = ?",
        )
        .bind(username)
        .bind(id)
        .fetch_optional(self.pool.as_ref())
        .await?;

        Ok(widget.map(|w| w.into()))
    }

    /// List the caller's widgets in display order.
    pub async fn find_by_username(&self, username: &str) -> Result<Vec<Widget>, DbError> {
        let widgets = sqlx::query_as::<_, WidgetRow>(
            "SELECT id, wtype, title, size, row_pos, col_pos, row_span, col_span, username
             FROM widgets WHERE username = ? ORDER BY row_pos, col_pos",
        )
        .bind(username)
        .fetch_all(self.pool.as_ref())
        .await?;

        Ok(widgets.into_iter().map(|w| w.into()).collect())
    }

    /// Number of tiles the caller owns (used to bound the grid).
    pub async fn count_for(&self, username: &str) -> Result<i64, DbError> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM widgets WHERE username = ?")
            .bind(username)
            .fetch_one(self.pool.as_ref())
            .await?;

        Ok(row.0)
    }

    /// Insert a new widget for its owner.
    pub async fn create(&self, widget: &Widget) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO widgets (username, id, wtype, title, size, row_pos, col_pos, row_span, col_span)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&widget.username)
        .bind(&widget.id)
        .bind(&widget.wtype)
        .bind(&widget.title)
        .bind(&widget.size)
        .bind(widget.row_pos)
        .bind(widget.col_pos)
        .bind(widget.row_span)
        .bind(widget.col_span)
        .execute(self.pool.as_ref())
        .await?;

        Ok(())
    }

    /// Delete one of the caller's widgets. Returns `true` when a row matched,
    /// so the handler can answer 404 for an id the caller does not own.
    pub async fn delete_for(&self, username: &str, id: &str) -> Result<bool, DbError> {
        let result = sqlx::query("DELETE FROM widgets WHERE username = ? AND id = ?")
            .bind(username)
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Remove every widget the caller owns (account teardown / reset).
    pub async fn delete_all_for(&self, username: &str) -> Result<u64, DbError> {
        let result = sqlx::query("DELETE FROM widgets WHERE username = ?")
            .bind(username)
            .execute(self.pool.as_ref())
            .await?;

        Ok(result.rows_affected())
    }

    /// Reorder the caller's grid: list index → 3-column row/col.
    ///
    /// Only rows owned by `username` are touched, so a request that names
    /// another account's id is silently ignored instead of moving it. Returns
    /// how many of the caller's rows were updated, which lets the handler
    /// answer 404 when the list matched nothing of theirs.
    pub async fn reorder_for(&self, username: &str, widget_order: &[String]) -> Result<u64, DbError> {
        if widget_order.is_empty() {
            return Ok(0);
        }

        let mut tx = self.pool.begin().await?;
        let mut updated = 0u64;

        // Iterate within a single transaction; no dynamic SQL construction.
        for (index, id) in widget_order.iter().enumerate() {
            let row_pos = (index / 3) as i32 + 1;
            let col_pos = (index % 3) as i32 + 1;
            let result = sqlx::query(
                "UPDATE widgets SET row_pos = ?, col_pos = ? WHERE username = ? AND id = ?",
            )
            .bind(row_pos)
            .bind(col_pos)
            .bind(username)
            .bind(id)
            .execute(tx.as_mut())
            .await?;
            updated += result.rows_affected();
        }

        tx.commit().await?;
        Ok(updated)
    }
}
