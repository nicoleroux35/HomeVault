# Development workflow (multi-instance, VM tests, client contract)

**Purpose:** the tooling a developer needs beyond `cargo run` — running several
HomeVault instances side by side, testing against a real host, and keeping the
client/server contract in sync. Developer-only: it documents the checkout, not
a feature an end user opens.

## Multiple instances on one machine (`.local` names)

`scripts/dev-instance.sh` starts an isolated instance with its own data
directory, port and mDNS name, so several branches/agents can run at once
without colliding:

```bash
scripts/dev-instance.sh            # name/port derived from the git branch
scripts/dev-instance.sh dev-1 8081 # explicit name + port
```

It sets:

| Variable | Purpose |
|----------|---------|
| `PORT` | the instance's TCP port (derived from the name when omitted) |
| `DATA_DIRECTORY` | `~/.local/share/homevault-dev/<name>` — isolated DB/uploads/logs |
| `HV_MDNS_HOSTNAME` | the `.local` name (`<name>.local` instead of `homevault.local`) |
| `LOG_LEVEL` | defaults to `debug` |

`HV_MDNS_HOSTNAME` is read by `crate::mdns::mdns_hostname()`
(`src/mdns.rs`); it overrides the default `homevault` and is also used for the
TLS certificate SANs. An override Avahi would reject is ignored, so a bad value
falls back to `homevault.local` rather than breaking mDNS.

## Real-VM tests (`*.vm.test.ts`)

The default Playwright suite runs against a scratch server and never touches
hardware. Tests that need a **real host** (real disks/SMART, real systemd, real
mDNS, a real reboot) live in `tests/vm/` and match `*.vm.test.ts`. They are run
with a separate config and are never picked up by `npx playwright test`:

```bash
HV_VM_BASE_URL=http://hv-test.local:8080 \
HV_VM_ADMIN_USER=admin HV_VM_ADMIN_PASS=secret \
HV_VM_HOSTNAME=hv-test \
npx playwright test --config=playwright.vm.config.ts
```

Without `HV_VM_BASE_URL` every VM spec skips itself with a message, so the
suite can sit in the repo (and in an optional CI job) without failing on a
machine that has no VM. `HV_VM_HOSTNAME` is only used by the mDNS assertion.

## Versioned client/server contract

The SPA and backend ship together, but a cached bundle can outlive an update.
The contract makes that fail loudly:

- `CONTRACT_VERSION` lives in **both** `src/api_contract.rs` and
  `frontend/src/lib/services/api.ts` — bump both, and only on a **breaking**
  API change.
- The SPA sends `X-HomeVault-Contract` on every request; the server answers
  `426 Upgrade Required` on a mismatch (`middleware::contract`), and the shell
  shows a reload banner.
- `GET /api/meta` is the unauthenticated discovery endpoint.

## Related

- [Architecture](../ARCHITECTURE.md) — build/run, request flow, the contract.
- [Backend Structure](../BACKEND.md), [Frontend Structure](../FRONTEND.md).
- `CLAUDE.md` — the testing rules every change must satisfy.
