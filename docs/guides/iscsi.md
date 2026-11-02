# iSCSI exports (disks over the network)

**Purpose:** publish a disk so another machine can mount it as a local block
device. The admin creates a **disk** (a file HomeVault owns, or a whole block
device) and an **export** (the IQN an initiator logs in to, with its portals,
LUNs, allowed initiators and optional CHAP). Saving only writes HomeVault's
metadata; **Apply now** is the single step that runs `targetcli` and makes the
export reachable, so a whole set of changes can be prepared and then published
at once — or prepared now and applied tonight.

**Who can use it:** any signed-in account can read the configuration; creating,
editing, deleting, switching the feature on/off and applying are **admin-only**
— they end in root-owned `targetcli` commands.

## Where it lives

- **Screen:** desktop app **iSCSI** → `frontend/src/routes/Iscsi.svelte`
  (Storage category, admin-only tile; hidden for a member session).
- **Backend:** `src/iscsi.rs` (model, validation, capability probe, `targetcli`
  argv generation, apply/teardown, backing-file lifecycle),
  `src/handlers/iscsi.rs` (HTTP surface),
  `src/storage/iscsi_repo.rs` (rows), migration `071_iscsi.sql`.
- **Storage:** SQLite for the disks, exports, portals, LUNs and initiators;
  `{DATA_DIRECTORY}/data/iscsi.json` for the feature switch and the last apply
  time.
- **Dependency:** `targetcli` (package **targetcli-fb**, which pulls
  `python3-rtslib`) plus the LIO kernel modules, installed as *optional* by
  `installer/install.sh` and reported by `src/dependencies.rs`. Without them the
  screen still loads, hides its editors and names exactly what is missing.

## Endpoints

| Method | Path | Guard |
|--------|------|-------|
| `GET`    | `/api/storage/iscsi` | `require_auth` |
| `PUT`    | `/api/storage/iscsi/settings` | `require_admin` — `{"enabled": true\|false}` |
| `POST`   | `/api/storage/iscsi/backstores` | `require_admin` — create a disk |
| `PUT`    | `/api/storage/iscsi/backstores/{id}` | `require_admin` — edit a disk |
| `DELETE` | `/api/storage/iscsi/backstores/{id}` | `require_admin` — `?delete_file=true&confirm=<name>` |
| `POST`   | `/api/storage/iscsi/targets` | `require_admin` — create an export |
| `PUT`    | `/api/storage/iscsi/targets/{id}` | `require_admin` — replace an export |
| `DELETE` | `/api/storage/iscsi/targets/{id}` | `require_admin` |
| `POST`   | `/api/storage/iscsi/apply` | `require_admin` — run `targetcli` now |

Every mutation answers with the **full payload**, so the screen never has to
guess what a change did: `{ settings, capabilities, backstores, targets,
defaults, paths, state, apply? }`. `state` carries `usable`, `enabled`,
`pending`, `applied_at` and `exported_targets`; `apply` is present on the
mutation that ran the pipeline and carries `{ applied, reason, reason_code,
commands, failed }` with a **`reason_code`** (`no_targetcli`, `no_kernel`,
`empty`, `off`) the UI localises instead of echoing English.

A CHAP secret is never returned: an initiator reads back as
`{ initiator_iqn, userid, has_secret }`.

## The two objects

**Disk (backstore).** `kind = fileio` is a regular file HomeVault creates and
sizes; `kind = block` is an existing block device and no size is enforced (ioctl
`BLKGETSIZE64` is the source of truth). A fileio disk must live **inside the
data directory or a storage pool**: the path is canonicalised, the parent folder
must already exist, and the result must stay under an allowed root — otherwise
`..` would let the admin point a root-owned `targetcli` at any file on the
system. `lun_count` is computed on every read, and a disk a LUN still serves
cannot be deleted.

**Export (target).** An IQN (≤223 chars, must parse as an iSCSI name and start
with `iqn.`), 1–8 portals, 0–32 LUNs each mapping a number (0–16383) to a disk,
0–32 initiators. An initiator entry is either bare (`userid` null) or CHAP
(`userid` + a 12–16 character secret, per RFC 7143). At most 32 exports and 64
disks.

## What gets built

One `targetcli` argv vector per object, run directly — never through a shell —
and only after the feature is on:

```
sudo targetcli /backstores/fileio create <name> <path> <size_bytes> [ro=1]
sudo targetcli /iscsi create <iqn>
sudo targetcli /iscsi/<iqn>/tpg1/portals create <address> <port>
sudo targetcli /iscsi/<iqn>/tpg1/luns create /backstores/<kind>/<name> <lun>
sudo targetcli /iscsi/<iqn>/tpg1/acls create <initiator_iqn>
sudo targetcli /iscsi/<iqn>/tpg1/acls/<initiator_iqn> set auth userid=<userid>
sudo targetcli /iscsi/<iqn>/tpg1/acls/<initiator_iqn> set auth password=<secret>
sudo targetcli /iscsi/<iqn>/tpg1 set attribute authentication=1
sudo targetcli /iscsi/<iqn>/tpg1 enable
```

- **Apply rebuilds from the metadata, and only names what HomeVault owns.**
  Modules are loaded first, then every target and disk the database lists is
  forgotten (`targetcli /iscsi delete <iqn>`, `/backstores/<kind> delete
  <name>`), then each disk is created, then each enabled export is rebuilt —
  portals, LUNs, ACLs, CHAP attributes — with `enable` **last**, so an
  initiator can never log in to a half-built export. A missing object is the
  normal first-apply case, so those teardown failures are ignored; a failing
  create stops the pipeline and is reported with its argv and stderr.
  This is what makes "remove a LUN", "remove a portal" and "disable this
  export" actually take effect, and what makes a second apply safe. The app
  never runs `targetcli clearconfig`, which would also delete a target an
  admin configured by hand.
- Turning the feature **off** unexports every listed target and removes the
  backstore objects HomeVault created; metadata and backing files stay on disk,
  so turning it back on is one click plus one apply.
- The only free-text values that reach a root command are file paths (canonical,
  root-checked), backstore names (`[A-Za-z0-9._-]`, ≤64), IQNs (parsed), initiator
  IQNs (parsed), portal addresses (IP literal or empty) and CHAP credentials
  (length-checked, never logged — `iscsi::redact_command` masks
  `password=` before anything is stored or shown).

## Apply is explicit

`state.pending` is true when the feature is on, at least one export is saved and
`settings.applied_at` is older than the newest relevant change. The status
banner turns that into a sentence, and **Apply now** is a toolbar button on every
visit. Nothing applies behind the admin's back: no scheduler, no watcher.

## Failure behaviour

- `targetcli` missing or a module absent → apply returns `applied: false`,
  `reason_code: no_targetcli` / `no_kernel` and an **empty** command list. The
  screen shows the reason and the editors stay hidden.
- A failing command stops the pipeline and returns the exact argv and its stderr
  in `failed`; the screen shows it as an alert in the status banner.
- The saved rows are re-validated on `POST /apply`, so a hand-edited database
  cannot reach `targetcli` unchecked.
- Deleting a disk with `delete_file=true` requires `confirm=<name>`; the name is
  compared after trimming, the path is re-checked against the allowed roots, and
  a block device is never unlinked.

## Security notes

- **Least privilege:** reads are `require_auth`, every mutation is
  `require_admin`; the routes sit behind the same CSRF middleware as the rest of
  the admin API.
- **CHAP secrets at rest:** sealed with AES-256-GCM by
  `src/storage/secret_box.rs`, with the associated data binding the target IQN
  and the initiator (`iscsi::chap_scope`) — a blob copied onto another export
  fails to open. Omitting `secret` on an update keeps the stored one; giving a
  new one re-seals with a fresh nonce. Secrets never leave the server, and are
  redacted from the command log.
- **Path safety:** every fileio path is canonicalised and must resolve under an
  allowed root; block paths must sit under `/dev`. Symlink escapes are rejected
  because the check runs on the resolved path.
- **No credential in the URL:** the delete confirmation travels in the query
  string only for the typed-name guard, and is never logged by the app.

## Hardware acceptance

Everything above is proven against the *metadata* and the *commands*, and the
unit/integration suites deliberately never touch the host. One gap remains until
someone runs the acceptance suite on a machine with a real LIO stack: that an
initiator actually logs in and reads back what it wrote.

`tests/iscsi_hardware_integration.rs` closes it. It is **opt-in** and skipped by
default, because CI has no LIO stack and a skipped test must not read as a pass.

```bash
sudo HV_ISCSI_HW=1 cargo test --test iscsi_hardware_integration -- --nocapture
```

Prerequisites, all on a disposable host:

| Need | Why |
|---|---|
| root | `targetcli` writes to configfs, `iscsiadm` loads the initiator modules |
| `targetcli` + `target_core_mod`, `iscsi_target_mod`, `target_core_file`, `target_core_iblock` | the target side |
| `/sys/kernel/config` mounted | where the LIO objects live |
| `open-iscsi` (`iscsiadm`) + a running `iscsid` | the initiator side |
| `dd`, `cmp` | the byte-for-byte check |

The suite exports a 64 MiB fileio backstore on `127.0.0.1:3260` with a
per-process IQN (so a leftover export never collides with the next run), then:

| Test | Proves |
|---|---|
| `hardware_login_read_write` | a real initiator logs in, writes 4 MiB of a random pattern, reads it back and `cmp` finds them identical — and the bytes really reached the backstore file |
| `hardware_chap_is_enforced` | the same target refuses the login with no credentials and with a wrong secret, and accepts the secret the admin typed (sealed at rest, opened at apply) |
| `hardware_survives_reapply` | a second `POST /apply` — what a reboot runs — rebuilds the export idempotently and the data written before it is still there |

`HV_ISCSI_HW_KEEP=1` leaves the export and the session in place after a failure
so it can be inspected with `targetcli ls` and `iscsiadm -m session -P 3`.

A skipped run prints the reason (`set HV_ISCSI_HW=1 …`, `the LIO stack is not
usable here: …`, `must run as root`), so the log always says whether the hardware
path was exercised.

## Related

- **Shares** (`docs/guides/shares.md`) publish *files* over SMB/NFS/WebDAV; iSCSI
  publishes a *block device*, which a client formats itself.
- **Storage** (`docs/guides/storage.md`) owns the pools a fileio disk may live
  on and the devices a block disk may point at.
- **Dependencies** (`docs/guides/dependencies.md`) reports `targetcli` and the
  LIO modules on the maintenance screen.