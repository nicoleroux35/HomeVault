-- ---------------------------------------------------------------------------
-- 072_directory.sql — LDAP/AD directory principals (ROADMAP P3.1, stage 1)
--
-- `users.auth_source` tells a local account (the default, and the only kind
-- that can be given a password) from an account HomeVault mirrors from a
-- directory. The mirror exists because `sessions.username` and a dozen other
-- tables have a foreign key to `users`; the directory row below stays the
-- source of truth for *who* the principal is, and a mirrored account can never
-- be signed into with a local password.
-- ---------------------------------------------------------------------------

ALTER TABLE users ADD COLUMN auth_source TEXT NOT NULL DEFAULT 'local';

CREATE INDEX IF NOT EXISTS idx_users_auth_source ON users(auth_source);

-- One row per principal seen from an external identity provider. `subject` is
-- the provider's own stable key: the DN for LDAP, `sub` for OIDC, the NameID
-- for SAML. Keeping it separate from `username` is what makes a rename in the
-- directory a rename here rather than a new account.
CREATE TABLE IF NOT EXISTS remote_identities (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    provider      TEXT    NOT NULL,
    subject       TEXT    NOT NULL,
    username      TEXT    NOT NULL UNIQUE,
    display_name  TEXT    NOT NULL DEFAULT '',
    email         TEXT    NOT NULL DEFAULT '',
    -- The group list exactly as the provider returned it (JSON array), kept so
    -- a settings change can re-resolve every principal's role without asking
    -- the directory again.
    groups        TEXT    NOT NULL DEFAULT '[]',
    role_id       INTEGER NOT NULL DEFAULT 1,
    disabled      INTEGER NOT NULL DEFAULT 0,
    last_login_at TEXT,
    created_at    TEXT    NOT NULL,
    updated_at    TEXT    NOT NULL,
    UNIQUE (provider, subject)
);

CREATE INDEX IF NOT EXISTS idx_remote_identities_role ON remote_identities(role_id);
CREATE INDEX IF NOT EXISTS idx_remote_identities_disabled ON remote_identities(disabled);
