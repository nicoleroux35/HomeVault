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

## Rehearsing it

A restore is a skill, not a document. Once a quarter:

1. Pick one replication job and one restic repository.
2. Restore a **single folder** to a scratch location on the appliance, and
   compare a checksum with the live copy (`sha256sum`).
3. Run **Check now** on each backup-destination tenant.
4. Confirm the configuration archive you would restore is recent — a backup
   nobody opens is a backup nobody knows is broken.

Write down what you verified and when. `restic check` passing and a hash
matching are the two facts that turn "I think we have backups" into "we have
backups".

## What this runbook deliberately does not promise

- **Snapshots do not survive a dead pool.** They live on it. Only replication
  or restic moves a copy elsewhere.
- **A replication job is not encryption.** The destination should be a machine
  you trust; use a LUKS-encrypted target if it is not.
- **A configuration restore does not restore data**, and does not re-create
  users' SSH keys, passkeys or API tokens tied to the old host.
- **No automated test restore of the whole appliance exists.** Per-tenant restic
  verification runs daily, and replication reports its last outcome; a full
  rebuild is rehearsed manually, above.

## Related

- [Backup destination](./backup-target.md), [Replication](./replication.md),
  [Snapshots](./snapshots.md), [Backups (restic)](./backup-restic.md),
  [Rsync server](./rsync.md), [Configuration backup](./settings.md),
  [Health](./health.md)
