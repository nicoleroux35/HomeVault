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
  session, runs `HealthService::run_all_checks()` (`src/health.rs`), and
  reads CPU/memory from `/proc/stat` and `/proc/meminfo`.
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

## Invariants

- `/health` requires a session (a past audit found dashboard endpoints open).
- Check names map to `health.*` translation keys; the timestamp uses the active UI
  locale.

## Related

- [Dashboard](./dashboard.md), [Desktop](./desktop.md), [API Contract](../API.md)
