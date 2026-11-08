-- Folder share links and targeted internal links.
--
-- `is_directory` + `archive_path` let a link point at a folder: the archive is
-- built once (a snapshot at link creation) under the appliance's share-archive
-- directory and served like any other file, so ranges, digests and the
-- download cap keep working. `allowed_users` / `allowed_groups` are
-- comma-separated names; empty means "any signed-in account" (the historical
-- `signed_in` behaviour).
ALTER TABLE explorer_share_links ADD COLUMN is_directory INTEGER NOT NULL DEFAULT 0;
ALTER TABLE explorer_share_links ADD COLUMN archive_path TEXT;
ALTER TABLE explorer_share_links ADD COLUMN allowed_users TEXT NOT NULL DEFAULT '';
ALTER TABLE explorer_share_links ADD COLUMN allowed_groups TEXT NOT NULL DEFAULT '';
