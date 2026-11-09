# Btrfs deduplication — design and benchmark gate (P2-4)

Status: **design only**. The tasklist requires a benchmark and a decision
before any dedupe pass runs in production. `src/dedup.rs` validates paths and
builds the argv; it does not execute anything.

## Options

| Option | Scope | Cost | Reversibility |
|---|---|---|---|
| `duperemove -r -d` with a persistent `--hashfile` | Selected Btrfs directories | CPU/RAM during the pass; hashfile on disk | Deleting a deduped file frees only the extents it still references; no pool property to undo |
| ZFS `dedup=on` | Whole pool | Permanent dedup table in RAM (roughly 320 bytes per unique block plus overhead) | `zfs set dedup=off` stops new dedupe but the table remains until all deduped blocks are rewritten |

Btrfs is the only candidate for a per-directory feature; ZFS dedup stays a
documented operator decision, never a HomeVault toggle.

## Benchmark plan

On a representative dataset (at least 500 GiB, with real duplicate content
such as snapshots plus a copy):

1. Record baseline `btrfs filesystem usage -b`, free space and snapshot
   shared/unshared bytes.
2. Run `duperemove -r -d -h --hashfile=<pool>/.duperemove-hash <dir>` on a
   copy, measure wall time, peak RSS and CPU.
3. Re-read `btrfs filesystem usage`: expect `Data,single` used to drop and the
   free-space estimate to rise. A pass that does not move either is rejected.
4. Re-run with the hashfile: it must be faster and must not re-read unchanged
   extents.
5. Take a snapshot before and after; verify existing snapshots still restore
   and that `btrfs filesystem du` shows the expected sharing.
6. Corrupt/truncate the hashfile: the next run must rebuild it, not dedupe
   against garbage.
7. Decide: if peak RSS × concurrent passes is acceptable for the low-power
   target, schedule a monthly pass per configured path; otherwise keep the
   argv planning only and document the manual workflow.

## Safety rules already encoded

- Paths must be absolute, contain no `..`, and never point inside
  `.snapshots` (`validate_dedup_path`, `plan`).
- At most 16 paths per pass (`MAX_DEDUP_PATHS`), so one request cannot walk the
  whole appliance.
- The hashfile must be absolute and outside `..`; a relative path would land
  in the process working directory.
- No execution endpoint exists yet: the feature cannot be enabled by accident
  before the benchmark above is recorded.

## Open questions

- Does the target kernel's `duperemove` version support the chosen flags
  (`-r -d -h --hashfile`)? The first lab run must confirm `--help`.
- Should the hashfile live under `data/state/` (backed up?) or on the pool
  (large, disposable)? Preference: pool-local and excluded from backups.
- Interaction with snapshots already taken: dedupe rewrites extents, so the
  benchmark must prove snapshot space does not grow unexpectedly.
