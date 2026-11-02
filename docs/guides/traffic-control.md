# Traffic control (per-service bandwidth limits)

**Purpose:** stop one protocol server from saturating a home connection. The
admin gives SMB, NFS, FTP, WebDAV or SFTP a ceiling in kbit/s; HomeVault shapes
the traffic the appliance **sends** through the interface they pick. An optional
weekly schedule narrows when the limits apply, and a pause-all switch releases
the link in one click without deleting the policy.

**Who can use it:** any signed-in account can read the policy and its state;
saving, pausing and re-applying are **admin-only** — the policy drives
root-owned `tc` commands.

## Where it lives

- **Screen:** desktop app **Traffic control** → `frontend/src/routes/TrafficControl.svelte`
  (Network category, admin-only tile; hidden for a member session).
- **Backend:** `src/traffic_control.rs` (policy, validation, schedule
  arithmetic, `tc` command generation, apply/scheduler),
  `src/handlers/traffic_control.rs` (HTTP surface).
- **Storage:** `{DATA_DIRECTORY}/data/traffic_control.json` (atomic JSON write —
  no migration, the policy is a single document).
- **Dependency:** `tc` from **iproute2** (`src/dependencies.rs`, installed by
  `installer/install.sh`). Without it the screen still loads and says so;
  nothing is shaped.

## Endpoints

| Method | Path | Guard |
|--------|------|-------|
| `GET`  | `/api/network/traffic-control` | `require_auth` |
| `PUT`  | `/api/network/traffic-control` | `require_admin` — save + apply |
| `POST` | `/api/network/traffic-control/pause` | `require_admin` — `{"paused": true\|false}` |
| `POST` | `/api/network/traffic-control/apply` | `require_admin` — re-apply now |

`GET` returns `{ settings, capabilities, interfaces, state }`. `state` carries
`shaping`, a `reason` sentence for the event log and a **`reason_code`**
(`off`, `paused`, `no_interface`, `all_unlimited`, `outside_schedule`) that the
UI localises instead of echoing English.

## Egress only — the honest limit

HomeVault shapes the **egress** direction on the chosen interface. That is the
direction the appliance controls: an `smbclient get` (a client downloading from
a share) leaves the NAS through this queue, so the acceptance case is covered.
**Ingress** shaping needs an intermediate functional block (IFB) device, and is
not applied. The API says `supports_download_limit: false` for every service,
and the screen repeats it, rather than implying a download limit exists.

## Management traffic is never shaped

`MANAGEMENT_PORTS = [22, 80, 443, 8443]` — SSH/SFTP, the web UI and the per-app
proxy. Their filters are skipped even if a service rule lists them, and the
default class stays generous. An admin must not be able to throttle their own
session out of the appliance.

## What gets built

One HTB queue, replaced (never stacked) on each apply:

```
sudo tc qdisc replace dev <iface> root handle 1: htb default 30
sudo tc class replace dev <iface> parent 1: classid 1:1   htb rate <total>kbit ceil <total>kbit
sudo tc class replace dev <iface> parent 1: classid 1:30  htb rate <total>kbit ceil <total>kbit
# per limited service:
sudo tc class  replace dev <iface> parent 1: classid 1:100 htb rate <rate>kbit ceil <rate>kbit
sudo tc filter replace dev <iface> protocol ip parent 1: prio 1 u32 match ip sport <port> 0xffff flowid 1:100
```

- Class ids start at `100`; the default (unshaped) class is `30`.
- The root class lends its full rate to the children (sum of the limited
  services), so one service can still use the whole link when the others idle.
- `replace` on every object makes a re-apply idempotent.
- Disabling the feature, pausing, or leaving the schedule tears the queue down
  with `tc qdisc del dev <iface> root`; a non-zero exit because no queue exists
  is the normal case and is ignored.
- Every command is an argv vector run directly — never a shell string. The
  interface name is gated by `valid_interface_name` (≤15 chars, `[A-Za-z0-9._:-]`,
  never `lo`), the only free-text field that reaches a root command.

## Schedule arithmetic

A window holds `days` (0 = Monday … 6 = Sunday), `start_minute` and
`end_minute` (minutes after midnight). A window whose end is less than or equal
to its start **wraps over midnight** (`22:00 → 06:00`): it owns the tail of its
own day and the head of the next one, so a Sunday 22:00 → Monday 06:00 window
covers Monday 05:59 and starts the previous Sunday evening.

No window means "always". At most **8** windows; each needs at least one day
and non-equal times. The scheduler evaluates in **UTC** (the same instant the
`run_scheduler` loop polls).

## Scheduler

`web::run` spawns `traffic_control::run_scheduler`, one pass per **60 s**. A
pass reads the saved policy and reapplies only when the *effective* signature
changed (`interface`, shape on/off, each limited rate, window count), so a
schedule boundary flips the queue on its own and an idle pass runs no command.

## Settings and their limits

| Field | Range | Notes |
|-------|-------|-------|
| Interface | valid kernel name, not `lo` | Empty is allowed while the feature is off |
| Services | the five known ids, no duplicates | `rate_kbps = 0` means unlimited |
| Rate | 0 – 10 000 000 kbit/s | Validated against `MAX_RATE_KBPS` |
| Windows | 0 – 8 | Days 0–6, times 00:00–23:59, start ≠ end |
| Pause | boolean | Keeps the policy, removes the queue |

## Failure behaviour

- `tc` missing → apply reports `"The `tc` tool is not installed"` and does not
  pretend it shaped anything.
- A failing command stops the pipeline and returns `applied: false` with the
  exact command and its stderr in `failed`; the screen shows it as an alert.
- The saved policy is validated on `PUT` **and** again on `POST /apply`, so a
  hand-edited JSON file cannot reach `tc` unchecked.

## Related

- **Replication** (`docs/guides/replication.md`) has its own per-job bandwidth
  cap (`pv -L`), which is right for one bulk transfer; traffic control is the
  host-wide, per-protocol policy.
- **Network** (`docs/guides/network.md`) owns the interfaces this screen picks.
