-- Per-share snapshot schedules (tasklist P1-12).
--
-- A schedule keeps targeting a pool, and may additionally name one share
-- (`share` + `share_type`). When it does, the run snapshots the dataset /
-- subvolume that contains that share instead of the pool root, and lists /
-- prunes only what that backend covers. The pair is stored, not the share's
-- path, so a later path change is picked up by the next run instead of
-- snapshotting a stale location.
--
-- NULL/'' in both columns is the historical pool-wide schedule. A row must
-- carry both columns or neither; the HTTP layer enforces that before writing.
ALTER TABLE snapshot_schedules ADD COLUMN share TEXT;
ALTER TABLE snapshot_schedules ADD COLUMN share_type TEXT;
