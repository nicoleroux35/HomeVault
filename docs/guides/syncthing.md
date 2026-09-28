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

## Frontend

- `frontend/src/routes/Syncthing.svelte`.
- `frontend/src/lib/services/api.ts`: `getSyncthingStatus`, `getSyncthingConfig`,
  `getSyncthingDevices`, `getSyncthingFolders`, `getSyncthingConnections`,
  `restartSyncthing`, `pauseSyncthingDevice`, `resumeSyncthingDevice`,
  `scanSyncthingFolder`, `getSyncthingErrors`.

## Related

- [Cloud Sync](./cloud-sync.md), [API Contract](../API.md)
