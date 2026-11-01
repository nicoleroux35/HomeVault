-- Optional password protection and atomic download accounting for public shares.
ALTER TABLE explorer_share_links ADD COLUMN password_hash TEXT;
ALTER TABLE explorer_share_links ADD COLUMN max_downloads INTEGER;
ALTER TABLE explorer_share_links ADD COLUMN download_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE explorer_share_links ADD COLUMN active_downloads INTEGER NOT NULL DEFAULT 0;
