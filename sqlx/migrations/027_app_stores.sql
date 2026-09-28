-- Community app stores for the Docker one-click catalog. Each row is a git
-- repository the admin has added as an extra source of app packages (a
-- directory per app, each holding a `homevault-app.toml` manifest plus a
-- `docker-compose.yml` template). The built-in "homevault" store is NOT
-- represented here - it is embedded in the binary and always present; this
-- table only holds the extra ones. `git_url` is cloned/pulled into
-- `apps_dir()/stores/<id>/` by the sync handler. `last_error` holds the
-- last sync failure message (NULL when the last sync succeeded), surfaced
-- in the catalog UI so a bad URL / unreachable host is visible.
CREATE TABLE IF NOT EXISTS app_stores (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    git_url TEXT NOT NULL UNIQUE,
    enabled INTEGER NOT NULL DEFAULT 1,
    last_synced_at INTEGER,
    last_error TEXT,
    created_at TEXT NOT NULL,
    created_by TEXT NOT NULL
);
