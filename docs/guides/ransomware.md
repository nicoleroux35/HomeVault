# Anti-Ransomware Protection (honeypots + rate detection)

**Purpose:** Heuristic, best-effort protection against ransomware-style
bulk file encryption/deletion: honeypot decoy files that should never be
touched, and a per-path sliding-window rate detector for bursts of
modify/rename/delete activity. Not a guarantee — see Invariants. Lives in
the **Settings** page as the Anti-Ransomware card.

## Backend

Migration `sqlx/migrations/019_ransomware.sql` adds three tables:

- `ransomware_settings` — singleton row (`id = 1`): `enabled`,
  `honeypots_enabled`, `rate_threshold_events` (default 50),
  `rate_threshold_seconds` (default 10), `auto_response`
  (`alert_only`/`lock_account`/`disable_share`, default `alert_only`).
- `ransomware_protected_paths` — one row per watched path: `id`, `path`,
  `share_name`/`share_type` (nullable — used to resolve the share for the
  `disable_share` response; `Share` in `src/shares.rs` has no numeric id,
  so it's identified by name+type), `honeypot_filename` (nullable),
  `created_at`.
- `ransomware_events` — detected activity: `id`, `protected_path_id` (FK,
  `ON DELETE SET NULL`), `event_type`
  (`honeypot_touched`/`mass_modify`/`mass_rename`/`mass_delete`),
  `detail`, `suspected_username` (nullable, best-effort), `response_taken`,
  `created_at`.

Storage layer `src/storage/ransomware_repo.rs` (`RansomwareRepository`).

Core logic `src/ransomware.rs`:

- `place_honeypot`/`remove_honeypot` — a plausible decoy filename (e.g.
  `Family_Photos_Backup.zip`, `Tax_Documents_2024_DO_NOT_DELETE.pdf`)
  chosen at random from a fixed list, written with harmless placeholder
  content.
- `record_event` — inserts the event row, sends a `NotificationRepository`
  alert (category `ransomware`, severity `critical`), and — unless
  `auto_response` is `alert_only` — executes the response via
  `execute_response`: `disable_share` calls
  `SharesService::set_share_enabled`; `lock_account` calls
  `AuthService::set_user_locked` using the caller-supplied
  `suspected_username`. Either falls back to `alert_only` (recorded as
  such) if it can't determine the share/username, rather than guessing.
- `resolve_username_from_uid` — best-effort `/etc/passwd` lookup used to
  attribute a detected event to a local account.

Watcher `src/ransomware_watcher.rs` (`run_watcher_loop`):

- Owns a `notify::RecommendedWatcher`, watching **only**
  user-configured protected paths — never the whole filesystem, a
  deliberate resource-budget choice for the low-power hardware this
  targets.
- `WatchCommand` (`AddPath`/`RemovePath`/`ReloadSettings`), sent over an
  `UnboundedSender` stored on `AppState` (`ransomware_watch_tx`), lets
  handlers reconfigure the *running* watcher live — no restart needed.
- Honeypot filename matches fire `honeypot_touched` immediately,
  bypassing the rate window — but debounced: `honeypot_debounce`
  (a `HashMap<protected_path_id, Instant>`, 10s window) collapses the two
  raw filesystem events a single logical write reliably produces (a
  data-modify and a close-write, confirmed live a few milliseconds
  apart) into one recorded event and one notification. Without this, one
  real touch produced two duplicate alerts.
- Non-honeypot modify/rename/delete events accumulate in a per-path
  `VecDeque<Instant>`; once the count within `rate_threshold_seconds`
  reaches `rate_threshold_events`, a `mass_*` event fires and the window
  is cleared (so a single burst doesn't fire on every subsequent event —
  though a burst spanning multiple threshold-windows worth of raw events
  can legitimately fire more than once, which is intended: ongoing
  activity should keep alerting, not go silent after the first hit).

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /ransomware/status` | auth | settings + protected-path count |
| `GET /ransomware/settings` | auth | current config |
| `PUT /ransomware/settings` | admin | update config, live-reloads the watcher |
| `GET /ransomware/protected-paths` | auth | list watched paths |
| `POST /ransomware/protected-paths` | admin | add a path (validated via `PathGuard`), optionally places a honeypot |
| `DELETE /ransomware/protected-paths/{id}` | admin | remove a path, cleans up its honeypot file |
| `GET /ransomware/events` | auth | recent events (last 100) |

All handlers in `src/handlers/ransomware.rs`. Read endpoints use
`require_auth`; mutations use `require_admin`.

## Frontend

- `frontend/src/lib/components/settings/AntiRansomwareCard.svelte` —
  settings form, protected-path add/remove, recent-events list. The
  protected-path input is **pick-only** via `theme/PathPickerField.svelte` +
  `theme/FloatingPathPicker.svelte` (the standard non-modal, draggable
  folder/file picker, portalled to `<body>`) — no manual path typing. See
  [Antivirus](./antivirus.md) → Frontend for the shared detail.
- `frontend/src/lib/services/api.ts`: `getRansomwareStatus`,
  `getRansomwareSettings`, `updateRansomwareSettings`,
  `listProtectedPaths`, `addProtectedPath`, `deleteProtectedPath`,
  `getRansomwareEvents`. Types `RansomwareSettings`, `ProtectedPath`,
  `RansomwareEvent`.
- Wired into `frontend/src/routes/Settings.svelte` as
  `<AntiRansomwareCard />`, after the "Privacy & Data" card.

## i18n

All strings under `[ransomware]` in `src/i18n/{en,fr,de,es}.toml`,
including `scope_notice` — the heuristic-not-a-guarantee disclosure shown
directly in the card.

## Invariants

- Heuristic and best-effort, explicitly not a guarantee: honeypots only
  catch an attacker that happens to touch the decoy file; rate detection
  can both false-positive (a legitimate bulk restore/rsync job counts the
  same as an attack) and false-negative (a slow, low-and-slow encryption
  run staying under the threshold). State this in any UI copy touching
  this feature.
- `lock_account`'s UID→username attribution is best-effort (SMB/shared
  accounts can attribute incorrectly); `disable_share` is the more
  reliable auto-response and should be the recommended default in UI
  copy.
- The watcher only covers explicitly protected paths, never the whole
  filesystem, by design — do not change this to a filesystem-wide watch
  without an explicit ask; it's a deliberate resource-budget decision.
- `honeypot_debounce` must not be removed — see the Backend section above
  for the duplicate-alert bug it fixes.

## Verified

Audited live end-to-end on 2026-07-16 — see the "Security suite" section
in `CLAUDE.md` for the full bug list and verification narrative (real
honeypot placement/touch/removal on disk, real rate-threshold crossing
with 60 rapidly-created files, live watcher reconfiguration via
add/remove protected path with no server restart).

## Related

- [Settings](./settings.md), [Antivirus](./antivirus.md),
  [Firewall](./firewall.md), [Shares](./shares.md), [Users](./users.md)
- Backend: `src/ransomware.rs`, `src/ransomware_watcher.rs`,
  `src/storage/ransomware_repo.rs`, `src/handlers/ransomware.rs`
