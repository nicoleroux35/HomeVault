# Network locations (CIFS/NFS remote mounts)

A *network location* is a share on **another** NAS, server or computer,
mounted under one directory HomeVault owns and browsable in the Explorer like
a local folder. It is DSM's "remote folder" without a new daemon: the same
`mount`/`umount` the appliance already uses for pools, under the same `sudo`
grants, with an allowlisted set of options.

## Design

- **One fixed root.** Every mount lands on `/mnt/homevault-remote/<name>`,
  where `<name>` is a validated slug (`[a-z0-9-]`, ≤ 32 chars). A request never
  names a target path, so a location can never be mounted over `/etc`, another
  pool or a share.
- **The admin picks a configured definition, not a command line.** A location
  is stored in `data/remote_mounts.json` (name, protocol, server, share/export,
  options, read-only, reconnect-at-boot). The server builds the `mount` argv
  itself from that file; nothing from a request is spliced into a shell.
- **Read-only by default.** A media or backup source is read far more often
  than written, and a mistake must not reach the other server. Read-write is an
  explicit checkbox, not an option string.
- **Options are an allowlist per protocol.** CIFS: `vers`, `uid`, `gid`,
  `file_mode`, `dir_mode`, `iocharset`, `nobrl`, `noperm`, `soft`, `noatime`.
  NFS: `vers`, `soft`, `hard`, `timeo`, `retrans`, `rsize`, `wsize`, `nolock`,
  `noatime`. `ro`/`rw` are the dedicated field, and credential-bearing keys
  (`credentials=`, `username=`, `password=`) are absent by construction — they
  cannot be smuggled through `options`.
- **Credentials never leave the appliance.** A CIFS password is written to
  `data/remote-mounts/<name>.cred` with mode **0600** and passed to
  `mount.cifs` as `credentials=<path>`. It is never on the command line, never
  in an event-log line and never in an API response — the API exposes
  `has_credentials` only. The file is deleted when the location is removed or
  its account is cleared.
- **No new privilege.** `mount`/`umount` over any path under `/mnt` are already
  granted by the scoped sudoers file (`src/setup.rs::StorageBins::mount_grants`),
  and the target directory comes from `mkdir -p /mnt/...`, also granted. The
  feature adds no root surface beyond the two commands the appliance already
  runs.
- **Every command is bounded.** `mount` gets 45 s (a dead server must not hold
  the blocking worker), `umount` 20 s, the capability probe 5 s. A timeout is
  reported as a sentence naming the command and the deadline, never as a hang.
- **Honest capability.** `mount -t cifs` needs `mount.cifs` (`cifs-utils`) and
  `mount -t nfs` needs `mount.nfs` (`nfs-common`). When a protocol's helper is
  missing the request is a `503` naming the tool, and the Explorer panel says
  so instead of offering a button that always fails.

## Endpoints

| Endpoint | Method | Guard | Purpose |
|---|---|---|---|
| `/api/remote-mounts` | GET | admin | configured locations + live state, `kinds` this host can mount, the root |
| `/api/remote-mounts` | POST | admin | add one; `mount_now: true` connects it in the same call |
| `/api/remote-mounts/{name}` | PUT | admin | edit one (a blank password keeps the stored one); a mounted location is unmounted, re-rendered and mounted again |
| `/api/remote-mounts/{name}` | DELETE | admin | unmount (when up), delete the credentials, forget it |
| `/api/remote-mounts/{name}/mount` | POST | admin | mount it (idempotent: an already-mounted target is a success) |
| `/api/remote-mounts/{name}/unmount` | POST | admin | unmount it (an already-unmounted target is a success) |
| `/api/remote-mounts/reconcile` | POST | admin | mount every enabled location that is not up yet — the same pass the server runs at startup |

An invalid name, server, share/export or option is a `400` whose message names
the rule (`Use lowercase letters, digits and dashes only…`,
`Use the share name only…`, `Option 'credentials' is not supported. Allowed: …`).
A location whose `mount` fails keeps the tool's stderr in `last_error`, which
the panel shows under the entry.

## Startup and the Explorer

- At startup a background pass mounts every location with
  `enabled = true` that is not already up. A NAS that is off is a warning, never
  a reason to keep HomeVault from starting.
- A mounted location appears in the Explorer sidebar as a **Network location**
  (`kind: "network"`), and in the disk list as a `network` device — the same
  categorisation the Explorer already used for hand-made CIFS/NFS mounts.
- The panel is **admin-only** (the list reveals which remote hosts the
  appliance talks to, and the mutations run root-owned `mount`). A member sees
  a network location only through a normal `folder` grant pointing at
  `/mnt/homevault-remote/<name>`, which the existing access model already
  enforces.
- The file search skips network locations, like it skips pools: walking a
  remote share from every search would be slow and is not what the sidebar
  promises.

## Invariants

1. The mount point is `<REMOTE_ROOT>/<validated name>` — never a request path.
2. A password exists only in a 0600 file; no argv, no log line, no response.
3. Options come from a per-protocol allowlist; `ro`/`rw` is a field.
4. Every host command has a deadline and reports the tool's own stderr.
5. A missing helper is a `503` naming it, never a silent failure.
6. Removing a location unmounts it first, then deletes its credentials.

## Tests

- Unit (`src/remote_mounts.rs`): name/server/source/option validation, argv
  shape for CIFS and NFS, credentials body, `/proc/mounts` parsing (octal
  escapes and segment boundaries), store round-trip and forward compatibility.
- Unit (`src/snapshots.rs`): the Btrfs version-path builders used by the
  Explorer timeline.
- Integration (`tests/remote_mounts_integration.rs`): admin gate, unsafe name,
  injected server/source, credential option, save-without-connecting, name
  validation before any syscall, reconcile with nothing configured. **No mount
  is ever executed** by the suite.
- E2E (`tests/e2e/explorer/explorer.spec.ts` §19): the panel and its dialog,
  the refusal shown verbatim, and "the API never leaks a password".

## Related

- [Explorer](./explorer.md), [Shares](./shares.md), [ISO mounting](./iso-mount.md),
  [API Contract](../API.md)
