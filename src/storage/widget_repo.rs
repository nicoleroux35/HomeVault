use crate::storage::database::DbError;
use crate::storage::models::{Widget, WidgetRow};
use crate::impl_repository;
use sqlx::SqlitePool;
use std::sync::Arc;

/// Repository for dashboard widget tiles.
#[derive(Clone)]
pub struct WidgetRepository {
    pool: Arc<SqlitePool>,
}

impl_repository!(WidgetRepository);

impl WidgetRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// Look up a widget by its ID.
    pub async fn find_by_id(&self, id: &str) -> Result<Option<Widget>, DbError> {
        let widget = sqlx::query_as::<_, WidgetRow>(
            "SELECT id, wtype, title, size, row_pos, col_pos, row_span, col_span, username FROM widgets WHERE id = ?"
        )
        .bind(id)
        .fetch_optional(self.pool.as_ref())
        .await?;

        Ok(widget.map(|w| w.into()))
    }

    /// List all widgets (position-sorted).
    pub async fn find_all(&self) -> Result<Vec<Widget>, DbError> {
        let widgets = sqlx::query_as::<_, WidgetRow>(
            "SELECT id, wtype, title, size, row_pos, col_pos, row_span, col_span, username FROM widgets ORDER BY row_pos, col_pos"
        )
        .fetch_all(self.pool.as_ref())
        .await?;

        Ok(widgets.into_iter().map(|w| w.into()).collect())
    }

    /// List widgets owned by a specific user.
    pub async fn find_by_username(&self, username: &str) -> Result<Vec<Widget>, DbError> {
        let widgets = sqlx::query_as::<_, WidgetRow>(
            "SELECT id, wtype, title, size, row_pos, col_pos, row_span, col_span, username FROM widgets WHERE username = ? ORDER BY row_pos, col_pos"
        )
        .bind(username)
        .fetch_all(self.pool.as_ref())
        .await?;

        Ok(widgets.into_iter().map(|w| w.into()).collect())
    }

    /// Insert a new widget.
    pub async fn create(&self, widget: &Widget) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO widgets (id, wtype, title, size, row_pos, col_pos, row_span, col_span, username) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&widget.id)
        .bind(&widget.wtype)
        .bind(&widget.title)
        .bind(&widget.size)
        .bind(widget.row_pos)
        .bind(widget.col_pos)
        .bind(widget.row_span)
        .bind(widget.col_span)
        .bind(&widget.username)
        .execute(self.pool.as_ref())
        .await?;

        Ok(())
    }

    /// Update all fields of a widget.
    pub async fn update(&self, widget: &Widget) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE widgets SET wtype = ?, title = ?, size = ?, row_pos = ?, col_pos = ?, row_span = ?, col_span = ?, username = ? WHERE id = ?"
        )
        .bind(&widget.wtype)
        .bind(&widget.title)
        .bind(&widget.size)
        .bind(widget.row_pos)
        .bind(widget.col_pos)
        .bind(widget.row_span)
        .bind(widget.col_span)
        .bind(&widget.username)
        .bind(&widget.id)
        .execute(self.pool.as_ref())
        .await?;

        Ok(())
    }

    /// Delete a widget by ID.
    pub async fn delete(&self, id: &str) -> Result<(), DbError> {
        sqlx::query("DELETE FROM widgets WHERE id = ?")
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;

        Ok(())
    }

    /// Remove all widgets.
    pub async fn delete_all(&self) -> Result<(), DbError> {
        sqlx::query("DELETE FROM widgets")
            .execute(self.pool.as_ref())
            .await?;

        Ok(())
    }

    /// Reorder widgets into a grid layout (3 columns, position from list index).
    pub async fn reorder(&self, widget_order: &[String]) -> Result<(), DbError> {
        if widget_order.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        let mut values = Vec::new();
        for (index, id) in widget_order.iter().enumerate() {
            let row_pos = (index / 3) as i32 + 1;
            let col_pos = (index % 3) as i32 + 1;
            values.push((id.as_str(), row_pos, col_pos));
        }

        // Iterate within a single transaction to avoid dynamic SQL construction.
        for (id, row_pos, col_pos) in values {
            sqlx::query("UPDATE widgets SET row_pos = ?, col_pos = ? WHERE id = ?")
                .bind(row_pos)
                .bind(col_pos)
                .bind(id)
                .execute(tx.as_mut())
                .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Delete multiple widgets by ID in a single query.
    pub async fn delete_many(&self, ids: &[String]) -> Result<u64, DbError> {
        if ids.is_empty() {
            return Ok(0);
        }

        use sqlx::QueryBuilder;

        let mut qb: QueryBuilder<sqlx::Sqlite> = QueryBuilder::new("DELETE FROM widgets WHERE id IN (");
        let mut separated = qb.separated(", ");
        for id in ids {
            separated.push_bind(id);
        }
        separated.push_unseparated(")");

        let result = qb.build().execute(self.pool.as_ref()).await?;
        Ok(result.rows_affected())
    }

    /// List widgets with pagination (position-sorted).
    pub async fn find_all_paginated(
        &self,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Widget>, DbError> {
        let widgets = sqlx::query_as::<_, WidgetRow>(
            "SELECT id, wtype, title, size, row_pos, col_pos, row_span, col_span, username
             FROM widgets
             ORDER BY row_pos, col_pos
             LIMIT ? OFFSET ?"
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(self.pool.as_ref())
        .await?;

        Ok(widgets.into_iter().map(|w| w.into()).collect())
    }

    /// Total number of widgets.
    pub async fn count(&self) -> Result<i64, DbError> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM widgets")
            .fetch_one(self.pool.as_ref())
            .await?;

        Ok(row.0)
    }
}
