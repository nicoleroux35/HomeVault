# HomeVault — Privacy & Records of Processing

This document is HomeVault's **Records of Processing Activities (RoPA)** and
privacy design notes, kept per the *Compliance & security baseline* in
`CLAUDE.md` (GDPR Art. 30 / ISO/IEC 27701 / ISO/IEC 29100 / NIST Privacy
Framework). It describes what personal data the **software** processes, why,
on what basis, and for how long. Keep it updated in the same change that adds
or alters a feature touching personal data.

## Roles

- **Controller:** the person or organisation that installs and operates a
  HomeVault instance. They decide who gets accounts and what data is stored.
- **Processor / software provider:** the HomeVault project ships the code;
  it runs entirely on the controller's own hardware and makes **no** call to
  a HomeVault-operated server. There is no vendor-side processing, no
  analytics, no telemetry.
- **Data subjects:** the instance's user accounts (admins and users) and,
  incidentally, anyone whose IP address reaches the server.

## Design principles applied

- **Local-first / no phone-home.** All processing is on-device. The few
  outbound calls (below) are either off by default, consent/action-gated,
  or strictly feature-scoped, and each is allow-listed in the CSP
  (`src/middleware/security_headers.rs`).
- **Data minimisation & storage limitation.** Only data a named feature
  needs is stored; each store has a retention story (below).
- **Privacy by default.** The protective setting is the shipped default:
  2FA available, sessions expire, journal auto-cleanup **on** (90 days),
  update check **manual** (no automatic outbound call). The only location
  lookup is the browser-side connection popover (best-effort, hidden on any
  failure); the server itself never makes a geo-location call.
- **Security.** Passwords, recovery codes and TOTP secrets are Argon2-hashed
  or stored only as needed to verify; secrets are redacted from exports and
  never logged; all mutations are CSRF-protected and authorisation-gated
  (`require_auth` / `require_admin`).
- **VM login credential (signed-off exception, 2026-09-09).** The password an
  operator sets for a new virtual machine's login account is hashed
  (SHA-512 crypt) before it is written into `cloud-init.user-data`; the
  plaintext is never logged, stored, or returned. A hash — not the plaintext
  — sits in the Incus instance config, which is root-only on the host, and
  every VM endpoint is admin-only. This is the minimum needed for a
  non-technical person to actually log into the machine they created. See
  `docs/guides/vm.md` → Security & privacy.
- **Encryption in transit.** HomeVault can terminate HTTPS itself (opt-in;
  `TLS_ENABLED` / Settings > Security > HTTPS/TLS) with a self-generated
  self-signed certificate or an admin-supplied one, plus an HTTP→HTTPS
  redirect. The self-signed path is fully local — no CA, no third-party call.
  Fronting HomeVault with a reverse proxy that terminates TLS is still
  supported and unchanged. See `docs/guides/tls.md`.

## Records of processing

| # | Data | Purpose | Lawful basis (typical) | Retention | Recipients |
|---|---|---|---|---|---|
| 1 | **Account** — username, optional full name, optional email, avatar seed, role, lock state | Authenticate and authorise users; SMB/OS account provisioning | Contract / legitimate interest of the controller | Until the account is deleted by an admin | Local only; a matching **OS/Linux account** is created (`useradd`) |
| 2 | **Password** — Argon2id hash only | Verify login | Contract | Until account deletion or password change | Local only |
| 3 | **Password history** — Argon2 hashes of previous passwords | Prevent password reuse | Legitimate interest (security) | Last N passwords per user; purged on account deletion | Local only |
| 4 | **Offline recovery code** — Argon2 hash | Self-service password reset without email | Contract | Until account deletion (code is not consumed on use) | Local only |
| 5 | **2FA/TOTP secret + recovery** | Second authentication factor | Contract | Until 2FA disabled or account deletion | Local only |
| 6 | **Sessions** — token, username, client IP, CSRF token, timestamps | Keep a user logged in | Contract | `expires_at` (short, or longer with "remember me"); expired rows deleted; cascade-deleted with the account | Local only |
| 7 | **Login attempts** — username/key, client IP, timestamp | Brute-force lockout, rate limiting | Legitimate interest (security) | Pruned after 7 days | Local only |
| 8 | **Application event log** (`app` journal source) — actor username, action, timestamp; some entries include target names/paths | Security & operational audit trail (logins, 2FA, user/permission/share/Docker/storage/network/settings changes) | Legitimate interest (security), legal obligation (accountability) | Auto-cleanup **on** by default, 90-day interval; admin-configurable in the System Journal app | Local only |
| 9 | **Rotating request/diagnostic logs** (`tracing` file appender) | Debugging, incident response | Legitimate interest | Bounded by file rotation; cleared with the event-log auto-cleanup | Local only |
| 10 | **Transfer quotas** — per user/group upload+download caps and running usage | Enforce fair-use limits on File-Explorer transfers | Legitimate interest of the controller | Deleted with the user/group | Local only |
| 11 | **UI preferences** — widget layout, monitor layout, weather location (city) | Personalise the desktop | Legitimate interest | Cascade-deleted with the account | Local only |
| 12 | **User files** — home-folder and share contents; SFTP chroot jails | Storage service the controller offers | Controller's own basis | Controlled by the controller; home dir optionally deleted on account deletion; SFTP jail purged on account deletion | Local; served over SMB/NFS/FTP/SFTP/WebDAV as the controller configures |
| 13 | **Notifications** — health/backup/security alert text (broadcast, not per-user) | Inform admins | Legitimate interest | Admin clears them | Local only |
| 14 | **Virtual-machine login credential** — the username in clear + the password as a **SHA-512 crypt hash** inside the machine's `cloud-init.user-data` (ready-made VMs only; an ISO install has no seeded account) | Let the operator log into a VM/container they created without any Linux knowledge (console + SSH) | Contract / controller's own basis | Lives in the Incus instance config (root-readable on the host only) until the machine is deleted | Local only; the machine is on the operator's own LAN |
| 15 | **VM graphical screen** — a passwordless VNC server bound to `127.0.0.1` on the host, reachable only through an admin-only, single-use 20 s WebSocket ticket | Show and control the OS installer for an ISO-install VM | Contract / controller's own basis | Only while the VM runs | Local only; never leaves loopback |

## Outbound third-party calls

| Endpoint | When | Default | Data sent | Purpose |
|---|---|---|---|---|
| `api.github.com` (releases) | Update check | **Manual only** — never automatic unless the admin picks a schedule | Server's outbound IP (implicit) | New-version notification |
| `api.open-meteo.com`, `geocoding-api.open-meteo.com` | Weather widget, only once a location is configured | Widget not shown until the admin adds it | City name / coordinates (browser → Open-Meteo) | Weather display |
| `ipwho.is` | Connection popover, best-effort | Row hidden on any failure; browser-side | Server's public IP | Show approximate server location |
| `api.ipify.org` / `ifconfig.me` / `icanhazip.com` | Network → Diagnostics page, on demand | Only when the admin opens diagnostics | Server's outbound request | Discover the server's public IP |
| Docker Hub / GHCR registry APIs | Docker app + app-catalog update detection | Only when Docker features are used | Image names being queried | Container image search & digest comparison (read-only, no pull) |
| `images.linuxcontainers.org` | Virtual Machines app — image download, on demand | Only when the admin downloads a VM image | Server's outbound request | Linux container image downloads |
| `fedorapeople.org` (virtio-win ISO) | Virtual Machines app — driver ISO download, on demand | Only when the admin clicks the VirtIO driver download | Server's outbound request | Windows VM driver ISO |
| SMTP server (admin-configured) | Notification emails | Off until the admin configures it | Alert content, recipient address | Email notifications |
| `webhook_url` (admin-configured) | Notification webhooks | Off until the admin sets a URL | Alert JSON | Discord/Slack-style notifications |

Any **new** outbound host must be added to this table and to the CSP
`connect-src` (browser calls) on purpose.

### Local-network broadcast (not a third-party call)

| Mechanism | When | Default | Data sent | Scope |
|---|---|---|---|---|
| mDNS / Avahi (`src/mdns.rs`) | Continuously while `avahi-daemon` runs | **On** (installed by `install.sh`, published automatically) — matches every NAS distro | The name `homevault.local` + the *type and port* of the web UI and any enabled share service. No credentials, no personal data, no account names. | **Link-local multicast only** (`224.0.0.251:5353` / `ff02::fb`) — never leaves the local segment, never reaches the internet |

Rationale for on-by-default: it carries strictly less information than an
open TCP port already does, stays on the LAN, and a name is the expected way
to reach a home server (every mainstream NAS distribution does the
same). Opt-out: `sudo systemctl disable --now avahi-daemon` (or remove the
package). Details in `docs/guides/mdns.md`.

## Data-subject rights

| Right | How it is served today | Gap |
|---|---|---|
| **Access / rectification** | Admins see and edit accounts in the Users app; a user edits their own profile in "My Account" | No self-service "download all my data" export yet (tracked) |
| **Erasure** | Admin deletes the user in the Users app → purges account, password history, 2FA, recovery code, transfer quota, sessions, UI prefs (FK cascade), SFTP jail, PTY sessions, and optionally the home folder. The event log keeps an audit entry naming the deleted account (legitimate interest / accountability). | Full personal-data export before erasure not automated |
| **Restriction / objection** | Admin can lock an account (blocks login without deleting data) | — |
| **Portability** | Home-folder contents are plain files the user can copy over SMB/SFTP | Structured account-data export not implemented (tracked) |

## Breach readiness

- Security-relevant events are in the event log (audit trail).
- `gitleaks` secret scanning and `cargo audit` / `npm audit` run in CI.
- The controller is responsible for the 72-hour supervisory-authority
  notification; HomeVault's job is to give them the log evidence to do so.

## Known privacy/security follow-ups

Tracked in `CLAUDE.md` (audit findings G3, G4, G10 and per-job progress
ownership). Not yet closed:

The 2026-09-22 audit pass closed **G11**: the dead plaintext-HTTP
`get_location()` (ip-api.com / ipapi.co / ipgeolocation.io) was removed
entirely. `/server/info` now returns only `ip`, `port` and `display_address`;
the server makes no geo-location call, so no row was needed in the outbound
table above.

- `login_attempts` pruning runs at startup, not on a timer.
- No structured self-service data export (right of access / portability).
- `/api/progress/*` jobs are not scoped per initiating user (any
  authenticated user can watch any active job's progress).
