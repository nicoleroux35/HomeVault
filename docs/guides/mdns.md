# mDNS / Zeroconf (`homevault.local`)

**Purpose:** make HomeVault reachable at a stable name — `homevault.local` —
on the local network without any DNS/DHCP setup, and advertise its running
share services so they appear in Finder / file-manager "Network" views and
generic mDNS browsers. Implemented with **Avahi**.

## Overview

- One backend module, `src/mdns.rs`, plus one read-only handler,
  `src/handlers/mdns.rs`. No frontend screen.
- Publishing is **automatic and on by default** — the same posture every NAS
  distro ships (OpenMediaVault, CasaOS, TrueNAS, Synology all publish
  `<host>.local` via Avahi/Bonjour out of the box).
- **Link-local only** — multicast `224.0.0.251:5353` / `ff02::fb`. No outbound
  traffic, no third party. It advertises only service *types + ports* that
  HomeVault already listens on; never credentials or personal data. See
  `PRIVACY.md`.
- Depends on the `avahi-daemon` package (`installer/install.sh` installs it
  via `install_optional`; `src/dependencies.rs` reports it in the startup
  check). No-op when it is absent.

## How the name is pinned

`src/mdns.rs::ensure_host_name()` sets `host-name=homevault` in the
`[server]` section of `/etc/avahi/avahi-daemon.conf` (a targeted edit — every
other line is preserved), so the address is always `homevault.local`
regardless of the OS hostname. Avahi's own conflict resolution renames a
second HomeVault box on the same segment to `homevault-2.local`. Changing the
`host-name` key needs a `systemctl restart avahi-daemon` (a plain reload does
not re-read it); a service-file-only change needs just `avahi-daemon
--reload`.

## What gets advertised

`src/mdns.rs::render_service_file()` writes
`/etc/avahi/services/homevault.service`:

| Service | Type | Port | Advertised when |
|---|---|---|---|
| Web UI | `_http._tcp` | `settings.port` (8080) | always |
| SMB | `_smb._tcp` | 445 | ≥1 enabled SMB share **and** `smbd` active |
| NFS | `_nfs._tcp` | 2049 | ≥1 enabled NFS share **and** `nfs-server` active |
| FTP | `_ftp._tcp` | `FtpConfig.port` | ≥1 enabled FTP share **and** `vsftpd` active |
| WebDAV | `_webdav._tcp` | `WebdavConfig.port` | ≥1 enabled WebDAV share **and** `homevault-webdav` active |
| SFTP | `_sftp-ssh._tcp` | `SftpConfig.port` (2222) | ≥1 enabled SFTP share **and** `homevault-sftp` active |

`detect_services()` derives this from `SharesService::get_shares()` (the
`enabled` flag) plus a `systemctl is-active` check per daemon — advertising a
dead port only makes clients time out.

## Keeping it in sync

`src/mdns.rs::run_publisher_loop(port)` is spawned at startup from
`src/web/mod.rs`. It calls `publish()` immediately, then every **120 s**
re-derives the advertised set and rewrites the service file **only if the
content changed** (a share was enabled/disabled on the Shares page, a daemon
came up or went down). Cheap when nothing changed — a few `systemctl
is-active` calls and a string compare. `publish()` is fully idempotent: it
touches `avahi-daemon.conf` / the service file only on a real diff and
reloads/restarts only when something was written.

## Endpoint

Path written without the `/api` prefix.

| Method | Path | Auth | Body / response |
|---|---|---|---|
| GET | `/mdns/status` | **auth** | `{ available, running, host_name, fqdn, http_url, services[] }` |

`require_auth`, not `require_admin`: mDNS already broadcasts this exact
information unauthenticated to every device on the segment, so gating it
behind admin would be theatre — but it is host-shaped, so it stays behind a
session. `src/handlers/mdns.rs::handle_mdns_status` — never fails on a host
without Avahi (`available: false`, still returns the canonical address).

## Invariants

- All writes to `/etc/avahi/**` go through `sudo tee` / `sudo rm` — the
  process may not be root and a bare `std::fs::write` would silently `EACCES`
  (same class of bug fixed in `network.rs` / `shares.rs`).
- `publish()` is a no-op (returns `Ok`) when `avahi-daemon` is not installed.
- `ensure_host_name()` never clobbers unrelated config lines/sections.
- The reconciler only reloads Avahi on an actual content change.
- `render_service_file()` XML-escapes every TXT record.

## Turning it off

Stop and disable Avahi: `sudo systemctl disable --now avahi-daemon`, or delete
`/etc/avahi/services/homevault.service`. The reconciler will recreate the
service file on its next tick if `avahi-daemon` is running again — remove the
package (or mask the unit) for a permanent opt-out.

## Client support

- **macOS**: native (Bonjour).
- **Windows 10/11**: built-in mDNS resolver.
- **Linux**: needs `libnss-mdns` + `mdns4_minimal` in `/etc/nsswitch.conf`
  (`installer/install.sh` installs `libnss-mdns` on the HomeVault host itself).
- **Android**: does not resolve `.local` natively.

## Tests

- Unit (`src/mdns.rs` `#[cfg(test)]`): `ensure_host_name` (insert / replace
  wrong+commented / no-op / append section / empty file), `render_service_file`
  (web UI always present, share ports, TXT escaping), `status` shape without
  Avahi.
- Integration (`tests/mdns_integration.rs`): `GET /mdns/status` — session
  required (401/`success:false`), returns `homevault.local` + the correct
  `http://` URL for any signed-in user, `services` always contains
  `_http._tcp`.
- No E2E (no screen).

## Related

- [Shares](./shares.md), [Network](./network.md), `PRIVACY.md`
