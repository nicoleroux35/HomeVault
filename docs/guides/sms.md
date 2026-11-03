# SMS alerts

**Purpose:** deliver one short message per alert to a phone, on the same events
and the same severity threshold as the email and the webhook. Useful when the
appliance is headless and an email sits unread in an inbox nobody checks at
night.

**Who can use it:** reading and saving the channel are **admin-only** — it holds
a credential and drives an outbound request.

## Where it lives

- **Screen:** Settings → Notifications → **Send an SMS** →
  `frontend/src/routes/Settings.svelte` (the notifications category).
- **Backend:** `src/sms.rs` (recipient validation, message composition,
  truncation, body templating, header building, request building — all pure)
  and `notification_engine::dispatch_sms` (the only part that performs a POST).
- **Storage:** `{DATA_DIRECTORY}/data/settings.json`, in the `notifications.sms`
  block. No migration: a file written before SMS existed simply has no key.
- **Dependencies:** none. The channel is a plain HTTPS POST.

## Endpoints

No dedicated route. The channel rides the existing notifications settings:

| Method | Path | Guard |
|--------|------|-------|
| `GET` | `/api/settings/notifications` | `require_admin` — answers `sms` as a **redacted view** |
| `PUT` | `/api/settings/notifications` | `require_admin` — validates, seals, persists |

## Provider-agnostic by design

HomeVault never embeds a vendor API. The admin supplies:

| Field | Meaning |
|---|---|
| `endpoint` | The URL to POST to |
| `body_template` | What is sent, with `%to%` and `%text%` replaced |
| `recipient` | The number, E.164 (`+33612345678`) |
| `extra_headers` | Optional additional headers |
| bearer token | Sent as `Authorization: Bearer …` when set |

Twilio, OVH, Vonage, a self-hosted gateway and a home-made relay all accept a
POST with the recipient and the text in it somewhere, so a vendor SDK is not
needed — and a vendor changing its API cannot break the appliance. The default
template is `{"to":"%to%","text":"%text%"}`, so filling in only the URL and the
number produces a working channel.

## Rules

- **The number must be in international form.** A leading `+`, then 8–15 digits
  and nothing else — E.164's own bound. That refuses the shapes that look
  plausible but are never deliverable: `0612345678` (no country code),
  `+33 6 12 34 56 78` (separators a gateway would have to guess at),
  `+33-6-12-34-56-78` (punctuation). The refusal **names the expected shape**
  rather than only rejecting.
- **A disabled configuration is never refused.** An admin clearing a
  half-filled form must not be told their empty number is invalid.
- **The token is sealed and never returned.** It arrives in clear on the wire,
  is sealed with the appliance key under the `sms-token` scope before it is
  written, and the read path answers `token_set: bool` instead. Verified live:
  `settings.json` holds `hv1:…`, and the plaintext appears nowhere.
- **A blank token keeps the stored one.** The read path never shows the
  credential, so it could not be echoed back; treating a blank value as
  "unchanged" is what lets an admin edit the number without re-typing it.
- **An unopenable token is a refusal, not a silent 401.** If the appliance key
  changed (a data directory restored on another machine), the dispatch logs
  which setting to re-enter and does not send.
- **A failure never propagates.** An unreachable gateway is logged and the
  email/webhook channels still run — and the health/backup scheduler tick is
  never blocked. The POST reuses `webhook_client()`, which carries a 10 s
  timeout and a 5 s connect timeout.
- **The alert fits one segment.** `[severity] Title: message`, whitespace
  collapsed, truncated to 160 **characters** (not bytes, so an accented message
  is never split) with a visible `…` marker. A silent cut would leave the reader
  unsure whether the text ended or was truncated.

## Status codes

| Situation | Status |
|---|---|
| Valid configuration | `200` |
| Number not in international form, endpoint missing while enabled, template missing `%to%`/`%text%` | `400`, naming the field and the rule |
| Disabled with empty fields | `200` — always allowed |
| No session / not an admin | `401` / `403` |

## Tests

| Suite | What it covers |
|-------|----------------|
| `src/sms.rs` (`#[cfg(test)]`) | 19 unit — E.164 accept/refuse matrix, the refusal naming the shape, disabled-is-allowed, endpoint/template requirements, token never in the view, `Authorization` built only from the token, whitespace collapse, truncation with marker and without splitting a multi-byte character, body templating, request building, refusal before any network call |
| `tests/sms_integration.rs` | 10 — auth matrix, the four validation refusals through HTTP, disabled-with-empty-fields accepted, token sealed and absent from the read, blank token keeps the stored one, the composed message shape |
| `frontend/src/routes/Settings.svelte` | covered by `npm run check` |

**Not covered by an automated test:** an actual delivery to a gateway. That
would make the test suite depend on an external service; the exact request
(endpoint, body, headers) is asserted by `build_request` instead.

## Related

- [Notifications](./notifications.md) — the persistent centre and the other channels
- [Settings](./settings.md) — the card the fields live in
- [Health monitoring](./health.md) — the events a message is raised for
