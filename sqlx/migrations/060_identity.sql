-- Migration: 060_identity.sql
-- Sign-in continuity for HomeVault: remember a browser so two-factor
-- authentication is not asked again every day, and give an account a verified
-- rescue address that can receive recovery codes.
--
-- Both tables are additive. An existing account has no trusted device and no
-- rescue address, which is exactly the old behaviour: every sign-in asks for
-- the second factor, and recovery stays on the offline recovery code.

CREATE TABLE IF NOT EXISTS trusted_devices (
    -- Opaque identifier handed to the client, so one device can be revoked
    -- without invalidating the others.
    id TEXT PRIMARY KEY,
    username TEXT NOT NULL,
    -- What the admin sees: a short, human label derived from the user agent.
    label TEXT NOT NULL,
    -- SHA-256 of the cookie value, never the value itself: a database copy
    -- must not be usable to skip a second factor.
    token_hash TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    FOREIGN KEY (username) REFERENCES users(username) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_trusted_devices_username ON trusted_devices(username);

-- One outstanding e-mail code per account. `purpose` separates "prove you own
-- this rescue address" from "recover this account", so a code issued for one
-- cannot be replayed against the other.
CREATE TABLE IF NOT EXISTS rescue_email_codes (
    username TEXT PRIMARY KEY,
    email TEXT NOT NULL,
    purpose TEXT NOT NULL,
    code_hash TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    FOREIGN KEY (username) REFERENCES users(username) ON DELETE CASCADE
);

-- The verified address itself, on the account. Null means "no rescue address":
-- recovery then relies on the offline recovery code, as before.
ALTER TABLE users ADD COLUMN rescue_email TEXT;
ALTER TABLE users ADD COLUMN rescue_email_verified_at TEXT;
