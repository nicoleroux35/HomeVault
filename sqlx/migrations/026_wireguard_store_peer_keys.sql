-- Store the peer's private key and preshared key so the client config/QR
-- code can be re-displayed later (e.g. for adding another device to the same
-- phone, or reconnecting after the app was reinstalled).
-- The original design deliberately discarded these at creation time, but
-- real-world use showed that users often need to reconnect or add a second
-- device after the initial setup modal is dismissed.
-- The `private_key` and `preshared_key` are still never exposed through the
-- peer-list endpoint; only the dedicated GET /wireguard/peers/{id}/config
-- endpoint returns the full client config, and only to an authenticated admin.
ALTER TABLE vpn_wireguard_peers ADD COLUMN private_key TEXT NOT NULL DEFAULT '';
ALTER TABLE vpn_wireguard_peers ADD COLUMN preshared_key TEXT NOT NULL DEFAULT '';
