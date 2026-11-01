# Backup destination

**Purpose:** HomeVault can be the **place another machine keeps its `restic`
backups**. Each source is a tenant with its own token, its own append-only
repository and its own quota. Off by default — receiving backups is a network
service an admin turns on deliberately.

## Where it lives

- **Screen:** desktop app **Backup destination** → route
  `frontend/src/routes/Destinations.svelte` (Backup → Destinations).
- **Backend:** `src/backup_target.rs` (model, validation, rendering, apply),
  `src/handlers/backup_target.rs` (HTTP surface),
  `src/backup_target_scheduler.rs` (quota watchdog + daily integrity check).
- **Unit file:** `homevault-rest-server.service`, running
  `rest-server --path <repos> --listen <addr>:<port> --htpasswd-file <auth>
  --append-only --private-repos`.
- **Repositories:** under the data directory, `<data>/backup-target/<tenant>/`,
  so a backup of the data directory carries the received backups with it (the
  screen shows the exact path).

## Settings

| Setting | Default | Notes |
|---------|---------|-------|
| Enable | off | The unit is started and enabled only when on |
| Port | `8001` | Deliberately not rest-server's `8000`, so a hand-run instance cannot collide |
| Listen | `0.0.0.0` | `127.0.0.1` keeps the role local until the admin opens it to the LAN |

`rest-server` is a separate program: when it is missing the screen says so and
the role degrades to unavailable instead of failing at the first request.

## Tenants

Creating a tenant returns **three secrets exactly once** — the repository
address, the token and the repository password. They are never shown again:

- the **token** is 256 bits of randomness, stored only as a SHA-256;
- the **repository password** is sealed with the appliance key and never
  returned by any read.

The row keeps: name, quota, used bytes, over-quota flag, last seen, and the
last integrity check. **Rotate** issues a new token and password (the old
token stops working immediately; the panel warns what to update);
**Delete** removes the tenant row and access, leaving the repository
directory on disk so nothing is destroyed by accident.

### Quota

The watchdog walks each repository every five minutes. When a tenant grows
past its quota, the tenant is **removed from the `htpasswd` file**, so
`rest-server` refuses every further request — upload and read alike — until an
admin raises the quota or frees space. Dropping back under the quota restores
the entry. A quota of `0` means unlimited.

### Integrity check

A tenant can opt into a daily `restic check`, and **Check now** runs one on
demand. The check has a ten-minute ceiling. Only a successful pass advances
the bookkeeping, so a repository that could not be reached keeps its last
known result instead of being reported green.

## Why append-only

`rest-server --append-only` is the point of the feature: a **compromised
source cannot `forget` or `prune` the history it depends on**, so ransomware
on the sending machine cannot reach round and delete its own backups. It also
means the destination never rewrites data on the source's behalf. `--private-repos`
keeps each tenant inside its own repository.

There is deliberately **no second backup format**: the source side is plain
`restic -r rest:http://<host>:<port>/<tenant>/`.

## Client side

On the machine whose backups this destination receives:

```sh
export RESTIC_REPOSITORY=rest:http://homevault.local:8001/laptop-julie/
export RESTIC_PASSWORD='<the password shown once>'
restic init
restic backup /home/julie
```

The token is the HTTP credential: `rest-server` expects it as the password in
the `rest:` URL (`rest:http://<token>@host:port/<tenant>/`) or through the
`RESTIC_REST_PASSWORD`/`RESTIC_PASSWORD` variables your client uses.

## API

| Method | Path | Auth |
|--------|------|------|
| `GET` | `/api/backup-target` | session (settings + tenants, no secrets) |
| `PUT` | `/api/backup-target/config` | admin |
| `POST` | `/api/backup-target/tenants` | admin (returns the secrets once) |
| `PUT` | `/api/backup-target/tenants/{id}` | admin (quota, verification) |
| `POST` | `/api/backup-target/tenants/{id}/rotate` | admin (new token + password) |
| `POST` | `/api/backup-target/tenants/{id}/verify` | admin (check now) |
| `DELETE` | `/api/backup-target/tenants/{id}` | admin |

## Tests

- **Unit** (`src/backup_target.rs`): tenant-name and path confinement, quota
  validation and the revoke/restore decision, repository-URL rendering for the
  LAN vs loopback bind, the generated unit file (it must contain
  `--append-only` and `--private-repos`), and secrets never appearing in the
  tenant JSON.
- **Integration** (`tests/backup_target_integration.rs`): the endpoints plus a
  real second-instance flow — create a tenant, push with `restic` into the
  generated repository, grow past the quota and see access refused, restore,
  verify the test restore by hash.
- **E2E** (`tests/e2e/settings/backup-target.spec.ts`): the screen renders,
  creating a tenant reveals the one-time secrets, cancelling a delete fires no
  request.
- **Scheduler** (`src/backup_target_scheduler.rs`): the pure quota decision and
  the daily-verification cadence.

## Related

- [Replication](./replication.md), [Configuration backup](./settings.md),
  [Disaster recovery](./disaster-recovery.md), [Backups (restic)](./backup-restic.md),
  [API Contract](../API.md)
