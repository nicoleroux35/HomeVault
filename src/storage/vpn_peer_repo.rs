//! Storage layer for the `vpn_wireguard_peers` registry.
//! Stores peer metadata (name, IP, public key) plus the private key and
//! preshared key needed to regenerate the client config / QR code on demand.

use sqlx::SqlitePool;
use std::sync::Arc;

#[derive(Debug, Clone, serde::Serialize)]
pub struct VpnWireGuardPeer {
    pub id: String,
    pub name: String,
    pub public_key: String,
    pub private_key: String,
    pub preshared_key: String,
    pub peer_ip: String,
    pub created_at: String,
    pub created_by: String,
}

#[derive(sqlx::FromRow)]
struct VpnWireGuardPeerRow {
    id: String,
    name: String,
    public_key: String,
    private_key: String,
    preshared_key: String,
    peer_ip: String,
    created_at: String,
    created_by: String,
}

impl From<VpnWireGuardPeerRow> for VpnWireGuardPeer {
    fn from(r: VpnWireGuardPeerRow) -> Self {
        Self {
            id: r.id,
            name: r.name,
            public_key: r.public_key,
            private_key: r.private_key,
            preshared_key: r.preshared_key,
            peer_ip: r.peer_ip,
            created_at: r.created_at,
            created_by: r.created_by,
        }
    }
}

pub struct VpnPeerRepository {
    pool: Arc<SqlitePool>,
}

impl VpnPeerRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    pub async fn list(&self) -> Result<Vec<VpnWireGuardPeer>, sqlx::Error> {
        let rows = sqlx::query_as::<_, VpnWireGuardPeerRow>(
            "SELECT id, name, public_key, private_key, preshared_key, peer_ip, created_at, created_by
             FROM vpn_wireguard_peers ORDER BY created_at ASC",
        )
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn insert(&self, peer: &VpnWireGuardPeer) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO vpn_wireguard_peers (id, name, public_key, private_key, preshared_key, peer_ip, created_at, created_by)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&peer.id)
        .bind(&peer.name)
        .bind(&peer.public_key)
        .bind(&peer.private_key)
        .bind(&peer.preshared_key)
        .bind(&peer.peer_ip)
        .bind(&peer.created_at)
        .bind(&peer.created_by)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    pub async fn find_by_id(&self, id: &str) -> Result<Option<VpnWireGuardPeer>, sqlx::Error> {
        let row = sqlx::query_as::<_, VpnWireGuardPeerRow>(
            "SELECT id, name, public_key, private_key, preshared_key, peer_ip, created_at, created_by
             FROM vpn_wireguard_peers WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.map(Into::into))
    }

    pub async fn delete(&self, id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM vpn_wireguard_peers WHERE id = ?")
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }
}
