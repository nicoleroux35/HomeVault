# mDNS / Zeroconf (`homevault.local`)

**Purpose:** make HomeVault reachable at a stable name — `homevault.local` —
on the local network without any DNS/DHCP setup, and advertise its running
share services so they appear in Finder / file-manager "Network" views and
generic mDNS browsers. Implemented with **Avahi**.

## Overview

- One backend module, `src/mdns.rs`, plus one read-only handler,
  `src/handlers/mdns.rs`. No screen of its own: the on/off control lives in
  **Settings → General** as `mdns_enabled` (see *Turning it off* below).
- Publishing is **automatic and on by default** — the same posture every
  mainstream NAS distribution ships (publishing `<host>.local` via
  Avahi/Bonjour out of the box) — and an admin can switch it off.
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
second HomeVault box on the same segment to `homevault-2.local`.

### Applying a name change without a restart (2026-10-24, ROADMAP 29.2)

`host-name` is read by `avahi-daemon` **at startup only** — `avahi-daemon
--reload` re-reads `/etc/avahi/services/*`, *not* `avahi-daemon.conf` (see
`avahi-daemon(8)`), so the old code restarted the daemon on every change. A
restart drops **every other advertisement on the host** for a few seconds: the
admin's printers, other appliances' shares, anything else Avahi publishes on
that segment.

The rename now goes through Avahi's D-Bus interface instead —
`SetHostName` on `org.freedesktop.Avahi.Server` via `sudo busctl call` — which
renames the running daemon in place. The config file is still written for the
next boot. The call is **verified** by reading `HostName` back with `busctl
get-property`: a call that returns 0 but leaves the old name in place must not
be reported as success, or the fallback never happens. A restart remains the
fallback for a host whose D-Bus call is refused (no `busctl`, no system bus,
Avahi not on the bus) and is logged at `warn` when it is taken.

A service-file-only change still needs just `avahi-daemon --reload`.

## What gets advertised

`src/mdns.rs::render_service_file()` writes
`/etc/avahi/services/homevault.service`:

| Service | Type | Port | Advertised when |
|---|---|---|---|
| Web UI | `_http._tcp` | `settings.port` (8080) | always |
| SMB | `_smb._tcp` | 445 | ≥1 enabled SMB share **and** `smbd` active |
| NFS | `_nfs._tcp` | 2049 | ≥1 enabled NFS share **and** `nfs-server` active |
| FTP | `_ftp._tcp` | `FtpConfig.port` | ≥1 enabled FTP share **and** `vsftpd` active |
| WebDAV | `_webdav._tcp` | `WebdavConfig.port` | ≥1 enabled WebDAV share **and** the in-process WebDAV server listening (`crate::webdav::running_port()`) |
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
| GET | `/mdns/status` | **auth** | `{ enabled, available, running, host_name, fqdn, http_url, services[] }` |

`enabled` is the **live** flag (`crate::mdns::is_enabled()`), not the stored
setting: they agree except in the window where a save failed to reach the host,
and the screen must show what the appliance actually does.

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
- `unpublish()` removes **both** artefacts — the service file *and* the
  `host-name` override — and reloads. Leaving the override behind kept the
  appliance answering as `homevault.local` (and kept the name reserved on the
  segment) after discovery was switched off.
- `run_publisher_loop` skips its tick while `is_enabled()` is false; without
  that, the reconciler would re-create the service file 120 s after the admin
  turned discovery off.
- Every subprocess of this module goes through
  `crate::utils::process::run_timed` / `run_timed_with_stdin` — `MDNS_PROBE_TIMEOUT`
  (10 s) for `which`/`systemctl is-active`/`busctl get-property`,
  `MDNS_WRITE_TIMEOUT` (30 s) for `sudo tee`/`sudo rm`, `MDNS_SERVICE_TIMEOUT`
  (60 s) for `systemctl`/`busctl call`/`avahi-daemon --reload`. A source guard
  (`every_subprocess_is_bounded`) fails if a bare `Command` invocation comes
  back.
- `/etc/avahi/**` paths resolve through `avahi_conf_path()` /
  `service_file_path()`, which honour `HV_AVAHI_CONF` /
  `HV_AVAHI_SERVICE_FILE`. Those overrides exist so a test can drive the whole
  publish/unpublish chain against a tempdir; unset in production.
- `ensure_host_name()` never clobbers unrelated config lines/sections.
- The reconciler only reloads Avahi on an actual content change.
- `render_service_file()` XML-escapes every TXT record.

## Turning it off (2026-10-24, ROADMAP 29.1)

**Settings → General → "Discoverable on the local network"** (`mdns_enabled`).
Turning it off calls `crate::mdns::apply_enabled(port, false)`, which:

1. deletes `/etc/avahi/services/homevault.service`,
2. drops the `host-name=` override from `avahi-daemon.conf` (so the daemon
   falls back to the host's own name),
3. renames the running daemon back through D-Bus (or restarts it when D-Bus
   refuses),
4. flips the flag the reconciler reads, so the service file is not recreated
   on the next tick.

Turning it back on republishes **immediately** instead of waiting up to 120 s
for the reconciler. A host that refuses the change (no `sudo`, no bus) is
reported in the save response as `mdns_notice` and shown in the card — a save
that could not reach the host must not look saved.

Before this existed, `unpublish()` had **no caller at all**: the only way to
stop advertising was to disable Avahi by hand or uninstall it. The setting was
the missing link, and the roadmap tracked the dead function as the evidence.

For a machine-wide opt-out (all Avahi clients, not just HomeVault):
`sudo systemctl disable --now avahi-daemon`, or mask the unit.

## Client support

- **macOS**: native (Bonjour).
- **Windows 10/11**: built-in mDNS resolver.
- **Linux**: needs `libnss-mdns` + `mdns4_minimal` in `/etc/nsswitch.conf`
  (`installer/install.sh` installs `libnss-mdns` on the HomeVault host itself).
- **Android**: does not resolve `.local` natively.

## Tests

- Unit (`src/mdns.rs` `#[cfg(test)]`): `ensure_host_name` (insert / replace
  wrong+commented / no-op / append section / empty file), `remove_host_name`
  (drops the override and the commented one, leaves every other line, no-op
  without one, exact inverse of `ensure_host_name`), `render_service_file`
  (web UI always present, share ports, TXT escaping), `status` shape without
  Avahi, the `enabled` flag round-trip, the path overrides, and
  `every_subprocess_is_bounded` (source guard, ROADMAP X.2).
- Integration (`tests/mdns_integration.rs`): `GET /mdns/status` — session
  required (401/`success:false`), returns `homevault.local` + the correct
  `http://` URL for any signed-in user, `services` always contains
  `_http._tcp`, `enabled` is reported; plus the **whole settings chain** with
  both Avahi paths redirected into a tempdir and a `sudo`/`which`/`systemctl`/
  `busctl` shim first on `PATH` — `PUT /settings/general {mdns_enabled:false}`
  really deletes the service file and the `host-name` line (and leaves the rest
  of the config intact), `{mdns_enabled:true}` writes both back, and the
  setting stays admin-only on both verbs.
- No E2E (no screen).

## Related

- [Shares](./shares.md), [Network](./network.md), `PRIVACY.md`
