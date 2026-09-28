-- Registry of provisioned WireGuard peers (roadwarrior clients: phones,
-- laptops, etc.) for the single, always-on `wg0` server interface. Only the
-- peer's public key is stored here - its private key and preshared key are
-- generated at creation time, returned once in the API response for the
-- client config/QR code, and never written to disk or the database, so a
-- later compromise of this DB cannot be used to reconstruct a device's
-- tunnel credentials.
CREATE TABLE IF NOT EXISTS vpn_wireguard_peers (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    public_key TEXT NOT NULL UNIQUE,
    peer_ip TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    created_by TEXT NOT NULL
);
