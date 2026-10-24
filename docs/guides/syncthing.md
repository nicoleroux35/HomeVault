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
| `POST /syncthing/folders` | admin | add a folder (see below) |
| `DELETE /syncthing/folders/{id}` | admin | remove a folder, files kept |
| `POST /syncthing/devices` | admin | pair a device (see below) |
| `DELETE /syncthing/devices/{id}` | admin | unpair a device, files kept |

`pause`/`resume`/`scan`'s `device`/`folder` values are URL-encoded
(`url::form_urlencoded::byte_serialize`) before being interpolated into the
Syncthing REST query string, closing a query-parameter-injection gap. The same
encoding is applied to the ids in the two `DELETE` routes, which are
re-validated on the way in even though they came out of the URL: the value ends
up in the daemon's REST path, and a decoded `..` must never get there.

### Adding and removing a folder or a device (ROADMAP 18.1)

Until this landed the screen was a monitor: it could show the topology and
pause/resume/rescan, but not add either, so a new phone or a new pool folder had
to be configured on the daemon directly.

**Every field is validated before the first daemon call.** Order matters: a
daemon that is down would otherwise answer with a transport error and mask the
fact that the request itself was never valid.

| Field | Rule | Why |
|---|---|---|
| Folder ID | `[A-Za-z0-9._-]`, 1–64 bytes, not blank | It is the folder's identity and is interpolated into a REST path. |
| Label | trimmed, ≤128 bytes; blank falls back to the ID | Display only. |
| Path | absolute, no `..`, not `/`, inside `state.path_guard.allowed_roots()` | Syncthing creates the directory, so an unconfined path would let an admin write **anywhere the service can**. Same rule as `validate_quarantine_dir`. |
| Sync mode | `sendreceive`, `sendonly`, `receiveonly` | `receiveencrypted` is refused on purpose: a home user cannot recover one from this screen, and a folder nobody can read is not a feature. |
| Devices | ≤64, each already paired, deduped | `POST config/folders` with an unknown device id leaves the folder half-configured. |
| Device ID | eight groups of seven base-32 characters, dashes and case optional | Normalised to the canonical dashed form before it is stored or compared. |
| Device name | ≤128, no control characters; blank falls back to the first group of the ID | Display only. |

Two things are deliberately **not** checked:

- **The device ID check digit.** Syncthing's Luhn variant is the daemon's
  business; the handler only guarantees the *shape*, and relays the daemon's
  refusal. Implementing it here would mean two authorities that can disagree.
- **Whether the folder path exists.** Syncthing creates it, so a path that does
  not exist yet is a legitimate answer — which is why the dialog uses
  `PathField` and not the read-only `PathPickerField`.

The running config (`GET system/config`, via `load_config`) is read once after
validation and serves two purposes: it is the duplicate check (folder id, device
id) and the allowlist of devices a folder may be shared with. The appliance's
own device id is injected into a new folder's `devices` array, and pairing or
removing that id is refused — the host cannot be unpaired from itself.

`POST` answers the created identity (`{"id": …}` or `{"device_id": …}`), which is
enough for the screen to reload; `DELETE` answers a confirmation string.
Both log an event naming the admin who made the change. Removal only edits the
daemon's configuration: files already synced stay on disk, and the screen's
confirmation says so.

### Known debt

`handle_syncthing_folders` calls `db/status?folder={id}` once per configured
folder — Syncthing's real REST API has no bulk "all folders" status call, so
this is a deliberate N+1, not an oversight: with *n* folders every 10 s poll
costs *n*+3 requests on loopback. It is bounded (each has the 10 s timeout, the
client is local, and a home install has a handful of folders), so it is not
scheduled for a fix; folding the per-folder status into a single cache warmed by
one poll loop is the change to make if a 20-folder install ever shows up.

`DbFolderStatus` intentionally has
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
  `scanSyncthingFolder`, `getSyncthingErrors`, `createSyncthingFolder`,
  `deleteSyncthingFolder`, `createSyncthingDevice`, `deleteSyncthingDevice`.

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

### The management dialogs (ROADMAP 18.1)

Each tab has an *Add* action (`syncthing-add-folder` / `syncthing-add-device`)
and the detail pane a *Remove* button (`syncthing-remove-folder` /
`syncthing-remove-device`). Three deliberate choices:

- **A refusal stays inside the dialog** (`syncthing-form-error`), with the typed
  values intact. A validation error is fixable; closing the dialog and dropping
  what the user typed is not. The page-level banner is for the actions that have
  no dialog left to keep open.
- **The defaults are the common case.** Sync mode starts at two-way, and "Share
  with" pre-ticks every paired device — that is what "sync this folder" means to
  a home user. Unticking all of them keeps the folder on this appliance only.
  The type select keeps its three options; "start paused" is omitted, since
  pausing is already one click away on the detail pane (simplicity rule 2).
- **Removal names its consequence.** The confirmation says the files already on
  disk stay, because that is the question a user actually has. It does not
  repeat the action's name twice or invent a second confirmation step.

Values are trimmed before they are sent, so the payload is exactly what the
daemon stores and the success banner (`syncthing-success`) cannot quote a padded
name. Enter in any text field submits when the required ones are filled; Create
stays disabled until the folder ID and path are both non-empty.

## Related

- [Cloud Sync](./cloud-sync.md), [API Contract](../API.md)
