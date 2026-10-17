-- Migration: 048_app_meta.sql
-- Bookkeeping about the database itself: which schema version this file has
-- been brought to, and by which HomeVault release.
--
-- Migrations are append-only and never reverted, so an older binary started
-- against a newer database would otherwise run happily and then fail on the
-- first query expecting a column it does not know about. `run_migrations`
-- compares `schema_version` against the highest migration embedded in the
-- binary and refuses to start when the file is newer (see
-- `DbError::SchemaTooNew`).
--
-- Singleton row (id = 1), same convention as `setup_state` / `setup_progress`.
CREATE TABLE IF NOT EXISTS app_meta (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    schema_version INTEGER NOT NULL,
    app_version TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
