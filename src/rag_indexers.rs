/* =============================================================================
 * File          : rag_indexers.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Document indexers for the RAG knowledge base: one collector per source
 *   (catalog manifests, project feature docs, translation keys, ...) that
 *   upserts documents and their chunks into the RAG tables on each full
 *   reindex. A new feature or app must add its FeatureDoc here.
 *
 * Dependencies:
 *   - sqlx (>= 0.9), internal app_catalog / i18n modules
 *
 * Usage:
 *   handlers/rag.rs reindex endpoint (index_all); see docs/guides/rag.md
 *   "Document Sources".
 * =============================================================================
 */

//! Document indexers for the RAG knowledge base.
//!
//! Each indexer knows how to collect documents from one source and upsert
//! them into the `rag_documents` / `rag_chunks` tables.

use crate::rag::EmbeddingProvider;
use sqlx::SqlitePool;

/// Index all document sources into the RAG knowledge base and **reconcile**
/// the result: each source reports the document ids it currently produces and
/// everything else under that source is pruned.
///
/// A source failure (a DB error, a malformed manifest that aborts, or an
/// embedding failure) **fails the whole run** and leaves the status at
/// `error`. Before 2026-10-22 each source failure was a `warn!` and the run
/// still ended `idle`, so a reindex that embedded nothing looked like a
/// success. Returns (total_documents, total_chunks) in the index afterwards.
pub async fn index_all<P: EmbeddingProvider>(
    pool: &SqlitePool,
    provider: &P,
) -> Result<(i32, i32), String> {
    crate::rag::update_index_status(pool, "indexing", 0, 0).await?;

    match index_sources(pool, provider).await {
        Ok(counts) => {
            crate::rag::update_index_status(pool, "idle", counts.0, counts.1).await?;
            Ok(counts)
        }
        Err(e) => {
            // Loud failure: the status becomes `error`, never `idle`.
            let _ = crate::rag::update_index_status(pool, "error", 0, 0).await;
            Err(e)
        }
    }
}

/// Run the three indexers in order, reconciling each source after it is
/// indexed, and sum their (documents, chunks).
async fn index_sources<P: EmbeddingProvider>(
    pool: &SqlitePool,
    provider: &P,
) -> Result<(i32, i32), String> {
    let mut total_docs = 0i32;
    let mut total_chunks = 0i32;

    // 1. App catalog
    let (d, c, ids) = index_catalog(pool, provider)
        .await
        .map_err(|e| format!("catalog indexing failed: {e}"))?;
    let pruned = crate::rag::prune_source(pool, "catalog", &ids).await?;
    tracing::info!("RAG: indexed catalog: {d} docs, {c} chunks ({pruned} stale removed)");
    total_docs += d;
    total_chunks += c;

    // 2. HomeVault features documentation
    let (d, c, ids) = index_features(pool, provider)
        .await
        .map_err(|e| format!("features indexing failed: {e}"))?;
    let pruned = crate::rag::prune_source(pool, "feature", &ids).await?;
    tracing::info!("RAG: indexed features: {d} docs, {c} chunks ({pruned} stale removed)");
    total_docs += d;
    total_chunks += c;

    // 3. i18n keys (all languages as structured docs)
    let (d, c, ids) = index_i18n(pool, provider)
        .await
        .map_err(|e| format!("i18n indexing failed: {e}"))?;
    let pruned = crate::rag::prune_source(pool, "i18n", &ids).await?;
    tracing::info!("RAG: indexed i18n: {d} docs, {c} chunks ({pruned} stale removed)");
    total_docs += d;
    total_chunks += c;

    Ok((total_docs, total_chunks))
}

/// Index the built-in app catalog manifests.
async fn index_catalog<P: EmbeddingProvider>(
    pool: &SqlitePool,
    provider: &P,
) -> Result<(i32, i32, Vec<String>), String> {
    use include_dir::include_dir;

    static BUILTIN_DIR: include_dir::Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/catalog");

    let mut doc_count = 0;
    let mut chunk_count = 0;
    let mut ids: Vec<String> = Vec::new();

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

        let inserted = crate::rag::upsert_document_with_chunks(
            pool,
            &doc_id,
            "catalog",
            &manifest.id,
            &manifest.name,
            &content,
            "en",
            provider,
        )
        .await?;

        ids.push(doc_id);
        doc_count += 1;
        chunk_count += inserted as i32;
    }

    Ok((doc_count, chunk_count, ids))
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
- Private per-user tags for files and folders, with tag-filtered global search
- Browse directories, view files with preview (text, images, code)
- Upload files (drag-and-drop or click), download files
- Create directories, rename, copy, move, delete
- Search files recursively by name, with an optional fuzzy (approximate spelling) mode
- Set file/directory permissions (chmod)
- Compress/extract archives (zip); extraction validates archive members before
  unpacking and refuses anything that could escape the destination
- Trash system (soft-delete with restore)
- One unified Trash: files deleted over the network from an SMB share's recycle bin appear in the same Trash as files deleted from the web UI, and can be restored from either place
- Trash retention: keep deleted files for a set number of days and cap the trash size; whatever passes a limit is removed automatically by a background cleanup, and an admin can run the cleanup on demand
- Recently visited folders, remembered per account and shared across browsers
- Favorites / pinned folders
- Safe-eject external drives
- Rewind: a per-file timeline of past ZFS snapshots, restore an older version by copying it out
- Browse past ZFS snapshots (read-only, copy to restore)
- Discover nearby network (SMB) shares from the toolbar
- Format a spare drive as ext4 or exfat
- Share a folder directly to the Shares app
- Instant copy on copy-on-write filesystems (Btrfs, reflink-enabled XFS): a
  copy shares the source's blocks instead of rewriting them, so "copy 40 GB"
  finishes immediately and costs no extra space until a side changes. When the
  filesystem cannot share blocks the copy silently falls back to a normal
  kernel copy and then to a verified byte stream — the activity row says
  "cloned" only when the clone really happened. An admin can turn the attempt
  off entirely with the Instant copy setting, which also lists which mounts
  support it
- Collision handling on copy/move/upload (overwrite, keep_both, skip)
- Inline text/code editor for quick edits
- Share a file by a revocable download link (URL + QR code; 'anyone' or 'signed-in' only; default 30-day expiry). A link can also require a password and allow a limited number of downloads — "one download" is the one-time link, and a download only counts once it has finished, so an interrupted transfer does not use it up. The recipient's page asks for the password only when the link has one, and the password is never put in the URL
- Drag a file onto a folder to move it, or drag a rubber-band box to select several at once
- Live auto-refresh pushed over SSE when files in the browsed directory change
- A transfers panel (three zones: uploads, downloads, copies) showing every
  running transfer with its progress bar, bytes transferred out of the total,
  speed, time remaining, and a cancel button per row. A badge on the toolbar
  button says how many are running, and an open upload can be cancelled from the
  panel. A failed transfer stays listed for a minute so its outcome is visible
- An upload that is interrupted resumes where it stopped, even after HomeVault
  restarts: the upload session is stored in the database, not only in memory
- An on-demand checksum of any file: SHA-256, SHA-512 or BLAKE3, computed in the
  request for small files and as a background job for large ones; a second file
  can be compared in the same call, which answers whether the two are identical
  (MD5 is deliberately not offered)
- A transfer history listing finished uploads, downloads, copies, moves and
  checksums with their size, duration, how the copy was made (cloned or copied
  byte for byte), whether it was verified, and whether it succeeded; each user
  sees their own, an administrator sees everyone's, and rows are kept 90 days
- Permanent delete is admin-only, path-guard confined, and refuses to delete a
  browsing root; resumable upload sessions are private to the user who opened
  them and capped in number

PathGuard restricts non-admin users to their home directory and allowed roots."#.to_string(),
        },
        FeatureDoc {
            id: "global-search".to_string(),
            title: "Global file search".to_string(),
            content: r#"Pressing Control+K (or Command+K) on the desktop opens one search box for everything. It finds apps, settings categories and quick actions instantly, and it also finds files anywhere the signed-in user is allowed to look: every folder their Explorer sidebar lists, including their own home directory, the shared folders and the attached storage pools, searched in one go. Each file result shows the file's name and the folder that holds it, so two files with the same name in different shares can be told apart; picking a result opens the Explorer on that folder with the file already selected. The file search looks at names only — it does not index file contents. It is deliberately bounded: a search only starts after two characters, at most sixteen locations are searched, each has a short time budget and the whole answer has a slightly longer one, and the list is capped. When part of the answer is missing — a location that did not answer in time, or one that could not be read — the overlay says so in a sentence under the results instead of pretending the list is complete, and a search that was cut short says that too. Searching for something like a path with slashes is refused with an explanation rather than silently returning nothing. An administrator's search is bounded by the same list of folders, so it never turns into a scan of the whole machine."#.to_string(),
        },
        FeatureDoc {
            id: "rsync".to_string(),
            title: "Rsync server".to_string(),
            content: r#"The Rsync server lets another machine copy files to or from your shares with plain rsync, the way most backup scripts expect. It is off until you turn it on. Each share you expose becomes a module with a name, an optional comment and the list of accounts allowed to use it; every module requires a password, so a module can never be an anonymous export. A module can only serve a folder that is already one of your shares, so this screen can never make another part of the machine reachable. The rsync password is separate from the HomeVault login: set one per account on the same screen, and it is only stored on the server — it is never shown again. Two more safeguards are on by default: the daemon is confined to each module's folder, and it does not wait for a reverse DNS lookup that a home network cannot answer. You do not need this server for a one-off copy: rsync over SSH works with the SFTP account and key you already configured, and that is the better choice for anything leaving your network."#.to_string(),
        },
        FeatureDoc {
            id: "backup-target".to_string(),
            title: "Backup destination".to_string(),
            content: r#"HomeVault can also be the place another computer keeps its backups. Turn the role on, and each computer that sends backups gets its own entry with its own token, its own private repository and its own quota. The token and the repository password are shown exactly once, when the entry is created or when you rotate it — write them down on that machine, because they are never displayed again. A repository receives restic backups in append-only mode, which means the computer that sends the backups cannot delete the history it depends on: ransomware on that machine cannot reach round and wipe its own backups. When a computer grows past its quota it is locked out of its repository until you raise the quota or free space, and the screen says which entry is over. An entry can also ask for a daily integrity check, and Check now runs one immediately. Removing an entry stops that computer's access but leaves its backups on the disk."#.to_string(),
        },
        FeatureDoc {
            id: "replication".to_string(),
            title: "Snapshot replication".to_string(),
            content: r#"Replication keeps a second copy of a dataset on another disk or another machine. A job names a source dataset and a destination — either another dataset on this machine or, over SSH, a dataset on another one — and HomeVault does the rest on a schedule you choose: it takes a snapshot of the source, sends it, and next time sends only what changed since the last successful run. A transfer can be interrupted and resumed instead of starting over, and an optional bandwidth limit keeps it from saturating a home connection. Retention only counts the copies the job itself created, so it never deletes a snapshot you made by hand and never touches the source. Before anything is sent you can preview the exact command the next run would execute, which is the quickest way to check that a destination path or SSH account is what you think it is. Replicating Btrfs snapshots over SSH is not supported yet; use a local destination for those."#.to_string(),
        },
        FeatureDoc {
            id: "traffic-control".to_string(),
            title: "Traffic control".to_string(),
            content: r#"Traffic control keeps one kind of transfer from saturating your home connection. You choose the network interface HomeVault sends through, then give SMB, NFS, FTP, WebDAV or SFTP a ceiling in kbit/s; a value of 0 leaves that service unlimited. The limit applies to traffic leaving HomeVault, which is the direction a client download from a share takes — uploads from clients are not shaped. Administration ports are never limited, so you cannot throttle your own session out of the appliance. An optional weekly schedule narrows when the limits apply: a window can run over midnight, and having no window means the limits apply all the time. Pause all releases the queue immediately while keeping your settings, which is the fastest response when the link is saturated, and Resume puts them back. Saving applies the new policy at once, and Apply now re-applies the saved one. If the shaping tool is not installed on the machine, the screen says so instead of pretending anything changed."#.to_string(),
        },
        FeatureDoc {
            id: "iscsi".to_string(),
            title: "iSCSI exports".to_string(),
            content: r#"iSCSI publishes a disk over the network so another machine can mount it as a local block device — a client formats it itself, unlike a share that serves files. You create two things: a disk, which is either a file HomeVault owns and sizes or a whole block device, and an export, which is the iSCSI name (IQN) an initiator logs in to. An export lists portals (the address and port clients connect to, 0.0.0.0 for every address), LUNs (which number maps to which disk) and the initiators allowed to log in. Listing no initiator allows every client; listing one restricts access to it, and an initiator can additionally be protected with CHAP, a user name plus a 12 to 16 character secret that is stored encrypted and never shown again. Saving only writes HomeVault's settings: press Apply now to run the iSCSI tools and make the exports reachable, which is also how a removed LUN, a removed portal or a disabled export actually stops being served. Apply rebuilds every object HomeVault itself created, so running it twice is safe, and it never touches a target you configured by hand. Turning the feature off disconnects every initiator at once but keeps the disks and exports for later. If the machine lacks the iSCSI tools or kernel modules, the screen hides the editors and names exactly what is missing."#.to_string(),
        },
        FeatureDoc {
            id: "directory-sso".to_string(),
            title: "Directory and single sign-on (LDAP / AD / OpenID Connect / SAML)".to_string(),
            content: r#"Directory and single sign-on let people sign in with the account they already have — on an LDAP or Active Directory server, or at the identity provider that already guards their mail and cloud accounts — instead of a second HomeVault password. One screen offers all three: a three-button switch picks Directory (LDAP / Active Directory), Single sign-on (OpenID Connect) or Single sign-on (SAML), and the list of remote accounts below is shared, with a badge naming the provider each account came from.

For a directory, you point HomeVault at the server, give it a read-only service account to search with, a search base, and a user filter that must contain {username}, and then say which directory groups mean Administrator: the highest matching role wins, and someone in no mapped group gets the default role you pick. Group names can be written as a full distinguished name or just as a group name, because Active Directory reports full names and hand-written mappings usually do not. The password is checked with the directory on every sign-in even when the search result is cached, and a wrong password is rejected immediately.

For single sign-on you register an application at the provider and paste four things here: the button text people will see on the login screen, the issuer URL exactly as the provider reports it, the client ID, and the callback URL to register at the provider, which must end in /api/auth/oidc/callback. A client secret is optional — leave it empty for a public client that only uses PKCE. The callback address is configured explicitly rather than guessed from the request, must be https, and http is accepted only for a loopback address, so a forged host header cannot move where a credential is sent. Behind Advanced you can rename the claims that carry the login name, display name, e-mail and groups, and edit the scopes, which must contain openid. Signing in then leaves for the provider, comes back to the callback, and the answer is verified before anything is stored: the signature against the provider's published keys, an algorithm from an allow-list (never none or a shared secret), the issuer, the audience, the expiry and the nonce from the start of the flow. The claims become a login name, a display name and an e-mail, the groups resolve to a role with exactly the same group mapping as the directory path, and the browser is handed a one-shot ticket rather than a token in the address bar.

SAML is the third way in, for a provider that does not speak OpenID Connect. You paste the provider's entity ID, its sign-in address and its signing certificates (one per line), or simply paste its metadata address and press Read metadata to fill those three fields for you — nothing is stored until you save. HomeVault then shows its own entity ID and sign-in address (the ACS URL) to give back to the provider, plus a copyable metadata document, and behind Advanced you can rename the three attributes that carry the display name, the e-mail and the groups. A sign-in leaves for the provider, comes back as a signed assertion, and the signature is checked over the canonicalised XML against your certificate before anything is stored; the assertion must be addressed to HomeVault, still be inside its validity window, and use a request that was issued here and never used before. The groups then resolve to a role with exactly the same mapping as the other two paths. If no certificate is configured, every assertion is refused rather than accepted unsigned.

Test before saving: the panel tries the form as typed, saving nothing, and for a directory it can also tell you which role a typed login name and password would get — the quickest way to catch a wrong mapping. A local account always keeps its own password and its own two-factor setting, so an integration that is off, misconfigured or unreachable can never lock an administrator out, and the screen always shows how many local administrators can still sign in. A remote account is not a local one: typing its name in the password form is refused with a sentence pointing at the single sign-on button, because the provider stored on the account decides which way in is allowed. Disabling an account here ends its sessions without touching the directory or the provider, and Forget removes the account HomeVault mirrored, so the next sign-in starts clean. The service-account password and the client secret are stored encrypted and never shown again; a SAML signing certificate is public, so there is no secret to keep."#.to_string(),
        },
        FeatureDoc {
            id: "dhcp".to_string(),
            title: "DHCP / TFTP / PXE server".to_string(),
            content: r#"The DHCP server hands out IPv4 addresses on a network you control, which is useful on an isolated lab or a home LAN where the router's own DHCP is switched off. You pick the interface to serve, the first and last address of the range, the subnet mask, an optional gateway, the DNS servers to advertise and how long a lease lasts; a static reservation gives one device the same address every time, matched on its hardware address. The same screen can serve boot files over TFTP, optionally with a PXE boot file, so another machine can start from files stored here. Only the interface you name is served. Turning the server on is confirmed first, because it is the one action that can hand out wrong addresses on a live network, and a second address server already answering on that interface is refused rather than raced. The screen also lists the leases that are currently active, with the address, hardware address, name and expiry. Only an administrator can change or start it, and the address server software must be installed on the machine — the card says so when it is missing instead of pretending anything changed."#.to_string(),
        },
        FeatureDoc {
            id: "local-dns".to_string(),
            title: "Local DNS server".to_string(),
            content: r#"The local DNS server lets HomeVault answer the names of your own network instead of leaving that to the router. It is off until an administrator turns it on under Settings, Security, Local DNS server, and turning it on is confirmed first, because a wrong record here breaks name resolution for every device on the network, not just for the screen. You choose the one interface that answers, and you list the upstream servers that handle the names HomeVault does not know - usually the router's address. Local names come in two kinds: an address record that always answers the address you give, and an alias that points at another local name (an alias whose target HomeVault does not serve is refused, because the name server would silently ignore it). Answered domains send every name in a domain to one address and never forward it, which is how a whole made-up domain is kept inside the house. One switch tells the devices that get their address from HomeVault to resolve their names here too. Nothing about the local network leaks outward: single names are never forwarded, reverse lookups of private addresses never leave, the appliance ignores its own system resolver and uses only the servers you list, and a loopback address is refused as an upstream. Devices on the local network are answered, never one that reached the appliance from the internet. The name server and the address server are the same program: stopping it from the address server screen is refused while the name server is on, and the card says which state the program is in. If the name server software is not installed, the card names the package instead of showing a form that cannot work."#.to_string(),
        },
        FeatureDoc {
            id: "reverse-proxy".to_string(),
            title: "Generic reverse proxy".to_string(),
            content: r#"The generic reverse proxy publishes a service that runs elsewhere on your network under a name of your own, so a name like ha.home.lan reaches a machine and port without anyone remembering the port. A rule names the exact host, an optional path prefix, the service address (a private LAN host and port), whether the service speaks HTTPS, whether a HomeVault sign-in is required, and whether the rule is active. Several rules can share a host: the longest path prefix wins, on a segment boundary, and the service receives the full path unchanged. The browser always reaches this appliance over plain HTTP; the HTTPS switch only encrypts the hop from the appliance to the service. A rule answers a name only if that name already resolves to the appliance, which is what the local DNS server card is for. The sign-in gate is on by default and never hands your dashboard session to the service: the browser is sent through the dashboard, which validates the real session and returns a one-time ticket that the proxy exchanges for a session valid only on that rule's own name. The proxy never trusts a client-supplied forwarding header, never forwards a HomeVault cookie, only reaches private or loopback addresses, refuses to point back at its own port or the dashboard, and relays WebSocket upgrades. If the listener cannot bind its port, the card says so with the reason and the rest of the appliance keeps working. Only an administrator can add, change, disable or delete a rule."#.to_string(),
        },
        FeatureDoc {
            id: "usb-copy".to_string(),
            title: "USB one-touch copy".to_string(),
            content: r#"USB one-touch copy takes files off a memory card or a USB disk and puts them in a folder on the appliance, so you do not have to open a file manager. A copy rule names the device, the folder inside the device to read (for example the photo folder on a camera card) and the folder on the appliance to write to, and it matches the device by its identifier rather than by its device name, which changes between reboots. A rule can be inactive, which keeps it on the list without ever running it. Two optional steps run after the copy: checking every copied file so a damaged copy is caught immediately, and ejecting the device so it is safe to unplug straight away. You can copy now from the rule, or let it run when the device is connected. The screen lists the removable devices that are attached, with their identifier, size, whether they are mounted, and which rule they matched, so plugging a device in and seeing nothing happen is answerable without reading logs. A disk that already belongs to a RAID or pool is refused as a source, because copying the raw members of an array would look like a backup and be useless as one. Only an administrator can create a rule or start a copy."#.to_string(),
        },
        FeatureDoc {
            id: "fan-control".to_string(),
            title: "Fan and thermal control".to_string(),
            content: r#"Fan control keeps the disks cool without making the appliance loud. HomeVault reads the temperature and fan channels the machine exposes, and you choose a profile: automatic, which hands control back to the firmware; quiet, balanced or performance, which each follow a different temperature curve; or a fixed speed you pick. A per-channel override can pin one fan to a different profile than the rest. Changing what the firmware does requires writing hardware registers, so only an administrator can apply a profile. Several safeguards are not optional: the speed never drops below twenty percent whatever the curve says, on any error the fans go back to firmware control, and a small temperature change does not move the fans so they do not become audible hunting up and down. Restoring automatic control also resets the saved profile, so a performance setting is not re-applied at the next boot. On a laptop the card explains that the embedded controller owns the fan and offers nothing to configure, and on a machine with no controllable fan it says so rather than showing an empty list."#.to_string(),
        },
        FeatureDoc {
            id: "ups-server".to_string(),
            title: "UPS server mode".to_string(),
            content: r#"UPS server mode makes HomeVault the machine that other computers ask about the battery, so everything on the network can shut down together when power runs out — the counterpart of the UPS card that watches a battery from this appliance's side. You choose the interface to serve, which is never the whole network by default, the port, how many clients may connect, and which machines are allowed: only the listed ones may read the battery state, and a client can be marked as allowed to shut the appliance down. The shutdown itself follows a policy you set, in order: stop the containers, stop the virtual machines, finish the running backups, stop the file sharing services, then power off. Two of those steps are not available in this version and are reported as skipped, never as done, so a partial shutdown is never mistaken for a complete one; the screen says which ones. A shutdown must be confirmed explicitly, because it ends the machine. Ports twenty-two, eighty, four-four-three and eighty-four-four-three are refused, the last one because it is HomeVault's own alternative web address and taking it would make the dashboard unreachable. Only an administrator can change the configuration or issue the shutdown, and the battery server software must be installed on the machine."#.to_string(),
        },
        FeatureDoc {
            id: "guided-redundancy".to_string(),
            title: "Guided redundancy (RAID layout suggestions)".to_string(),
            content: r#"Guided redundancy shows what the disks you have could become, so you do not have to know which RAID level to pick. Given the attached disks, it offers the layouts their number and sizes allow — a mirror, a stripe, parity with one or two disk tolerance, a stripe of mirrors, or a mirror with a hot spare — and for each one shows the usable capacity, the efficiency as a percentage of the raw disks, how many disk failures it tolerates, how long a rebuild would take and a plain-language risk assessment. The smallest disk bounds a parity layout, because the array can only use that much of every member. Safety is weighed above raw capacity: a layout with no redundancy is penalised in the ranking, and an option that accepts no redundancy is only shown when you ask for it. Disks already in use are excluded from the suggestions. The recommendation engine is complete, but creating the array from a suggestion is not implemented yet: the apply action answers that it is unavailable and points at the storage screen, and the dialog says so before you click. Only an administrator can reach the apply action."#.to_string(),
        },
        FeatureDoc {
            id: "iso-mount".to_string(),
            title: "ISO and IMG mounting (read-only)".to_string(),
            content: r#"Mounting a disc image opens it as a folder, so the contents of an installer ISO, a disk image or a virtual-machine appliance can be browsed and copied out without extracting anything first — a six gigabyte image opens instantly instead of becoming six gigabytes of files. You pick an image in the file explorer and choose to mount it; it then appears as one more location on the left, and a second click unmounts it. The image must be a file with the iso or img extension, and it must be in a folder you can already read, so mounting never reaches anywhere the file browser would not. Everything is mounted read-only, which means browsing cannot damage the source image, and the mount is marked so that a crafted image cannot introduce a program that runs with extra rights or a device file into the appliance. Each image gets its own folder named after the file plus a short code derived from its full path, so two files with the same name in different folders mount side by side instead of one replacing the other, and mounting the same image twice is refused rather than stacking a second device on it. Mounting and unmounting are administrator-only, because both need root access and because a mount point is a folder the whole appliance can browse. If the mounting tools are not installed on the machine, the panel says which ones are missing and offers no action instead of failing on every click."#.to_string(),
        },
        FeatureDoc {
            id: "sms".to_string(),
            title: "SMS alerts".to_string(),
            content: r#"SMS alerts send one short text message per alert to a phone, on the same events and the same severity threshold as the email and the webhook — useful when the appliance is headless and nobody is watching an inbox at night. HomeVault does not embed any vendor's API: you paste the address of your gateway, the body it expects with the recipient and the text marked by placeholders, and an optional access token, so any provider works — Twilio, OVH, a self-hosted gateway — and a vendor changing its API cannot break the appliance. The phone number must be written in international form with a leading plus sign and 8 to 15 digits, for example +33612345678; a number without a country code is refused with a message that shows the expected shape, because it would be billed but never delivered. A configuration that is switched off is never refused, so a half-filled form can always be cleared. The access token is stored encrypted and is never shown again: the settings report only whether one is stored, and leaving the field blank when saving keeps the existing one, which is how you can change the number without retyping a credential you cannot see. Each message leads with the severity and the title, collapses line breaks and runs of spaces because they waste the character budget, and is truncated with a visible marker at one text-message length, so a long alert is visibly cut rather than silently shortened. If the text gateway cannot be reached the failure is logged and the other channels still send; a stored token that can no longer be decrypted, which happens when a data directory is restored on another machine, is reported so you know which setting to re-enter rather than sending a request that would be rejected."#.to_string(),
        },
        FeatureDoc {
            id: "disaster-recovery".to_string(),
            title: "Disaster recovery".to_string(),
            content: r#"Disaster recovery is the plan for getting your data back, and each kind of copy answers a different question. Snapshots undo a mistake — a deleted folder, an overwritten file — but they live on the same disk, so they disappear with the pool. Replication keeps a second copy on another disk or another machine, and a backup received from another computer is a copy of that computer's data. Configuration backup covers accounts, groups, shares, permissions, quotas and settings, not data: restoring it makes the appliance behave as before, then you restore the data on top. A real rebuild is: install HomeVault, restore the configuration archive, then re-attach the data from the pool itself, from a replica, or with a restic restore — and finally re-issue what was tied to the old machine, such as the certificate, share links, tokens and passkeys. Restores are worth rehearsing: restore one folder to a scratch location and compare a checksum, and run the integrity check on each backup destination, so you know the backups are good before you need them."#.to_string(),
        },
        FeatureDoc {
            id: "snmp-agent".to_string(),
            title: "SNMP agent".to_string(),
            content: r##"The SNMP agent lets a monitoring tool on the local network read this appliance's interfaces and disk usage. It is off by default: nothing answers an SNMP request until an administrator enables it under Settings, Monitoring, SNMP agent. The read-only community is generated on enable (32 characters) and never "public" or "private"; a custom one must be 8 to 32 characters with no spaces, quotes or "#". The agent binds either to this machine only (127.0.0.1) or to one of the appliance's own private addresses for the local network - never to a wildcard and never to a public address, so SNMP is never published to the internet. It answers read-only queries only, limited to the system, interface and storage groups. Disabling stops and disables the service and deletes the configuration file. If the snmpd package is not installed the settings card says so instead of showing an unusable form. This is separate from the SNMP client feature, which queries other devices."##.to_string(),
        },
        FeatureDoc {
            id: "file-tags".to_string(),
            title: "File tags".to_string(),
            content: r#"File tags let each account add short private labels to files and folders in the Explorer. A path can have up to twenty tags, and a tag can filter the file-name results in the desktop search. Tags never grant access: adding or listing them is limited to paths the account can already browse, and search only filters results from the account's currently visible roots. Tags are stored on the appliance and removed when the account is deleted."#.to_string(),
        },
        FeatureDoc {
            id: "storage".to_string(),
            title: "Storage Management".to_string(),
            content: r#"The Storage app manages disks, partitions, and virtual disks:
- View all physical disks and partitions with usage
- Create RAID arrays (mdadm) via the RAID wizard
- Create ZFS pools with various redundancy levels
- Create Btrfs volumes
- Virtual disks tab: list, scrub/verify, rebuild degraded arrays, grow an mdadm array onto a free disk, destroy
- Check a virtual disk's scrub / consistency-check status inline
- Import an existing (foreign) ZFS or Btrfs pool found on attached disks, without recreating it
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
- Container lifecycle: start, stop, restart, pause, resume, kill, rename, remove, view logs
- Live log streaming (tail size, timestamps, follow), with download
- Container details (image, state, limits, mounts, ports, networks, environment, labels)
- Change a container's memory, CPU and restart policy in place, without recreating it
- Run a one-off command inside a container
- Update a container to its newest image (pull, then recreate with the same settings)
- Edit a container's docker-compose.yml in place (Compose-managed containers), then save or save and redeploy
- Image management: pull, build from a host folder, remove, view layers
- Private registry sign-in and sign-out for pulling private images
- Disk usage report and cleanup of unused images, volumes, containers, networks and build cache
- Network management
- Volume management: list, create, delete, see which containers use a volume
- Docker engine settings (daemon.json) editable from the interface
- App Catalog: install pre-configured apps from manifests
- Community stores: add third-party app repositories
- Per-app reverse proxy with session-cookie auth gate
- Port conflict detection before container creation
- Docker Compose deploy and management, including pulling the newest images
- Container I/O stats (network rx/tx, block read/write)
- App update detection (image digest comparison with registries)"#.to_string(),
        },
        FeatureDoc {
            id: "virtual-machines".to_string(),
            title: "Virtual Machines Management".to_string(),
            content: r#"The Virtual Machines app is a guided, non-technical interface for Incus VMs and system containers:
- A create wizard: pick an operating system from a short curated list (Ubuntu, Debian, Fedora, Rocky, Alpine, and Windows 11/10), name the machine, choose a size (small/medium/large), and set a login username + password
- Windows entries are offered as an ISO installation (no ready-made image); the wizard switches to ISO mode automatically
- The login account is seeded into the machine automatically via cloud-init; the password is stored only as a SHA-512 hash, never in plain text
- Optional sound card for a virtual machine
- VM lifecycle: create, start, stop, restart, delete (long operations are awaited so status is always accurate; image downloads run as a tracked background job)
- Live status, IPv4 address, CPU/memory/disk and snapshot count for every machine
- Change CPU or memory after creation
- Saved states (snapshots): create, roll back, delete
- A built-in shell console (xterm) into any running machine — no VNC client needed
- A graphical screen for ISO installations (noVNC), which can be popped out into its own browser tab
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
- Restore from snapshots, with a pre-flight check of the space the restore will need
- Reminders when a repository has not been backed up for a while
- Retention policies (keep last N, daily, weekly, monthly)
- Backup scheduling with cron expressions
- Progress tracking via SSE, and a floating progress panel that stays visible while you use other apps

Schedule lifecycle: create schedule -> set retention policy -> enable. Failed runs are rescheduled automatically.
HomeVault also snapshots its own database. That matters because the database runs in write-ahead-log mode: copying the single database file on its own gives a file with no tables in it, so the obvious manual backup silently produces nothing restorable. The snapshots here are self-contained single files, written automatically at startup when due, daily, and before any upgrade that changes the database layout — and on demand from Settings, where the schedule and how many to keep are also configurable. Restoring one is a matter of stopping the service, copying the file over the live database and starting it again."#.to_string(),
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
- A password the admin chose or generated (at account creation, or in the
  Security card) must be replaced by its owner: the next sign-in accepts only
  a password change, and the session is limited to that step until it is done
- The Security card also lists the account's active sessions (token masked to
  an 8-character reference - the admin can audit but never replay) with a
  "Sign out everywhere" button that revokes all of them at once
- An "Account expiry" card sets a date after which the account can no longer
  sign in (its live sessions are revoked too); "No expiry" is the default and
  removing the date restores the account without touching its lock state
- The access card also lists the grants the member inherits from their groups
  ("Inherited from groups") and can grant a resource to one of those groups;
  every member of the group gets it, strongest level winning, and leaving the
  group removes it at once
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
- WebDAV: HTTP-based file access, served by HomeVault itself (in-process Rust server on its own port)

Features:
- Per-user ACL grants on shares
- Per-member share access grants (a member sees only the shares they hold)
- SFTP jail: users are chrooted to their granted shares
- Public key authentication for SFTP
- Share enable/disable toggle
- A per-share recycle bin for SMB: files deleted over the network are kept in the share's own trash folder, then show up in the File Explorer's Trash where they can be restored
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
- A raw-config editor for advanced rclone settings (admin only)

Mount feature: mounts a remote as a local directory using FUSE. The server owns the
mount process: a mount is reported successful only once it appears in the kernel's
mount table, its errors are shown with rclone's own message, and it is unmounted when
the server shuts down. Mount points live under /mnt/rclone.

Safety: the rclone config file holds cloud credentials, so it is written owner-only
(0600) together with its dated backup. Sync and copy jobs are saved definitions; a job
runs only when you click "Run now" — there is no automatic schedule yet."#.to_string(),
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
- Link aggregation: combine physical network cards into a bond in 802.3ad (LACP, the switch must be configured for it) or active-backup mode (no switch support needed), and add more cards to a live bond later
- VLANs: create eth0.42-style tagged interfaces on a network card or on a bond, with ids from 1 to 4094
- Bonds and VLANs get their address through the same per-interface configuration card as a physical card; only physical network cards can join a bond or carry a VLAN, and a change that would cut the administrator's own connection is refused with the reason

DNS updates preserve existing search/domains. IPv6 nameservers are appended."#.to_string(),
        },
        FeatureDoc {
            id: "health".to_string(),
            title: "System Health Monitoring".to_string(),
            content: r#"The Health page shows the appliance's 9 built-in health checks, each on its own card plus one overall status strip (the worst individual check wins):
1. Authentication: performs a real read of the account store in the database; an unreachable or damaged database shows as unknown.
2. Storage: the appliance's own data directory is readable and writable.
3. Memory: RAM/swap usage enough to warn on pressure.
4. Disk Space: per-disk free space via df.
5. Disk S.M.A.R.T.: per-disk overall health via one sudo smartctl -H probe per disk, cached 10 minutes.
6. RAID arrays: degraded or failed mdadm arrays raise a warning or error.
7. Network: link state of every interface, read from /sys/class/net (operstate), not from traffic counters; loopback is always counted as up.
8. Services: cron, login manager, network manager plus the appliance's own systemd unit, whose name is parsed from the process cgroup; the appliance's own unit failing is unhealthy.
9. Temperatures: CPU and drive temperature sensors.

A check that cannot run (a missing binary, an unreadable file, a database that will not answer) degrades to "unknown" on its own card; the other cards still run and the page still answers. The overall verdict uses exactly four values: healthy, warning, unhealthy, unknown. If the page cannot load the data at all it shows an explicit error message with a Retry button instead of pretending to have no data.
For monitoring, two small public addresses answer without a login: one says the server process is alive, the other says it is actually ready to serve — that second one fails when the database cannot be queried or was found damaged at startup, so an external monitor can take the appliance out of rotation instead of feeding it requests that will fail. Neither reveals anything about the machine.

The same page carries the Security Advisor, above the per-check grid and for administrators only: one read-only answer to "is this box safely configured?". It runs fifteen checks over state the other screens already show — an administrator exists and none uses a default account name (admin, administrator, root), every administrator uses two-factor authentication, no administrator still holds an administrator-chosen password, HTTPS is on with a trusted certificate that is not about to expire, automatic certificate renewal, the firewall is on with the rules that keep SSH and the web interface reachable, SMB refuses guests, requires signing and does not accept SMB1, no share folder is world-writable, share size limits, snapshots scheduled on the data pools, a configuration backup exists, failed sign-ins are blocked, and HomeVault is up to date. Each finding carries a severity (pass, info, warn, fail), the evidence it is based on, the sentence that fixes it and a link to the screen that owns the setting; the card shows a weighted score out of 100 and lists the actionable findings worst first, with the rest behind a disclosure. A check that cannot read its signal says so and is never counted as a pass. The advisor only reads: it never changes a setting, and nothing it answers is cached, so a fixed finding disappears on the very next run. The report names accounts, the certificate, the firewall and the shares, so it is administrator-only."#.to_string(),
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
- Locale-independent status detection (LC_ALL=C)
The interface is delivered with a strict content policy: inline scripts are only allowed when the server stamped them for that exact page load, so a script injected into the page cannot run — which matters because the session credential lives in browser storage. The installer also handles the rest of the appliance's life: it can roll back to the previous build, uninstall while keeping all data, or uninstall and delete everything after an explicit typed confirmation, and every mode has a preview that changes nothing."#.to_string(),
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
- No ACME/Let's Encrypt yet. An external reverse proxy terminating TLS is still supported.
Turning HTTPS on redirects plain-HTTP visitors to it by default, so the password never travels in clear once encryption is configured. Until it is, the first-run wizard and the login screen say so plainly whenever the page is opened over plain HTTP from another machine — localhost is exempt, since nothing leaves the computer there. The session cookie is marked as HTTPS-only exactly when it was issued over HTTPS."#.to_string(),
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
- Appearance: wallpaper (bundled) and accent colour
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
- Wake-on-LAN: save a machine by its network-card address and wake it from the
  settings card, or let a scheduled task wake it and wait before it runs
- Connected clients: see who is using the box right now over SMB, NFS, SFTP
  and FTP — the account, the address, the share or export they hold and how
  many files are open — and disconnect a session that can be dropped (an NFS
  export cannot: the kernel holds it)
- Full configuration backup: export one archive holding the database (accounts,
  groups, shares, permissions, quotas and settings), the exported settings and
  the generated service configurations, optionally encrypted with a passphrase;
  by default no working credential is inside. A restore is validated and staged,
  then applied at the next start, keeping the previous database on disk
- Per-share size quotas: cap one shared folder so it cannot fill the pool — the
  limit is enforced by the filesystem holding it (ZFS, a Btrfs subvolume, or XFS
  project quotas), so writes over the network are bound as well as uploads in
  the browser, and the Shares screen shows a used-of-quota bar
- Failed sign-in blocking: after a set number of wrong passwords from one
  address, that address is blocked for a chosen time — the block survives a
  restart, can optionally be installed in the firewall, and can be pardoned
  in one click (which also removes the firewall rule). A request-rate limit
  refuses floods of unauthenticated requests
- Remote syslog: forward the audit events to a collector over UDP, TCP or TLS,
  in RFC 5424 or RFC 3164, with a pinned TLS certificate. A slow or unreachable
  collector only fills a bounded queue and never slows a request, and the card
  shows what is waiting, what was sent and what was dropped

Application events are always in plain English for audit trail purposes."#.to_string(),
        },
        FeatureDoc {
            id: "activity".to_string(),
            title: "Activity Taskbar".to_string(),
            content: r#"A dock button on the desktop (visible only while background tasks run) with a badge showing the active-task count. Clicking it opens a panel above the dock with one row per task: name, a progress bar, and a percentage.

Sources shown: aria2 downloads, File Explorer copy/move of large folders, Restic backups/restores/verifications, antivirus scans, disk cache mover, Docker app install/update, disk cloning (ddrescue) and file recovery (photorec), knowledge-base reindexing, AI model downloads, and cloud sync. A server-side poller also reports work HomeVault did not start itself: a RAID array resyncing, recovering or reshaping (read from /proc/mdstat), a ZFS or btrfs scrub in progress, and a Syncthing folder that is still catching up.

Endpoint: GET /api/activity (any signed-in user; non-admins only see their own tasks). The panel is display-only. Not covered: a LUKS format is a one-shot command with no progress to read, so it never appears as a row."#.to_string(),
        },
        FeatureDoc {
            id: "auth".to_string(),
            title: "Login, Two-Factor Authentication & Recovery".to_string(),
            content: r#"Signing in, and getting back in when something is lost.

- Log in with a username and password. When two-factor authentication is enabled, the login returns a second step that asks for the current 6-digit code before the session opens.
- Five wrong passwords lock the account for five minutes. The lock lifts itself — waiting, or resetting the password with the recovery code, gets you straight back in — so a mistyped password can never lock you out of your own appliance for good.
- Attempts are also throttled per client address, so a password tried against many usernames at once is slowed down instead of running unchecked.
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

The wizard is reboot-safe: the current step is remembered on the server, so a reboot or a closed browser resumes where the person left off (passwords and secrets are never saved).

- Step 1 — welcome: what HomeVault is.
- Step 2 — account: choose the administrator username, with availability checked as you type, and optionally a full name.
- Step 3 — password: type it and confirm it. Two-factor authentication can be enrolled right here: a QR code is generated in the browser from a fresh secret and the first 6-digit code is verified before the account exists, so a mis-scanned code is caught immediately instead of locking the administrator out later.
- Step 4 — review and create: a summary of the chosen account, then creation. A one-time recovery code is shown once afterwards and has to be stored somewhere safe; it is the only offline way back into the account. When two-factor authentication was enrolled, the one-time backup codes are shown next.

The interface language is never asked: it is detected from the operating system or browser (English when that language is not one HomeVault ships) and can be changed afterwards in Settings.

Storage pools, RAID arrays and backup restores are deliberately not part of the wizard: the appliance is installed on an existing system, and disks are configured afterwards from the Storage app while a backup is restored from the Backup app.

The whole wizard, including the QR-code step, follows the interface language detected from the operating system or browser — English when that language is not one HomeVault ships — and that language can be changed in Settings. The wizard refuses to run twice: once an administrator exists, the setup endpoints report that the server is already configured.

The administrator password must be at least 12 characters, mix upper and lower case with a digit, must not be a commonly used password, and must not repeat one character more than four times in a row. The same rule applies everywhere else a password is set.

For safety, the first-run endpoints only answer clients on the local network and only from HomeVault's own interface in the browser — a website open in the same browser cannot claim the administrator account — and they are rate-limited. If a first-run setup is interrupted (crash, power cut), the leftover lock is detected as abandoned and the setup can be run again instead of being blocked forever. If the hardware fingerprint changes later (after moving the system disk or cloning the machine), the interface asks an administrator to sign in once and confirm the new hardware instead of showing the wizard again."#.to_string(),
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
- Groups are the sharing unit: a file-share permission can be granted to a group instead of to each person individually, a resource access (app, folder or share) can be granted to a group, and a transfer quota can be set per group. A group grant applies to every current member and stops applying the moment someone is removed.
- A group can be renamed and described from its detail view. Renaming keeps everything the group was granted (resource access, quotas, share permissions) attached, and the admin group cannot be renamed because it carries the root-level sudo grant.
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

- Encrypt a device that is not currently in use — unassigned, and not part of an array. Formatting runs in the background with a progress bar; you can close the window while it finishes.
- Lock and unlock volumes, change a volume's passphrase, and delete an encrypted volume, which erases the LUKS header and wipes the leftover filesystem signatures.
- A new passphrase must be at least 12 characters, must not be a common password and must not repeat the volume name. Existing volumes stay unlockable with their original passphrase.
- Deleting is refused while the volume is still mounted or still unlocked, so key material is never destroyed under a live mount.
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
- Three steps: choose the disks, choose the level and the target, then confirm. A diagram shows the usable capacity and the redundancy each level buys, and the recap lists exactly what will be created; disks of unequal sizes are flagged with the raw capacity their difference wastes.
- Every candidate disk is listed: free disks can be selected, disks already in use stay visible with a short reason why (partitions, a filesystem, a mount, another array) and their S.M.A.R.T. health. Extra checked disks can be attached as hot spares on redundant mdadm levels, and an mdadm mirror/RAID 5/RAID 6 can be created deliberately degraded (a slot reserved for a disk added later, up to two missing on RAID 6).
- Creation runs in the background as a tracked job: the wizard shows live milestones (array creation, resync, formatting, mounting) and the result appears in Storage when done.

This screen only creates. Listing the arrays, verifying or scrubbing them, rebuilding a degraded one, growing an mdadm array onto an extra disk ("Add disk" in Storage), and destroying them all live in the Storage app's "Virtual disks" tab, which is the single place a virtual disk is managed once it exists. Array health is also surfaced by the dashboard Health screen's RAID check and its notifications."#.to_string(),
        },
        FeatureDoc {
            id: "snapshots".to_string(),
            title: "Snapshots & Rollback".to_string(),
            content: r#"Point-in-time snapshots of ZFS and Btrfs pools: create them, list them, delete them, and roll a pool back to one.

Where snapshots are used:
- The File Explorer can browse the snapshots that cover a folder (read-only, on ZFS), so an older version of a file can be copied back out without rolling anything back.
- Backups lists the snapshots of a repository.
- A virtual machine can take a snapshot and be restored to it.
- Storage's Snapshots tab manages scheduled snapshots: each schedule takes a snapshot of a pool on a cron and applies GFS (grandfather-father-son) retention — keep the newest N hourly, daily, weekly, monthly and yearly generations. Pruning only ever deletes snapshots carrying the schedule's own prefix (default hv-auto-); manual and backup snapshots are never touched. Schedules are admin-only.

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
- Folders: the synchronized folders with their state. A folder can be added, paused, resumed, rescanned, and removed.
- Devices: the known devices with their connection state, and pairing or removing one.
- Sharing: a new folder is shared with the devices you tick, two-way by default; a send-only or receive-only mode is available, and leaving every device unticked keeps the folder on this appliance.
- The folder path must be inside a storage location HomeVault manages. Syncthing creates the directory, so a folder that does not exist yet is fine — but nothing outside those locations is accepted.
- A device is paired by pasting the 56-character ID the other device shows under Actions, then Show ID. Dashes and lower-case are accepted.
- Removing a folder or a device only changes Syncthing's configuration: files already on disk are kept, and a removed device can be paired again later.
- The daemon itself can be restarted from the screen.

Syncthing keeps running independently of HomeVault — this screen is a management view over it, not a replacement. Folder paths and device keys are sensitive, so every action here requires an administrator. If the daemon is not reachable, or if its own configuration needs attention (for example an API key that HomeVault cannot use), the screen explains what to fix instead of failing silently. A rejected value is reported next to the field that is wrong, with what you typed still in place."#.to_string(),
        },
        FeatureDoc {
            id: "downloader".to_string(),
            title: "Downloader (aria2)".to_string(),
            content: r#"A download manager backed by aria2: paste a link and let the server fetch it.

- Start the download service from the screen when it is stopped — opening the app never starts a background process on its own.
- Add a download, optionally choosing a sub-folder inside the download folder and the output file name; then pause, resume or remove it.
- Accepted links are http, https, ftp, sftp and magnet. Local paths (file://) and other schemes are refused, and every file is saved inside the server's download folder — a sub-folder can be named but cannot escape it.
- Follow progress live: percentage, speed, size and connection count.
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
- The knowledge base is managed from Settings, under AI Assistant: that screen shows how many documents are indexed, says whether the embedding model is installed (and offers to install it), and starts a re-index, whose progress is followed live.
- Indexing is an explicit administrator action and runs in the background, so the request returns immediately and the progress is polled.
- Documents are split into chunks of roughly 400 words with an 80-word overlap, converted into vectors through Ollama, and stored in the database.
- At question time the question is converted the same way, the closest chunks are found by cosine similarity, and the best matches are inserted into the instructions given to the model.
- Re-indexing updates the existing documents, adds new ones, and removes those the sources no longer contain instead of leaving them behind. It does not duplicate anything. Nothing is read from the source tree: everything indexed is embedded in the installed application.
- A re-index that cannot reach the embedding model fails and reports the failure rather than storing documents no search could ever find.

The embedding model is fixed to nomic-embed-text, which has to be pulled before the first index. The chat screen points at the knowledge base when it is still empty, and if the embedding step is unavailable the chat still works, simply without retrieved context."#.to_string(),
        },
        FeatureDoc {
            id: "notifications".to_string(),
            title: "Notifications".to_string(),
            content: r#"Turns system events into notifications that can be read in the interface, sent by email, or posted to a webhook.

- A background check runs once a minute. It evaluates the system health checks and the recent backup jobs.
- Health problems are reported on transition: a check becoming degraded, or recovering from one, produces a notification. A problem that simply persists does not re-notify every minute.
- Backup results are reported once per genuinely new completed or failed job.
- The bell in the top bar carries an unread count and opens a persistent notification center where the full history is kept: filter by severity, show only unread, or search by text, then mark items read or clear them.
- New notifications also arrive live while the page is open, so the badge updates without waiting for the next refresh.
- After an update, a "What's new" summary of the release highlights is shown once per account; acknowledging it keeps it from reappearing on other devices.
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
- The Edit Widgets app is a drag-and-drop layout editor: pin up to three widgets as featured, reorder the list, and toggle visibility. Reordering is also available with arrow buttons for keyboard and touch.
- The Weather panel additionally remembers the city it was set to.

The panels are read-only: they display system state and link to the app that manages it."#.to_string(),
        },
        FeatureDoc {
            id: "shortcuts".to_string(),
            title: "Web Shortcuts".to_string(),
            content: r#"Pin web pages as icons on the desktop so a frequently used site is one click away.

- Add a shortcut from the app launcher: its last tile, after every app, opens the add dialog where you enter a web address; HomeVault reads the page's title, description and icon to build the tile.
- Each shortcut opens the page in a new browser tab. Shortcuts can be removed individually.
- Shortcuts are remembered per account, so each person sees their own set.
- Only regular web addresses (http and https) are accepted; addresses pointing back at the server itself or at a private network are refused."#.to_string(),
        },
        FeatureDoc {
            id: "desktop".to_string(),
            title: "Desktop Shell & Windows".to_string(),
            content: r#"Every app opens inside the desktop — a windowed environment living in the browser tab, arranged like a desktop operating system.

- App launcher: the searchable grid of every available app, opened from the top bar. Its last tile adds a web shortcut to the desktop.
- Top bar: the floating strip carrying the launcher, the open windows, the network indicator, the widgets toggle, notifications, and the power menu, which also holds the account menu.
- Dock: the row of app shortcuts along the bottom, with a badge when a background task is running and a label on hover.
- Windows: each app opens in a window that can be dragged, resized, minimized, maximized and closed. Windows are listed in the top bar and can be brought back to the front.
- Two kinds of window content: an app's own interface, or an embedded web page — which is how an installed catalog application is shown, in a sandboxed frame.
- Floating desktop panels (see Widgets) sit above the wallpaper, alongside the windows.
- Power menu: restart the server, shut it down, or log out. Restart and shutdown require administrator rights and an explicit confirmation.
- The wallpaper and the accent colour are chosen here and apply to the whole shell.
- The layout adapts to the window size, from a wide screen down to a phone: below a narrow threshold the maximized window insets instead of hiding the chrome, and the launcher switches to a scrolling list."#.to_string(),
        },
        FeatureDoc {
            id: "apps-center".to_string(),
            title: "Apps Center (app launcher)".to_string(),
            content: r#"The launcher: a searchable grid of every application HomeVault offers, opened from the top bar and also reachable with a keyboard shortcut.

- A row of category chips filters the grid in place; "All apps" restores the whole catalog. The chips are listed alphabetically.
- The apps are listed alphabetically, A to Z, whatever filter is active, so you can find one by scrolling to its name instead of looking through groups.
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
            content: r#"The look of the whole appliance: the wallpaper and one accent colour. There is no dark or light theme and no density setting.

- The accent colour recolours interactive elements — buttons, links, switches, selection states — across every screen.
- The look belongs to the appliance, not to an account: an administrator chooses it once and everyone sees it. A non-administrator can see the wallpaper picker but cannot change it, and the screen says so.
- Changes are staged while you adjust them and applied when you save, so the interface is not repainted on every click.
- The choice is stored on the server, so the same account gets the same look from any browser, and the wallpaper is picked from the bundled set.
- Every screen reads the shared colour and spacing tokens rather than hard-coding values, which is what makes an accent change apply everywhere at once."#.to_string(),
        },
        FeatureDoc {
            id: "wallpapers".to_string(),
            title: "Wallpapers".to_string(),
            content: r#"The desktop background, chosen from a set of images shipped with HomeVault.

- Forty-four bundled wallpapers, each shown as a thumbnail in the appearance settings; selecting one applies it to the desktop.
- The thumbnails are small versions of the same images, so the grid stays light.
- There is no wallpaper upload: the set is fixed, on purpose, so nothing unbounded can be pushed into the interface.
- The choice belongs to the appliance: an administrator saves it and every account's desktop picks it up, on every browser and every address. Only an administrator can change it."#.to_string(),
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

Scope, precisely: this applies to transfers made through the built-in File Explorer. Shares served by SMB, NFS and FTP are handled by separate system services, and WebDAV by the in-process server, with no per-request visibility here, so transfers over those protocols are neither counted nor capped by this feature, whatever is configured.

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
- Discovery can be turned off in Settings, under General, with the "Discoverable on the local network" switch. Turning it off removes the advertisement and the pinned name from the machine, so the appliance stops answering as homevault.local until it is switched back on.
- Changing the server's advertised name does not interrupt the other devices and printers that this machine announces on the network.

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
            id: "acme".to_string(),
            title: "Certificate Automation".to_string(),
            content: r#"Gets a certificate a browser accepts without the admin renewing it by hand every three months.

- One public name per certificate. The admin picks the name, a contact address, and whether the authority's terms are accepted.
- Two ways to prove the name is yours: an answer served on port 80 of that name, or a TXT record published by a DNS provider. The first needs port 80 to reach the appliance; the second works behind NAT but needs the provider credential, or a manual step.
- With a manual DNS provider the flow stops and shows the record to create; once it exists the admin confirms and the run finishes. With a provider credential the record is published, validated and removed automatically.
- Renewal runs on its own a month before expiry, with a backoff on failure. A refusal by the authority is final: the appliance stops retrying until the name, the challenge, the provider or the directory changes, so a permanently broken configuration does not hammer anyone.
- The provider credential is write-only: it is never returned by the API and never appears in a log line.
- Once a certificate is issued, the built-in HTTPS listener can use it. Switching the listener to that mode is refused until a certificate actually exists.
- Two notification events: a certificate was installed, and a renewal failed."#.to_string(),
        },
        FeatureDoc {
            id: "ddns".to_string(),
            title: "Dynamic DNS".to_string(),
            content: r#"Keeps a public name pointed at the address the ISP currently hands out, so a certificate stays valid and a bookmark keeps working.

- Supported providers: DuckDNS, dynv6, Cloudflare (an A record managed through its API), and any service that accepts an update URL with the address in it.
- The worker looks at the address every five minutes and contacts the provider only when it actually changed — a home connection must not hammer a free service.
- The address is detected without contacting anyone: a UDP socket is connected to a public resolver and the kernel's chosen source address is read. Nothing is sent.
- The provider credential is write-only from the API's point of view and never travels in a URL when the provider offers a header.
- A custom update URL must be HTTPS: plain HTTP would publish the address to anyone on the path.
- The card shows the address the host would publish, the address the provider is believed to hold, and the reason the last attempt failed, if any."#.to_string(),
        },
        FeatureDoc {
            id: "reachability".to_string(),
            title: "Reachability Advisor".to_string(),
            content: r#"Answers whether a browser outside the house will actually reach the appliance, with a certificate it trusts.

- The report is an ordered list of conditions, each with a state (ok, warning, failure), what is wrong, and what to do about it — not a score.
- Order matters: it starts with whether HTTPS is on, then whether the certificate is one a browser accepts, then whether renewal is automatic, then whether a stable name exists, then whether the names agree, then the ports.
- A self-signed certificate is a warning, an expired one is a failure. A certificate inside its renewal window is a warning that names the number of days left.
- A failed certificate renewal or a failed dynamic-DNS update is reported with the provider's or the authority's own reason.
- The check on port 80 appears only when the port-80 challenge is actually configured, so DNS-based setups are not told to open a port they do not need.
- Read-only and derived entirely from settings and the installed certificate: no probing, no external call."#.to_string(),
        },
        FeatureDoc {
            id: "identity".to_string(),
            title: "Identity (trusted devices and rescue e-mail)".to_string(),
            content: r#"Makes a second factor survivable, so account security is not traded away for convenience.

- A browser can be trusted for thirty days, after which the second factor is asked again. The owner sees a short label derived from the browser, when it was last used, and can revoke one device or all of them.
- The device cookie is stored as a hash, never in the clear, and only ever works for its own account: presenting someone else's cookie does not skip anything.
- Turning the second factor off revokes every trusted device, so re-enabling it does not silently restore trust.
- A rescue e-mail address is stored only once a code sent to it has come back — a typo cannot hand account recovery to a stranger.
- Recovery codes are six digits, valid for fifteen minutes, single use, destroyed after five wrong attempts.
- A locked-out user asks for a code from the sign-in screen. The answer is identical whether or not the account exists, so the form cannot be used to discover names.
- A correct recovery code is exchanged for a fresh offline recovery code, so there is one password-reset path with one password policy.
Passkeys (WebAuthn) are a third, separate credential: an account can sign in with the fingerprint, face or PIN of its own device — a phone passkey, Touch ID, Windows Hello, or a hardware security key — instead of a password. Registering one never changes the password or the two-factor setting, and revoking one never removes either; the account card lists each passkey with a name the owner can change, when it was added and last used, and whether the authenticator confirmed the user with a PIN or biometric. Only the public half is ever stored. The sign-in screen offers a "sign in with a passkey" button when the browser supports it; a cancelled prompt says it was cancelled rather than claiming the credentials were wrong, and a locked account cannot be entered with a passkey either. Passkeys are bound to the address the appliance answers to, which the card shows — a passkey created for another address will not work, and the guide explains how to set that hostname.
"#.to_string(),
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
        },        FeatureDoc {
            id: "scheduled-tasks".to_string(),
            title: "Scheduled Tasks".to_string(),
            content: r#"A generic cron engine for recurring admin-defined jobs, instead of one bespoke scheduler per feature. Task types:

- command: run one absolute-path executable (no shell, no interpolation) with arguments; 30-minute timeout.
- snapshot: create a pool snapshot (ZFS/Btrfs) named hv-auto-<timestamp>.
- smart_test: start a S.M.A.R.T. self-test (short, long, conveyance, offline) on a disk.
- power: reboot, shutdown or suspend the host.

Each task has a name, a 5-field cron expression and an enabled toggle. The task list shows the last run status/error and the computed next run; every execution is recorded in a run history (status, timing, output tail). Run now triggers a task immediately regardless of the cron. A task whose previous execution is still running is never started twice.

Everything is admin-only: tasks execute host actions. Payloads are validated at creation and again at run time — commands must be absolute existing executables (no shell), devices bare kernel names, pools well-formed names, power actions from a fixed whitelist."#.to_string(),
        },
        FeatureDoc {
            id: "metrics".to_string(),
            title: "Metrics (historical performance)".to_string(),
            content: r#"Historical performance charts for the server: CPU usage, memory usage, 1-minute load average, network throughput (receive/transmit) and the maximum thermal-zone temperature. The collector samples once a minute into the database and keeps 8 days of history; three ranges are charted: one hour (1-minute buckets), 24 hours (15-minute buckets) and 7 days (2-hour buckets).

Buckets without samples (the server was down) show as gaps in the chart, never as a flat zero line. The right edge of a chart is the bucket currently in progress, so the view stays live. The Metrics app is read-only: only the background collector writes samples. Available to every signed-in user, same as the dashboard."#.to_string(),
        },
        FeatureDoc {
            id: "power".to_string(),
            title: "Power management & disk spindown".to_string(),
            content: r#"Host-level power management in Settings (admin only):

- CPU scaling governor: see the current governor, the governors the kernel offers, and switch between them (for example performance vs powersave).
- Host power actions: reboot, shutdown or suspend the whole machine (confirmed in the UI). These are distinct from the process-level restart/shutdown of the Server control area.
- Disk spindown: per-disk hdparm settings — the APM level (1-255) and the idle timeout in minutes after which the disk parks its heads and spins down. Settings are saved per disk, applied immediately, and re-applied automatically after a server restart. A disk that refuses APM (SSD, USB bridge) is reported as not applied, but the setting stands for the next boot.

Power actions can also be scheduled from the Scheduled Tasks app (for example a nightly suspend)."#.to_string(),
        },

    ]
}

/// Index HomeVault feature documentation into the knowledge base.
async fn index_features<P: EmbeddingProvider>(
    pool: &SqlitePool,
    provider: &P,
) -> Result<(i32, i32, Vec<String>), String> {
    let features = feature_docs();

    let mut doc_count = 0;
    let mut chunk_count = 0;
    let mut ids: Vec<String> = Vec::new();

    for feature in features {
        let doc_id = format!("feature:{}", feature.id);

        let inserted = crate::rag::upsert_document_with_chunks(
            pool,
            &doc_id,
            "feature",
            &feature.id,
            &feature.title,
            &feature.content,
            "en",
            provider,
        )
        .await?;

        ids.push(doc_id);
        doc_count += 1;
        chunk_count += inserted as i32;
    }

    Ok((doc_count, chunk_count, ids))
}

/// Index i18n strings as structured documents per language.
async fn index_i18n<P: EmbeddingProvider>(
    pool: &SqlitePool,
    provider: &P,
) -> Result<(i32, i32, Vec<String>), String> {
    use std::collections::HashMap;

    let mut doc_count = 0;
    let mut chunk_count = 0;
    let mut ids: Vec<String> = Vec::new();

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

                let inserted = crate::rag::upsert_document_with_chunks(
                    pool,
                    &doc_id,
                    "i18n",
                    section,
                    &format!("i18n: {} - {}", section, lang_name),
                    &content,
                    lang_code,
                    provider,
                )
                .await?;

                ids.push(doc_id);
                doc_count += 1;
                chunk_count += inserted as i32;
            }
        }
    }

    Ok((doc_count, chunk_count, ids))
}

struct FeatureDoc {
    id: String,
    title: String,
    content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Offline stand-in for Ollama: deterministic embeddings, or a hard
    /// failure when `fail` is set (how the fail-loud contract is asserted).
    struct StubEmbedder {
        fail: bool,
    }

    impl crate::rag::EmbeddingProvider for StubEmbedder {
        fn embed<'a>(&'a self, texts: &'a [String]) -> crate::rag::EmbedFuture<'a> {
            let fail = self.fail;
            let n = texts.len();
            Box::pin(async move {
                if fail {
                    return Err("no embedding backend".to_string());
                }
                Ok(vec![vec![0.1f32, 0.2, 0.3, 0.4]; n])
            })
        }
    }

    async fn memory_pool() -> std::sync::Arc<sqlx::SqlitePool> {
        let db = crate::storage::Database::new("sqlite::memory:")
            .await
            .expect("in-memory db");
        db.run_migrations().await.expect("migrations");
        db.pool()
    }

    /// Guides that document *how the codebase is built* rather than a feature a
    /// user can open. They are deliberately **not** in the knowledge base: the
    /// assistant answers questions about the product, and a guide about design
    /// tokens would only pollute retrieval with internal vocabulary.
    const DEVELOPER_ONLY_GUIDES: &[&str] = &["design-system.md", "development.md"];

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
        ("file-tags", "file-tags.md"),
        ("snmp-agent", "snmp-agent.md"),
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
        ("identity", "identity.md"),
        ("antivirus", "antivirus.md"),
        ("ransomware", "ransomware.md"),
        ("firewall", "firewall.md"),
        ("tls", "tls.md"),
        ("acme", "acme.md"),
        ("ddns", "ddns.md"),
        ("reachability", "reachability.md"),
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
        ("scheduled-tasks", "scheduled-tasks.md"),
        ("metrics", "metrics.md"),
        ("power", "power.md"),
        ("cache", "cache.md"),
        ("syncthing", "syncthing.md"),
        ("downloader", "downloader.md"),
        ("chat", "chat.md"),
        ("rag", "rag.md"),
        ("notifications", "notifications.md"),
        ("widgets", "widgets.md"),
        ("shortcuts", "shortcuts.md"),
        ("desktop", "desktop.md"),
        ("apps-center", "apps-center.md"),
        ("themes", "themes.md"),
        ("wallpapers", "wallpapers.md"),
        ("transfer-quotas", "transfer-quotas.md"),
        ("mdns", "mdns.md"),
        ("server-control", "server-control.md"),
        ("avatars", "avatars.md"),
        ("mcp", "mcp.md"),
        ("global-search", "global-search.md"),
        ("rsync", "rsync.md"),
        ("backup-target", "backup-target.md"),
        ("replication", "replication.md"),
        ("traffic-control", "traffic-control.md"),
        ("iscsi", "iscsi.md"),
        ("directory-sso", "directory-sso.md"),
        ("disaster-recovery", "disaster-recovery.md"),
        ("iso-mount", "iso-mount.md"),
        ("sms", "sms.md"),
        ("dhcp", "dhcp.md"),
        ("local-dns", "local-dns.md"),
        ("reverse-proxy", "reverse-proxy.md"),
        ("usb-copy", "usb-copy.md"),
        ("fan-control", "fan-control.md"),
        ("ups-server", "ups-server.md"),
        ("guided-redundancy", "guided-redundancy.md"),
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
        let pool = memory_pool().await;
        let embedder = StubEmbedder { fail: false };

        let (docs, chunks) = super::index_all(pool.as_ref(), &embedder)
            .await
            .expect("index_all");
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

        // Every chunk carries a real embedding — never a NULL left by a
        // swallowed embedding failure.
        let nulls: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM rag_chunks WHERE embedding IS NULL")
                .fetch_one(pool.as_ref())
                .await
                .expect("null-embedding count");
        assert_eq!(nulls, 0, "no chunk may be stored without an embedding");

        // Re-indexing upserts rather than duplicating documents.
        let (docs2, _) = super::index_all(pool.as_ref(), &embedder)
            .await
            .expect("second index");
        assert_eq!(
            docs, docs2,
            "re-indexing must not change the document count"
        );
    }

    #[tokio::test]
    async fn test_index_all_reconciles_stale_documents_away() {
        let pool = memory_pool().await;
        let embedder = StubEmbedder { fail: false };

        // A document that the current sources no longer produce — as a deleted
        // guide or a renamed app would leave behind.
        sqlx::query(
            "INSERT INTO rag_documents (id, source, source_id, title, content, language)
             VALUES ('feature:ghost', 'feature', 'ghost', 'Ghost', 'gone', 'en')",
        )
        .execute(pool.as_ref())
        .await
        .expect("seed stale document");

        super::index_all(pool.as_ref(), &embedder)
            .await
            .expect("index_all");

        let ghosts: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM rag_documents WHERE id = 'feature:ghost'")
                .fetch_one(pool.as_ref())
                .await
                .expect("count");
        assert_eq!(ghosts, 0, "a document no source produces must be pruned");
    }

    #[tokio::test]
    async fn test_index_all_fails_loudly_when_embedding_fails() {
        let pool = memory_pool().await;
        let embedder = StubEmbedder { fail: true };

        let err = super::index_all(pool.as_ref(), &embedder)
            .await
            .expect_err("an embedding failure must fail the run");
        assert!(err.contains("embedding failed"), "unexpected error: {err}");

        // The run status says `error`, it is never reported as idle/success.
        let status = crate::rag::get_index_status(pool.as_ref())
            .await
            .expect("status");
        assert_eq!(status.status, "error");

        // And nothing was half-indexed.
        let docs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rag_documents")
            .fetch_one(pool.as_ref())
            .await
            .expect("count");
        assert_eq!(docs, 0);
    }
}
