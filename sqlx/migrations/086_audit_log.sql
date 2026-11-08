-- P1-18 — queryable audit trail.
--
-- Until now every application event went to tracing/syslog only: readable in
-- the System Journal, but not filterable by actor/action/severity and not
-- exportable. This table is the accountability record. It deliberately stores
-- no file contents and no request bodies — only the plain-English event
-- sentence plus the optional structured fields a caller supplies.
CREATE TABLE IF NOT EXISTS audit_log (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at TEXT NOT NULL,
    severity   TEXT NOT NULL CHECK (severity IN ('info', 'warning')),
    actor      TEXT,
    action     TEXT,
    target     TEXT,
    client_ip  TEXT,
    message    TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_audit_log_created_at
    ON audit_log (created_at DESC);
CREATE INDEX IF NOT EXISTS idx_audit_log_severity
    ON audit_log (severity);
CREATE INDEX IF NOT EXISTS idx_audit_log_actor
    ON audit_log (actor);
CREATE INDEX IF NOT EXISTS idx_audit_log_action
    ON audit_log (action);
