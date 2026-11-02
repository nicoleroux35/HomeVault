# Guided redundancy (RAID layout suggestions)

**Purpose:** given the disks the machine actually has, show what layouts they
could become — RAID 0/1/5/6/10, and a mirror with a hot spare — with usable
capacity, efficiency, tolerated failures, rebuild time and a risk assessment.
A home user who does not know which RAID level to pick can compare the options
in plain language instead of reading `mdadm` documentation.

**Who can use it:** any signed-in account may read the recommendations. The
`apply` route is **admin-only from the start**, even though it does not create
anything yet — it is the route a destructive implementation will hang off, so
its privilege boundary is already the right one.

## Where it lives

- **Screen:** Storage → **Guided redundancy** →
  `frontend/src/lib/components/storage/GuidedRedundancyDialog.svelte`.
- **Backend:** `src/guided_redundancy.rs` (layout construction, capacity
  arithmetic, scoring, risk) and `src/handlers/guided_redundancy.rs`.
- **Storage:** none. The dialog is a pure recommendation engine.

## Endpoints

| Method | Path | Guard |
|--------|------|-------|
| `GET`  | `/api/storage/guided-redundancy?min_redundancy=none\|single\|double` | `require_auth` |
| `POST` | `/api/storage/guided-redundancy/apply/{id}` | `require_admin` → **`501`** |

## What is implemented, and what is not

**Implemented:** the recommendation engine. Layouts are generated from the disk
count and sizes, a capacity is computed per layout (the smallest disk bounds a
parity layout), each layout is scored, and the list is sorted by score
descending. `min_redundancy` filters the candidates: `none` allows RAID 0,
`single` excludes it, `double` adds RAID 6.

**Not implemented: creating the array.** `POST …/apply/{id}` answers `501` with
a sentence pointing at the Storage screen. The dialog says the same thing
*before* the click, so the button is never a trap.

> This is a deliberate, stated limitation rather than a silent one. The route
> exists so the screen can ask "can this be applied?" and get an honest answer
> instead of a `404` it would have to guess about. Wiring it to `disk.rs` is a
> separate change with its own risk review, because the same route would then
> start creating arrays.

**The disk inventory is empty for now.** `handle_recommend` builds an empty
`Vec<DiskInfo>`, so the dialog currently reports "no recommendation available,
every disk is already in use". Connecting it to `disk.rs`'s inventory is a
precondition for the feature being useful end to end, and is the first step of
any follow-up work.

## Rules

- **Every id is stable.** `raid0`, `raid1`, `raid5`, `raid6`, `raid10`,
  `raid1_spare`, `single`. A layout is never offered under two ids, and the ids
  are what the score comparison and the tests key on.
- **RAID 0 and RAID 5 are never offered together.** `raid0_layout` is only
  generated below three disks and `raid5_layout` only from three up, so a
  comparison between the two is meaningless — a test that asserted it was fixed
  to compare the two layouts three disks actually produce.
- **Redundancy is rewarded, no-redundancy is penalised.** `score_recommendation`
  adds for double/single redundancy and subtracts thirty points from a
  no-redundancy layout, so capacity alone never outranks safety.
- **In-use disks are excluded.** `recommend_layouts` filters on `!in_use`
  before anything else; a disk that already belongs to an array is never a
  candidate.
- **The guard runs before the body is parsed.** `apply` takes raw `Bytes` and
  deserializes after `require_admin`, so an unauthenticated caller gets `401`.

## Accessibility note

The layout cards are real `<input type="radio">` elements inside a
`role="radiogroup"` with a label, not a clickable `<div>`. The previous version
selected a card with `on:click` only, which mouse and touch could use but a
keyboard could not. Hover and focus reveal the same affordance
(`:focus-within`, not `:hover` alone).

## Tests

| Suite | What it covers |
|-------|----------------|
| `src/guided_redundancy.rs` (`#[cfg(test)]`) | capacity arithmetic, layout ids, scoring |
| `tests/guided_redundancy_integration.rs` | disk-count matrix, filtering by `min_redundancy`, sorting, scoring |
| `tests/phase4_auth_integration.rs` | `401` without a session, `403` for a member, `501` for an admin |

## Related

- [RAID](./raid.md) — the creation wizard this dialog recommends layouts for
- [Storage](./storage.md) — where the array is actually created
- [Snapshots](./snapshots.md) — what protects the data once the array exists
