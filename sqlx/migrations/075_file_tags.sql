-- User-owned labels on Explorer paths. Tags are scoped to the owner so one
-- account cannot infer another account's private folders from labels.
CREATE TABLE IF NOT EXISTS explorer_file_tags (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    owner TEXT NOT NULL,
    path TEXT NOT NULL,
    tag TEXT NOT NULL COLLATE NOCASE,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE(owner, path, tag)
);
CREATE INDEX IF NOT EXISTS idx_explorer_file_tags_owner_path
    ON explorer_file_tags(owner, path);
CREATE INDEX IF NOT EXISTS idx_explorer_file_tags_owner_tag
    ON explorer_file_tags(owner, tag);
