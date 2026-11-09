# Failure injection and recovery rehearsals

**Purpose:** prove that each recovery path works *after* a real failure, not
only on a clean machine. Each scenario below says whether it is automated, what
a manual run must observe, and which fact counts as proof. Every run is recorded
in the table at the end (date, result, evidence) so "we tested it once" has a
date.

Run manual scenarios on a machine you can reset. Never inject a failure into an
appliance that holds the only copy of important data.

## 1. Interrupted scrub (SIGKILL + reboot)

| | |
|---|---|
| **Automated** | `src/scrub.rs` unit tests cover marker round-trip and `resume_decision`; `tests/scheduled_tasks_integration.rs` covers the task entry point |
| **Manual** | Start a scrub from Storage, `kill -9` the server while it runs, restart the service |

**Expected.** `data/state/scrub-in-progress` still names the pool. On start,
`resume_interrupted` sees `in progress` and starts nothing (no double scrub);
when the kernel reports `finished`/`no scrub in progress`, the marker is
removed. A fresh boot with no marker does nothing.

**Proof.** `journalctl -u homevault | grep -i scrub` shows either "still
running; not starting another" or "resumed the interrupted scrub", and the
marker is gone once the scrub completes.

## 2. Crash between temporary write and rename

| | |
|---|---|
| **Automated** | `src/storage/atomic.rs` — `a_stale_temporary_file_does_not_disturb_the_target` |
| **Manual** | `SIGKILL` the server while it saves a Samba/NFS configuration, then restart |

**Expected.** The target (`/etc/samba/smb.conf`, `/etc/exports`, a JSON state
file) is either the previous content or the complete new content — never
truncated or empty. A leftover `.<name>.tmp.<pid>.<nanos>.<seq>` is inert: the
next read and the next write both succeed, and only the target is used.

**Proof.** `testparm -s` accepts the file, `exportfs -ra` succeeds, and the
configuration shown in the UI is one of the two valid versions.

## 3. Corrupted mount / filesystem superblock

| | |
|---|---|
| **Automated** | `tests/recovery_hardware_integration.rs` (opt-in, loop device): `sudo HV_RECOVERY_HW=1 cargo test --test recovery_hardware_integration` |
| **Manual** | `dm-flakey` or zero a Btrfs superblock at 64 KiB, then boot |

**Expected.** The recovery ladder tries `mdadm --assemble`, `btrfs rescue
super-recover`, `e2fsck -p`, then mounts `ro` and raises one `health.storage`
notification naming the pool. It never formats or "repairs" data away.

**Proof.** `findmnt` shows the mount with `ro`, the notification exists once,
and the status endpoint reports `mounted-ro`.

## 4. USB copy corruption

| | |
|---|---|
| **Automated** | `src/usb_copy.rs` — `test_verify_copy_rejects_a_corrupted_destination`, plus the strategy/conflict tests (FIAB-13) |
| **Manual** | Run a copy with "check every copied file", then flip one byte in a destination file and re-run the verification |

**Expected.** A size match is not enough: the SHA-256 comparison fails the copy,
the profile's `last_status` is `Failed` with the mismatching path, and the
source is never deleted. Photo import verifies each file as it is written.

**Proof.** The USB Copy card shows the failed run; the source file is still on
the card; the journal records the copy failure once.

## 5. Truncated restic pack / altered repository

| | |
|---|---|
| **Automated** | `tests/restic_integration.rs` — `record_check` stores `last_check_ok = false` with the reason; `cargo test --lib storage::restic` covers the verdict |
| **Manual** | Truncate one file under `packs/` in a scratch repository, then run `restic check` from Backup |

**Expected.** `restic check` exits non-zero, the scheduled task is marked
failed, and the last successful backup is **not** deleted or overwritten. The
Backup card shows "Failed" with the reason instead of claiming the repository is
healthy.

**Proof.** `GET /api/restic/repos` shows `last_check_ok: false`
and a non-empty `last_check_error`; the previous snapshot is still listed.

## 6. SMB client disconnection (durable handles)

| | |
|---|---|
| **Automated** | No — a real SMB client is required |
| **Manual** | Open a file over SMB, pull the network cable for 30–60 s, restore it |

**Expected.** With `durable handles = yes` and `strict sync = yes`, the client
reconnects and the write continues or fails cleanly; the share does not need a
restart. A kernel without the feature simply drops the session (documented
limit).

**Proof.** The client logs a reconnect, the file's checksum after the write
matches what was written, and the SMB service did not restart.

## 7. iSCSI on a `configfs` host

| | |
|---|---|
| **Automated** | `tests/iscsi_integration.rs` covers the argv and status surface |
| **Manual** | On a host with `target_core_mod`, verify `emulate_write_cache`, `emulate_fua_write`, `emulate_tpu`, then fill the volume past its threshold |

**Expected.** The attributes match the selected write semantics; crossing the
volume-full threshold switches the export to read-only and raises one warning
instead of letting the initiator see I/O errors first.

**Proof.** `cat /sys/kernel/config/target/core/.../attrib/*` matches the UI, and
the initiator sees the volume read-only at the threshold.

## 8. One alert per new SMART finding

| | |
|---|---|
| **Automated** | `src/storage/disk_health_repo.rs` — `a_repeated_finding_stays_one_active_row` |
| **Manual** | Simulate a worsening attribute (or replay a sample) and run "Sample now" twice |

**Expected.** The finding is stored once, the notification is sent only when it
first becomes active, and a resolved-then-reappearing finding reopens the same
row and notifies once more.

**Proof.** The Health → Disks findings list has one row per fingerprint, and the
notification center has one alert, not one per sample.

## Results log

Copy this table into the change or the release notes when a run happens. An
empty cell means "not rehearsed yet"; never fill it from memory.

| Date | Scenario | Result | Evidence |
|------|----------|--------|----------|
| 2026-11-09 | 1 — interrupted scrub | written, not run | `src/scrub.rs` unit tests; hardware run pending |
| 2026-11-09 | 2 — atomic config crash | automated | `src/storage/atomic.rs` |
| 2026-11-09 | 3 — corrupted superblock | written, not run | opt-in `recovery_hardware_integration` |
| 2026-11-09 | 4 — USB copy corruption | automated | `src/usb_copy.rs` |
| 2026-11-09 | 5 — truncated restic pack | automated (verdict) + field run pending | `tests/restic_integration.rs` |
| 2026-11-09 | 6 — SMB disconnect | manual, pending | — |
| 2026-11-09 | 7 — iSCSI `configfs` | manual, pending | — |
| 2026-11-09 | 8 — one alert per finding | automated | `src/storage/disk_health_repo.rs` |

## Related

- [Disaster recovery](./disaster-recovery.md) — rebuilding a machine
- [RAID](./raid.md) — scrub and rebuild status
- [USB one-touch copy](./usb-copy.md) — checksum and copy modes
- [Backups (restic)](./backup-restic.md) — `restic check`
- [Health](./health.md) — disk findings and notifications
- [Security](./security.md) — AppArmor and sandboxing of the service
