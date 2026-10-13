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

    static BUILTIN_DIR: include_dir::Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/catalog");

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
        let mut content = format!(
            "Application: {}\nCategory: {}\n",
            manifest.name, manifest.category
        );

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
        let embeddings = crate::rag::generate_embeddings(&chunk_texts)
            .await
            .unwrap_or_default();

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

/// Every feature documented in the knowledge base.
///
/// **One entry per guide in `docs/guides/`**, minus the developer-only ones
/// (`design-system.md`, which documents tokens rather than a feature a user can
/// open). The `test_every_guide_has_a_feature_doc` test at the bottom of this
/// file enumerates that directory and fails when a guide has no entry — so the
/// rule in `CLAUDE.md` ("a new feature gets a `FeatureDoc`") breaks the build
/// instead of silently leaving the assistant unable to answer about a shipped
/// feature. It was prose-only until 2026-10-12, and 25 of the 46 guides had
/// drifted out of the index.
///
/// `FeatureDoc.id` is the stable key (`feature:<id>`); it is deliberately not
/// always the guide's filename (`backup` ← `backup-restic.md`,
/// `virtual-machines` ← `vm.md`), which is why the test carries an explicit
/// id → guide table rather than deriving one from the other.
///
/// Content is written for the person asking the assistant a question: what the
/// feature does, what it is for, and how to reach it. No file paths, no handler
/// names, no internal identifiers — those are true of a checkout, not of the
/// installed product the assistant is describing.
fn feature_docs() -> Vec<FeatureDoc> {
    vec![
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
- Deleting an account revokes the share permissions granted to it directly (the
  real filesystem access control entries and the stored grants), purges its
  home directory, its SFTP jail, its sessions, its terminal sessions and its
  transfer-quota row — so recreating an account with the same name inherits
  nothing. Permissions granted to a group it belonged to are left alone: those
  belong to the group, and its other members keep them. The operator chooses
  whether the home folder is deleted along with the account.
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

Each of the three is described in more depth in its own entry in this
knowledge base (search for antivirus, anti-ransomware, or firewall).

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
        FeatureDoc {
            id: "auth".to_string(),
            title: "Login, Two-Factor Authentication & Recovery".to_string(),
            content: r#"Signing in, and getting back in when something is lost.

- Log in with a username and password. When two-factor authentication is enabled, the login returns a second step that asks for the current 6-digit code before the session opens.
- Two-factor authentication is TOTP: it can be turned on from the account page, during the first-run wizard, or by an administrator for someone else's account. Any standard authenticator app works (Google Authenticator, Microsoft Authenticator, Aegis, Ente Auth, 2FAS).
- One-time backup codes are issued when two-factor authentication is enabled, for the case where the authenticator device is lost.
- Offline recovery: an account can generate a single recovery code from its own page. Entering that code at the login screen opens a "choose a new password" step. Using it does not consume it — regenerating the code invalidates the previous one.
- Every login creates a session, listed on the account page with its IP address, creation time and expiry. One session can be revoked on its own, or all other sessions can be ended at once.
- A recent-login history records the IP address, the timestamp, whether the attempt succeeded, and the browser's user agent.
- Logging out invalidates the session on the server and clears the session cookie.

There is no email "forgot password" flow: recovery is code-based only. A first-run check tells the interface whether an administrator account exists yet, which is what routes a fresh install to the setup wizard."#.to_string(),
        },
        FeatureDoc {
            id: "setup".to_string(),
            title: "Setup Wizard (first run)".to_string(),
            content: r#"The wizard shown on a fresh installation, while no administrator account exists yet. Until it is finished, the web interface only serves this flow.

- Step 1 — account: choose the administrator username, with availability checked as you type, and optionally a full name.
- Step 2 — password: type it and confirm it. Two-factor authentication can be enrolled right here: a QR code is generated in the browser from a fresh secret and the first 6-digit code is verified before the account exists, so a mis-scanned code is caught immediately instead of locking the administrator out later.
- Step 3 — review: a summary of the chosen account, then creation.
- Step 4 — recovery code: the administrator account is created and a one-time recovery code is shown once. It has to be stored somewhere safe; it is the only offline way back into the account. When two-factor authentication was enrolled, the one-time backup codes are shown next.

The whole wizard, including the QR-code step, follows the chosen interface language. It refuses to run twice: once an administrator exists, the setup endpoints report that the server is already configured.

For safety, the first-run endpoints only answer clients on the local network, so a fresh appliance on a shared network cannot have its administrator account claimed by a stranger. If the hardware fingerprint changes later (after moving the system disk or cloning the machine), the interface asks an administrator to sign in once and confirm the new hardware instead of showing the wizard again."#.to_string(),
        },
        FeatureDoc {
            id: "account".to_string(),
            title: "My Account (self-service)".to_string(),
            content: r#"The signed-in person's own settings, opened from the user menu in the top bar. Everything here concerns the current account only — an administrator who wants to manage someone else uses the Users app.

- Profile: display name and email address.
- Password: changing your own password requires the current one. It also ends every other session of that account, the current one included, so the page signs out and returns to the login screen.
- Two-factor authentication: enable TOTP with a QR code, disable it (the current password is required), and view or regenerate the one-time backup codes.
- Avatar: choose an avatar seed with a live preview; it is stored on the account and served as an image.
- Active sessions: one row per live session with its IP address, creation time and expiry, a marker on the one you are using, a button to revoke any individual session, and a button to log out every other device.
- Recent logins: the last sign-in attempts with their IP address, timestamp, success or failure, and the browser's user agent."#.to_string(),
        },
        FeatureDoc {
            id: "groups".to_string(),
            title: "Groups & Membership".to_string(),
            content: r#"Administrator management of user groups.

- Create a group — the name is up to 32 characters of lowercase letters, digits and hyphens — view the groups, open one to see its members, and delete it.
- Add and remove members from a group's detail view, or from the Users app, which offers the same membership as chips on each account.
- Groups are the sharing unit: a file-share permission can be granted to a group instead of to each person individually, and a transfer quota can be set per group. A group grant applies to every current member and stops applying the moment someone is removed.
- The homevault_admin group is special — it grants root-level access on the host. It is never offered as an ordinary tickable group: it appears as a read-only chip driven by the account's role, so membership cannot be granted by accident.
- Accounts get shell-less Linux users of the same name so they can reach the server over SFTP; the group membership is what a share grant is evaluated against."#.to_string(),
        },
        FeatureDoc {
            id: "antivirus".to_string(),
            title: "Antivirus (ClamAV scanning)".to_string(),
            content: r#"Malware scanning with ClamAV, in two modes:

- On demand: choose a folder or a path and start a scan immediately.
- Scheduled: a cron expression — weekly on Sunday at 03:00 by default — scans a configured path unattended.

What it records:
- One history row per scan: the path, whether it was manual or scheduled, the status, how many files were scanned, and how many threats were found.
- One row per detected threat, with the file's path and the signature that matched.
- A quarantine folder (its location is configurable) where detected files are moved; a threat can also be recorded as left in place instead.
- A running scan can be cancelled.

A deliberate limit: this is on-demand and scheduled scanning only. There is no real-time, on-access scanner and no resident daemon — a conscious fit to the low-power hardware HomeVault targets. Scanning is the Antivirus card in Settings."#.to_string(),
        },
        FeatureDoc {
            id: "ransomware".to_string(),
            title: "Anti-Ransomware Protection".to_string(),
            content: r#"Heuristic protection against ransomware-style bulk encryption or deletion, in two layers:

- Honeypot files: decoy files placed in watched folders that nothing legitimate ever touches. Something opening, modifying or deleting one raises an event immediately.
- Rate detection: a sliding window per watched path flags a burst of modifying, renaming or deleting activity — 50 events within 10 seconds by default — as suspected ransomware behaviour.

What it records:
- Watch one or more folders, each with its own honeypot file; a watched folder can be tied to a share so the reaction can target that share.
- An event history: what triggered it (honeypot touched, mass modify, mass rename, mass delete), the details, the suspected account when it can be determined, and the response that was taken.
- A configurable automatic response: alert only (the default and the safest), lock the suspected account, or disable the affected share.

This is best-effort protection against a behaviour pattern, not a guarantee: a slow or targeted encryption can stay below the threshold. It is the Anti-Ransomware card in Settings."#.to_string(),
        },
        FeatureDoc {
            id: "firewall".to_string(),
            title: "Firewall (ufw)".to_string(),
            content: r#"Host firewall management through ufw, on Debian and Ubuntu.

- See whether the firewall is active and which rules are currently in effect.
- Add and remove rules, and enable or disable the firewall.
- An anti-lockout guard: the rule that keeps the SSH port open and the rule that keeps the HomeVault web port open cannot be removed, so a remote administrator cannot cut off their own access. Enabling the firewall additionally asks for an explicit confirmation.
- Every change made from HomeVault — enabling, disabling, adding or removing a rule — is written to an audit log that records who made it and when.
- What the host is actually enforcing is always read live from ufw; the rules are not mirrored in HomeVault's database, so the screen cannot drift away from reality.

It is the Firewall card in Settings."#.to_string(),
        },
        FeatureDoc {
            id: "encryption".to_string(),
            title: "Disk Encryption (LUKS2)".to_string(),
            content: r#"Whole-device encryption for a single spare block device, using LUKS2.

- Encrypt a device that is not currently in use — unassigned, and not part of an array.
- Lock and unlock volumes, change a volume's passphrase, and delete an encrypted volume, which erases the LUKS header and wipes the leftover filesystem signatures.
- Unlocking is always manual and per boot: no key material is ever kept on the server, so a volume stays locked after a restart until an administrator types the passphrase again.
- Passphrases are piped to the encryption tool over standard input, never passed as a command-line argument, so they cannot be read from the process list.
- The stored record keeps only what is needed to address the volume — device, mapper name, UUID, filesystem, label and mount point — never the passphrase itself.

Scoped to standalone devices on purpose: RAID arrays belong to the RAID wizard, and ZFS pools use ZFS's own native dataset encryption."#.to_string(),
        },
        FeatureDoc {
            id: "raid".to_string(),
            title: "RAID — virtual disk creation".to_string(),
            content: r#"A creation-only wizard that turns several physical disks into one "virtual disk" with redundancy.

- Levels offered: RAID 0 (stripe), RAID 1 (mirror), RAID 5 (single parity), RAID 6 (double parity) and RAID 10 (mirror plus stripe).
- Targets: a Linux mdadm array, a ZFS pool, or a native Btrfs volume.
- Three steps: choose the disks, choose the level and the target, then confirm. A diagram shows the usable capacity and the redundancy each level buys, and the recap lists exactly what will be created.
- Only free, unassigned disks are offered, so an in-use device cannot be selected by mistake.

This screen only creates. Listing the arrays, verifying or scrubbing them, rebuilding a degraded one and destroying them all live in the Storage app's "Virtual disks" tab, which is the single place a virtual disk is managed once it exists."#.to_string(),
        },
        FeatureDoc {
            id: "snapshots".to_string(),
            title: "Snapshots & Rollback".to_string(),
            content: r#"Point-in-time snapshots of ZFS and Btrfs pools: create them, list them, delete them, and roll a pool back to one.

Where snapshots are used:
- The File Explorer can browse the snapshots that cover a folder (read-only, on ZFS), so an older version of a file can be copied back out without rolling anything back.
- Backups lists the snapshots of a repository.
- A virtual machine can take a snapshot and be restored to it.

Rolling a pool back is destructive — everything written since the snapshot is lost — so it is restricted to administrators and confirmed explicitly. Restoring a virtual machine is an administrator action as well.

One documented limitation: rolling back the default mounted subvolume of a Btrfs pool is not supported. It returns a clear error rather than appearing to succeed and doing nothing."#.to_string(),
        },
        FeatureDoc {
            id: "cache".to_string(),
            title: "Disk Cache Tier".to_string(),
            content: r#"A fast disk can be designated as a write cache: incoming writes land on it first and a periodic mover drains them to the slower target pool.

- Designate, create or remove the cache disk from the Storage app's Pools tab.
- Configure the mover and the per-share cache mode from the Cache tab.
- Mover options: a cron schedule, a minimum amount of free space to leave on the target, moving only files older than a given number of days, exclude patterns, and a bandwidth limit.
- The mover can also be started immediately, with live progress in the desktop activity taskbar; every run is recorded with the number of files and bytes moved.
- Each share's cache mode decides whether it uses the cache at all and how eagerly it is drained, so a small hot dataset can sit on the fast disk while bulk data goes straight to the array.

The point is to absorb bursts of writes on fast media and let the slower array catch up out of hours."#.to_string(),
        },
        FeatureDoc {
            id: "syncthing".to_string(),
            title: "Syncthing".to_string(),
            content: r#"Continuous, peer-to-peer file synchronization, driven through the Syncthing daemon running on the same machine.

- Overview: whether the daemon is running, transfer statistics, and how many errors it has recorded.
- Folders: the synchronized folders with their state, adding a folder, pausing and resuming one, and triggering a rescan.
- Devices: the known devices with their connection state, adding a device, and managing which ones are accepted.
- The daemon itself can be restarted from the screen.

Syncthing keeps running independently of HomeVault — this screen is a management view over it, not a replacement. Folder paths and device keys are sensitive, so every action here requires an administrator. If the daemon is not installed or not running, the screen says so instead of failing silently."#.to_string(),
        },
        FeatureDoc {
            id: "downloader".to_string(),
            title: "Downloader (aria2)".to_string(),
            content: r#"A download manager backed by aria2: paste a link and let the server fetch it.

- Add a download, optionally choosing the destination folder and the output name; then pause, resume or remove it.
- Follow progress live: percentage, speed, size and connection count.
- Start and stop the download daemon itself from the screen.
- Global statistics: the overall speed, and how many downloads are active, waiting or stopped.
- Two panes: the list of downloads on the left, the detail of the selected one on the right, re-resolved on every poll so it follows a download as it progresses.
- Active downloads also appear in the desktop activity taskbar.

Administrator-only, like the other apps that drive a system service."#.to_string(),
        },
        FeatureDoc {
            id: "chat".to_string(),
            title: "AI Assistant (Chat)".to_string(),
            content: r#"A chat interface to a local AI model served by Ollama — no cloud provider, nothing leaves the machine.

- Send a message and read the answer as it is streamed back, token by token.
- Answers follow the interface language: the instructions given to the model are built for the language the interface is currently in.
- When the knowledge base has been indexed, the passages most relevant to the question — about HomeVault's own features, its application catalog and its translations — are retrieved and handed to the model, so the assistant can explain how to use the product instead of answering only in general terms.
- Model management: list the installed models, pull a new one (with progress in the activity taskbar), and choose which model answers.
- A recommendation view suggests models suited to the detected hardware — available memory, processor and graphics card.

Ollama has to be installed and at least one model pulled; the embedding model used by the knowledge base is separate and has to be pulled before the first index. The active model is also selectable from Settings."#.to_string(),
        },
        FeatureDoc {
            id: "rag".to_string(),
            title: "Knowledge Base (RAG)".to_string(),
            content: r#"Retrieval-augmented generation: before the assistant answers, HomeVault looks up the passages most relevant to the question and passes them to the model.

Three sources are indexed, all built into the application:
- The application catalog: every installable app, with its name, description, category, developer, website, port and hardware requirements.
- The feature documentation: what each part of HomeVault does and how to use it.
- The interface translations: the wording of every screen, in all four languages.

How it works:
- Indexing is an explicit administrator action and runs in the background, so the request returns immediately and the progress is polled.
- Documents are split into chunks of roughly 400 words with an 80-word overlap, converted into vectors through Ollama, and stored in the database.
- At question time the question is converted the same way, the closest chunks are found by cosine similarity, and the best matches are inserted into the instructions given to the model.
- Re-indexing updates the existing documents rather than duplicating them. Nothing is read from the source tree: everything indexed is embedded in the installed application.

The embedding model is fixed to nomic-embed-text, which has to be pulled before the first index. If the embedding step is unavailable the chat still works, simply without retrieved context."#.to_string(),
        },
        FeatureDoc {
            id: "notifications".to_string(),
            title: "Notifications".to_string(),
            content: r#"Turns system events into notifications that can be read in the interface, sent by email, or posted to a webhook.

- A background check runs once a minute. It evaluates the system health checks and the recent backup jobs.
- Health problems are reported on transition: a check becoming degraded, or recovering from one, produces a notification. A problem that simply persists does not re-notify every minute.
- Backup results are reported once per genuinely new completed or failed job.
- The bell in the top bar carries an unread count and opens a panel where notifications are read, marked as read, or cleared.
- If alerts are enabled and a notification meets the configured severity threshold, it is also emailed through the configured mail server, and/or posted as JSON to a webhook address.

Configured in Settings: the mail server and its recipients, the alert threshold and the webhook address. The test button there sends a real message through the configured server and reports genuine failures rather than assuming success."#.to_string(),
        },
        FeatureDoc {
            id: "widgets".to_string(),
            title: "Desktop Widgets (monitor panels)".to_string(),
            content: r#"A catalog for switching the floating desktop panels on and off and for resetting their positions.

The eleven panels: Resource, RAID, Security, VPN, Downloader, Syncthing, Calendar, Docker, Weather, Active Sessions and System Health.

- Each panel is a draggable card that floats over the desktop and can be closed from its own title bar.
- Positions and visibility are remembered per account, so two people using the same server each get their own layout.
- Panels are re-clamped against the current window size whenever it changes, so none is left off-screen after a resize or a rotation.
- The catalog lists every panel with an icon, a short description and a switch, plus a button that restores the default arrangement.
- The Weather panel additionally remembers the city it was set to.

The panels are read-only: they display system state and link to the app that manages it."#.to_string(),
        },
        FeatureDoc {
            id: "desktop".to_string(),
            title: "Desktop Shell & Windows".to_string(),
            content: r#"Every app opens inside the desktop — a windowed environment living in the browser tab, arranged like a desktop operating system.

- App launcher: the searchable grid of every available app, opened from the top bar.
- Top bar: the floating strip carrying the launcher, the open windows, the network indicator, the widgets toggle, notifications, and the power menu, which also holds the account menu.
- Dock: the row of app shortcuts along the bottom, with a badge when a background task is running and a label on hover.
- Windows: each app opens in a window that can be dragged, resized, minimized, maximized and closed. Windows are listed in the top bar and can be brought back to the front.
- Two kinds of window content: an app's own interface, or an embedded web page — which is how an installed catalog application is shown, in a sandboxed frame.
- Floating desktop panels (see Widgets) sit above the wallpaper, alongside the windows.
- Power menu: restart the server, shut it down, or log out. Restart and shutdown require administrator rights and an explicit confirmation.
- The wallpaper, accent colour, density and theme are chosen here and apply to the whole shell.
- The layout adapts to the window size, from a wide screen down to a phone: below a narrow threshold the maximized window insets instead of hiding the chrome, and the launcher switches to a scrolling list."#.to_string(),
        },
        FeatureDoc {
            id: "apps-center".to_string(),
            title: "Apps Center (app launcher)".to_string(),
            content: r#"The launcher: a searchable grid of every application HomeVault offers, opened from the top bar and also reachable with a keyboard shortcut.

- A row of category chips filters the grid in place; "All apps" restores the whole catalog.
- Typing in the search field narrows the grid as you go. When nothing matches, the grid is replaced by an empty state with a button that clears the filter and brings the apps back.
- Each tile carries the app's own icon and colour, with its name underneath.
- Choosing a tile opens that app as a window on the desktop.
- Applications installed from the Docker catalog appear here as real tiles too, not only inside the Docker app; they open as an embedded web page, and if the browser refuses to embed a plain-HTTP page inside an HTTPS interface, a message points at the direct address instead.
- The launcher holds no app list of its own — it renders whatever the desktop's registry contains, so it cannot offer something that does not exist.
- Keyboard: focus stays inside the launcher while it is open, and Escape closes it and returns focus to the dock."#.to_string(),
        },
        FeatureDoc {
            id: "themes".to_string(),
            title: "Themes & Appearance".to_string(),
            content: r#"The look of the whole interface: theme, accent colour, density and wallpaper.

- The theme chooses the overall light or dark treatment; the accent colour recolours interactive elements — buttons, links, switches, selection states — across every screen.
- Density tightens or loosens the spacing of lists, tables and toolbars, which is what makes the interface usable on a large monitor and on a small laptop alike.
- Changes are staged while you adjust them and applied when you save, so the interface is not repainted on every keystroke of a slider.
- The appearance is stored on the server, so the same account gets the same look from any browser, and the wallpaper is picked from the bundled set.
- Every screen is expected to read the shared colour and spacing tokens rather than hard-coding values, which is what makes a theme change apply everywhere at once."#.to_string(),
        },
        FeatureDoc {
            id: "wallpapers".to_string(),
            title: "Wallpapers".to_string(),
            content: r#"The desktop background, chosen from a set of images shipped with HomeVault.

- Fifteen bundled wallpapers, each shown as a thumbnail in the appearance settings; selecting one applies it to the desktop immediately.
- The thumbnails are small versions of the same images, so the grid stays light.
- There is no wallpaper upload: the set is fixed, on purpose, so nothing unbounded can be pushed into the interface.
- Selection is remembered per account, alongside the theme and accent colour."#.to_string(),
        },
        FeatureDoc {
            id: "transfer-quotas".to_string(),
            title: "Transfer Quotas (upload / download caps)".to_string(),
            content: r#"Byte caps on how much a user or a group may upload and download.

- Set per user or per group, from the Users and Groups apps. Each direction is configured separately: a limit and a unit (megabytes, gigabytes, terabytes, petabytes), or disabled for no limit at all.
- A disabled direction reads as "Unlimited", which means no restriction — not zero.
- The Quotas app gives a read-only overview of every subject at once, with live usage bars.
- A user's effective limit is their own cap plus every group they belong to, each evaluated independently: exceeding any of them blocks the transfer.
- Usage is reserved before a transfer starts and debited when it lands, and refunded if an upload or download is abandoned — so a failed transfer does not consume quota. Concurrent transfers of the same subject are serialised, so two parallel transfers cannot both slip past the check.

Scope, precisely: this applies to transfers made through the built-in File Explorer. Shares served by SMB, NFS, FTP and WebDAV are handled by separate system services with no per-request visibility here, so transfers over those protocols are neither counted nor capped by this feature, whatever is configured.

Not to be confused with the disk-space quota, which limits how much data a group may store on a drive."#.to_string(),
        },
        FeatureDoc {
            id: "mdns".to_string(),
            title: "mDNS / homevault.local".to_string(),
            content: r#"Makes HomeVault reachable at a stable name on the local network — homevault.local — with no DNS or DHCP configuration, and advertises its file-sharing services so they show up in the "Network" view of a file manager.

- Works on the local network only, over link-local multicast. No outbound traffic, no third party.
- The advertised name is pinned to homevault, independent of the machine's own hostname, so the address is predictable. If a second HomeVault box appears on the same network, it is given a numbered name instead of fighting over the first one.
- Only service types and ports are advertised — never credentials or personal data.
- It relies on the Avahi service being present; when it is not installed, everything else works exactly as before and the name simply does not resolve.

This is what lets a user type homevault.local in a browser or a file manager instead of an IP address."#.to_string(),
        },
        FeatureDoc {
            id: "server-control".to_string(),
            title: "Server Control (restart / shutdown)".to_string(),
            content: r#"Restarting or shutting down the server, from the desktop's power menu or from Settings.

- Restart: stops and starts the HomeVault service. Useful after a change that needs a restart to take effect, such as enabling HTTPS.
- Shutdown: powers the machine off cleanly, rather than pulling the plug.
- Both require administrator rights and an explicit confirmation, so a single misplaced click cannot take the NAS down.
- Because the server goes away, the session ends with it: the interface returns to the login screen, or shows a "server unavailable" state until the machine is back.
- A small information endpoint reports the addresses and port the server is answering on, which is what the interface shows after a restart."#.to_string(),
        },
        FeatureDoc {
            id: "avatars".to_string(),
            title: "Avatars".to_string(),
            content: r#"The generated profile pictures shown beside an account's name.

- Avatars are generated on the server from a seed — a short string — so the same account always gets the same picture, and changing the seed changes the face.
- There is nothing to upload: the picture is drawn locally, with no outbound request and no third-party service involved.
- Choosing one: pick a seed and see the result in a live preview before saving it; it is then used everywhere the account appears — the user menu in the top bar, the Users list, a group's member list, and the account page.
- The generated image is cached per account and replaced whenever the seed changes."#.to_string(),
        },
        FeatureDoc {
            id: "mcp".to_string(),
            title: "External Agents (MCP server)".to_string(),
            content: r#"Lets an external AI agent — a coding assistant or automation running on another machine — drive HomeVault through the Model Context Protocol, over HTTP.

- The server is off by default and is switched on by an administrator. Switching it off again stops every agent call immediately, while keeping the tokens and permissions for later.
- Access is granted through tokens, each with a name so they can be told apart and revoked individually. A token is shown in full exactly once, at creation; afterwards only its prefix identifies it, and at most twenty tokens can exist at once.
- One shared permission set applies to every token: which apps the agent may see and control, whether it may install new apps from the catalog, which folders it may list, whether it may query the knowledge base, which virtual machines it may power-cycle, and whether it may reboot the appliance. Everything defaults to denied.
- The agent sees a tool list filtered to its permissions: appliance identity and resource usage, catalog search and app control, folder listing inside the granted scope, virtual-machine power actions, semantic search over the knowledge base, and — only when explicitly granted — reboot.
- Folder access never crosses its scope: system directories are always excluded, and links are resolved and re-checked before listing.
- Every agent action is recorded in the event log under its token, like any administrator action."#.to_string(),
        },
    ]
}

/// Index HomeVault feature documentation into the knowledge base.
async fn index_features(pool: &SqlitePool) -> Result<(i32, i32), String> {
    let features = feature_docs();

    let mut doc_count = 0;
    let mut chunk_count = 0;

    for feature in features {
        let doc_id = format!("feature:{}", feature.id);
        let chunks = crate::rag::chunk_text(&feature.content);
        let chunk_texts: Vec<String> = chunks.iter().map(|(t, _)| t.clone()).collect();
        let embeddings = crate::rag::generate_embeddings(&chunk_texts)
            .await
            .unwrap_or_default();

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
                let embeddings = crate::rag::generate_embeddings(&chunk_texts)
                    .await
                    .unwrap_or_default();

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
    use super::*;

    /// Guides that document *how the codebase is built* rather than a feature a
    /// user can open. They are deliberately **not** in the knowledge base: the
    /// assistant answers questions about the product, and a guide about design
    /// tokens would only pollute retrieval with internal vocabulary.
    const DEVELOPER_ONLY_GUIDES: &[&str] = &["design-system.md"];

    /// `FeatureDoc.id` → the guide that documents it.
    ///
    /// The two names do not always match (`backup` ← `backup-restic.md`,
    /// `virtual-machines` ← `vm.md`, `setup` ← `setup-wizard.md`,
    /// `cloudsync` ← `cloud-sync.md`), which is why the mapping is written out
    /// instead of derived. It is asserted against the *real* `feature_docs()`
    /// in both directions below, so neither half can drift alone.
    const FEATURE_GUIDES: &[(&str, &str)] = &[
        ("dashboard", "dashboard.md"),
        ("explorer", "explorer.md"),
        ("storage", "storage.md"),
        ("docker", "docker.md"),
        ("virtual-machines", "vm.md"),
        ("backup", "backup-restic.md"),
        ("users", "users.md"),
        ("groups", "groups.md"),
        ("quotas", "quotas.md"),
        ("shares", "shares.md"),
        ("cloudsync", "cloud-sync.md"),
        ("network", "network.md"),
        ("health", "health.md"),
        ("terminal", "terminal.md"),
        ("security", "security.md"),
        ("antivirus", "antivirus.md"),
        ("ransomware", "ransomware.md"),
        ("firewall", "firewall.md"),
        ("tls", "tls.md"),
        ("vpn", "vpn.md"),
        ("rescue", "rescue.md"),
        ("settings", "settings.md"),
        ("journal", "journal.md"),
        ("activity", "activity.md"),
        ("auth", "auth.md"),
        ("setup", "setup-wizard.md"),
        ("account", "account.md"),
        ("encryption", "encryption.md"),
        ("raid", "raid.md"),
        ("snapshots", "snapshots.md"),
        ("cache", "cache.md"),
        ("syncthing", "syncthing.md"),
        ("downloader", "downloader.md"),
        ("chat", "chat.md"),
        ("rag", "rag.md"),
        ("notifications", "notifications.md"),
        ("widgets", "widgets.md"),
        ("desktop", "desktop.md"),
        ("apps-center", "apps-center.md"),
        ("themes", "themes.md"),
        ("wallpapers", "wallpapers.md"),
        ("transfer-quotas", "transfer-quotas.md"),
        ("mdns", "mdns.md"),
        ("server-control", "server-control.md"),
        ("avatars", "avatars.md"),
        ("mcp", "mcp.md"),
    ];

    fn guides_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/guides")
    }

    /// Every guide in `docs/guides/` is claimed by exactly one `FeatureDoc`, and
    /// every claimed guide exists.
    ///
    /// This is the mechanical half of the `CLAUDE.md` rule "a new feature gets a
    /// `FeatureDoc` + a `rag.md` Document Sources row": adding a guide without
    /// indexing it fails the build here. When this was prose-only, **25 of the
    /// 46 guides** had quietly never been indexed (found 2026-10-12) — the
    /// assistant answered "I don't know about that" for shipped features such as
    /// Syncthing, the downloader, notifications, the desktop shell and LUKS
    /// encryption.
    #[test]
    fn test_every_guide_has_a_feature_doc() {
        let mut on_disk: Vec<String> = std::fs::read_dir(guides_dir())
            .expect("docs/guides must be readable from the crate root")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".md"))
            .filter(|n| !DEVELOPER_ONLY_GUIDES.contains(&n.as_str()))
            .collect();
        on_disk.sort();
        assert!(
            !on_disk.is_empty(),
            "no guides found in {} — the test would pass vacuously",
            guides_dir().display()
        );

        // ...and every guide is indexed exactly once.
        let mut indexed: Vec<String> = FEATURE_GUIDES
            .iter()
            .map(|(_, guide)| guide.to_string())
            .collect();
        indexed.sort();
        assert_eq!(
            indexed, on_disk,
            "the set of indexed guides must match docs/guides/ exactly (modulo {DEVELOPER_ONLY_GUIDES:?})"
        );

        // ...and each claimed guide really is on disk (a typo would otherwise
        // show up as a missing guide rather than as a bad path).
        for (id, guide) in FEATURE_GUIDES {
            assert!(
                guides_dir().join(guide).is_file(),
                "FeatureDoc '{id}' claims {guide}, which does not exist"
            );
        }

        // ...and the table describes the docs actually shipped to the indexer.
        let mut ids: Vec<String> = feature_docs().into_iter().map(|f| f.id).collect();
        ids.sort();
        let mut mapped: Vec<String> = FEATURE_GUIDES
            .iter()
            .map(|(id, _)| id.to_string())
            .collect();
        mapped.sort();
        assert_eq!(
            ids, mapped,
            "FEATURE_GUIDES and feature_docs() must list the same ids"
        );
    }

    /// The entries are the model's whole picture of the product: an empty or
    /// stub one is worse than a missing one, because it retrieves and then
    /// misleads.
    #[test]
    fn test_feature_docs_are_unique_and_substantive() {
        let docs = feature_docs();
        assert!(
            docs.len() >= 45,
            "expected the full catalog, got {}",
            docs.len()
        );

        let mut ids: Vec<&str> = docs.iter().map(|d| d.id.as_str()).collect();
        ids.sort_unstable();
        let unique = {
            let mut u = ids.clone();
            u.dedup();
            u.len()
        };
        assert_eq!(unique, ids.len(), "duplicate FeatureDoc ids in {ids:?}");

        for doc in &docs {
            assert!(!doc.id.trim().is_empty(), "a FeatureDoc has an empty id");
            assert!(
                doc.id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '-' || c.is_ascii_digit()),
                "FeatureDoc id {:?} must be a lowercase slug",
                doc.id
            );
            assert!(
                doc.title.len() > 3,
                "FeatureDoc '{}' has a stub title",
                doc.id
            );
            // The longest stubs are ~400 chars; anything under 200 is a
            // placeholder rather than documentation.
            assert!(
                doc.content.len() >= 200,
                "FeatureDoc '{}' has a stub body ({} chars)",
                doc.id,
                doc.content.len()
            );
            // The assistant speaks to a user, not to a maintainer.
            for leak in ["src/", ".svelte", "fn ", "`"] {
                assert!(
                    !doc.content.contains(leak),
                    "FeatureDoc '{}' leaks an implementation detail ({leak:?})",
                    doc.id
                );
            }
        }
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
        assert_eq!(
            docs, docs2,
            "re-indexing must not change the document count"
        );
    }
}
