/* =============================================================================
 * File          : upload_session.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Durable storage for the Explorer's resumable chunked upload sessions
 *   (HV-UX-03): the row behind every in-flight upload, so a session survives
 *   a process restart instead of dying with the in-memory map. Also answers
 *   the concurrency caps and the TTL sweep, and lists a user's in-flight
 *   uploads for the transfer tray.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 *
 * Usage:
 *   `UploadSessionRepository::new(state.db.pool().clone())` from
 *   `handlers::explorer`; see docs/guides/explorer.md → "Resumable uploads".
 * =============================================================================
 */

//! Durable resumable-upload sessions (`upload_sessions`, migration 074).
//!
//! Before this existed, the four upload endpoints shared one process-wide
//! `HashMap`: restarting HomeVault (a settings change, an upgrade, a crash)
//! silently dropped every partial upload, and a client resuming from its own
//! offset got "Upload session not found or expired" with the temp file still
//! on disk. The row here is the durable half of that state; the map stays as a
//! write-through cache because the chunk path is hot.
//!
//! Two rules the schema and this layer exist to keep:
//!
//! 1. **The offset only moves on a full chunk.** `update_offset` is the single
//!    writer, and it also refreshes `last_chunk_at` (the TTL is idle-based: a
//!    slow 4 GB upload is not dead after 30 minutes, an abandoned one is).
//! 2. **Expiry is a real unlink.** `prune_expired` returns the rows it removed
//!    so the caller can delete each temp file — a row without its file would
//!    make the next `complete` fail with a confusing error, and a file without
//!    its row is dead space on the pool.

use sqlx::{Row, SqlitePool};
use std::sync::Arc;

/// A session row as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredUploadSession {
    pub session_id: String,
    pub username: String,
    /// Directory the finished file lands in (absolute, already validated
    /// against the caller's allowed roots when the session opened).
    pub dir: String,
    pub filename: String,
    /// Hidden sibling the chunks are written into (`.name.hvupl-<uuid>`).
    pub tmp_path: String,
    /// Bytes durably written so far — the client's resume point.
    pub offset: u64,
    /// Declared final size.
    pub total_bytes: u64,
    /// `overwrite` | `keep_both` | `skip`, applied on `complete`.
    pub strategy: String,
    pub expected_sha256: Option<String>,
    pub created_at: i64,
    pub last_chunk_at: i64,
    /// Throughput of the last accepted chunk in bytes/second, when it could be
    /// measured. What the transfer tray shows next to the progress bar; `None`
    /// on a session that has not taken a chunk yet.
    pub speed_bps: Option<u64>,
}

impl StoredUploadSession {
    /// `true` when the session has been idle longer than `ttl_secs`.
    ///
    /// Idle-based, not creation-based: a big upload that keeps receiving
    /// chunks stays alive however long it takes, which is what "30 minute
    /// session" means to a user resuming a slow transfer.
    pub fn is_expired(&self, now: i64, ttl_secs: i64) -> bool {
        now.saturating_sub(self.last_chunk_at) >= ttl_secs
    }

    /// Seconds left, from the last chunk's throughput. `None` when there is no
    /// measurement or no speed — the tray renders "unknown", never "0 s".
    pub fn eta_seconds(&self) -> Option<u64> {
        let speed = self.speed_bps.filter(|s| *s > 0)?;
        if self.offset >= self.total_bytes {
            return Some(0);
        }
        Some((self.total_bytes - self.offset).div_ceil(speed))
    }

    /// Progress as a percentage, `None` when the declared size is 0.
    pub fn progress(&self) -> Option<f64> {
        (self.total_bytes > 0).then(|| {
            ((self.offset.min(self.total_bytes) as f64 / self.total_bytes as f64) * 100.0)
                .clamp(0.0, 100.0)
        })
    }
}

/// Reads and writes `upload_sessions`.
#[derive(Clone)]
pub struct UploadSessionRepository {
    pool: Arc<SqlitePool>,
}

impl UploadSessionRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// Insert or replace a session row (the temp-file path is authoritative:
    /// a resumed session keeps the file it started with).
    pub async fn save(&self, session: &StoredUploadSession) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO upload_sessions
                (session_id, username, dir_path, filename, tmp_path, offset, total_bytes,
                 strategy, expected_sha256, created_at, last_chunk_at, speed_bps)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(session_id) DO UPDATE SET
                offset = excluded.offset,
                last_chunk_at = excluded.last_chunk_at,
                speed_bps = excluded.speed_bps",
        )
        .bind(&session.session_id)
        .bind(&session.username)
        .bind(&session.dir)
        .bind(&session.filename)
        .bind(&session.tmp_path)
        .bind(session.offset as i64)
        .bind(session.total_bytes as i64)
        .bind(&session.strategy)
        .bind(&session.expected_sha256)
        .bind(session.created_at)
        .bind(session.last_chunk_at)
        .bind(session.speed_bps.map(|b| b as i64))
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    /// Load one session by id.
    pub async fn load(&self, session_id: &str) -> Result<Option<StoredUploadSession>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT session_id, username, dir_path, filename, tmp_path, offset, total_bytes,
                    strategy, expected_sha256, created_at, last_chunk_at, speed_bps
             FROM upload_sessions WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.map(|row| StoredUploadSession {
            session_id: row.get("session_id"),
            username: row.get("username"),
            dir: row.get("dir_path"),
            filename: row.get("filename"),
            tmp_path: row.get("tmp_path"),
            offset: row.get::<i64, _>("offset") as u64,
            total_bytes: row.get::<i64, _>("total_bytes") as u64,
            strategy: row.get("strategy"),
            expected_sha256: row.get("expected_sha256"),
            created_at: row.get("created_at"),
            last_chunk_at: row.get("last_chunk_at"),
            speed_bps: row.get::<Option<i64>, _>("speed_bps").map(|b| b.max(0) as u64),
        }))
    }

    /// Advance the resume point and refresh the idle clock — one statement, so
    /// a crash between the two can never leave an offset that looks fresher
    /// than it is.
    pub async fn update_offset(
        &self,
        session_id: &str,
        offset: u64,
        speed_bps: Option<u64>,
        now: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE upload_sessions SET offset = ?, last_chunk_at = ?, speed_bps = ? \
             WHERE session_id = ?",
        )
        .bind(offset as i64)
        .bind(now)
        .bind(speed_bps.map(|b| b as i64))
        .bind(session_id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }

    /// Delete one session row (completed, aborted or expired).
    pub async fn delete(&self, session_id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM upload_sessions WHERE session_id = ?")
            .bind(session_id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }

    /// How many live sessions exist in total and for one user.
    ///
    /// The concurrency caps are enforced against the **database**, not the
    /// in-memory map: after a restart the map is empty while the durable
    /// sessions still hold temp files, and a cap that forgets them would let
    /// one account start twenty more on top.
    pub async fn counts(
        &self,
        username: &str,
        cutoff: i64,
    ) -> Result<(u64, u64), sqlx::Error> {
        let total: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM upload_sessions WHERE last_chunk_at > ?")
                .bind(cutoff)
                .fetch_one(self.pool.as_ref())
                .await?;
        let mine: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM upload_sessions WHERE last_chunk_at > ? AND username = ?",
        )
        .bind(cutoff)
        .bind(username)
        .fetch_one(self.pool.as_ref())
        .await?;
        Ok((total.max(0) as u64, mine.max(0) as u64))
    }

    /// A user's live sessions, oldest first — the transfer tray's upload list.
    pub async fn list_for_user(
        &self,
        username: &str,
        cutoff: i64,
    ) -> Result<Vec<StoredUploadSession>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT session_id, username, dir_path, filename, tmp_path, offset, total_bytes,
                    strategy, expected_sha256, created_at, last_chunk_at, speed_bps
             FROM upload_sessions
             WHERE username = ? AND last_chunk_at > ?
             ORDER BY created_at ASC",
        )
        .bind(username)
        .bind(cutoff)
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| StoredUploadSession {
                session_id: row.get("session_id"),
                username: row.get("username"),
                dir: row.get("dir_path"),
                filename: row.get("filename"),
                tmp_path: row.get("tmp_path"),
                offset: row.get::<i64, _>("offset") as u64,
                total_bytes: row.get::<i64, _>("total_bytes") as u64,
                strategy: row.get("strategy"),
                expected_sha256: row.get("expected_sha256"),
                created_at: row.get("created_at"),
                last_chunk_at: row.get("last_chunk_at"),
                speed_bps: row.get::<Option<i64>, _>("speed_bps").map(|b| b.max(0) as u64),
            })
            .collect())
    }

    /// Delete every session idle since before `cutoff` and return the removed
    /// rows, so the caller can unlink each temp file.
    pub async fn prune_expired(&self, cutoff: i64) -> Result<Vec<StoredUploadSession>, sqlx::Error> {
        let rows = sqlx::query(
            "DELETE FROM upload_sessions WHERE last_chunk_at <= ?
             RETURNING session_id, username, dir_path, filename, tmp_path, offset, total_bytes,
                       strategy, expected_sha256, created_at, last_chunk_at, speed_bps",
        )
        .bind(cutoff)
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| StoredUploadSession {
                session_id: row.get("session_id"),
                username: row.get("username"),
                dir: row.get("dir_path"),
                filename: row.get("filename"),
                tmp_path: row.get("tmp_path"),
                offset: row.get::<i64, _>("offset") as u64,
                total_bytes: row.get::<i64, _>("total_bytes") as u64,
                strategy: row.get("strategy"),
                expected_sha256: row.get("expected_sha256"),
                created_at: row.get("created_at"),
                last_chunk_at: row.get("last_chunk_at"),
                speed_bps: row.get::<Option<i64>, _>("speed_bps").map(|b| b.max(0) as u64),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    //! Pure logic — no DB. Run: `cargo test upload_session::tests`
    use super::*;

    fn session(offset: u64, total: u64, last_chunk_at: i64) -> StoredUploadSession {
        StoredUploadSession {
            session_id: "s-1".into(),
            username: "alice".into(),
            dir: "/data/files".into(),
            filename: "movie.mkv".into(),
            tmp_path: "/data/files/.movie.mkv.hvupl-1".into(),
            offset,
            total_bytes: total,
            strategy: "overwrite".into(),
            expected_sha256: None,
            created_at: 1_000,
            last_chunk_at,
            speed_bps: None,
        }
    }

    #[test]
    fn test_is_expired_is_idle_based() {
        let s = session(10, 100, 1_000);
        assert!(!s.is_expired(1_000 + 1_799, 1_800), "still receiving chunks");
        assert!(s.is_expired(1_000 + 1_800, 1_800), "the boundary is expired");
        assert!(s.is_expired(99_999, 1_800));
    }

    #[test]
    fn test_is_expired_survives_a_clock_that_went_backwards() {
        // A clock correction must not expire a live upload.
        let s = session(10, 100, 2_000);
        assert!(!s.is_expired(1_000, 1_800));
    }

    #[test]
    fn test_progress_is_none_without_a_declared_size() {
        assert_eq!(session(0, 0, 1_000).progress(), None);
        assert_eq!(session(0, 100, 1_000).progress(), Some(0.0));
        assert_eq!(session(50, 100, 1_000).progress(), Some(50.0));
        assert_eq!(session(100, 100, 1_000).progress(), Some(100.0));
    }

    #[test]
    fn test_eta_seconds_from_the_last_chunk_throughput() {
        let mut s = session(1_000, 5_000, 1_000);
        assert_eq!(s.eta_seconds(), None, "no measurement yet");
        s.speed_bps = Some(1_000);
        assert_eq!(s.eta_seconds(), Some(4));
        s.speed_bps = Some(0);
        assert_eq!(s.eta_seconds(), None, "a stalled chunk is not an estimate");
        s.speed_bps = Some(1_000);
        s.offset = 5_000;
        assert_eq!(s.eta_seconds(), Some(0));
    }

    #[test]
    fn test_progress_never_exceeds_one_hundred() {
        // A client that declared 100 bytes and somehow wrote 150 must not make
        // the tray show "150 %".
        assert_eq!(session(150, 100, 1_000).progress(), Some(100.0));
    }
}
