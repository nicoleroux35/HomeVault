-- Passkeys / WebAuthn (Phase 1 P1.3).
--
-- Two tables, one migration: the credentials an account holds, and the
-- short-lived challenges a ceremony is bound to. Both are keyed so that
-- neither can be replayed outside its own ceremony.
--
-- What is stored about a credential is exactly what verification needs and
-- nothing more: the public key (never a private one — a passkey's private half
-- never leaves the authenticator), the algorithm, the signature counter, and
-- the attestation format as evidence. The AAGUID is recorded as a label
-- ("which kind of authenticator"), never as an authorization decision.
CREATE TABLE IF NOT EXISTS passkey_credentials (
    -- base64url, no padding — the id the browser echoes back on every use.
    credential_id TEXT PRIMARY KEY,
    username TEXT NOT NULL,
    -- The human label the owner gave it ("MacBook Touch ID").
    label TEXT NOT NULL DEFAULT '',
    -- `es256` | `eddsa` (see `KeyAlgorithm` in src/passkey.rs).
    algorithm TEXT NOT NULL,
    -- base64url of the raw verification key: 65 bytes (0x04||X||Y) for ES256,
    -- 32 for EdDSA. A public key only.
    public_key TEXT NOT NULL,
    -- Last signature counter seen. `0` means the authenticator does not
    -- implement one (the spec exempts it from the monotonic check).
    sign_count INTEGER NOT NULL DEFAULT 0,
    -- Attestation format as received (`none`, `packed`, …). Evidence, not trust:
    -- HomeVault does not verify attestation chains, and the guide says so.
    attestation_fmt TEXT NOT NULL DEFAULT 'none',
    -- Hex AAGUID, or NULL when the authenticator sent none / zeros.
    aaguid TEXT,
    -- 1 when the ceremony reported the user was verified (PIN/biometric).
    user_verified INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    last_used_at TEXT,
    FOREIGN KEY (username) REFERENCES users(username) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_passkey_credentials_username
    ON passkey_credentials(username);

-- One outstanding ceremony per (user, ceremony): starting a new registration
-- invalidates a half-finished one, so a challenge can never pile up.
CREATE TABLE IF NOT EXISTS passkey_challenges (
    username TEXT NOT NULL,
    -- `registration` | `authentication`
    ceremony TEXT NOT NULL,
    -- The challenge itself (base64url). Server-generated, single use.
    challenge TEXT NOT NULL,
    created_at TEXT NOT NULL,
    -- RFC 3339. Past it the challenge is swept, whichever comes first.
    expires_at TEXT NOT NULL,
    PRIMARY KEY (username, ceremony)
);
