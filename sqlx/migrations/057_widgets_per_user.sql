-- Widgets become per-account (ROADMAP 15.1).
--
-- Before this migration a widget tile was identified by `id` alone and its
-- `username` was written as NULL (`handle_add_widget` hardcoded
-- `username: None`), so every tile belonged to nobody and was shown to every
-- account: any authenticated user could delete or reorder anyone's grid, and
-- two users could not each own a tile called "cpu".
--
-- The table is rebuilt with `(username, id)` as the identity, `username`
-- NOT NULL and `ON DELETE CASCADE`, so a tile always has an owner and an
-- account deletion removes its tiles with it.
--
-- Existing rows: rows that already name an owner keep it. Rows with
-- `username IS NULL` were a single grid shared by everybody; each existing
-- account gets its own copy instead of the rows being dropped, so no visible
-- tile disappears. On a database with no users yet (or no legacy rows) the
-- migration is a no-op beyond the schema change.

CREATE TABLE widgets_new (
    username TEXT NOT NULL,
    id TEXT NOT NULL,
    wtype TEXT NOT NULL,
    title TEXT NOT NULL,
    size TEXT NOT NULL,
    row_pos INTEGER NOT NULL DEFAULT 1,
    col_pos INTEGER NOT NULL DEFAULT 1,
    row_span INTEGER NOT NULL DEFAULT 1,
    col_span INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (username, id),
    FOREIGN KEY (username) REFERENCES users(username) ON DELETE CASCADE
);

-- Rows that already carry an owner keep their exact position and size. A row
-- naming a user that no longer exists would violate the new foreign key, so
-- it is skipped (it was unreachable anyway: nothing reads widgets by id
-- across accounts any more).
INSERT INTO widgets_new (username, id, wtype, title, size, row_pos, col_pos, row_span, col_span)
SELECT w.username, w.id, w.wtype, w.title, w.size, w.row_pos, w.col_pos, w.row_span, w.col_span
FROM widgets w
WHERE w.username IS NOT NULL
  AND w.username IN (SELECT username FROM users);

-- Legacy ownerless rows: give each existing account its own copy.
INSERT INTO widgets_new (username, id, wtype, title, size, row_pos, col_pos, row_span, col_span)
SELECT u.username, w.id, w.wtype, w.title, w.size, w.row_pos, w.col_pos, w.row_span, w.col_span
FROM widgets w
CROSS JOIN users u
WHERE w.username IS NULL;

DROP TABLE widgets;
ALTER TABLE widgets_new RENAME TO widgets;

-- The 001/005 indexes referenced the dropped table; recreate the ones the
-- scoped queries use (list one owner's grid in display order, and the
-- global-yet-bounded position lookup used by diagnostics).
CREATE INDEX IF NOT EXISTS idx_widgets_username ON widgets(username, row_pos, col_pos);
CREATE INDEX IF NOT EXISTS idx_widgets_position ON widgets(row_pos, col_pos);
