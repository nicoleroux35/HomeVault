# Reachability advisor

**Purpose:** answer one question before an admin opens a port — *will a browser
outside my house actually reach this box, with a certificate it trusts?* The
answer is a short, ordered list of concrete conditions and their fixes, not a
score. Lives in **Settings > Security > Reachability**.

## Overview

`src/reachability.rs` builds the report; `src/handlers/reachability.rs` serves it.
The assessment is a **pure function of its inputs** — the TLS, automation and
dynamic-DNS settings, the port, and what can be read from the installed
certificate — so it is deterministic and has no external probing.

Ordering is the point. "Your certificate is self-signed" only matters once HTTPS
is on, and "port 80 is closed" only matters for the HTTP-01 challenge. A flat
list would give the wrong advice first.

## The checks

| Id | State when | Fix shown |
|---|---|---|
| `tls.enabled` | `fail` when HTTPS is off | enable HTTPS/TLS |
| `tls.trusted_certificate` | `warn` self-signed, `warn` ≤ 30 days, `fail` expired, `warn` unreadable | automate, renew, regenerate |
| `acme.automation` | `fail` on an error (with the authority's reason), `warn` pending / off / never issued | fix the reason, create the record, enable it |
| `ddns.enabled` | `fail` on an update error, `warn` stale or never sent | check the token, send an update |
| `names.agree` | `warn` when the certificate name and the public name differ | use the same name for both |
| `ports.https` | `warn` when HTTPS is not on 443 | forward 443, or run on 443 |
| `ports.http01` | `ok` on 80, `warn` otherwise — and only for `http-01` | forward port 80, or use DNS-01 |
| `tls.mode` | `warn` when a certificate is automated but HTTPS is off | turn HTTPS on |

`worst()` folds the list down to one state for the summary line, and `counts()`
gives the ok / warn / fail totals.

## Report shape

```json
{
  "checks": [
    { "id": "tls.enabled", "state": "fail",
      "detail": "HTTPS is off, so the login form travels in clear text over the network",
      "fix": "enable HTTPS/TLS in Settings → Security" }
  ],
  "state": "fail",
  "counts": { "ok": 0, "warn": 1, "fail": 1 }
}
```

`id` is stable API: the card keys its hints off it, so renaming one is a breaking
change for the UI.

## Endpoint

| Method | Path | Guard |
|---|---|---|
| `GET` | `/api/settings/reachability` | admin |

Read-only. It re-reads the settings each time, so the report follows a change made
on another card without a reload of the server.

## Tests

- Unit (`src/reachability.rs`): HTTPS off is the first failure and the worst
  state; a self-signed certificate warns rather than fails; an expired one fails;
  a certificate inside the renewal window warns with the day count; an automation
  error carries the authority's message; automation off warns; a stale dynamic-DNS
  address reports both addresses; an up-to-date one is ok; a name mismatch warns;
  HTTP-01 on a non-standard port is called out and DNS-01 is not; a non-standard
  HTTPS port warns; counts and `worst` track the entries.
- Integration (`tests/ddns_integration.rs`): the report over HTTP — 401 / 403, the
  first check is `tls.enabled`, it is a `fail` with a fix, and the counts add up
  to the number of checks.
- E2E (`tests/e2e/settings/ddns.spec.ts`, section 5): the rendered checklist
  starts with HTTPS and the displayed counts add up to the number of rows —
  the screen agrees with the API rather than re-deriving the order in the UI.

## Related

- [HTTPS / TLS](./tls.md), [Certificate automation](./acme.md),
  [Dynamic DNS](./ddns.md), [Firewall](./firewall.md)
