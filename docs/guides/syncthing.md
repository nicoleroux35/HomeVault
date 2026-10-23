# Syncthing

**Purpose:** Continuous file synchronization via Syncthing (proxies the local
Syncthing daemon's REST API).

## Backend

Handlers in `src/handlers/syncthing.rs`. Every handler calls a local
`require_admin(&state, &headers)` — device topology, folder paths, and sync
control are treated as privileged, matching VPN's bar. (A past audit found
none of these handlers checked a session at all; that gap is closed.)

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /syncthing/status\|config\|devices\|folders\|connections\|errors` | admin | read |
| `POST /syncthing/restart` | admin | restart daemon |
| `POST /syncthing/pause` | admin | pause device (`{ device }`) |
| `POST /syncthing/resume` | admin | resume device (`{ device }`) |
| `POST /syncthing/scan` | admin | scan folder (`{ folder }`) |

`pause`/`resume`/`scan`'s `device`/`folder` values are URL-encoded
(`url::form_urlencoded::byte_serialize`) before being interpolated into the
Syncthing REST query string, closing a query-parameter-injection gap.

`handle_syncthing_folders` calls `db/status?folder={id}` once per configured
folder — Syncthing's real REST API has no bulk "all folders" status call, so
this is a deliberate N+1, not an oversight. `DbFolderStatus` intentionally has
no `id`/`label`/`path`/`receive_only` fields (those come from `system/config`
instead) because the real `db/status?folder=X` response doesn't have them —
adding them back would silently break deserialization again.
`SyncthingDevice.cert_name` needs `#[serde(alias = "certName")]`, matching
its sibling fields, since Syncthing's JSON uses camelCase.

### REST client (ROADMAP 18.2)

Every call goes through `syncthing_request` (`syncthing_get` /
`syncthing_post`). Three invariants hold there:

| Invariant | Why |
|---|---|
| **Certificate verification is on** | `http_client()` used to set `danger_accept_invalid_certs(true)` unconditionally, so any impostor answering on the configured address over TLS was trusted. The address is `http://127.0.0.1:8384` today, which is why the flag was inert — it becomes a real man-in-the-middle the day the address is configurable. A unit test scans this module's source and fails if the flag comes back (reqwest exposes no read-back for it). |
| **10 s timeout on every request** | Syncthing answers on loopback; a slower reply means the daemon is wedged, not busy. The client is built with the timeout and *not* with the former `.unwrap_or_default()` fallback, which silently returned a client with **no** timeout. |
| **The API key is untrusted input** | `<apikey>` is read out of `config.xml`, a file an administrator can edit by hand, and is interpolated into an outbound header. `HeaderValue::from_str(..).unwrap()` used to sit on that path: a key holding a control character or a non-ASCII byte **panicked the request task** (and, with `panic = "abort"`, the whole appliance). |

The key rules, in order:

1. **Trimmed.** XML pretty-printing indents the element
   (`<apikey>\n  abc\n</apikey>`) and that leading newline alone is an invalid
   header value — so a hand-formatted (or GUI-rewritten) `config.xml` used to
   be the most likely way to hit the panic.
2. **Blank = no key.** `<apikey></apikey>` sends no `X-API-Key` header, which is
   what a daemon with the GUI API key disabled expects. It is not an error.
3. **Anything else invalid is a named error**, never a panic and never an echo
   of the key: the message starts with `Syncthing API key` and tells the
   administrator to regenerate it in Syncthing's GUI. Every endpoint answers
   `{"success":false,"error":"Syncthing API key …"}` (HTTP 200, the module's
   convention) and `GET /syncthing/status` reports it too — it used to answer
   "Installed but not running" for a daemon that was running perfectly well.

The key read itself (`find_syncthing_config`, up to four paths) is blocking file
I/O and runs on `tokio::task::spawn_blocking`, once per proxied request; it used
to run straight inside the async handler (ROADMAP X.1).

## Frontend

- `frontend/src/routes/Syncthing.svelte`.
- `frontend/src/lib/services/api.ts`: `getSyncthingStatus`, `getSyncthingConfig`,
  `getSyncthingDevices`, `getSyncthingFolders`, `getSyncthingConnections`,
  `restartSyncthing`, `pauseSyncthingDevice`, `resumeSyncthingDevice`,
  `scanSyncthingFolder`, `getSyncthingErrors`.

One `Alert` (`data-testid="syncthing-error"`, title
`syncthing.error_title`, body = the server's own reason) carries every failure
this screen can produce: a refused pause/resume/scan/restart (the three
`else {}` branches were empty, so a refusal looked like a slow request) and a
`GET /syncthing/status` error, which is always the configuration fault above.
The banner is not dismissible while the fault persists, and a successful poll
clears it. When that error is present the "Syncthing is not available / install
it" panel is suppressed: the daemon may be installed and running, and offering
to install it would be a lie.

Devices/folders *load* failures stay silent on purpose — they are what the
"not available" empty state already says.

## Related

- [Cloud Sync](./cloud-sync.md), [API Contract](../API.md)
