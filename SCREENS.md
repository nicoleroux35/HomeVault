# HomeVault — List of screens and features

---

## 1. Login (`/login`) TESTS

**Features:**
- Username/password form with show/hide password
- "Remember me" checkbox
- "Forgot password" link — offline numeric recovery code (no email)
- Password reset flow (recovery code → new password)
- Two-factor authentication (2FA) — entry of the 6-digit TOTP code
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

### 26 available applications

| # | Application | Category | Description |
|---|---|---|---|
| 1 | Dashboard | System | Overview and real-time statistics |
| 2 | Storage | Storage | Disks, virtual volumes, encryption, cache |
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
| 24 | Virtual Machines | System | VMs and containers via Incus — guided wizard, shell console, graphical screen, snapshots |
| 25 | Scheduled Tasks | System | Generic cron engine — commands, pool snapshots, SMART self-tests, power actions, run history |
| 26 | Metrics | Monitoring | Historical performance charts (CPU, memory, load, network, temperature) over 1 h / 24 h / 7 d |

> That is 23 entries in `appDefs` + Terminal + Settings + My account. Storage gained a 7th tab
> ("Snapshots" — scheduled snapshots with GFS retention) and Settings gained the Power card.
> The 27 Svelte routes = these 24 apps + Login + Setup Wizard + Desktop.

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
- 2FA disabling (with password confirmation)
- Display of backup codes

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

**5 tabs:**

### Physical disks
- Detailed disk cards: name, size, vendor, model, serial number, type, SMART status, temperature
- Modal with a partition table and a pie chart
- SMART tests (short/long/conveyance)

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

---

## 10. RAID

**Features:**
- RAID array list: name, level, status (healthy/degraded/failed), disk count, size/used/free
- RAID creation (3-step wizard): type (0/1/5/6/10), disk selection, name + filesystem
- RAID array deletion
- Rebuild of a degraded array: selection of the replacement disk

---

## 11. Health

**Features:**
- 8 health checks displayed in order:
  - Authentication, Storage, Memory, Disk space, Disk SMART, Network, Services, Temperatures
- Each check: status icon, translated label, message, expandable details
- Global status badge (healthy/warning/critical/unknown)
- Refresh button

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
- Source selector: `app` (business event logs), `system` (journald), text files (syslog/auth.log/kern.log/daemon.log)
- Entry list: timestamp, unit, priority, PID, message
- Filtering by unit, by priority, text search
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

### WiFi
- Network list: SSID, signal strength, frequency, channel, security
- Network scan
- Connect/disconnect
- WiFi status display

---

## 15. VPN

**2 tabs:**

### WireGuard
- Server status: public key, listening port, state
- Peer (device) list: name, IP, creation date, status, last handshake
- Device addition: config generation + QR code
- Re-view of config/QR for an existing peer
- Peer deletion
- Server start/stop

### OpenVPN
- Connection list: name, status, interface, local/remote IP
- Connection addition: name, config file, credentials
- Deletion, start/stop

---

## 16. File explorer (Explorer)

**Features:**
- Dual view: list and grid
- Folder navigation: breadcrumb, parent folder, entries with size, permissions, owner
- Sidebar: physical disks, removable disks, favorite locations
- File operations:
  - Create a folder
  - Rename
  - Delete (to the trash)
  - Copy / Move (with a folder picker)
  - Upload files (with a progress bar, quota-aware)
  - Download (quota-aware)
- Text file preview
- Permission management (chmod)
- File search
- Sorting: by name, size, modification date
- Trash system: view, restore, purge, empty
- Archive extraction (.zip/.tar/.gz/.bz2/.xz)
- Compression of selected items

---

## 17. Shares/Protocols (Shares)

**Features:**

### Share list
- List: name, path, type (SMB/NFS/FTP/WebDAV/SFTP), enabled status, comment
- Share creation: name, path, type, comment

### Configuration tabs (6 protocols)
- **Shares** — CRUD, service start/stop, status badges
- **SMB** — workgroup, security mode, protocol version, guest access, advanced options
- **NFS** — networks, default access, version, root squash, custom options
- **FTP** — port, passive ports, max clients, TLS, anonymous, chroot, welcome message
- **WebDAV** — port, path prefix, SSL, auth, compression, locking, allowed methods
- **SFTP** — port, list of beneficiaries (user + shares + key state), user addition, public key addition, jail rebuild

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

**4 tabs:**

### Containers
- Container cards: name, image, status, creation date, ports, memory/CPU, available-update indicators
- Start/Stop/Restart
- Deletion
- Logs (real-time stream)
- Editing of `docker-compose.yml` (containers from a Compose project managed by HomeVault): modal editor, syntax validation, save, save + redeploy
- Creation: name, image, ports, volumes (path via FolderPicker), environment variables, networks
- Refresh every 5s

### Images
- Image list: repository, tag, size, date
- Download from Docker Hub (search + results)
- Deletion

### Networks
- Network list: name, driver, scope, subnet, gateway, connected containers

### Compose / Catalog
- Compose projects: list with services and status, start/stop/restart/deletion, editing of the compose file
- App catalog: one-click install/uninstall (Plex, Jellyfin, Nextcloud, Pi-hole, Vaultwarden, Home Assistant, Grafana, etc.)

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

---

## 21. Downloader

**Features:**
- Global statistics: download/upload speed, number active/pending/stopped
- Task list: GID, name, status, progress, speed, error message
- URL addition with a destination directory
- Actions: pause, remove, delete (3-step fallback chain)
- Status icons: active, paused, completed, error, pending
- Refresh every 2s

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
- Snapshots: creation, list, restore, deletion
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
- Model selector
- Conversation history (localStorage)
- Message copying
- Response regeneration
- New conversation
- Markdown rendering in responses
- Localized system prompt

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
2. **Security** (4 merged cards):
   - **HTTPS / TLS** — Enablement of the built-in HTTPS listener (port, self-signed or provided certificate, HTTP→HTTPS redirect, certificate info, regeneration); applies after restart
   - **Antivirus (ClamAV)** — Status, scheduled scans, execution, history, threats, quarantine, definition updates
   - **Anti-Ransomware** — Enable/disable, honeypot, thresholds, auto-response mode, protected paths, history
   - **Firewall (ufw)** — Enable/disable with preview, rules (port/protocol/action/source), protection of mandatory rules
3. **Appearance** — Wallpaper (15 presets)
4. **Widgets** — Active monitors (the 11 real monitors, driven by `monitorStore` → `/api/monitor-layout`) + refresh interval (2/5/10/30/60 s, stored in the same blob) + Weather widget (Open-Meteo location, display fields) — Weather was moved out of Appearance on 2026-09-09
5. **Notifications** — Email/SMTP server (server, port, credentials, TLS, send test) + alerts (threshold, channels) + webhook
6. **AI Assistant** — Ollama models, current model, recommendations based on RAM/GPU, quick install
7. **Monitoring** (2 cards):
   - **UPS Monitoring** — UPS list: battery/runtime/voltage/load/status
   - **SNMP** — Community, version, system info, interfaces, storage queries
   - _(the per-group "File quotas" card was removed on 2026-09-09 — quotas are managed from Users/Groups)_
8. **System** — Software updates (frequency, current/latest version) + system information (IP/port, platform, version) + data management (settings export/import/reset) + privacy (local processing, cookies, offline)

---

## Summary by category

| Category | Number of screens |
|---|---|
| Authentication / Setup | 2 (Login, Setup Wizard) |
| Desktop | 1 (Desktop + 11 floating widgets + search + notifications) |
| System | 7 (Dashboard, Users, Quotas, Groups, My account, Terminal, Virtual Machines) |
| Storage | 4 (Storage, RAID, Explorer, Backup — Shares ∈ Storage on the desktop registry side, can be opened via Explorer) |
| Network | 4 (Network, VPN, Cloud Sync, Syncthing) + 1 (Shares) |
| Monitoring | 2 (Health, System journal) |
| Productivity | 2 (AI Assistant, Downloader) |
| Infrastructure | 2 (Docker, Rescue) |
| Configuration | 1 (Settings — 8 categories) |
| **Total** | **27 main screens** (= the 24 desktop apps + Login + Setup Wizard + Desktop) |
