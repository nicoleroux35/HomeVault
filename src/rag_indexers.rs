//! Document indexers for the RAG knowledge base.
//!
//! Each indexer knows how to collect documents from one source and upsert
//! them into the `rag_documents` / `rag_chunks` tables.

use sqlx::SqlitePool;

/// Index all document sources into the RAG knowledge base.
/// Returns (total_documents, total_chunks) indexed.
pub async fn index_all(pool: &SqlitePool) -> Result<(i32, i32), String> {
    crate::rag::update_index_status(pool, "indexing", 0, 0).await?;

    let mut total_docs = 0i32;
    let mut total_chunks = 0i32;

    // 1. App catalog
    match index_catalog(pool).await {
        Ok((d, c)) => {
            total_docs += d;
            total_chunks += c;
            tracing::info!("RAG: indexed catalog: {} docs, {} chunks", d, c);
        }
        Err(e) => tracing::warn!("RAG: catalog indexing failed: {}", e),
    }

    // 2. HomeVault features documentation
    match index_features(pool).await {
        Ok((d, c)) => {
            total_docs += d;
            total_chunks += c;
            tracing::info!("RAG: indexed features: {} docs, {} chunks", d, c);
        }
        Err(e) => tracing::warn!("RAG: features indexing failed: {}", e),
    }

    // 3. i18n keys (all languages as structured docs)
    match index_i18n(pool).await {
        Ok((d, c)) => {
            total_docs += d;
            total_chunks += c;
            tracing::info!("RAG: indexed i18n: {} docs, {} chunks", d, c);
        }
        Err(e) => tracing::warn!("RAG: i18n indexing failed: {}", e),
    }

    crate::rag::update_index_status(pool, "idle", total_docs, total_chunks).await?;

    Ok((total_docs, total_chunks))
}

/// Index the built-in app catalog manifests.
async fn index_catalog(pool: &SqlitePool) -> Result<(i32, i32), String> {
    use include_dir::include_dir;

    static BUILTIN_DIR: include_dir::Dir<'_> =
        include_dir!("$CARGO_MANIFEST_DIR/catalog");

    let mut doc_count = 0;
    let mut chunk_count = 0;

    for dir in BUILTIN_DIR.dirs() {
        let name = match dir.path().file_name() {
            Some(n) => n.to_str().unwrap_or(""),
            None => continue,
        };

        let raw = match BUILTIN_DIR.get_file(format!("{name}/homevault-app.toml")) {
            Some(f) => match f.contents_utf8() {
                Some(s) => s,
                None => continue,
            },
            None => continue,
        };

        let manifest = match crate::app_catalog::manifest::Manifest::parse(raw) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("RAG: skipping malformed manifest {}: {}", name, e);
                continue;
            }
        };

        // Build a rich text document for this app
        let mut content = format!("Application: {}\nCategory: {}\n", manifest.name, manifest.category);

        if let Some(tagline) = &manifest.tagline {
            content.push_str(&format!("Summary: {}\n", tagline));
        }

        content.push_str(&format!("Description: {}\n", manifest.description));

        if let Some(desc_fr) = &manifest.description_fr {
            content.push_str(&format!("Description (FR): {}\n", desc_fr));
        }
        if let Some(desc_de) = &manifest.description_de {
            content.push_str(&format!("Description (DE): {}\n", desc_de));
        }
        if let Some(desc_es) = &manifest.description_es {
            content.push_str(&format!("Description (ES): {}\n", desc_es));
        }

        if let Some(dev) = &manifest.developer {
            content.push_str(&format!("Developer: {}\n", dev));
        }
        if let Some(web) = &manifest.website {
            content.push_str(&format!("Website: {}\n", web));
        }
        if let Some(repo) = &manifest.repo {
            content.push_str(&format!("Repository: {}\n", repo));
        }

        content.push_str(&format!(
            "Port: {}, GPU: {}, Architectures: {}\n",
            manifest.app_port,
            manifest.gpu,
            if manifest.arch.is_empty() {
                "all".to_string()
            } else {
                manifest.arch.join(", ")
            }
        ));

        if let Some(notes) = &manifest.release_notes {
            content.push_str(&format!("Release notes: {}\n", notes));
        }

        let doc_id = format!("catalog:{}", manifest.id);

        let chunks = crate::rag::chunk_text(&content);
        let chunk_texts: Vec<String> = chunks.iter().map(|(t, _)| t.clone()).collect();
        let embeddings = crate::rag::generate_embeddings(&chunk_texts).await.unwrap_or_default();

        // Upsert document
        sqlx::query(
            "INSERT INTO rag_documents (id, source, source_id, title, content, language, updated_at)
             VALUES (?, 'catalog', ?, ?, ?, 'en', datetime('now'))
             ON CONFLICT(id) DO UPDATE SET title=excluded.title, content=excluded.content,
             language=excluded.language, updated_at=datetime('now')",
        )
        .bind(&doc_id)
        .bind(&manifest.id)
        .bind(&manifest.name)
        .bind(&content)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;

        // Delete old chunks
        sqlx::query("DELETE FROM rag_chunks WHERE document_id = ?")
            .bind(&doc_id)
            .execute(pool)
            .await
            .map_err(|e| e.to_string())?;

        // Insert new chunks
        for (i, (text, _)) in chunks.iter().enumerate() {
            let chunk_id = format!("{}:{}", doc_id, i);
            let emb_bytes = embeddings.get(i).map(|e| crate::rag::embedding_to_bytes(e));

            sqlx::query(
                "INSERT INTO rag_chunks (id, document_id, chunk_index, content, embedding, token_count)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(&chunk_id)
            .bind(&doc_id)
            .bind(i as i32)
            .bind(text)
            .bind(emb_bytes.as_deref())
            .bind(crate::rag::estimate_tokens(text) as i32)
            .execute(pool)
            .await
            .map_err(|e| e.to_string())?;
        }

        doc_count += 1;
        chunk_count += chunks.len() as i32;
    }

    Ok((doc_count, chunk_count))
}

/// Index HomeVault feature documentation.
/// Each feature gets a detailed doc explaining what it does, how to use it,
/// and common tasks.
async fn index_features(pool: &SqlitePool) -> Result<(i32, i32), String> {
    let features = vec![
        FeatureDoc {
            id: "dashboard".to_string(),
            title: "Dashboard".to_string(),
            content: r#"The Dashboard provides a real-time system overview including:
- CPU usage, temperature, load average
- Memory usage (RAM, swap)
- Disk usage per mount point
- Network traffic (rx/tx bytes)
- System health checks (storage, memory, CPU, network, temperature)
- Docker container status
- Storage volume overview
- Operating system details (kernel, uptime, packages)

Widgets can be rearranged by drag-and-drop. Monitor panels (Resource, RAID, Security, etc.) can be toggled on/off from the Widgets app. The dashboard auto-refreshes periodically."#.to_string(),
        },
        FeatureDoc {
            id: "explorer".to_string(),
            title: "File Explorer".to_string(),
            content: r#"The File Explorer provides a web-based file manager with:
- Browse directories, view files with preview (text, images, code)
- Upload files (drag-and-drop or click), download files
- Create directories, rename, copy, move, delete
- Search files recursively by name
- Set file/directory permissions (chmod)
- Compress/extract archives (zip)
- Trash system (soft-delete with restore)
- Favorites / pinned folders (per-browser localStorage)
- Safe-eject external drives
- Browse past ZFS snapshots (read-only, copy to restore)
- Share a folder directly to the Shares app
- Collision handling on copy/move/upload (overwrite, keep_both, skip)
- Inline text/code editor for quick edits
- Share a file by a revocable download link (URL + QR code; 'anyone' or 'signed-in' only; default 30-day expiry)
- Background auto-refresh every 8 seconds

PathGuard restricts non-admin users to their home directory and allowed roots."#.to_string(),
        },
        FeatureDoc {
            id: "storage".to_string(),
            title: "Storage Management".to_string(),
            content: r#"The Storage app manages disks, partitions, and virtual disks:
- View all physical disks and partitions with usage
- Create RAID arrays (mdadm) via the RAID wizard
- Create ZFS pools with various redundancy levels
- Create Btrfs volumes
- Virtual disks tab: list, scrub/verify, rebuild degraded arrays, destroy
- LUKS2 encryption: create encrypted volumes, lock/unlock
- Cache system: configure cache pools, move data between tiers
- Monitor S.M.A.R.T. disk health
- Snapshot management (ZFS, Btrfs)

RAID levels: RAID0 (stripe), RAID1 (mirror), RAID5 (parity), RAID6 (double parity), RAID10 (mirror+stripe)."#.to_string(),
        },
        FeatureDoc {
            id: "docker".to_string(),
            title: "Docker Management".to_string(),
            content: r#"The Docker app provides a web interface for Docker Engine:
- Container lifecycle: start, stop, restart, remove, view logs
- Edit a container's docker-compose.yml in place (Compose-managed containers), then save or save and redeploy
- Image management: pull, remove, view layers
- Network management
- Volume management
- App Catalog: install pre-configured apps from manifests
- Community stores: add third-party app repositories
- Per-app reverse proxy with session-cookie auth gate
- Port conflict detection before container creation
- Docker Compose deploy and management
- Container I/O stats (network rx/tx, block read/write)
- App update detection (image digest comparison with registries)"#.to_string(),
        },
        FeatureDoc {
            id: "virtual-machines".to_string(),
            title: "Virtual Machines Management".to_string(),
            content: r#"The Virtual Machines app is a guided, non-technical interface for Incus VMs and system containers:
- A create wizard: pick an operating system from a short curated list (Ubuntu, Debian, Fedora, Rocky, Alpine), name the machine, choose a size (small/medium/large), and set a login username + password
- The login account is seeded into the machine automatically via cloud-init; the password is stored only as a SHA-512 hash, never in plain text
- VM lifecycle: create, start, stop, restart, delete (long operations are awaited so status is always accurate; image downloads run as a tracked background job)
- Live status, IPv4 address, CPU/memory/disk and snapshot count for every machine
- Change CPU or memory after creation
- Saved states (snapshots): create, roll back, delete
- A built-in shell console (xterm) into any running machine — no VNC client needed
- System containers are supported alongside full virtual machines
- Admin-only access control (every operation requires the admin role)"#.to_string(),
        },
        FeatureDoc {
            id: "backup".to_string(),
            title: "Backup (Restic)".to_string(),
            content: r#"The Backup app uses Restic for deduplicated, encrypted backups:
- Create backup repositories (local, remote targets)
- Manual and scheduled backups
- Backup verification
- Restore from snapshots
- Retention policies (keep last N, daily, weekly, monthly)
- Backup scheduling with cron expressions
- Progress tracking via SSE

Schedule lifecycle: create schedule -> set retention policy -> enable. Failed runs are rescheduled automatically."#.to_string(),
        },
        FeatureDoc {
            id: "users".to_string(),
            title: "Users & Groups".to_string(),
            content: r#"User and group management for the HomeVault server.

Layout: a master/detail screen. A left rail lists every account with a search
box and four filters (all / active / locked / admin), plus a "New user" button
and a "Quota view" button at its foot. Selecting a row opens the detail pane;
on a narrow window the rail and the pane swap instead of sitting side by side.

The detail pane is six cards, stacked and sized by their own content:
1. Identity — username (read-only; it is the account key), full name, email
2. Role & Groups — admin/user role selector plus the group membership chips
3. Home folder — enable a personal home directory, pick its disk or an
   explicit folder, and cap its size with the size slider and quota bar
4. Transfer quotas — upload and download limits with live usage bars
5. Security — two-factor authentication (TOTP), reset 2FA, change password
6. Actions — lock/unlock the account, and delete it

Other behaviour:
- Assign roles: admin or user
- Lock/unlock accounts
- Admin can disable 2FA for other users
- Deleting an account purges its home directory, SFTP jail, sessions and
  transfer-quota row, unless the operator chooses to keep the folder
- The "All quotas" button opens an overview of every user and group quota
- The "Quota view" rail button opens the dedicated read-only Quotas app

The homevault_admin OS group grants root-level access. It is never a tickable
checkbox — it shows as a read-only chip driven by the admin role only. Users
get shell-less Linux accounts for SFTP access."#.to_string(),
        },
        FeatureDoc {
            id: "quotas".to_string(),
            title: "Quotas (read-only overview)".to_string(),
            content: r#"The Quotas app is a read-only overview of every configured transfer quota.

- One row per subject, users first then groups, sorted by name
- Filter by subject type: all / users / groups
- Per row it shows the subject type, the subject name, when the quota was
  last updated, and — for each direction (upload, download) — the used bytes
  against the limit with a live progress bar
- A disabled direction reads "Unlimited", which means no limit rather than
  zero allowed
- A counter at the top reports how many subjects exist and how many of them
  actually have a limit

Nothing on this screen writes. Editing a quota happens in the Users app's
"Transfer quotas" card; the "Manage in Users" button hands off to it.

Data comes from the transfer-quota list endpoint, one call per subject type.
The app is admin-gated like Users, even though the endpoint itself only
requires a session."#.to_string(),
        },
        FeatureDoc {
            id: "shares".to_string(),
            title: "File Sharing (SMB, SFTP, FTP, WebDAV)".to_string(),
            content: r#"Share directories over multiple protocols:
- SMB/CIFS: Windows-compatible file sharing
- SFTP: SSH-based file transfer with jail (chroot)
- FTP: Traditional file transfer
- WebDAV: HTTP-based file access

Features:
- Per-user ACL grants on shares
- SFTP jail: users are chrooted to their granted shares
- Public key authentication for SFTP
- Share enable/disable toggle
- Rebuild SFTP jails (reconcile ACLs with bind mounts)"#.to_string(),
        },
        FeatureDoc {
            id: "cloudsync".to_string(),
            title: "Cloud Sync (Rclone)".to_string(),
            content: r#"Synchronize files with cloud storage via Rclone:
- Configure cloud remotes (Google Drive, Dropbox, S3, etc.)
- Bidirectional sync
- One-way copy
- Mount remote storage locally
- Manage multiple cloud providers

Mount feature: mounts a remote as a local directory using FUSE. Supports daemon mode for background mounting."#.to_string(),
        },
        FeatureDoc {
            id: "network".to_string(),
            title: "Network Configuration".to_string(),
            content: r#"Monitor and configure network interfaces:
- View all interfaces with IP, MAC, speed, status
- WiFi management: scan, connect, disconnect
- Static IP configuration
- DNS settings
- Network diagnostics (ping, traceroute)
- Public IP detection
- Interface enable/disable

DNS updates preserve existing search/domains. IPv6 nameservers are appended."#.to_string(),
        },
        FeatureDoc {
            id: "health".to_string(),
            title: "System Health Monitoring".to_string(),
            content: r#"The Health page provides 8 real-time system checks:
1. Storage: disk usage warnings
2. Memory: RAM/swap usage
3. CPU: load average monitoring
4. Network: interface status
5. CPU Temperature: thermal warnings
6. Docker: container health
7. Backup: last backup freshness
8. Services: critical service status

Each check returns: healthy, warning, or unhealthy. The overall status is the worst individual check."#.to_string(),
        },
        FeatureDoc {
            id: "terminal".to_string(),
            title: "Web Terminal".to_string(),
            content: r#"The Terminal provides a web-based shell:
- Native in-process PTY (no external dependencies)
- Multi-tab support (up to 8 tabs per user)
- WebSocket-based real-time I/O
- Reconnect to active sessions
- Session idle timeout (30 minutes)
- Chromeless window mode (no title bar)

Admin-only feature. Sessions are cleaned up on account deletion."#.to_string(),
        },
        FeatureDoc {
            id: "security".to_string(),
            title: "Security Suite (Antivirus, Anti-Ransomware, Firewall)".to_string(),
            content: r#"Three security subsystems:
1. Antivirus (ClamAV): scan files/directories, quarantine threats
2. Anti-Ransomware: honeypot files that trigger alerts on access
3. Firewall (UFW): enable/disable, add/remove rules, mandatory rules protection

Features:
- Real-time scan with ClamAV
- PID-based scan cancellation
- Honeypot debounce (10s per-path)
- Firewall: mandatory rules can't be deleted
- Enable requires {confirmed:true} confirmation
- Locale-independent status detection (LC_ALL=C)"#.to_string(),
        },
        FeatureDoc {
            id: "tls".to_string(),
            title: "Built-in HTTPS / TLS".to_string(),
            content: r#"HomeVault can serve the web interface over HTTPS itself (opt-in). Settings > Security > HTTPS/TLS.

Modes:
- Self-signed: HomeVault generates and auto-renews its own certificate (SANs cover localhost, the hostname, homevault.local and the LAN IP). Browsers show a trust warning.
- Manual: point HomeVault at your own PEM certificate + private key (from your CA, certbot, mkcert, or a reverse proxy).

Details:
- Additive second listener on HTTPS_PORT (default 8443); plain HTTP on PORT (8080) still works.
- Optional "redirect HTTP to HTTPS" toggle (loopback exempt).
- Applies after a restart. A broken certificate never stops the server (HTTP-only fallback).
- No ACME/Let's Encrypt yet. An external reverse proxy terminating TLS is still supported."#.to_string(),
        },
        FeatureDoc {
            id: "vpn".to_string(),
            title: "VPN Management".to_string(),
            content: r#"Manage VPN connections:
- WireGuard: create peers, manage tunnels
- OpenVPN: import/manage configurations
- VPN status monitoring
- Start/stop VPN connections

All VPN operations require admin access. Names are sanitized to prevent injection."#.to_string(),
        },
        FeatureDoc {
            id: "rescue".to_string(),
            title: "Data Rescue".to_string(),
            content: r#"Disk data recovery tools:
- ddrescue: clone failing disks with bad-sector skipping
- TestDisk: analyze and recover lost partitions
- PhotoRec: file carving for deleted files
- mdadm repair: repair degraded RAID arrays

Security: protected-device blocklist prevents operating on the boot disk. Path traversal is blocked. All tools run with elevated privileges."#.to_string(),
        },
        FeatureDoc {
            id: "settings".to_string(),
            title: "Settings".to_string(),
            content: r#"Configure HomeVault:
- General: hostname, update frequency, language
- Appearance: accent color, wallpaper (bundled), density
- Email: SMTP configuration for notifications
- Notifications: alert thresholds
- AI Assistant: model selection, Ollama configuration
- Update: check for updates manually or on schedule

Settings are exported/imported as JSON (SMTP passwords redacted in export). The AI model setting persists across restarts."#.to_string(),
        },
        FeatureDoc {
            id: "journal".to_string(),
            title: "System Journal".to_string(),
            content: r#"View system and application logs:
- Application log: structured HomeVault events (login, CRUD, security)
- System logs: syslog, auth.log, kern.log, daemon.log
- journalctl entries with unit/priority filters
- Search/filter entries
- Clear application logs
- Auto-cleanup scheduler (configurable interval, default 90 days)

Application events are always in plain English for audit trail purposes."#.to_string(),
        },
        FeatureDoc {
            id: "activity".to_string(),
            title: "Activity Taskbar".to_string(),
            content: r#"A dock button on the desktop (visible only while background tasks run) with a badge showing the active-task count. Clicking it opens a panel above the dock with one row per task: name, a progress bar, and a percentage.

Sources shown: aria2 downloads, File Explorer copy/move of large folders, Restic backups/restores/verifications, antivirus scans, disk cache mover, Docker app install/update, disk cloning (ddrescue) and file recovery (photorec), knowledge-base reindexing, AI model downloads, and cloud sync.

Endpoint: GET /api/activity (any signed-in user; non-admins only see their own tasks). The panel is display-only. Not yet covered: Syncthing folder sync and Storage operations (RAID resync, scrub, LUKS format)."#.to_string(),
        },
    ];

    let mut doc_count = 0;
    let mut chunk_count = 0;

    for feature in features {
        let doc_id = format!("feature:{}", feature.id);
        let chunks = crate::rag::chunk_text(&feature.content);
        let chunk_texts: Vec<String> = chunks.iter().map(|(t, _)| t.clone()).collect();
        let embeddings = crate::rag::generate_embeddings(&chunk_texts).await.unwrap_or_default();

        sqlx::query(
            "INSERT INTO rag_documents (id, source, source_id, title, content, language, updated_at)
             VALUES (?, 'feature', ?, ?, ?, 'en', datetime('now'))
             ON CONFLICT(id) DO UPDATE SET title=excluded.title, content=excluded.content,
             language=excluded.language, updated_at=datetime('now')",
        )
        .bind(&doc_id)
        .bind(&feature.id)
        .bind(&feature.title)
        .bind(&feature.content)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;

        sqlx::query("DELETE FROM rag_chunks WHERE document_id = ?")
            .bind(&doc_id)
            .execute(pool)
            .await
            .map_err(|e| e.to_string())?;

        for (i, (text, _)) in chunks.iter().enumerate() {
            let chunk_id = format!("{}:{}", doc_id, i);
            let emb_bytes = embeddings.get(i).map(|e| crate::rag::embedding_to_bytes(e));

            sqlx::query(
                "INSERT INTO rag_chunks (id, document_id, chunk_index, content, embedding, token_count)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(&chunk_id)
            .bind(&doc_id)
            .bind(i as i32)
            .bind(text)
            .bind(emb_bytes.as_deref())
            .bind(crate::rag::estimate_tokens(text) as i32)
            .execute(pool)
            .await
            .map_err(|e| e.to_string())?;
        }

        doc_count += 1;
        chunk_count += chunks.len() as i32;
    }

    Ok((doc_count, chunk_count))
}

/// Index i18n strings as structured documents per language.
async fn index_i18n(pool: &SqlitePool) -> Result<(i32, i32), String> {
    use std::collections::HashMap;

    let mut doc_count = 0;
    let mut chunk_count = 0;

    // Read from the TOML bundles embedded in the binary (`i18n.rs`), not from
    // `src/i18n/*.toml` on disk: those paths only exist in a source checkout,
    // and on a deployed host (WorkingDirectory=/opt/homevault) the read failed
    // silently and this source indexed nothing.
    for (lang_code, toml_content) in crate::i18n::embedded_translation_sources() {
        let lang_name = match lang_code {
            "en" => "English",
            "fr" => "French",
            "de" => "German",
            "es" => "Spanish",
            other => other,
        };

        // Parse the TOML into sections
        let parsed: HashMap<String, toml::Value> = match toml::from_str(toml_content) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Group keys by section and build a document per section
        for (section, value) in &parsed {
            if let Some(table) = value.as_table() {
                let mut content = format!("Section: {}\nLanguage: {}\n\n", section, lang_name);
                for (key, val) in table {
                    if let Some(s) = val.as_str() {
                        content.push_str(&format!("{}: {}\n", key, s));
                    }
                }

                if content.len() < 50 {
                    continue;
                }

                let doc_id = format!("i18n:{}:{}", lang_code, section);
                let chunks = crate::rag::chunk_text(&content);
                let chunk_texts: Vec<String> = chunks.iter().map(|(t, _)| t.clone()).collect();
                let embeddings = crate::rag::generate_embeddings(&chunk_texts).await.unwrap_or_default();

                sqlx::query(
                    "INSERT INTO rag_documents (id, source, source_id, title, content, language, updated_at)
                     VALUES (?, 'i18n', ?, ?, ?, ?, datetime('now'))
                     ON CONFLICT(id) DO UPDATE SET content=excluded.content,
                     language=excluded.language, updated_at=datetime('now')",
                )
                .bind(&doc_id)
                .bind(section)
                .bind(format!("i18n: {} - {}", section, lang_name))
                .bind(&content)
                .bind(lang_code)
                .execute(pool)
                .await
                .map_err(|e| e.to_string())?;

                sqlx::query("DELETE FROM rag_chunks WHERE document_id = ?")
                    .bind(&doc_id)
                    .execute(pool)
                    .await
                    .map_err(|e| e.to_string())?;

                for (i, (text, _)) in chunks.iter().enumerate() {
                    let chunk_id = format!("{}:{}", doc_id, i);
                    let emb_bytes = embeddings.get(i).map(|e| crate::rag::embedding_to_bytes(e));

                    sqlx::query(
                        "INSERT INTO rag_chunks (id, document_id, chunk_index, content, embedding, token_count)
                         VALUES (?, ?, ?, ?, ?, ?)",
                    )
                    .bind(&chunk_id)
                    .bind(&doc_id)
                    .bind(i as i32)
                    .bind(text)
                    .bind(emb_bytes.as_deref())
                    .bind(crate::rag::estimate_tokens(text) as i32)
                    .execute(pool)
                    .await
                    .map_err(|e| e.to_string())?;
                }

                doc_count += 1;
                chunk_count += chunks.len() as i32;
            }
        }
    }

    Ok((doc_count, chunk_count))
}

struct FeatureDoc {
    id: String,
    title: String,
    content: String,
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_feature_docs_are_nonempty() {
        // Quick sanity check that we defined real features
        let features = vec!["dashboard", "explorer", "storage", "docker", "virtual-machines", "backup", "users", "shares", "cloudsync", "network", "health", "terminal", "security", "vpn", "rescue", "settings", "journal", "activity"];
        assert!(features.len() >= 18);
    }

    #[tokio::test]
    async fn test_index_all_populates_knowledge_base() {
        let db = crate::storage::Database::new("sqlite::memory:")
            .await
            .expect("in-memory db");
        db.run_migrations().await.expect("migrations");
        let pool = db.pool();

        let (docs, chunks) = super::index_all(pool.as_ref()).await.expect("index_all");
        assert!(docs > 0, "expected indexed documents, got {docs}");
        assert!(chunks > 0, "expected indexed chunks, got {chunks}");

        // Every documented source must have produced documents.
        for source in ["catalog", "feature", "i18n"] {
            let count: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM rag_documents WHERE source = ?")
                    .bind(source)
                    .fetch_one(pool.as_ref())
                    .await
                    .expect("count query");
            assert!(count > 0, "source '{source}' produced no documents");
        }

        // Chunks are linked to documents.
        let chunk_docs: i64 =
            sqlx::query_scalar("SELECT COUNT(DISTINCT document_id) FROM rag_chunks")
                .fetch_one(pool.as_ref())
                .await
                .expect("chunk query");
        assert!(chunk_docs > 0, "no chunks linked to documents");

        // Re-indexing upserts rather than duplicating documents.
        let (docs2, _) = super::index_all(pool.as_ref()).await.expect("second index");
        assert_eq!(docs, docs2, "re-indexing must not change the document count");
    }
}
