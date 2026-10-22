# Health Monitoring

**Purpose:** System health checks and status overview (memory, disk space,
S.M.A.R.T., RAID arrays, network, services, temperatures).

## Probes for monitoring (2026-10-17)

`GET /api/health` cannot be used as a probe: it answers **HTTP 200** even when
the caller is unauthenticated and even when the database is unusable. A
monitoring system built on it can never detect an outage. Two public,
deliberately terse routes fix that (`src/handlers/health_probe.rs`):

| Route | Auth | Answers |
|-------|------|---------|
| `GET /api/health/live` | none | `200 {"status":"ok"}` whenever the HTTP stack responds |
| `GET /api/health/ready` | none | `200 {"status":"ready"}` or `503 {"status":"degraded","checks":{…}}` |

`/ready` runs `SELECT 1` against the pool (memoised for one second, so a
one-second poll does not hammer the database) and reports `degraded` when:

- the database is unreachable or times out (2 s), or
- the **boot-time integrity check failed** — `run_migrations` is followed by
  `PRAGMA quick_check(1)`, and a damaged file is logged and surfaced here
  instead of waiting for the first query to blow up.

A corrupt database does **not** stop the appliance from starting: a degraded
server that can still export its data is more useful than one that refuses to
boot. `PRAGMA integrity_check` (the slow, complete check) is available
separately for on-demand use.

Both routes are **public**, so they must never carry host detail: no hostname,
no disk names, no version, no machine id. Anything added to their body is
public information.

Example probes:

```ini
# systemd (a slow answer counts as a failure)
ExecStartPost=/usr/bin/curl -fsS --max-time 5 http://127.0.0.1:8080/api/health/live
```

```yaml
# docker-compose
healthcheck:
  test: ["CMD", "curl", "-fsS", "http://127.0.0.1:8080/api/health/ready"]
  interval: 30s
  timeout: 5s
  retries: 3
```

Point a load balancer / reverse proxy at `/ready` and a supervisor restart
policy at `/live`.

## Backend

- `GET /health` (`handle_health_endpoint`, `src/web/mod.rs`) requires a valid
  session, awaits `HealthService::run_all_checks(&pool)` (`src/health.rs`),
  and reads CPU/memory from `/proc/stat` and `/proc/meminfo`.
  Returns `{ success, data: { checks[], overall_status, cpu_usage, memory_usage } }`
  — field names are `checks`/`overall_status`, not `status`/`metrics`.
- `GET /dashboard/health` (`handle_health_data`) feeds the Dashboard health card.

Status values on the wire: `healthy` / `warning` / `unhealthy` / `unknown`
(the `HealthStatus` Rust enum, `src/health.rs`) — **not** `critical`.
`critical` is only the *display* label the frontend shows for the wire
value `unhealthy` (see `Health.svelte`'s `getStatusLabel`); a past bug
had the frontend's own status type/switches expect a literal
`'critical'` that never actually appears on the wire, silently falling
back to an untranslated default. Don't reintroduce that mismatch in any
new consumer of this endpoint.

## Per-check degradation contract (2026-10-22)

`run_all_checks` / `run_check` never shell out on the async runtime: the
eight system checks (`df`, `lsblk` + one `sudo smartctl` per disk, `sudo
mdadm`, `systemctl`, `/proc` + `/sys` reads) are offloaded to
`spawn_blocking` *inside* `HealthService` — callers simply `.await` it and
must not re-wrap the call. A hung `smartctl` can slow one answer; it can
never park a Tokio worker or delay another request.

Every check degrades independently and the payload is always complete:

- A check that errors (unreadable file, missing binary, unreachable
  database) becomes an `Unknown` entry named after its slot, with the
  failure in its human message (`"Check unavailable: …"`).
- The remaining checks still run; the aggregate is derived normally and
  `/api/health` keeps answering `200 success:true`.
- A whole-payload `success:false` can therefore only mean "the run itself
  failed" (a panicking check task) — which is also what the frontend's
  explicit error state renders (see below).

The vocabulary is exactly the four `HealthStatus` values above. There is no
fifth value: anything a consumer can't express in `healthy | warning |
unhealthy | unknown` is the consumer's bug (the Dashboard once emitted a
HUD-only `"critical"` — removed).

## Frontend

- `frontend/src/routes/Health.svelte` (self-contained — renders its check
  list inline; no dedicated card components).
- **Overall status lives in the body, not the header** (2026-09-14). It used to
  be a badge in the window header; the header was standardized to icon + title
  only, so the reading moved to the `data-testid="health-overall"` strip at the
  top of the page body, which keeps the original `data-testid="health-badge"`
  on its label pill. Its heading is the i18n key `health.overall_status`
  (renamed from the now-unused `health.subtitle`, so the four locale files stay
  at key parity).
- **A failed load is an explicit error state, not the empty state**
  (2026-10-22, ROADMAP 1.5). `api.healthCheck()` resolves to
  `{success:false, error}` on network failure and the endpoint reports
  failures in-body, so any outcome without `data` renders a dedicated
  `data-testid="health-error"` block (title `health.error_title`, the actual
  error as description) with a Retry button (`data-testid="health-retry"`,
  `health.retry`). The auto-revalidation poll keeps running — the retry is
  the immediate manual path. Never route a failure into the "no checks"
  empty state again.
- `frontend/src/lib/services/api.ts`: `healthCheck`, `getHealthData`.
- The desktop "System Health" monitor widget (`frontend/src/routes/
  Desktop.svelte`, `.hv-health-monitor`) is a second, independent
  consumer of `healthCheck()` — a read-only, at-a-glance tally of the
  checks' statuses, owned by the Widgets app rather
  than this one.

## The checks (9 as of 2026-10-19)

`HealthService::run_all_checks` (`src/health.rs`) runs all of these and
derives `overall_status` from the worst result; adding a check means
adding a function + the `CHECK_LABEL_KEYS` row in `Health.svelte`, the
i18n keys in the four locale files, and the `EXPECTED_CHECKS` constant
in `tests/e2e/health/health.spec.ts`:

`authentication`, `storage`, `memory`, `disk_space`, `disk_smart`,
`raid_arrays` (degraded/failed mdadm arrays — see
[RAID](./raid.md)), `network`, `services`, `temperatures`.

Per-check contracts that are easy to regress (all fixed 2026-10-22):

- **`authentication` is a real database read** — it counts the accounts
  table; a corrupt/unreachable database yields `Unknown`
  (`"Check unavailable: …"`), never a hardcoded "operational". Only a
  scalar count is read (this endpoint is household-readable: no tokens,
  usernames or session material).
- **`network` reports the link state, not the traffic counters** — the
  verdict comes from `/sys/class/net/<iface>/operstate` (`up` …, `lo`
  special-cased as up; `operstate` is also echoed in the details), the
  rx/tx counters from `/proc/net/dev` are detail only. Never infer "up"
  from rx/tx activity again: an idle LAN port used to report `warning`
  on a healthy appliance.
- **`services` includes the appliance's own unit, derived** — the unit is
  parsed from `/proc/self/cgroup` (`…/system.slice/<unit>.service`), not
  hardcoded, and it is **not** checked when the process runs outside
  systemd (dev checkout / container). The appliance's own unit failing is
  `Unhealthy` (the service is still being served from a half-dead restart
  loop); another
  monitored service failing stays `Warning`. Base list: `cron`,
  `systemd-logind`, `network-manager` + the derived own unit.
- **`disk_smart` shells out through the shared helper** (`src/smart.rs`),
  which puts every `smartctl` invocation in the codebase behind the same
  sudo-wrapped argv — Health's `sudo smartctl -H`, Dashboard's
  `run_smartctl` (with the USB `-d sat` retry) and Storage's read /
  temperature / self-test paths (RAID wizard `get_available_devices`,
  `get_smart_details`, `get_smart_capabilities`, `start_smart_test`,
  self-test status/remaining) and the Scheduled-Tasks SMART-test action
  all build their command there. A bare (unprivileged) `smartctl` call
  sees nothing on `660 root:disk` devices and reports healthy hardware as
  "no SMART data" — never reintroduce one. Results are cached 600 s
  (`DISK_SMART_CACHE`).

## Invariants

- `/health` requires a session (a past audit found dashboard endpoints open).
- Check names map to `health.*` translation keys; the timestamp uses the active UI
  locale.
- `run_all_checks` / `run_check` offload the system checks to
  `spawn_blocking` themselves — a caller re-wrapping them is a bug (double
  offload) and calling the old synchronously-named API from an async
  handler directly is a correctness failure (a hung subprocess parks the
  runtime). The notification engine's `check_health` and both health
  handlers rely on this shape.
- `tests/health_integration.rs` pins: the nine-slot order
  (authentication first), the dispatch of every `run_check` arm, the real
  database read of `authentication`, and — via a parked `users` table —
  that one unreadable check degrades to `Unknown` while the other eight
  still run and the endpoint stays `success: true`.

## Related

- [Dashboard](./dashboard.md), [Desktop](./desktop.md), [API Contract](../API.md)
