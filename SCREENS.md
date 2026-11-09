# HomeVault — List of screens and features

---

## 1. Login (`/login`) TESTS

**Features:**
- Username/password form with show/hide password
- "Remember me" checkbox
- "Forgot password" link — offline numeric recovery code (no email)
- Password reset flow (recovery code → new password)
- **Account recovery by e-mail** — "I lost my recovery code": submit the username, the answer is the same whether or not the account exists; a six-digit code read from the rescue address is exchanged for a fresh offline recovery code
- Two-factor authentication (2FA) — entry of the 6-digit TOTP code, with "remember this browser for 30 days"
- **Sign in with a passkey** — a button under the form, offered only when the browser can run a WebAuthn ceremony and only once a username is typed; a cancelled prompt reads as cancelled, never as a wrong password
- Lockout: 5 wrong passwords lock the account for 5 minutes, then it lifts itself; the message points at the recovery-code path
- Per-client-IP throttle on login and 2FA attempts (429)
- Error modal with translated messages (invalid credentials, locked account, rate limiting, invalid TOTP, network timeout); inline error is an assertive live region, fields carry `aria-invalid`, focus returns to the form on close
- Unable-to-connect state with a reconnect button
- First-launch detection — shows the Setup Wizard instead

---

## 2. Setup Wizard TESTS

**Shown on the Login page on first launch.**

**Features (4 steps):**
- Step 0 — Welcome: presentation of the features (accounts, encryption, security)
- Step 1 — Account: username, optional full name, avatar selection (DiceBear)
- Step 2 — Password: entry + confirmation with a real-time strength indicator and a requirements checklist; optional TOTP enrollment (toggle → QR code → verified 6-digit code)
- Step 3 — Verification: account summary, server-side username availability check, then creation + one-time recovery code
- Interface language detected from the OS/browser locale (English when unsupported), changed in Settings — never asked by the wizard
- Reboot-safe progress: the current step persists server-side
- Storage pools, RAID arrays and backup restores are configured from their own apps *after* setup — the wizard has no storage/RAID/restore step
- Password policy: 12+ chars, mixed case + digit, no common password, no 5+ repeated run (live checklist of all 6 rules)
- Mandatory, blocking recovery-code screen (retry on failure — never skippable)
- Bootstrap endpoints: local-network + same-origin + rate-limited; self-healing setup lock
- Progress stepper with animations

---

## 3. Desktop — `/desktop`

**Main shell of the application.**

### Topbar
- User chip (avatar + name); menu: My account, Settings, Logout
- Localized date/time display (clock + date)
- **Network** button (wired/WiFi icon + label) with a popover for local IP + public IP
- **Widgets** button: opens the Widgets app
- **Notifications** button (icon + label) with a counter and a dropdown panel —
  the counter is a badge **anchored to the corner of the button**, not to the label: it
  lives in the band above the text line, so a "99+" widens
  into empty space without ever covering the label
- **Power** button: Logout, Restart the server, Shut down the server

The four buttons are **icon + label** chips (approved mockup): labels
visible from 901px, icon only below that — the touch target stays
44×44 in both cases. Order: Network, Widgets, Notifications, Power.

### Application dock
- Pinned apps, in dock order: Dashboard, Terminal, Docker,
  Health, AI Assistant, Settings (`PINNED_DOCK_APP_IDS`)
- Running / minimized apps with a restore animation

### Activity taskbar
- Transient dock button (visible only during an activity) with a
  badge = number of active tasks; click → panel above the dock
- One line per task: name, progress bar, % (sliding bar if
  progress is indeterminate) — display only
- Sources: aria2 downloads, Explorer copies/moves, Restic backups,
  antivirus scans, cache mover, Docker install/update, Rescue
  clone/recovery, RAG reindexing, AI model download, cloud sync
- Powered by `GET /api/activity` (2 s poll); store `activityStore.ts`

### Global search (Ctrl+K)
- Search bar with fuzzy matching on app names/descriptions
- Keyboard navigation (arrows + Enter)

### 29 available applications

| # | Application | Category | Description |
|---|---|---|---|
| 1 | Dashboard | System | Overview and real-time statistics |
| 2 | Storage | Storage | Disks, virtual volumes (grow/level change), encryption, cache |
| 3 | RAID | Storage | Virtual disk creation (mdadm/ZFS/Btrfs) |
| 4 | Explorer | Storage | Browsing of shared files |
| 5 | Docker | System | Containers, images and application catalog |
| 6 | Network | Network | Interfaces and routing |
| 7 | VPN | Network | WireGuard and OpenVPN access |
| 8 | Backup | Storage | Repositories, snapshots and restores (restic) |
| 9 | Users | System | Accounts and permissions |
| 10 | Groups | System | User groups and roles |
| 11 | Quotas | System | Read-only overview of transfer quotas |
| 12 | Shares | Storage | Network shares (SMB/NFS/FTP/WebDAV/SFTP) |
| 13 | Cloud Sync | Network | Remote cloud storage (rclone) |
| 14 | Syncthing | Network | Continuous file synchronization |
| 15 | Health | Monitoring | System status and alerts |
| 16 | AI Assistant (Chat) | Productivity | Conversation with the AI assistant |
| 17 | Downloader | Productivity | Download management (aria2) |
| 18 | Rescue | System | Recovery and repair |
| 19 | Widgets | System | Add/remove desktop widgets |
| 20 | System journal | Monitoring | System logs and audit events |
| 21 | Terminal | System | Full PTY terminal |
| 22 | Settings | System | Server configuration |
| 23 | My account | System | Personal profile and security |
| 24 | Virtual Machines | System | VMs and containers via Incus — guided wizard, shell console, graphical screen, snapshots, clone |
| 25 | Scheduled Tasks | System | Generic cron engine — commands, pool snapshots, SMART self-tests, power actions, run history |
| 26 | Metrics | Monitoring | Historical performance charts (CPU, memory, load, network, temperature) over 1 h / 24 h / 7 d |
| 27 | Backup destination | Storage | Receive restic backups from another machine (append-only tenants, quotas) |
| 28 | Replication | Storage | Copy pool snapshots to another dataset, locally or over SSH |
| 29 | Traffic control | Network | Per-service bandwidth limits (SMB/NFS/FTP/WebDAV/SFTP), weekly schedule, pause-all |
| 30 | iSCSI | Storage | Export disks (file or block device) over iSCSI — targets, portals, LUNs, initiator ACLs, CHAP, apply-now |
| 31 | Directory sign-in | System | Sign in with an LDAP / Active Directory account **or** an OpenID Connect provider — group→role mapping, break-glass local admin |

> That is 29 entries in `appDefs` (28 app tiles + the Widgets editor) + Terminal + Settings +
> My account. Storage gained a 7th tab ("Snapshots" — scheduled snapshots with GFS retention
> **and protection locks**) and Settings gained the Power card (Monitoring). Shares gained a
> 7th protocol tab (Rsync) and the SMB tab gained the per-share Time Machine card.
> The 36 Svelte routes = these 31 apps + the Widgets editor + Login + Setup Wizard + Desktop
> + the public share page (`/share/<token>`, outside the app shell).

### Floating monitoring widgets (11 panels)

Each widget is a resizable, movable panel, with position persistence.

| # | Widget | Data displayed | Refresh |
|---|---|---|---|
| 1 | Resource monitor | CPU, RAM, temperature, network traffic | 2s |
| 2 | RAID monitor | RAID array status | 5s |
| 3 | Security monitor | Antivirus, firewall, anti-ransomware | 15s |
| 4 | VPN monitor | WireGuard/OpenVPN connections | 15s |
| 5 | Downloader monitor | Ongoing downloads (aria2) | 5s |
| 6 | Syncthing monitor | Synced devices and folders | 15s |
| 7 | Calendar monitor | Grid of the current month | — |
| 8 | Docker monitor | Running containers | 10s |
| 9 | Weather monitor | Weather via Open-Meteo (browser) | 10 min |
| 10 | Active sessions monitor | Number of connected users | 15s |
| 11 | System health monitor | healthy/warning/critical counter | 15s |

### App window management
- Windows with a title bar, minimize/maximize/close buttons
- Drag-and-drop, resizing, edge snapping
- Adaptive initial geometry (auto-maximizes below 820px)

---

## 4. My account (Account) TESTS

**Features:**
- Profile display: avatar, username, full name, email, role, creation date
- Avatar regeneration
- Profile editing (full name, email)
- Password change (invalidates all sessions → automatic logout)
- 2FA enrollment: TOTP secret, QR code, verification, backup codes
- 2FA disabling (with password confirmation) — also revokes every trusted device
- Display of backup codes
- **Trusted devices** — one row per browser (label derived from the user agent, first seen, last used, expiry), revoke one or all; a trusted browser skips the second factor for 30 days
- **Rescue e-mail** — address with a verification code (nothing is stored until the code comes back), the verified address, change and remove
- **Passkeys (WebAuthn)** — one row per registered authenticator (label, added, last used, a "verified by PIN/biometric" badge), rename, revoke (with confirmation), "Add a passkey" with an optional name, the origin the passkeys are bound to, and an explanation instead of a button when the browser cannot run a ceremony

---

## 5. Users (Users)

**Master/detail anatomy (2026-09-14):** account rail on the left, detail pane on
the right; in a narrow window the two panes swap (threshold ≈ 1065px window width).

**Features:**
- Rail: "New user" button, search, filters (all/active/locked/admin),
  counter, one row per account (avatar, name, role badge, `@username`, locked/active state)
- Rail footer: "Quotas view" button → opens the Quotas app
- User creation: username, password, full name, email, role, groups, home folder
- Detail pane in 6 cards, each sized by its content:
  1. **Identity** — read-only username (account key), full name, email
  2. **Role and groups** — role selector + group chips in full-width rows
  3. **Home folder** — enablement, explicit disk or folder, size slider + allocated/free bar
  4. **Transfer quotas** — upload/download (toggle + value + unit + usage bar) and an "All quotas" button
  5. **Security** — 2FA (state + reset), password change
  6. **Actions** — lock/unlock, delete (option to keep/delete the folder)
- Group membership: add/remove; `homevault_admin` as a read-only chip derived from the role
- Password change (admin for other users)
- User deletion (purges home folder, SFTP jail, sessions, quota row)
- 2FA enable/disable (admin recovery)

---

## 6. Quotas (Quotas)

**Read-only screen — no writes, no edit button.**

**Features:**
- One row per configured subject: users first, then groups, sorted by name
- Filters: all / users / groups
- Per row: subject type, name, last update date
- Per direction (upload, download): bytes used / limit + real progress bar
- A disabled direction shows "Unlimited" (no limit, not "zero allowed")
- Per row: **History** opens a read-only dialog with the hourly usage samples (90-day retention); a 90% crossing raises one system notification per crossing
- Counter row: number of subjects, of which how many have a limit
- "Manage in Users" button → opens the Users app (`hv-open-app` event)

**Data source:** `GET /transfer-quotas/{subject_type}` (one request per type),
the same endpoint as the "All quotas" modal of the Users screen.
The screen is nevertheless reserved for admins, like Users.
Quota editing stays in the "Transfer quotas" card of Users
and in the Groups screen.

---

## 7. Groups (Groups)

**Features:**
- Group list with search, member counter
- Group creation: name + initial members
- Member management: add/remove users with search
- Group deletion (`homevault_admin` group protected)
- Management of transfer quotas per group

---

## 8. Dashboard

**Features (refresh every 5s):**
- Health score gauge (circular SVG)
- LED status row: CPU, RAM, Disk, Network
- CPU panel: model, frequency, per-core bars, temperature, core/thread count
- Memory panel: usage bar, RAM type/speed, swap
- GPU panel (one per GPU): model, type (NVIDIA/AMD), usage, VRAM, frequency, temperature, power
- Motherboard panel: manufacturer, model, BIOS version/date
- Storage panel: filesystem usage bars
- Network panel: per interface — IP, MAC, speed, driver, RX/TX + real-time rate
- Disk health panel: SMART healthy/total count
- Hardware sensor temperatures
- Manual refresh button

---

## 9. Storage

**7 tabs:**

### Physical disks
- Detailed disk cards: name, size, vendor, model, serial number, type, SMART status, temperature
- Modal with a partition table and a pie chart
- SMART tests (short/long/conveyance)

### Virtual disks
- mdadm/ZFS/Btrfs virtual disks with health, usage, member count and mount point
- Verify/scrub and check status, rebuild, destroy (typed confirmation)
- **Add disk** (grow an mdadm array) and **Change RAID level** (mdadm in-place
  migration: only the allowed targets are selectable, each refused level shows
  its reason, and the action waits for an explicit backup acknowledgement)
- **Btrfs rollback layout**: reports legacy vs `@`/`@snapshots`, migrates an
  existing pool after the pool name is typed; a migrated pool's snapshot list
  gains **Roll back** (safety snapshot, `@` swap, previous subvolume kept)

### Partitions
- List of all partitions with mount point, filesystem, size, used, available, percentage

### Removable
- List of USB/removable disks with mount/unmount

### Encryption (LUKS2)
- Encrypted volumes (unlock status, mapper, mount point)
- Encrypted volume creation (device, filesystem, label, passphrase)
- Lock/unlock, deletion

### Cache Tier
- Selectable cache pool (including RAID)
- Move scheduling (cron, age)
- Per-share cache mode
- Move history
- **ZFS read cache / separate log** card (admin): every ZFS pool with its
  attached `L2ARC`/`SLOG` devices (state, error counters, detach), and an
  *Attach* form (pool, role, disk). A spinning disk is listed but disabled with
  the reason, and a host without `zpool` (or without a ZFS pool) says so
  instead of offering a dead action

### Snapshots
- Scheduled snapshots per pool **or per share** with cron and GFS retention
- Share schedules snapshot the dataset/subvolume that contains the share, list
  only that backend's snapshots, and expose file-level restore (relative path
  + overwrite confirmation)
- Run now, delete, and last-run status/error; lock/break-glass protection for
  pool snapshots

---

## 10. RAID

**Features:**
- RAID array list: name, level, status (healthy/degraded/failed), disk count, size/used/free
- RAID creation (3-step wizard): type (0/1/5/6/10/Z3), disk selection (with optional hot spares on mdadm and ZFS), name + filesystem; RAID Z3 is ZFS-only and pins the filesystem to ZFS
- RAID array deletion
- Rebuild of a degraded array: selection of the replacement disk
- Growing an array and changing its RAID level in place are managed from
  Storage → Virtual disks (the RAID app stays creation-only)

---

## 11. Health

**Features:**
- Security Advisor card (administrators only, above the checks): hardening
  score out of 100 with a progress bar, the summary counts, the actionable
  findings worst first (each with its severity, evidence, fix sentence and an
  "Open …" deep link to the screen that owns the setting), and the rest of the
  checklist behind a native disclosure. Read-only — no write path; a failing
  call shows an error instead of a stale score
- 9 health checks displayed in order:
  - Authentication, Storage, Memory, Disk space, Disk SMART, Network, Services, Temperatures
- Each check: status icon, translated label, message, expandable details
- Global status badge (healthy/warning/critical/unknown)
- Refresh button

---

### Global search overlay (⌘K / Ctrl+K, desktop shell)

**Features:**
- Apps, settings categories and quick actions, filtered and ranked locally
- **File results across every readable root** (QW-12): each row shows the file
  name and the folder it lives in, so identical names in different shares are
  distinguishable; picking one opens the Explorer on that folder with the entry
  selected
- A "Looking for matching files…" line while the cross-root walk runs, so
  "no results" never shows while a search is still in flight
- An honesty note under the results when the answer is partial (a location did
  not answer) or cut (too many matches), and when the file search itself is
  unavailable
- Keyboard: arrows to move, Enter to run, Escape to close

---

## 12. Terminal

**Features:**
- Full PTY terminal (xterm.js)
- WebSocket connection with an authentication ticket
- Automatic reconnection
- Adaptive resizing
- Clickable links in the output
- Status indicator (Connected/Disconnected)

---

## 13. System journal (Journal)

**Features:**
- Source selector: `app` (business event logs), **`audit`** (queryable audit trail: severity, actor/action, 90-day retention), `system` (journald), text files (syslog/auth.log/kern.log/daemon.log)
- Entry list: timestamp, unit, priority, PID, message; the audit source shows `[action]` before the sentence
- Filtering by unit, by priority, text search (the audit source supports text search in the Journal; structured filters and CSV/JSON export are on `/api/audit`)
- **Export CSV** downloads the current source/search/priority view (same privilege model, ≤ 5 000 entries)
- Auto-refresh toggle
- Clearing of logs per source with confirmation
- Scheduled cleanup settings (enablement, interval in days)

---

## 14. Network

**2 tabs:**

### Ethernet
- Interface cards: name, type, MAC, status, IPv4/IPv6, MTU, speed, duplex, driver, RX/TX + rate
- Configuration: DHCP/Static, manual IP/netmask/gateway/DNS
- Connection summary: local IP, gateway, DNS, public IP
- Diagnostics: gateway reachability, DNS test, Internet reachability, latency
- Link aggregation and VLANs (bonds and 802.1Q devices): what exists, create a `bondN` (`802.3ad` with a note that the switch must be configured for it, or `active-backup`), create `<parent>.<vid>` with the id bounded to 1–4094, delete either one after a confirmation that names what the network loses. A change that would cut the administrator's own session is refused with the reason
- Static routes: a card lists saved IPv4/IPv6 routes with destination, next hop, interface, metric and whether the kernel currently carries them; add a route (CIDR, next hop, interface, optional metric) or delete one after confirmation. Applied with `ip route replace`, read back from the kernel and persisted for reboot; a missing `ip` command is reported instead of offering a dead form

### WiFi
- Network list: SSID, signal strength, frequency, channel, security
- Network scan
- Connect/disconnect
- WiFi status display

---

## 15. VPN

**3 tabs:**

### WireGuard
- **Public endpoint** card: resolved value, source badge (Manual / Dynamic DNS / Detected IP / Automatic), inline warning when unset or LAN/CGNAT, and a **Change** dialog (hostname/IP or "Use automatic")
- Server status: public key, listening port, state
- Peer (device) list: name, IP, creation date, status, last handshake
- Device addition: config generation + QR code
- Re-view of config/QR for an existing peer
- Peer deletion
- Server start/stop

### WireGuard client
- Import a provider `.conf` (file picker or paste), optional kill switch
- Client list: name, interface, endpoint, address, peer count, live status
- Connect / disconnect / delete
- "Connect at boot" toggle (reconnects after startup)

### OpenVPN
- Connection list: name, status, interface, local/remote IP
- Connection addition: name, config file, credentials
- Deletion, start/stop

---

## 16. File explorer (Explorer)

**Features:**
- Dual view: list and grid
- Folder navigation: breadcrumb, parent folder, entries with size, permissions, owner
- Sidebar: physical disks, removable disks, favorite locations and private file tags (with an all-folders filter)
- **Network locations** (admin): CIFS/SMB and NFS shares on another NAS or computer, mounted under `/mnt/homevault-remote` and browsable like any folder, with connect/disconnect, read-only by default and reconnect-after-restart
- File operations:
  - Create a folder
  - Rename
  - Delete (to the trash)
  - Copy / Move (with a folder picker)
  - Upload files (with a progress bar, quota-aware)
  - Download (quota-aware)
- Text file preview
- Permission management (chmod)
- File search, including an optional private-tag filter in the desktop search overlay
- Sorting: by name, size, modification date
- Trash system: view, restore, purge, empty
- Trash retention (admin): keep deleted files for a number of days, cap the trash size, and a "Clean up now" button that runs the pass immediately; past a limit the oldest items are removed automatically
- Transfers panel: a toolbar button (with a badge counting what is running) opens three zones — uploads, downloads, copies. Each row shows the file name, a progress bar, the bytes transferred out of the total, the speed, the time remaining and how a copy was made, with a cancel button that really stops it. An interrupted upload resumes where it stopped, even after HomeVault restarts. A collapsible section lists the last finished transfers (size, duration, method, "verified", and whether it succeeded)
- Checksum on demand: pick SHA-256, SHA-512 or BLAKE3 for any file (small files answer immediately, large ones run as a background job), and optionally compare a second file to answer "are these two identical?"
- Instant copy (admin): copies share the original's data on filesystems that support it (Btrfs, reflink-enabled XFS), so a large copy finishes immediately and costs no extra space; the modal can turn it off ("Always make a full copy") and lists which folders can copy instantly. When the filesystem cannot share data the copy falls back to a normal verified copy, and the message says "cloned" only when it really was
- Archive extraction (.zip/.tar/.gz/.bz2/.xz)
- Compression of selected items
- Share a file by link: a revocable public URL (copyable text + QR code) that expires after 1/7/30 days or never, is open to anyone with the link or only to signed-in users, and can optionally require a password and allow only a set number of downloads — "one download" being the one-time link. The modal shows how many downloads a capped link has used, says when the limit is reached, and lets the owner revoke it at any time

### Public share page (`/share/<token>`)

Not an app: the page a recipient opens, rendered without signing in. It is
served for a browser navigation that cannot be answered with the file itself —
a password-protected link, a spent/revoked one, or a signed-in-only link. A
link that needs nothing downloads straight away and never shows a page.

- Asks for the password only when the link has one
- The password is checked once and never appears in the URL (no query string, no history entry)
- Downloads stream through the browser (resume and partial ranges work, so a large file is not buffered in memory); share responses are never cached
- Clear states while checking, for a wrong password and for too many attempts (with a retry), an expired/spent/revoked link, and a link reserved to signed-in users (with a link to the sign-in page — a signed-in browser is recognised by its session cookie)
- A one-time link is dead after the first completed download; an interrupted download does not use it up
- Not indexed by search engines (`noindex`)

---

## 16b. Explorer sidebar — Disc images (ISO mounting)

**Screen:** the **Disc images** panel in the Explorer sidebar →
`frontend/src/lib/components/explorer/IsoMountPanel.svelte`

**Features:**
- The mounted images with their label, size and mount point
- **Mount this image**, offered only when the selected entry is a `.iso`/`.img`
  file
- A per-row unmount action, named after the image
- A missing `losetup`/`mount` is stated in place, with no action offered

**Invariants:** reads need a session, mount/unmount are admin-only; the mount
point is derived from a deterministic slug under `{DATA_DIRECTORY}/iso`, never
from the request; the mount carries `ro,nosuid,nodev`; the image must be inside
the caller's Explorer roots; a `..` is refused before `canonicalize`; the
filesystem (`/proc/mounts`) is the authority on what is mounted, not the index;
a failed mount detaches its loop device.

**Guides:** [guides/iso-mount.md](./guides/iso-mount.md)

---

## 16c. Explorer sidebar — Network locations (CIFS/NFS)

**Screen:** the **Network locations** panel in the Explorer sidebar →
`frontend/src/lib/components/explorer/RemoteMountPanel.svelte`

**Features:**
- The configured network locations with their state (Connected / Not connected), `server:share`, read-only or read-write, and the last error under an entry that failed
- A per-row **connect/disconnect** action, named after the location, plus **Edit** and **Remove**
- **Add a network location**: name (a lowercase slug used as the folder name), protocol (Windows/SMB share or NFS export), server, share name or export path, optional user name / password / domain for SMB, read-only (on by default), "reconnect automatically after a restart" and "connect now"
- "Reconnect all at startup locations" re-runs the boot pass without restarting HomeVault
- A host with neither `mount.cifs` nor `mount.nfs` says so in place, with no Add action offered
- Only administrators see the panel (the list reveals which remote hosts the appliance talks to)

**Invariants:** the mount point is `/mnt/homevault-remote/<validated name>` and never comes from the request; a password lives only in a 0600 credentials file (never argv, log or response); options are a per-protocol allowlist and `ro`/`rw` is its own field; every host command has a deadline and reports the tool's own stderr; the panel never uses `window.confirm`.

**Guides:** [guides/remote-mounts.md](./guides/remote-mounts.md)

---

## 17. Shares/Protocols (Shares)

**Features:**

### Share list
- List: name, path, type (SMB/NFS/FTP/WebDAV/SFTP), enabled status, comment
- Size quota per share: a ceiling on one shared folder, enforced by the filesystem that holds it (ZFS `refquota`, a Btrfs subvolume's qgroup, an XFS project quota) so network clients are bound too; shown as a used-of-quota bar with the percentage, set in GB, cleared with one button, and explained instead of offered when the filesystem cannot enforce one. The Explorer also refuses an upload or copy that would cross it
- Share creation: name, path, type, comment

### Configuration tabs (7: the share list + 5 protocols + Rsync)
- **Shares** — CRUD, service start/stop, status badges
- **SMB** — workgroup, security mode (*User* or *Share* only; domain/ADS is refused until winbind exists), protocol minimum (SMB2/SMB3; NT1 refused), guest access, **SMB Multichannel** toggle (off by default), **Windows network discovery** (WS-Discovery via `wsdd2`, on by default, discovery only — the card says when `wsdd2` is not installed), advanced options, per-share recycle bin toggle (files deleted over the network are kept and shown in the Explorer's Trash), and per-share **Time Machine** (`vfs_fruit`, with an optional maximum size in GB; the share is advertised over Bonjour `_adisk._tcp` so macOS lists it without typing an address — AFP is not used)
- **NFS** — networks (one client clause per line), default access, version (Auto or 3/4/4.2), root squash, custom options; the values are applied to `/etc/exports` on save, not merely persisted
- **FTP** — port, passive ports, max clients, TLS, anonymous, chroot, welcome message, and an optional account whitelist (`/etc/vsftpd.user_list`)
- **WebDAV** — port, path prefix, SSL, auth, compression, locking, allowed methods
- **SFTP** — port, list of beneficiaries (user + shares + key state), user addition, public key addition, jail rebuild
- **Rsync** — opt-in rsync daemon (enable, port, maximum connections, per-module confinement), one authenticated module per share (name, share picked from the existing shares only, comment, read-only, allowed accounts, enable), one rsync password per account (write-only, never displayed again), and the rsync-over-SSH alternative spelled out

### Permission management
- View current permissions (beneficiary, r/rw level, recursive)
- Grant a permission (user/group selector)
- Revoke a permission
- Synchronize permissions

---

## 18. Cloud Sync (rclone)

**3 tabs:**

### Remotes
- List of configured remotes (name, type)
- Remote addition: provider selection, name, raw config
- Remote deletion
- View of the rclone config

### Mounts
- List of active mounts: remote, mount point, status, PID
- Mount/unmount a remote

### Synchronization
- Source (local path or cloud remote)
- Destination (cloud remote or local path)
- Direction: local→cloud, cloud→local, bidirectional
- Dry-run mode
- Option to delete after sync
- Execution and status

---

## 19. Docker

**7 tabs: Containers, Images, Volumes, Networks, Discover, App Catalog, Compose.**

The screen refreshes on tab switch, on window focus and every 45 s
(`onAppVisible(refreshCurrent, { pollMs: 45_000 })`) — not every 5 s.

### Containers
- Container cards: name, image, status (running / restarting / paused / stopped), creation date, ports, memory/CPU, "update available" badge
- Start / Stop / Restart, Logs, Edit compose file
- "More actions" menu: Details (inspect), Pause / Resume, Kill (SIGKILL, confirmed), Rename, Change limits (memory / CPU / restart policy, applied in place), Run a command (exec), Update to newest image (recreate, confirmed)
- Logs: live SSE tail — tail size (100–5000), timestamps, follow toggle, status pill (live / connecting / ended / disconnected), download, restart stream
- Editing of `docker-compose.yml` (containers from a Compose project managed by HomeVault): modal editor, syntax validation, save, save + redeploy
- Creation: name, image, ports, volumes (path via FolderPicker), environment variables, networks
- Toolbar: Daemon settings (`daemon.json` editor, validated before write, restarts Docker)

### Images
- Image list: repository, tag, size, date
- Download from Docker Hub (search + results), build an image from a host folder (path-guarded), private registry sign-in / sign-out
- Deletion

### Volumes
- Disk usage table (`docker system df`): type, total, active, size, reclaimable
- Prune: unused images, unused volumes, build cache, everything unused (all confirmed)
- Volume list: name, driver, mountpoint, in-use badge, containers using it
- Creation and deletion (force-confirmed when the volume is attached)

### Networks
- Network list: name, driver, scope, subnet, gateway, connected containers

### Discover
- Docker Hub search and image discovery

### App Catalog
- Manifest-driven packages from the embedded store plus admin-added community stores (15 built-in: BookStack, FreshRSS, Gitea, Grafana, Home Assistant, Immich, Jellyfin, n8n, Navidrome, ownCloud, Paperless-ngx, Pi-hole, Uptime Kuma, Vaultwarden, Vikunja)
- One-click install/uninstall with dependency, settings and compatibility review; per-app reverse proxy with optional auth gate; per-app logs; update detection and "update all"; an opt-in **Scheduled updates** disclosure (interval 6–168 h, first run one interval after enabling, never a restart without an update, Run now)
- **Media apps (Immich, Jellyfin, Navidrome)**: the install dialog offers a **Library** selector (default: the app's own folder, or one of the configured shares, mounted read-only) and a **GPU** checkbox for hardware transcoding (disabled with the reason when the host has no `/dev/dri`). The installed card shows the bound share; both choices survive updates

### Compose
- Compose projects: list with services and status, start/stop/restart/update-to-newest-images/deletion, editing of the compose file

---

## 20. Syncthing

**3 tabs:**

### Overview
- Syncthing status: available, version, device ID, uptime
- Quick statistics
- Restart/Pause/Resume

### Devices
- Device list: name, ID, addresses, connection status, version, completion, data transferred
- Pause/Resume per device

### Folders
- Folder list: label, path, status, file counters, connected devices
- Pause/Resume per folder
- Manual scan (triggers resync)
- **Versions** dialog: the folder's versioning policy (off, trash can, last N
  versions, or until an age limit, with the cleanout period) and the archived
  copies Syncthing holds — file, version date, size — each with a **Restore**
  action. Turning versioning off states that a replaced file is then lost

---

## 21. Downloader

**Features:**
- Global statistics: download/upload speed, number active/pending/stopped
- Task list: GID, name, status, progress, speed, error message
- URL addition with a destination directory, an output name and an optional **speed limit** for that download
- A completed download offers **Extract archives**; the archive itself is kept
- Actions: pause, remove, delete (3-step fallback chain)
- Status icons: active, paused, completed, error, pending
- Refresh every 2s
- **RSS subscriptions** (toolbar button): the feed list with per-feed
  *checked by the schedule* and *queue new items* switches, **Check** and
  **Download new**, the items the last check found, subscribing by address with
  an optional name, and **Check all**
- **Automatic extraction** switch in the same window (off by default): a
  finished `.zip`/`.tar` archive is unpacked in its own download folder

---

## 22. Backup / Restic

**3 views:**

### Repository list
- Repository cards: name, URL, source paths, retention policy, snapshot count, last backup
- Repository creation: name, URL, password, source paths, exclusion patterns, tags, source (directory/snapshot pool), retention policy
- Repository deletion

### Repository detail

#### Snapshots
- Snapshot list: ID, timestamp, paths, hostname, tags
- Navigation inside a snapshot's content
- Restore with a target (FolderPicker), dry-run mode
- **Browse repository** (`restic mount`, read-only FUSE): the mounted path plus an Unmount action; the action is replaced by a "FUSE unavailable" note when the host has no `/dev/fuse`

#### Jobs
- Job history: status, files, data, duration, errors
- Trigger of a new backup
- Job details

#### Scheduling
- Scheduled backups: cron expression, enablement, last/next run
- Creation/editing/deletion of schedules

### Retention policies
- Max count, max age (days), max size (GB)

---

## 23. Virtual Machines

**Features:**
- Machine list (VMs + system containers): name, type, state, IPv4 address, profiles
- Creation wizard in two modes: cloud image from the OS catalog or local ISO
- Presets: CPU count, memory, disk size
- Initial connection identifier seeded via cloud-init (SHA-512 crypt hashed password)
- Lifecycle: start, stop, restart, deletion (with operation wait)
- Clone: duplicate a machine (optionally from a snapshot) with a new name; the clone stays stopped
- Snapshots: creation, list, restore, deletion
- **Export / import**: the toolbar's *Export / import* dialog exports the
  selected machine (current state or a snapshot, optional compression) into one
  archive, lists the archives with their size, date and absolute path, imports
  one as a new machine (optional name) and deletes one; without Incus the
  actions are disabled with the reason and the archive list stays readable
- Shell console (xterm) relayed over WebSocket
- Graphical screen (noVNC) with a single-use 20 s ticket
- VirtIO driver download (fedorapeople.org)

---

## 24. Rescue

**4 tools (Simple/Advanced mode for each):**

### ddrescue
- Disk cloning/recovery
- Source/destination selection (disk or image file)
- Advanced options: block size, retry passes, direct I/O, reverse
- SSE progress tracking

### TestDisk
- Partition analysis and reconstruction
- Device selection
- Options: deep search, log path, forced mode

### PhotoRec
- File recovery
- Device/output directory selection
- Options: file type selection, keep corrupted files, max size

### mdadm
- RAID array diagnostics and repair
- Array name, displayed diagnostic
- Options: action type (repair/assemble), forced, verbose
- Repair with SSE progress

---

## 25. AI Assistant (Chat)

**Features:**
- Conversation interface with message bubbles
- Streaming responses (SSE) with a cancel button
- Answers questions about HomeVault itself: the prompt is enriched with
  passages from the local knowledge base (docs, app catalog, settings)
- Model selector over the models installed on the appliance
- Conversation history (localStorage)
- Message copying
- Response regeneration
- New conversation
- Markdown rendering in responses
- Localized system prompt
- Works fully offline: the model runs on the appliance, no provider is
  contacted
- **External agents (MCP)** are configured from Settings → AI Assistant
  (server-wide, admin-only) — see the Settings section below

---

## 26. Widgets

**Features:**
- Catalog of 11 monitoring widgets
- Each widget: translated icon, color, name and description
- Enable/disable toggle (persisted)
- "Show all" / "Hide all" / "Reset" buttons

**Available widgets:**
1. Resource monitor (CPU/RAM/Temp/Network)
2. RAID monitor
3. Security monitor
4. VPN monitor
5. Downloader monitor
6. Syncthing monitor
7. Calendar monitor
8. Docker monitor
9. Weather monitor
10. Active sessions monitor
11. System health monitor

---

## 27. Settings

**8 categories in the sidebar** (grouped 2026-09-09 from 12 flat tabs, re-themed 2026-09-14 from the Figma mockup "Settings Screen"):

1. **General** — Server name, language (EN/FR/ES/DE), time zone, automatic startup
2. **Security** (merged cards):
   - **HTTPS / TLS** — Enablement of the built-in HTTPS listener (port, self-signed, provided certificate or an automatically issued one, HTTP→HTTPS redirect, certificate info, regeneration); applies after restart
   - **Certificate automation** — One public name, contact address, challenge (`http-01` / `dns-01`), DNS provider (`manual` / Cloudflare with a write-only token), status badge, issued/Renew, pending-record confirmation, "test port 80"
   - **Dynamic DNS** — Provider (DuckDNS, dynv6, Cloudflare, OVH DynHost, Namecheap, deSEC, custom HTTPS URL), host name, provider-specific DynHost username / registered domain fields, write-only token or password, the address the host would publish vs. the one recorded, last result and reason, "Update now"
   - **Outbound proxy** — one switch, an optional HTTP and HTTPS proxy URL, a bypass list, and the effective exemptions (loopback is always added); applied to certificate renewal, dynamic DNS, update checks, alerts and Web Push
   - **Reachability** — read-only ordered checklist (HTTPS on, trusted certificate, automation, dynamic DNS, names agree, ports 443/80) with a fix under each failing check
   - **Antivirus (ClamAV)** — Status, scheduled scans, execution, history, threats, quarantine, definition updates
   - **Anti-Ransomware** — Enable/disable, honeypot, thresholds, auto-response mode, protected paths, history
   - **Firewall (ufw)** — Enable/disable with preview, rules (port/protocol/action/source), protection of mandatory rules, and **GeoIP countries**: allow/deny a country from a manually maintained CIDR database, apply reconciles the managed rules, missing database/ufw shown explicitly
   - **Password expiry** — global number of days before a password must be changed (`0` = never, 1–365); an expired account is limited to the change flow at its next sign-in
3. **Appearance** — Wallpaper (15 presets)
4. **Widgets** — Active monitors (the 11 real monitors, driven by `monitorStore` → `/api/monitor-layout`) + refresh interval (2/5/10/30/60 s, stored in the same blob) + Weather widget (Open-Meteo location, display fields) — Weather was moved out of Appearance on 2026-09-09
5. **Notifications** — Email/SMTP server (server, port, credentials, TLS, send test) + alerts (threshold, channels) + webhook
6. **AI Assistant** — current model, **five models ranked for this host's RAM/GPU** (each with its RAM/VRAM floor, size and speed; a model that needs more memory than the appliance has is flagged, never marked "best choice"), one-click install & load, manual model install, Ollama status + refresh + **External agents (MCP)**: endpoint, kill-switch, tool permissions and API tokens
7. **Monitoring** (4 cards):
   - **UPS Monitoring** — UPS list: battery/runtime/voltage/load/status
   - **SNMP** — Community, version, system info, interfaces, storage queries
   - **SNMP agent** — serve SNMP *from* this appliance: off by default, SNMPv2c with a generated read-only community (`public`/`private` refused) or SNMPv3 with a dedicated user, SHA authentication and AES privacy (passphrases stored encrypted, never shown again), a scope of "this machine only" or "the local network" on one of the appliance's own private addresses, location and contact, and the live agent state; says which package to install when `snmpd` is absent
   - **Full configuration backup** — one archive that can rebuild the box: the database snapshot (accounts, groups, shares, ACLs, quotas, settings), the exported settings and the generated service configs; optionally encrypted with a passphrase, with credentials left out by default (sessions, API tokens and private keys are removed). A restore is validated then **staged**, and applied at the next start, keeping the previous database as `homevault.db.pre-restore`
   - **Wake-on-LAN** — saved machines (name + network-card address): wake one now, or let a scheduled task wake it first; shows which interface a packet would leave through and says "sent, not confirmed" (only the machine itself can prove it woke)
   - **Connected clients** — who is connected right now over SMB / NFS / SFTP / FTP (not just the web UI): client address and host name, account, the share or export they hold, how many files are open, and the process evidence; each source says whether its collector answered and why not; a session can be disconnected (two clicks, and only for the protocols where a per-connection process exists — an NFS export is kernel-held and says so)
   - **Failed sign-in blocking** — block an address for a configurable time after N wrong passwords: the block survives a restart, can also be installed in the firewall (`ufw deny from …`), lists the blocked addresses with their remaining time, and offers a one-click **Pardon** (which also removes the firewall rule); a request-rate row caps unauthenticated requests per minute so a flood is refused instead of reaching the database. The address you are using is never blocked
   - **Remote syslog** — forward the audit events to a collector: UDP/TCP/TLS, RFC 5424 or 3164, facility, the pinned CA for TLS, live queue counters (waiting/sent/dropped) and a "send a test message" probe that reports the collector's own error
   - **Power** — CPU scaling governor, host reboot/shutdown/suspend and per-disk spindown (hdparm APM + idle timer)
   - _(the per-group "File quotas" card was removed on 2026-09-09 — quotas are managed from Users/Groups)_
8. **System** — Software updates (frequency, current/latest version, and the **last update result**: applied / rolled back / failed with the version concerned, read from the installer's signed/atomic engine) + **Database snapshots** (interval, retention, take one now, and **Verify restore** — a non-destructive drill that checks the newest DB snapshot with SQLite's integrity check and decodes the newest configuration archive, with an optional passphrase for an encrypted one) + system information (IP/port, platform, version) + data management (settings export/import/reset) + privacy (local processing, cookies, offline)

---

## 28. Scheduled Tasks

**Features:**
- List of recurring jobs with type, cron expression, enabled state, last run
  and next run
- Create/edit dialog: task type, cron expression (5 fields), payload fields
  per type
- Five task types: `command` (single executable, no shell), `snapshot`
  (pool snapshot, optional recursive), `smart_test` (S.M.A.R.T. self-test),
  `power` (reboot / shutdown / suspend) and **`service`** (trigger an existing
  subsystem: Docker update, trash purge, database snapshot, or a notification
  with a message)
- Run now, enable/disable, delete (history included) and per-task run history
  (50 newest runs, output tail)

**Invariants:** admin-only on every endpoint; the payload is validated at
creation, not at run time.

---

## 29. Metrics

**Features:**
- Five chart cards: CPU, memory, load average, network throughput and max
  temperature
- Range toggle group: 1 h / 24 h / 7 d
- **History retention** select (8 / 30 / 90 days; admin-only write, 1–365
  accepted by the API)
- Empty state when the collector has not stored a sample yet

**Invariants:** the charts are read-only (`require_auth`, same class as
Dashboard) and samples come from the one-minute collector only; the single
write path is the admin retention setting. Empty buckets render a gap, never a
flat line at zero.

---

## 30. Backup destination (Destinations)

**Features:**
- Receive role: on/off, port (default 8001), bind (this machine only / local network), live service state
- One entry per sending computer (tenant): name, repository address, used-of-quota bar with the percentage, over-quota warning, last integrity check, delete
- **Secrets are shown exactly once**, on creation or rotation: repository address, token, repository password, plus a copy-paste `restic init` block
- Quota per tenant in GB (empty = unlimited); over quota the computer's access is revoked until the quota is raised
- Rotate a token (asks first — the old token stops working immediately), check integrity now, optional daily check
- Warnings when the programs the role needs (`rest-server`, `restic`) are missing

**Invariants:** every repository is append-only and private to its tenant, so a
compromised source cannot prune the history it depends on; no read ever returns
a token or a password; mutations are admin-only.

---

## 31. Replication

**Features:**
- Job list: name, source → destination, storage type, schedule, copies kept, bandwidth cap, last result with its error, running state
- Capability line: what this host can actually do (ZFS, Btrfs, SSH, bandwidth limiting)
- Create/edit: name, storage type, source dataset, destination (another dataset here, or another machine over SSH with host/user/port), interval in minutes (0 = manual only), copies to keep, bandwidth in kB/s, enabled
- **Preview** the exact command the next run will execute, without sending anything
- Run now (asks first), delete (warns that copies already made stay on the destination)
- Inline validation: datasets must be dataset paths, Btrfs over SSH is refused, a local destination cannot be the source

**Invariants:** a run creates its own `hv-repl-<id>-` snapshots and prunes
**only** those on the destination; the source is never pruned and a hand-made
snapshot is never touched; every stage is an argv vector, so no credential or
raw argument list reaches a log or a shell.

---

## 32. Traffic control

**Features:**
- Live status: shaping on/off with the reason (off, paused, no interface, every
  service unlimited, outside the schedule) and the exact `tc` commands in play
- Enable switch, interface picker (name + IPv4), one rate per service in kbit/s
  (`0` = unlimited) with a "≈ MB/s" reading
- Egress-only statement and a notice when `tc` (iproute2) is missing
- Weekly schedule: add/remove windows, per-day toggle buttons, start/end times;
  a window that ends before it starts runs over midnight
- Pause all / Resume in one click; Save (applies immediately) and Apply now
- Inline validation errors from the server and the failing `tc` command with
  its stderr

**Invariants:** management ports (22, 80, 443, 8443) are never shaped; the
interface name is the only free-text field reaching a root command and is gated
to the kernel's character set; every command is an argv vector; disabling or
pausing tears the queue down; a missing `tc` reports honestly instead of
pretending to limit anything.

---

## 33. iSCSI

**Features:**
- Live status banner (`role="status"`): on with the number of reachable exports,
  saved-but-not-applied, off, or "this host cannot export" with the exact
  missing pieces (`targetcli`, each kernel module, configfs)
- Feature switch with a named consequence ("turning this off disconnects every
  initiator immediately; disks and exports are kept")
- **Disks to share** — a file HomeVault creates and sizes, a whole block
  device, or a **thin ZFS volume** (offered only when `zfs` is installed and a
  ZFS pool exists); read-only flag; per-disk "used by N LUN(s)" and a refusal to
  delete a disk a LUN still serves
- **Snapshots of a thin volume** — a per-disk panel (camera action) that lists
  the snapshots, takes one instantly, clones a snapshot into a new disk (named
  in the panel) and deletes one; the cloned disk then exports as its own LUN
- **Exports** — IQN (immutable once created), enabled flag, portals (address +
  port), LUNs (number → disk), allowed initiators with optional CHAP
- Delete a disk's backing file (or a thin volume, snapshots included) only after
  ticking "also delete" **and** typing the disk name
- **Apply now** in the toolbar: the only action that runs `targetcli`

**Invariants:** saving only writes metadata — nothing reaches the kernel until
Apply; apply rebuilds only the objects HomeVault itself created
(`targetcli /iscsi delete <iqn>`, `/backstores/<kind> delete <name>`) and never
runs `clearconfig`; `enable` is the last command so an initiator never logs in
to a half-built export; fileio paths are canonicalised and must stay under the
data directory or a pool; block paths must be real devices under `/dev`; CHAP
secrets are sealed with the target IQN + initiator as associated data, never
returned, and redacted from the rendered command list; the editors are hidden
when the host lacks the LIO stack. A zvol disk's kind, name and path are not
editable in place (only its read-only flag): snapshot/clone endpoints refuse a
backstore that is not `/dev/zvol/<pool>/homevault-iscsi/<name>`.

---

## 34. Directory sign-in (LDAP / AD + OpenID Connect + SAML)

**Features:**
- A three-button segment — **Directory (LDAP / Active Directory)** / **Single
  sign-on (OpenID Connect)** / **Single sign-on (SAML)** — picks which provider
  the form below configures; a `role="group"` of `aria-pressed` buttons
  (deliberately not a half tablist). The toolbar Save and the footer counts
  follow the active tab, and the remote-accounts list is shared, each account
  badged with its provider
- Live health banner (`role="status"`): on with the number of known remote
  accounts, off, or "the saved configuration needs attention" with the exact
  server sentence — plus, always, how many **local** administrators can still
  sign in (red at zero)
- **Directory server** — type picker (LDAP / Active Directory, defaults only),
  URL, StartTLS (disabled for `ldaps://`), certificate verification with the
  cost of turning it off spelled out, search base, read-only service account
  and its write-only password (`Keep` / `Forget the stored password`)
- **Test the connection** before saving: the service account alone, or a full
  sign-in with a name and password that reports the role it would grant
- **Who can sign in** — user filter (`{username}` required), optional group
  filter (`{dn}`), and, behind **Advanced**, the four attribute names plus the
  lookup-cache TTL and hit/miss counters
- **Single sign-on** — the button text shown on the login screen, issuer URL,
  client ID, write-only client secret (`Keep` / `Clear the stored secret`),
  the explicit callback URL, and, behind **Advanced**, the four claim names and
  the scopes; its own **Check the provider** panel reads discovery without
  saving anything
- **SAML** — an on/off switch (the status card says "off" *and* the switch is
  right there), the provider entity ID, its sign-in URL and its signing
  certificates (one per line), and a **metadata import**: paste the provider's
  metadata address, *Read metadata*, see what came back, and only then *Use
  these details* — still nothing is written until Save. HomeVault's own entity
  ID and ACS URL are shown read-only, with a copyable service-provider metadata
  document; behind **Advanced**, the three assertion attribute names
- **Roles from groups** — a default role plus `group → role` rows, shared by
  all three providers; a bare group name and a full DN both match
- **Remote accounts** — each principal with its role, provider badge, last
  sign-in, group count, Enable/Disable and Forget (confirm first; the message
  names the consequence and says the remote account is untouched)

**Invariants:** every settings route is `require_admin`, and the two mutating
SAML settings routes authenticate **before** parsing the body (the three
`/api/auth/oidc/*` and three `/api/auth/saml/*` sign-in routes plus
`/api/auth/methods` are public because a signed-out browser must reach them, and
none trusts a credential from the request body alone); the login path is decided
by `users.auth_source` **and** by the provider stored on the principal, never by
the request body, so a local account is never sent to the directory and a remote
account is never checked against a local hash; the directory password is
verified with a fresh bind on every attempt even when the search result is
cached; an outage is a 503 (or a `303` to `/login?sso_error=unavailable`) and
never enters the lockout ladder or the IP ban list; the service-account password
(`directory-bind`) and the OIDC client secret (`oidc-client-secret`) are sealed
with AES-256-GCM and never returned; every substituted filter value is RFC
4515-escaped; the callback URL is configured, never derived from `Host`, must be
`https://` (loopback may use `http://`) and must end in
`/api/auth/oidc/callback`; an ID token is verified before anything is stored —
signature, `alg` ∈ {`RS256`, `ES256`, `EdDSA`} and matching key type, `iss`,
`aud`, `exp`/`iat`, `nonce` — and no token ever reaches a URL (the callback
answers `303` to `/login#sso=<one-shot-ticket>`, spent once within 120 s); a SAML
assertion is verified the same way before anything is stored — an XML DTD is
refused, the signature is checked over exclusive canonicalisation of the signed
element against the configured IdP certificate, the request ID is single-use and
expires, and audience, destination and recipient must all be us; an empty
certificate list refuses every assertion instead of accepting an unsigned one;
limits are re-checked on load and on each sign-in; a corrupt `directory.json`,
`oidc.json` or `saml.json` falls back to "off" rather than to a lockout.

---

## 35. USB one-touch copy (USB Copy)

**Screen:** desktop app **USB Copy** → `frontend/src/routes/UsbCopy.svelte`

**Features:**
- **Copy rules** — name, device (chosen from the attached devices, matched by
  UUID), source folder inside the device, destination folder on the appliance,
  per-rule "check every copied file" and "eject when done", active flag
- **Copy now** per rule; an inactive rule cannot be run
- **Attached devices** — label, size, UUID, mounted/not-mounted state and which
  rule each device matched
- Empty states that name the next action ("No copy rule yet. Create one to get
  started.")

**Invariants:** reads need a session, every mutation is admin-only; the path id
wins over the body id; a copy is scheduled (the response returns before it
finishes) and the task writes `last_run`/`last_status` itself; a pool member is
refused as a source; `lsblk`, the array probes and the mount/unmount/power-off
all carry deadlines.

**Guides:** [guides/usb-copy.md](./guides/usb-copy.md)

---

## 36. Guided redundancy (RAID layout suggestions)

**Screen:** Storage → **Guided redundancy** dialog →
`frontend/src/lib/components/storage/GuidedRedundancyDialog.svelte`

**Features:**
- One card per layout the attached disks allow, each with usable capacity,
  efficiency, tolerated failures, risk level and technology
- Expanding a card shows the risk explanation, rebuild time, disk grouping and
  parity overhead
- Disk/available summary at the top; filtering by minimum redundancy

**Invariants:** reading needs a session, `apply` is admin-only; every layout id
is stable (`raid0`, `raid1`, `raid5`, `raid6`, `raid10`, `raid1_spare`,
`single`); RAID 0 and RAID 5 are never offered together; safety outweighs raw
capacity in the ranking; in-use disks are excluded; **creating the array is not
implemented** — `apply` answers `501` and the dialog says so before the click.

**Guides:** [guides/guided-redundancy.md](./guides/guided-redundancy.md)

---

## Settings card: Notifications — SMS channel

Settings → Notifications → the **Send an SMS** block
(`frontend/src/routes/Settings.svelte`)

**Features:** the enable switch, the phone number (international form), the
gateway URL, the request body with `%to%`/`%text%`, and the access token (a
password field whose placeholder says "Stored — leave blank to keep it" once one
is saved).

**Invariants:** admin-only; the number is refused unless it starts with `+` and
carries 8–15 digits, with a message showing the expected shape; a disabled
configuration is never refused; the token is sealed server-side and never
returned, so the field starts empty and blank means "unchanged".

**Guides:** [guides/sms.md](./guides/sms.md)

---

## Settings cards: DHCP server, local DNS server, fan control, UPS server mode

These four live inside **Settings** (screen 27) rather than as desktop apps.

### DHCP server (Settings → Security)

`frontend/src/lib/components/settings/DhcpServerCard.svelte` — DHCP/TFTP
switches, interface, address range, netmask, gateway, lease time, static
reservations (add/remove), TFTP root and PXE boot file, live server state, the
active lease table, and a focus-trapping confirmation before turning either
server on. A missing `dnsmasq` is reported in place, with the package to
install. **Guides:** [guides/dhcp.md](./guides/dhcp.md)

### Local DNS server (Settings → Security)

`frontend/src/lib/components/settings/LocalDnsCard.svelte` — the enable switch,
the answering interface, the upstream forwarder list (add/remove, with the
appliance's current resolvers offered as suggestions), local names (address or
alias, one row each), answered domains, the "tell the LAN's DHCP clients to
resolve here" switch, the shared-daemon state badge, and a focus-trapping
confirmation before turning the server on. A missing `dnsmasq` is reported in
place, with the package to install. It is the **same daemon** as the DHCP card
above it. **Guides:** [guides/local-dns.md](./guides/local-dns.md)

### Fan control (Settings → Hardware)

`frontend/src/lib/components/settings/FanControlCard.svelte` — the detected
hwmon channels with speed, temperature and firmware-control support; the
profile picker (automatic / quiet / balanced / performance / fixed speed); the
fixed-speed slider floored at 20 %; the check interval; and the four stated
safeguards. A laptop reports that the embedded controller owns the fan and
offers nothing. **Guides:** [guides/fan-control.md](./guides/fan-control.md)

### UPS server mode (Settings → Hardware)

`frontend/src/lib/components/settings/UpsServerCard.svelte` — the enable
switch, bind interface/port/max clients, the per-machine allowlist (name,
address, username, "may shut this appliance down"), the five-stage shutdown
policy with a standing note that two stages are **not available** in this build,
and the daemon state with start/stop. **Guides:**
[guides/ups-server.md](./guides/ups-server.md)

**Shared invariants for all four:** reads need a session, every mutation is
admin-only, and the guard runs **before** the body is deserialized so an
unauthenticated caller always gets `401` and never a `422` that echoes the
schema.

---

## Summary by category

| Category | Number of screens |
|---|---|
| Authentication / Setup | 2 (Login, Setup Wizard) |
| Desktop | 1 (Desktop + 11 floating widgets + search + notifications) |
| System | 10 (Dashboard, Users, Quotas, Groups, My account, Terminal, Virtual Machines, Scheduled Tasks, Widgets, Directory sign-in) |
| Storage | 6 (Storage, RAID, Explorer, Backup, iSCSI, USB Copy — Shares ∈ Storage on the desktop registry side, can be opened via Explorer) |
| Network | 5 (Network, VPN, Cloud Sync, Syncthing, Traffic control) + 1 (Shares) |
| Monitoring | 3 (Health, System journal, Metrics) |
| Productivity | 2 (AI Assistant, Downloader) |
| Infrastructure | 2 (Docker, Rescue) |
| Configuration | 1 (Settings — 8 categories, incl. the DHCP server, local DNS server, fan control and UPS server cards) |
| **Total** | **37 main screens** (= the 32 desktop apps + Widgets editor + Login + Setup Wizard + Desktop + the public share page) |
