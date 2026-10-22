-- Migration: 053_snapshot_schedule_unique_prefix.sql
-- One schedule per (pool, prefix) — the rule 046 already documented but never
-- enforced (ROADMAP 5).
--
-- Two schedules sharing a pool and a prefix both list and prune the same
-- snapshots: each run deletes what the other just decided to keep, and the
-- UI shows two rows claiming to own the same history. The handler now also
-- refuses a duplicate with a 409; this index makes the rule hold even for a
-- row inserted outside the API (a restore, a manual SQL fix).

-- Deduplicate first: the oldest row of each pair keeps its history and the
-- newest duplicates go. Deleting the newer rows is the safe direction —
-- their cron/retention is the redundant configuration, not the original.
DELETE FROM snapshot_schedules
WHERE rowid NOT IN (
    SELECT MIN(rowid) FROM snapshot_schedules GROUP BY pool, prefix
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_snapshot_schedules_pool_prefix
    ON snapshot_schedules(pool, prefix);
