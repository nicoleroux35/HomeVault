# Disaster recovery

**Purpose:** what to do when the appliance is gone, partly broken, or has lost
data — and what each of the three independent recovery paths can and cannot
bring back. This is a runbook: read it before you need it, keep a printed copy
of the secrets table.

## What protects what

| Layer | Survives | Loses | Feature |
|-------|----------|-------|---------|
| **Configuration backup** | accounts, groups, shares, ACLs, quotas, settings, generated service configs | nothing about the data | Settings → Configuration backup ([Settings](./settings.md)) |
| **Snapshots** | filesystem state at a point in time, on the same pool | the pool itself (a dead disk loses them) | [Snapshots](./snapshots.md) |
| **Replication** | a second copy of each dataset, on another disk or another machine | nothing, if the destination is a different machine | [Replication](./replication.md) |
| **Backup destination** | `restic` repositories another machine pushed here | nothing, until the quota revokes that machine | [Backup destination](./backup-target.md) |
| **Restic backups** | files, deduplicated and encrypted, on whatever repository you configured | nothing, if the repository is off-box | [Backups (restic)](./backup-restic.md) |

The rule that matters: **a copy on the same disk is not a backup.** Snapshots
answer "I deleted the wrong folder"; replication and restic answer "the pool is
gone".

## Scenario 1 — a file or folder was deleted

1. Explorer → open the containing folder → **Rewind**, or *Snapshots* on the
   pool, and copy the old version out. Rollback is destructive; copying is not.
2. If the network client deleted it, check Explorer → **Trash** — the SMB
   recycle bin and the Explorer Trash are the same place.
3. If the snapshot is gone too, go to Scenario 3.

**Check the protection locks first.** A locked snapshot (Storage → Snapshots →
*Protection locks*) cannot be destroyed before its date, which is exactly what
makes this scenario recoverable after a ransomware run. Do not release a lock
to "clean up" before confirming the copy you need is elsewhere.

## Scenario 2 — the system came back but data is missing or wrong

1. Restore the **configuration** first, so accounts, shares and ACLs match
   what the data expects: Settings → Configuration backup → **Restore**, pick
   the archive. The restore is **staged** and applied at the next start —
   HomeVault says so rather than pretending it is instant. It keeps a
   `.pre-restore` copy of the database, so a restore of the wrong archive is
   itself recoverable.
2. Restore the data from a snapshot, a replica, or a restic repository
   (Scenario 3), then verify sizes and spot-check files.

## Scenario 3 — restore data from a restic repository

On the machine that holds the repository (or from the appliance itself when it
is the destination):

```sh
export RESTIC_REPOSITORY=rest:http://homevault.local:8001/laptop-julie/
export RESTIC_PASSWORD='<the password from the one-time panel>'
restic snapshots                      # pick the snapshot id
restic restore <id> --target /tmp/restored --include /home/julie/documents
```

Verify before overwriting anything live:

```sh
# Compare what came back with what the source recorded
restic ls <id> | wc -l
du -sb /tmp/restored/home/julie/documents
```

A repository on a HomeVault **backup destination** is append-only by
construction, so restoring reads it; nothing in that flow can prune it. If a
tenant went over quota its access was revoked — raise the quota (or free space)
in the destination screen and the htpasswd entry is restored on the next
watchdog pass.

## Where the data directory lives (FIAB-04, 2026-11-09)

The SQLite database, JSON settings and configuration archives live under
`DATA_DIRECTORY` (default `/var/lib/homevault`). A pool holds the files, but
the pool does not hold the configuration: if `/` is a single disk and it fails,
accounts, shares, ACLs, quotas and settings are lost even though the data
survives.

The installer accepts **`HV_DATA_DIR`** (or the legacy `DATA_DIRECTORY`) and
refuses a relative path, `/`, whitespace or `..`. When the chosen directory is
on the same filesystem as `/`, it prints a warning with the remedy. To keep the
configuration on a redundant pool:

```bash
sudo HV_DATA_DIR=/mnt/pool/homevault bash installer/install.sh
# the pool must be a ZFS mirror, a Btrfs RAID1 or an mdadm RAID1/5/6/10
```

For an existing install, stop the service, copy `data/` (preserving ownership)
to the new path, update `DATA_DIRECTORY` in the systemd unit (or reinstall with
`HV_DATA_DIR`), then start and run **Verify restore**. The directory must be
writable by the service account (`root` by default, `homevault` in the split
deployment).

This reduces the blast radius; it does not replace an **off-machine** copy. A
system-disk loss still needs a configuration archive plus a data backup that
are not on the failed machine — that is the bare-metal rebuild below.

## Scenario 4 — bare-metal rebuild on new hardware

1. **Install HomeVault** on the new machine (`installer/install.sh`, or the
   same release you were running). Do not import any pool yet.
2. **Complete the setup wizard** far enough to have an admin, then *stop* — a
   restore replaces the database.
3. **Restore the configuration archive.** Settings → Configuration backup →
   Restore → restart. Accounts, shares, ACLs, quotas and the generated service
   configurations come back; the data does not.
4. **Re-attach the data**, in this order, stopping at the first one that works:

   | Source | Command / screen |
   |--------|------------------|
   | The old disks, still healthy | Storage → *Import* the existing ZFS/Btrfs pool — never recreate it |
   | A replication destination | Create the job in reverse (destination → this machine) and run it once; the dataset arrives intact |
   | A restic repository | `restic restore` as in Scenario 3 |
   | Another machine's copy | Recreate the share and `rsync` it back (see [Rsync server](./rsync.md)) |

5. **Encrypted volumes** need their passphrase before anything on them is
   readable: Storage → Encryption → unlock. A lost passphrase means the data on
   that volume is lost; there is no recovery path, by design.
6. **Re-issue credentials that were tied to the old machine**: TLS certificate
   (ACME re-issues on the next attempt), SSH host keys, share links, API
   tokens, and any **passkey** — a passkey is bound to the appliance's
   hostname, so it keeps working only if the hostname is unchanged
   ([Identity](./identity.md)).
7. **Re-check the security posture**: Health → Security Advisor should be green
   before the appliance goes back on the network, and the firewall rules should
   match what the appliance was doing.

## Automated restore drill (P2-9, 2026-11-08)

Settings → System → Database snapshots carries **Verify restore**. The drill is
non-destructive: it never attaches the live database, never writes to the data
directory, and removes its temporary copy when it finishes.

1. Copies the newest `data/backups/*.db` snapshot into a temporary directory,
   opens it **read-only**, runs `PRAGMA integrity_check`, and counts the
   application tables and users. A file without SQLite's header is refused as
   "not a readable SQLite database" before it is opened.
2. Decodes the newest `data/config-backups/*.hvbak` archive. Every entry is
   checksum-verified; an **encrypted** archive is only fully verified when its
   passphrase is supplied (the field on the card, sent as a request header and
   never stored).
3. Reports the newest restic backup job. A job older than seven days, a failed
   job, or no job at all is a **warning**: replication or immutable snapshots
   may still cover the data, but the drill says it cannot prove it.

The verdict is **Ready to restore** only when the database snapshot and the
configuration archive are both present and fully verified. The result is saved
to `data/restore-drill.json`, so the card shows the last run after a restart.
API: `GET/POST /api/settings/backup/restore-drill` (admin; optional
`x-backup-passphrase` header).

This proves the **local recovery chain**. It does not prove that the data
volumes themselves are on a surviving medium — that is the restic/replication
check, and the quarterly rehearsal below.

## Rehearsing it

A restore is a skill, not a document. Once a quarter:

1. Run **Verify restore** (above) and confirm the verdict is *Ready to
   restore*.
2. Pick one replication job and one restic repository.
3. Restore a **single folder** to a scratch location on the appliance, and
   compare a checksum with the live copy (`sha256sum`).
4. Run **Check now** on each backup-destination tenant.
5. Confirm the configuration archive you would restore is recent — a backup
   nobody opens is a backup nobody knows is broken.

Write down what you verified and when. `restic check` passing, the drill saying
*Ready to restore*, and a hash matching are the facts that turn "I think we have
backups" into "we have backups".

## What this runbook deliberately does not promise

- **Snapshots do not survive a dead pool.** They live on it. Only replication
  or restic moves a copy elsewhere.
- **A replication job is not encryption.** The destination should be a machine
  you trust; use a LUKS-encrypted target if it is not.
- **A configuration restore does not restore data**, and does not re-create
  users' SSH keys, passkeys or API tokens tied to the old host.
- **The automated drill does not boot a VM.** It proves the local database
  snapshot and configuration archive are restorable, and reports the last
  data-backup job; a full rebuild on new hardware is still rehearsed manually,
  above. Per-tenant restic verification runs daily and replication reports its
  last outcome.

## Related

- [Backup destination](./backup-target.md), [Replication](./replication.md),
  [Snapshots](./snapshots.md), [Backups (restic)](./backup-restic.md),
  [Rsync server](./rsync.md), [Configuration backup](./settings.md),
  [Health](./health.md)
