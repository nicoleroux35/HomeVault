# Web Push notifications

HomeVault can send its notifications to a phone, tablet or computer even when
no dashboard tab is open. The service worker (`frontend/public/sw.js`) already
had the receiving half; this feature is the missing backend: a VAPID key pair,
per-device subscriptions, RFC 8291 encryption and a sender in
`src/notification_engine.rs`.

Settings → Notifications → **Push notifications**
(`GET /api/notifications/push`, `POST|DELETE /api/notifications/push/subscribe`).

---

## Requirements and consent

Push needs a **secure context**: HTTPS, or `localhost` during development. On a
plain-HTTP LAN address the browser refuses to register a service worker, and the
card says so instead of showing a dead button. iOS/iPadOS also requires the PWA
to be installed on the home screen.

Consent is explicit and per device:

1. the user presses **Enable on this device**;
2. the browser asks its own notification permission;
3. the browser creates a `PushSubscription` and the page sends it to HomeVault;
4. only then does a row exist in `push_subscriptions`.

Refusing the browser prompt stores nothing. Removing a device revokes it for
that account. Deleting the account removes its devices through the foreign key.

## What is sent, and what the push service sees

The notification title, its message (truncated to 600 bytes), a link to the
dashboard, the severity and the notification id. The payload is encrypted
**separately for every device** with RFC 8291 `aes128gcm`; the push service
stores an opaque body it cannot read. It does see the endpoint, the timing and
the approximate size — that metadata is inherent to Web Push and is documented
here rather than hidden.

HomeVault never sends file contents, paths, credentials or session tokens.
Event opt-outs (`/notifications/events/config`) and the severity threshold apply
to push exactly as they do to email, SMS and the webhook: a muted event or a
notification below the threshold is never pushed.

## Keys

- A VAPID P-256 key pair is generated once, at first use.
- The private key is sealed with the appliance key (`secret_box`, `hv1:` blob)
  and the file (`{DATA_DIRECTORY}/data/web_push.json`) is `0600`.
- Only the public key is returned by the API; it is safe to be public.
- Deleting `web_push.json` generates a new pair: every existing subscription
  stops working and each device must be enabled again. That is the deliberate
  reset path.

## Delivery semantics

- **Best effort.** There is no retry queue: a notification that fails is gone,
  like an email the MTA refused. The next notification is independent.
- **TTL 24 h** (`TTL: 86400`): the push service may hold an undelivered
  message for a day.
- **Expired devices are removed.** A `404`/`410` from the push service deletes
  the subscription immediately; ten consecutive other failures also remove it.
  A `401`/`403` is treated as a VAPID configuration problem — the device is
  kept and no failure is counted, because deleting every device over a wrong key
  would be worse.
- **One push per device per notification.** Up to 10 devices per account.
- The sender runs in its own task, so a slow push service never delays the
  notification scheduler.

## Security

- A subscription endpoint must be **HTTPS on port 443**, with no credentials or
  fragment, and must resolve to a **public** address. This stops a signed-in
  user from turning the sender into an SSRF probe against the LAN.
- Redirects are not followed.
- The `p256dh` key must be a 65-byte uncompressed P-256 point and the auth
  secret exactly 16 bytes; anything else is refused before storage.
- Every encryption primitive is `ring` (ECDH P-256, HKDF-SHA256, AES-128-GCM,
  ECDSA P-256); the key schedule and record format are pinned by the RFC 8291
  Appendix A vector in `src/web_push/crypto.rs`.
- The three subscription secrets (endpoint, `p256dh`, `auth`) are never
  returned by the API; the screen shows the endpoint host only.

## Failure answers

| Situation | What happens |
|---|---|
| No `HTTPS` / no Push API | Card explains the requirement; nothing is shown to click |
| Browser permission denied | A sentence tells the user to allow it in site settings |
| Endpoint not HTTPS/443/public | `400`; nothing stored |
| Device limit reached | `400` asking to remove one first |
| VAPID keys unusable | `GET` answers `available: false` with the reason |
| Push service says `404`/`410` | Device removed and the screen updates on next load |
| Push service unreachable | Delivery fails; the notification is already in the bell |

## Tests and verification

| Level | File |
|---|---|
| Unit | `src/web_push/crypto.rs` — RFC 8291 Appendix A key schedule, ciphertext and header/body; VAPID JWT verified with the minted public key; base64 and key-length refusals |
| Unit | `src/web_push/mod.rs` — endpoint syntax, public-address policy, payload bounds/UTF-8 truncation, endpoint host redaction |
| Integration | `tests/web_push_integration.rs` — `401` matrix, VAPID key surface, every subscription refusal before a connection, per-user isolation, re-subscribe upsert, account-deletion cascade |
| E2E | `tests/e2e/settings/web-push.spec.ts` — unavailable server, insecure context, empty state, scripted enable flow, denied permission, named remove, responsive |

**Never exercised live:** a real push service delivery (FCM/Mozilla/Apple). The
suite has no public push endpoint and the payload is pinned by the RFC vector;
a manual round trip on a phone with the PWA installed is the remaining check.
