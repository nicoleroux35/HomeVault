-- iSCSI target management (`docs/guides/iscsi.md`, phase P3.3).
--
-- The metadata HomeVault owns, and nothing about the kernel side: which
-- backstores exist, which targets export them, on which portals, through which
-- LUNs, to which initiators. Applying it writes `targetcli` objects; dropping
-- a row only forgets the metadata.
--
-- Two rules are structural here:
--
--   * a backstore has no `ON DELETE CASCADE` from any consumer, so a LUN can
--     never leave a backstore row dangling (the repository refuses to forget a
--     backstore that a LUN still uses);
--   * a target owns its portals, LUNs and ACLs — deleting the target removes
--     them in one statement, which is what makes "forget this export" atomic.
--
-- CHAP secrets are sealed with the appliance key (`hv1:` blobs, see
-- src/storage/secret_box.rs) and are never returned by the API: the payload
-- carries `has_secret` only.
CREATE TABLE IF NOT EXISTS iscsi_backstores (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Object name in `targetcli` (`/backstores/fileio/<name>`): a conservative
    -- charset, enforced in `iscsi::valid_backstore_name`.
    name TEXT NOT NULL,
    -- 'fileio' (a file HomeVault sizes) | 'block' (an existing device).
    kind TEXT NOT NULL,
    -- Absolute path: the backing file for `fileio`, the device for `block`.
    path TEXT NOT NULL,
    -- Declared size in bytes for `fileio`; 0 for `block` (the device decides).
    size_bytes INTEGER NOT NULL DEFAULT 0,
    readonly INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    -- A CHAP secret is per initiator, never per backstore.
    UNIQUE (path)
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_iscsi_backstores_name
    ON iscsi_backstores(name COLLATE NOCASE);

CREATE TABLE IF NOT EXISTS iscsi_targets (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- The target's IQN: simultaneously its `targetcli` object name, so it is
    -- unique at the database level and validated before it can be created.
    iqn TEXT NOT NULL UNIQUE,
    -- 0 keeps the metadata without exporting it; apply only touches enabled
    -- targets (and tears down the ones that were just disabled).
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS iscsi_portals (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    target_id INTEGER NOT NULL,
    -- An IP literal (IPv4 or IPv6). A host name would need DNS at apply time.
    address TEXT NOT NULL,
    port INTEGER NOT NULL DEFAULT 3260,
    FOREIGN KEY (target_id) REFERENCES iscsi_targets(id) ON DELETE CASCADE,
    -- One portal cannot be listed twice on the same target.
    UNIQUE (target_id, address, port)
);

CREATE INDEX IF NOT EXISTS idx_iscsi_portals_target
    ON iscsi_portals(target_id);

CREATE TABLE IF NOT EXISTS iscsi_luns (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    target_id INTEGER NOT NULL,
    -- The LUN number the initiator sees (0–16383, the iSCSI 14-bit field).
    lun INTEGER NOT NULL,
    backstore_id INTEGER NOT NULL,
    FOREIGN KEY (target_id) REFERENCES iscsi_targets(id) ON DELETE CASCADE,
    -- Deliberately not `ON DELETE CASCADE`: forgetting a backstore must be
    -- refused while a LUN still exports it, not silently unexport it.
    FOREIGN KEY (backstore_id) REFERENCES iscsi_backstores(id),
    UNIQUE (target_id, lun)
);

CREATE INDEX IF NOT EXISTS idx_iscsi_luns_target
    ON iscsi_luns(target_id);

CREATE INDEX IF NOT EXISTS idx_iscsi_luns_backstore
    ON iscsi_luns(backstore_id);

CREATE TABLE IF NOT EXISTS iscsi_acls (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    target_id INTEGER NOT NULL,
    -- The initiator's IQN. Case-insensitive uniqueness: IQNs are compared
    -- lower-cased everywhere, so two spellings would be one initiator.
    initiator_iqn TEXT NOT NULL,
    -- CHAP userid, or NULL for an initiator that uses no authentication.
    userid TEXT,
    -- Sealed CHAP secret (`hv1:` blob), never the plaintext.
    chap_secret_sealed TEXT,
    created_at TEXT NOT NULL,
    FOREIGN KEY (target_id) REFERENCES iscsi_targets(id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_iscsi_acls_target_initiator
    ON iscsi_acls(target_id, initiator_iqn COLLATE NOCASE);
