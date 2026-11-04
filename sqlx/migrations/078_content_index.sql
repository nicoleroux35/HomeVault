-- Full-text content index (phase D1, docs/guides/content-index.md).
--
-- One appliance-wide index of text files under administrator-chosen roots.
-- Results are filtered per caller by the Explorer's own roots, so a shared
-- index never grants access. The FTS5 table is an external-content index over
-- `content_index_docs`, kept in sync by triggers; deleting a document removes
-- its postings in the same statement.
--
-- `last_seen` drives deletion (a root walked completely drops documents that
-- were not seen) and retention (a root that stayed unreadable ages out).
-- `modified`/`size_bytes` let a run skip a file whose bytes did not change.
CREATE TABLE IF NOT EXISTS content_index_docs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT NOT NULL UNIQUE,
    root TEXT NOT NULL,
    name TEXT NOT NULL,
    body TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    -- Unix seconds; 0 when the filesystem would not say.
    modified INTEGER NOT NULL DEFAULT 0,
    last_seen TEXT NOT NULL,
    indexed_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_content_index_docs_root
    ON content_index_docs(root, last_seen);
CREATE INDEX IF NOT EXISTS idx_content_index_docs_seen
    ON content_index_docs(last_seen);

CREATE VIRTUAL TABLE IF NOT EXISTS content_index_fts USING fts5(
    name,
    body,
    content='content_index_docs',
    content_rowid='id',
    tokenize='unicode61 remove_diacritics 2'
);

CREATE TRIGGER IF NOT EXISTS content_index_docs_ai
AFTER INSERT ON content_index_docs BEGIN
    INSERT INTO content_index_fts(rowid, name, body)
    VALUES (new.id, new.name, new.body);
END;

CREATE TRIGGER IF NOT EXISTS content_index_docs_ad
AFTER DELETE ON content_index_docs BEGIN
    INSERT INTO content_index_fts(content_index_fts, rowid, name, body)
    VALUES ('delete', old.id, old.name, old.body);
END;

CREATE TRIGGER IF NOT EXISTS content_index_docs_au
AFTER UPDATE OF name, body ON content_index_docs BEGIN
    INSERT INTO content_index_fts(content_index_fts, rowid, name, body)
    VALUES ('delete', old.id, old.name, old.body);
    INSERT INTO content_index_fts(rowid, name, body)
    VALUES (new.id, new.name, new.body);
END;
