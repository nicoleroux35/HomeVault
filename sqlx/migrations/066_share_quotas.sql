-- Per-share size quotas (QUICK-WIN QW-07).
--
-- A share is derived from a config file (smb.conf, /etc/exports, …), so it has
-- no row of its own to hang a limit on. This table is the one place a
-- "this folder may not grow past N bytes" decision lives; `quota_bytes = 0`
-- means "no limit" and is stored as the absence of a row.
--
-- The enforcement itself belongs to the filesystem (ZFS `refquota`, Btrfs
-- qgroups, XFS project quotas), so the table holds only the intent: the backend
-- is detected at read time from where the share currently lives. A share that
-- moves to a filesystem without quota support reports the limit as "not
-- enforceable" instead of pretending.
CREATE TABLE IF NOT EXISTS share_quotas (
    share_name TEXT NOT NULL,
    -- `smb` | `nfs` | `ftp` | `webdav` | `sftp` — the same string `ShareType::as_str` prints.
    share_type TEXT NOT NULL,
    quota_bytes INTEGER NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (share_name, share_type)
);
