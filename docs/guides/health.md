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

## Deadlines on every host probe (2026-10-24, ROADMAP X.2)

Each check used to shell out with a bare `Command::output`. The checks run on
the blocking pool (and the notification engine runs them every 60 s), so a `df`
parked on a hung NFS mount, an `mdadm --detail` blocked on a failing disk or a
`smartctl` on a spun-down drive pinned that thread with no ceiling and the
probes piled up behind it. Every probe now goes through `run_timed` via a local
`run_command`: `df`/`lsblk`/`mdadm`/`systemctl` at `HOST_PROBE_TIMEOUT` (10 s),
`smartctl` at `SMART_PROBE_TIMEOUT` (20 s, a drive may need to spin up). A
timeout or a failed spawn degrades that check to `Unknown` (through `degrade`),
never a fabricated healthy reading. A source guard
(`test_no_subprocess_of_this_module_runs_without_a_deadline`) fails if a bare
`.output()`/`.spawn()`/`.status()` comes back.

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

## Security Advisor (QW-10 / DSM-18)

Health answers "is the machine running?". The Security / Firewall / Antivirus /
TLS cards each answer one question about one subsystem. Nothing answered *"is
this box safely configured, and what should I fix first?"* — which is the
question that actually brings a home user to this screen. The Security Advisor
does, and only that.

**It is strictly read-only.** There is one route, `GET /api/security/advisor`
(admin-only), and no write route at all. Every fix is a deep link to the screen
that owns the setting, so a finding can never disagree with the card it points
at.

### Endpoint

| Route | Auth | Answers |
|---|---|---|
| `GET /api/security/advisor` | admin | `{ score, counts, findings[], generated_at, app_version }` |

`401` for no session or an unknown token, `403` for a signed-in non-admin — the
report names accounts, the certificate, the firewall and the shares, so it is
the security posture of the appliance, not public information.

Response shape:

```json
{
  "score": 76,
  "counts": { "pass": 4, "info": 4, "warn": 7, "fail": 0, "total": 15 },
  "findings": [
    {
      "id": "admins-2fa",
      "severity": "warn",
      "title": "1 administrator(s) do not use two-factor authentication",
      "evidence": "1 admin(s) without 2FA",
      "fix": "Turn on 2FA for every administrator: …",
      "link": { "app": "account", "category": null }
    }
  ],
  "generated_at": "2026-10-31T18:08:48+00:00",
  "app_version": "1.0.0"
}
```

`id` is API: the frontend keys on it and the tests assert on it. `severity` is
one of `pass` / `info` / `warn` / `fail`. `link.app` is a desktop app id
(`settings`, `users`, `shares`, `account`, `health`); `link.category` names the
Settings category when the target is Settings.

### The checklist

Fifteen checks, each a pure function of an `AdvisorFacts` value
(`src/security_advisor.rs`), so both directions of every check are unit-tested
without an appliance.

| id | Checks | Severity when it trips |
|---|---|---|
| `admins-count` | at least one administrator exists | `warn` if none |
| `default-admin-name` | no account is named `admin` / `administrator` / `root` | `warn` |
| `admins-2fa` | every administrator uses two-factor authentication | `warn` |
| `admins-password-change` | no administrator holds an administrator-chosen password | `warn` |
| `tls-enabled` | HTTPS is on | `warn` |
| `tls-trusted` | the certificate is not self-signed | `info` (fine on a LAN) |
| `tls-expiry` | the certificate is valid and not about to lapse | `fail` expired, `warn` < 14 days |
| `acme` | automatic renewal is on | `info` |
| `firewall-enabled` | `ufw` is active | `warn` |
| `firewall-rules` | the SSH and web ports are still allowed through | `fail` (locks the admin out) |
| `smb-guest` | guest access is off | `fail` |
| `smb-signing` | `server signing = mandatory` | `warn` |
| `smb-protocol` | the minimum protocol is not SMB1 / NT1 | `fail` |
| `shares-world-writable` | no share folder carries the `other write` bit | `warn` |
| `shares-quota` | at least one share has a size limit | `info` (optional safety net) |
| `snapshots` | at least one snapshot schedule is enabled | `warn` |
| `config-backup` | a configuration archive (`.hvbak`) exists | `warn` |
| `login-auto-block` | repeated failed sign-ins are blocked | `info` (recommended) |
| `updates` | no HomeVault update is available | `warn` |

Checks whose premise is absent are **skipped**, not passed: `tls-*` and `acme`
are not judged when HTTPS is off, `firewall-rules` when the firewall is off,
the three `smb-*` checks when no share uses SMB.

### Three rules the implementation is built on

1. **A signal that cannot be read is never a pass.** Every fact is an
   `Option`; an unreadable one becomes an `info` finding saying so, with the
   screen that owns the signal as its link. A green tick on an unknown state is
   the one thing an audit must never produce.
2. **The score is arithmetic, not opinion.** `pass` and `info` weigh 1, `warn`
   ½, `fail` 0; the score is the weighted percentage, **floored** so 99.6 can
   never display as a pass-looking 100. The counting is a pure function, pinned
   by fixture tests.
3. **Nothing is cached.** Each call re-reads the appliance, so a fixed finding
   disappears on the very next run — no restart, no invalidation step.

The update check reads the **cached** answer
(`handlers::settings::CACHED_UPDATE_RESULT`) only: opening the advisor never
contacts a third party, and "not checked yet" is its own `info` finding rather
than a pass.

No secret ever reaches the payload: the report carries verdicts, evidence and
deep links — no hash, no TOTP secret, no token. Pinned by
`tests/security_advisor_integration.rs`.

### Frontend

`frontend/src/lib/components/health/SecurityAdvisorCard.svelte`, rendered at the
top of `frontend/src/routes/Health.svelte` for `$isAdmin` sessions only. The
score is a real `role="progressbar"` with `aria-valuenow`; the full checklist is
a native `<details>`; every actionable finding carries an "Open …" button that
dispatches the shared `hv-open-app` event (the same convention Explorer → Shares
uses) with the Settings category when there is one. On a failed call the card
shows an error instead of the previous score — an old "100" beside a network
error would be the one misleading answer possible.

### Where the signals come from

`security_advisor::collect` reads the services the other screens already read —
`UserRepository` + `User2faRepository`, `tls::load_settings` +
`tls::certificate_info`, `acme::load_settings`, `firewall::ufw_available` +
`firewall::get_status`, `SharesService::get_smb_config` and the share paths'
permission bits, `ShareQuotaRepository`, `SnapshotScheduleRepository`,
`IpBanRepository`, `PoolService::scan_pools` and `data/config-backups/`. The
host-side probes (certificate files, SMB config, `stat` on each share path,
pool scan, the `sudo ufw --version` probe) run inside one `spawn_blocking`.

## Disk health history (P0-8, 2026-11-05)

`src/disk_health.rs` samples the watched S.M.A.R.T. attributes once a day
(`disk_health_scheduler`, at most one sample per 20 hours) and stores one row
per attribute in `disk_health_samples`. The watched set is reallocated
sectors (5), pending/offline sectors (197/198), reported uncorrectable (187),
command timeout (188), UDMA CRC (199), SSD wear (177/231), temperature
(190/194) and power-on hours (9).

A sample is compared with the previous value and a worsening condition becomes
a **finding** in `disk_health_findings`, deduplicated by
`serial:attribute:condition`. A persistent condition updates `last_seen`; it
does not re-notify every day. Thresholds: reallocated +5, any increase of
187/197/198, wear ≤ 10 %, temperature ≥ 55 °C on **three consecutive
samples**, `smartctl -H` FAILED. A single hot sample is noise and does not
raise a finding. Findings raise the `health.disk_smart` notification event.

The same pass adds system-wide findings: storage I/O errors from
`journalctl -k` over 24 h, `btrfs device stats`, `zpool status -x`, and the
`mdadm --detail` mismatch counter. All commands carry a 20 s deadline.

Endpoints:

| Endpoint | Auth | Notes |
|---|---|---|
| `GET /api/health/disk-health` | session | active findings, worst first |
| `GET /api/health/disk-health/samples?serial=…&limit=…` | session | per-disk history (cap 5 000) |
| `POST /api/health/disk-health/sample` | admin | run one sample now |

The Health screen adds a **Disk health history** card (findings list +
**Sample now** for admins). The card reads the endpoint on mount and on app
focus, so a fixed finding disappears on the next sample without a restart.

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
- **Every host probe carries a deadline** (see above): `run_command` is the
  only allowed shape, and a timed-out probe degrades to `Unknown`, never to a
  fabricated value.
- **The Security Advisor is read-only and never a green tick on an unknown.**
  `GET /api/security/advisor` is admin-only, has no write counterpart, and
  every `None` fact becomes an `info` finding that says the signal could not be
  read. `tests/security_advisor_integration.rs` pins the admin matrix, the
  report shape, the "fix it and the finding disappears on the next run"
  criterion and the no-secret payload; `src/security_advisor.rs`'s unit tests
  pin each check in both directions.

## Failure prediction (P2-10, 2026-11-05)

`GET /api/health/disk-health/prediction?serial=…` (session) fits an ordinary
least-squares line to the stored samples of attributes **5, 187, 188, 194,
197 and 198** and returns, per attribute, the slope per day, `r_squared`, and
(when the slope is positive and the fit is not flat) `days_remaining` with a
residual-based low/high interval.

- At least **5 samples** are required (`MIN_POINTS`); fewer is noise, and the
  endpoint returns no trend for that attribute rather than a fabricated one.
- Thresholds: 5 → 10, 187/188/197/198 → 1, 194 → 60 °C.
- This is a linear trend, not ML. It tells an admin "this counter is heading
  for its threshold"; it does not decide that the disk is failing.
- `GET /api/health/disk-health/predictions` returns the same trends for every
  sampled disk, and the Health screen's **Failure prediction** card renders
  them (attribute, estimated days, fit quality). A disk with fewer than five
  samples simply has no trend.

## Background schedulers (FIAB-02, 2026-11-09)

`src/scheduler_supervisor.rs` wraps every long-running periodic loop. The loop
runs inside an inner `tokio::spawn`, so a panic is returned as a `JoinError`
instead of killing the supervisor; the outcome (`returned unexpectedly`,
`panicked`, `cancelled`) is recorded and the loop is started again after an
exponential backoff (5 s, doubling to 300 s, reset to 5 s after 60 s of
healthy running). One-shot boot tasks (static routes, firewall rules, storage
recovery, remote-mount restore) are deliberately not supervised: restarting
them would repeat a side effect.

`GET /api/health/schedulers` (admin) returns `name`, `running`, `restarts`,
`last_started` and `last_error` for each loop; the Health screen shows the same
in the **Background schedulers** card.

## Weekly health report (P2-12, 2026-11-05)

`src/health_report.rs` sends one aggregated notification per week
(`health.storage`, deduplicated by the notification engine):

- active disk findings and how many are critical;
- the newest S.M.A.R.T. sample timestamp across all disks;
- free space per mounted pool;
- a pending/interrupted scrub, when the marker exists;
- the last `restic check` result.

The report is generated at most weekly (`state/weekly-health-report` holds the
last send) and the scheduler checks every 6 h after a 15 min startup delay.
**Every missing measurement renders as `unknown`, never as `OK`** — a report
that cannot read a fact says so. The `restic check` slot reads the newest
`restic_check` task run (status + finish time). Set
`HOMEVAULT_WEEKLY_HEALTH_REPORT=0|false|no|off` to disable the report.

## Related

- [Dashboard](./dashboard.md), [Desktop](./desktop.md), [API Contract](../API.md)
