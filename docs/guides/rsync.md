# Rsync server

**Purpose:** let another machine pull or push files with plain `rsync` — the
daemon the backup scripts of a home network usually expect — without exposing
the whole appliance. Off by default; one authenticated module per share.

## Where it lives

- **Screen:** Shares → **Rsync** tab (`frontend/src/routes/Shares.svelte`).
  Admins see the whole tab; a non-admin does not get the mutations.
- **Backend:** `src/rsync_server.rs` (pure model, validation, rendering,
  apply) and `src/handlers/rsync.rs` (the HTTP surface).
- **Unit file:** `homevault-rsync.service` — HomeVault's own instance, never
  the host's `rsyncd`. An admin who already runs a system `rsyncd` keeps it.
- **Generated files** (root-owned, `/etc/homevault-rsync/`):

  | File | Mode | What it is |
  |------|------|-----------|
  | `rsyncd.conf` | `0644` | pid file, port, `use chroot`, `max connections`, one `[module]` per share |
  | `rsyncd.secrets` | `0600` | `user:password` lines for the accounts allowed by a module |
  | `modules.json` | `0600` | the state the screen edits — settings, modules, per-user passwords |

  Both the module list and the credentials come from **one** write, so an
  edit can never leave the secrets file and the modules disagreeing.

## API

| Method | Path | Auth |
|--------|------|------|
| `GET` | `/api/shares/rsync` | session (modules, settings, share candidates) |
| `PUT` | `/api/shares/rsync/config` | admin (enable, port, max connections, chroot) |
| `POST` | `/api/shares/rsync/modules` | admin (add or replace a module) |
| `PUT` | `/api/shares/rsync/modules/{name}` | admin (toggle, read-only, allowed users) |
| `DELETE` | `/api/shares/rsync/modules/{name}` | admin |
| `PUT` | `/api/shares/rsync/users/{user}/secret` | admin (set a password) |
| `DELETE` | `/api/shares/rsync/users/{user}/secret` | admin (forget a password) |

A password is written and never read back: the API only reports *whether* an
account has one (`has_secret`).

## Rules the server enforces

1. **A module can only serve a share.** The path is validated against the
   live share inventory on every apply, so the API can never turn into
   "export any directory on this box".
2. **Authentication is mandatory.** A module with no allowed users is refused
   — an anonymous read/write export of a home directory is exactly the
   accident this feature must not make possible.
3. **Module names are lowercase and unique**, at most **32** modules, and
   rsync's own reserved names are refused.
4. **A listed user must exist** and must have an rsync password; otherwise the
   module would advertise an account nobody can authenticate as.
5. **`use chroot = yes` by default**, plus `munge symlinks = yes`, so a
   symlink inside a module cannot escape it. An admin can turn chroot off
   deliberately (the panel says so) for a host that runs the daemon
   unprivileged.
6. **`reverse lookup = no`** — a home LAN has no DNS PTR records and rsync
   would otherwise spend seconds per connection waiting for one.
7. **Validation happens before anything is written.** A refused edit writes
   nothing at all, so a typo cannot leave a half-applied module list behind.
   The add/edit dialog applies the same three rules first (a share must be
   chosen, the name must be lowercase, at least one account must be allowed) and
   shows the reason next to the fields, so the common mistake never costs a
   round trip.

When the `rsync` binary is missing, the tab says so and refuses to enable the
service, rather than starting a unit that will exit immediately.

## Client side

A pull (read-only module, the safest shape for a backup):

```sh
rsync -av --delete rsync://backup@homevault.local:873/documents/ ./documents/
```

A push into a writable module:

```sh
rsync -av --partial ./photos/ rsync://backup@homevault.local:873/photos/
```

The password is the **rsync password** set on Shares → Rsync, not the
HomeVault login. `--partial` is worth keeping: an interrupted transfer resumes
from the temporary file instead of starting over.

### When you do not need this at all

A one-off copy, or a script that can use SSH, does not need the daemon: rsync
over SSH works with the SFTP account and key already configured on the Shares
→ SFTP tab.

```sh
rsync -av ./documents/ backup@homevault.local:/documents/
```

That path is also the recommended one for anything leaving the local network,
because SSH gives confidentiality and host authentication that plain `rsync://`
does not.

## Applied on change

Every accepted edit re-renders the config, rewrites the secrets file, restarts
`homevault-rsync.service` and records the outcome in the journal. Deleting the
last module and turning the server off stops and disables the unit, leaving the
generated files in place so the history of the state stays readable.

## Tests

- **Unit** (`src/rsync_server.rs`): module-name and user validation, refusal of
  an anonymous module, path confinement against the share inventory, config
  rendering (one `[module]` per share, `read only`, `auth users`), secrets file
  rendering and permissions, and the "a refused state writes nothing" rule.
- **Integration** (`tests/rsync_integration.rs`): the real endpoints with a
  temporary config directory — create/update/delete a module, set and clear a
  password (never returned), admin-only enforcement on every mutation, and the
  generated files on disk.
- **E2E** (`tests/e2e/shares/shares.spec.ts`): the tab renders its modules and
  empty state; adding a module opens the modal on the share picker.

## Related

- [Shares](./shares.md), [SFTP jails](./shares.md), [Backup destination](./backup-target.md),
  [API Contract](../API.md)
