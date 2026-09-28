# Firewall (ufw management)

**Purpose:** Manage the host firewall via `ufw` (Debian/Ubuntu only,
matching `installer/install.sh`'s existing OS check) — view status, add/
remove rules, enable/disable — with a hard, server-enforced guard against
locking yourself out of a remote NAS. Lives in the **Settings** page as
the Firewall card.

## Backend

Migration `sqlx/migrations/020_firewall.sql` adds two tables:

- `firewall_settings` — singleton row (`id = 1`): `enabled` (a
  best-effort cache; live `ufw status` is always the source of truth for
  reads), `ssh_port` (default 22), `app_port` (default 8080),
  `confirmed_at`.
- `firewall_change_log` — genuine audit trail (not a rules mirror) of
  changes made through HomeVault: `id`, `action`
  (`enabled`/`disabled`/`rule_added`/`rule_deleted`), `rule_description`,
  `performed_by`, `created_at`.

There is **no** `firewall_rules` table — rules are never mirrored in the
database. `ufw show added`/`ufw status` on the live system is always the
source of truth, mirroring how `src/vpn.rs` reads `wg show` live rather
than caching WireGuard state. This avoids drift between what HomeVault
displays and what the kernel is actually enforcing.

Storage layer `src/storage/firewall_repo.rs` (`FirewallRepository`).

Core logic `src/firewall.rs`:

- `ufw_available()` — checked via `sudo ufw --version`, **not** a bare
  `Command::new("ufw")`. `ufw` lives in `/usr/sbin`, commonly absent from
  a non-root service account's `PATH` even though `sudo`'s own
  `secure_path` (used by every real call below) always includes it — a
  bare-PATH check was confirmed live to report "not installed" on a
  correctly-provisioned real server.
- `get_status()` — returns `(enabled, Vec<FirewallRule>)`. `enabled`
  comes from `sudo ufw status`. The rule list comes from `sudo ufw show
  added`, **not** `ufw status numbered` — confirmed live that `ufw
  status` only lists rules while ufw is *active*, so a rule added before
  ever enabling the firewall (the natural first-time flow) would
  otherwise appear to not exist.
- `parse_added_rule_line` — parses both the simple (`ufw allow 22/tcp
  comment '...'`) and source-restricted (`ufw allow from <ip> to any port
  <port> proto <proto> comment '...'`) forms `ufw show added` emits.
  `is_mandatory` is derived purely by matching the rule's own **comment
  string** against `MANDATORY_SSH_COMMENT`/`MANDATORY_APP_COMMENT` —
  never from client input.
- `add_rule`/`delete_rule_by_spec` — `delete_rule_by_spec` deletes by
  exact spec (action/port/protocol), not numeric index, since indices
  shift between calls and a stale index race could delete the wrong rule.
- `ensure_mandatory_rules(ssh_port, app_port)` — idempotently asserts the
  two rules that must always exist. Called unconditionally at server
  startup (if `ufw_available()`) and as the first statement in the
  enable-confirm handler, both using server-detected ports
  (`detect_ssh_port()` parses `/etc/ssh/sshd_config`, default 22;
  `detect_app_port()` reuses `crate::config::Settings::load().port`,
  the exact port the server itself binds to) — never a client-supplied
  value.

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /firewall/status` | auth | `available`, `enabled`, live rule list |
| `GET /firewall/enable/preview` | auth | ports + rules that would become active |
| `POST /firewall/enable` | admin | **only** with `{"confirmed": true}`; re-asserts mandatory rules first |
| `POST /firewall/disable` | admin | |
| `POST /firewall/rules` | admin | add a rule |
| `DELETE /firewall/rules` | admin | remove a rule by spec; 403 if it's mandatory |

All handlers in `src/handlers/firewall.rs`. Read endpoints use
`require_auth`; mutations use `require_admin`.

### Self-lockout safety (enforced server-side)

- No generic "set enabled=true" endpoint exists — enabling only happens
  through `handle_firewall_enable`, which requires an explicit
  `{"confirmed": true}` body and always re-asserts the mandatory rules
  first, every time.
- `handle_firewall_rule_delete` re-fetches live status and checks the
  target rule's server-derived `is_mandatory` flag, returning 403 before
  ever calling `delete_rule_by_spec` — a raw `curl DELETE` targeting the
  SSH or app-port rule is rejected exactly like a UI click would be, both
  while ufw is active and inactive (verified live in both states).

## Frontend

- `frontend/src/lib/components/settings/FirewallCard.svelte` — rule
  table (mandatory rows render with a "required" badge and no delete
  button at all, not merely a disabled one), add-rule form, and an
  enable confirmation modal that fetches `/firewall/enable/preview` and
  requires an explicit checkbox acknowledgment before calling
  `enableFirewall()`.
- `frontend/src/lib/services/api.ts`: `getFirewallStatus`,
  `getFirewallEnablePreview`, `enableFirewall`, `disableFirewall`,
  `addFirewallRule`, `deleteFirewallRule`. Type `FirewallRule`.
- Wired into `frontend/src/routes/Settings.svelte` as `<FirewallCard />`,
  after the "Privacy & Data" card.

## i18n

All strings under `[firewall]` in `src/i18n/{en,fr,de,es}.toml`, including
the confirm-modal warning and `confirm_ssh_rule`/`confirm_app_rule`
(`%port%` interpolated).

## Sudoers & installer

`ufw` is invoked as `sudo ufw …` via the `ufw_command()` helper (which also
forces `LC_ALL=C` + `LANGUAGE=""` so status parsing is locale-independent).
Since 2026-09-04 the process runs as **root** and no sudoers file is generated
(`configure_sudoers()` is dead code — see
[Architecture § Deployment model](../ARCHITECTURE.md#deployment-model-runs-as-root));
the `sudo` wrappers stay for a possible future non-root mode.
`installer/install.sh` installs `ufw` via the
existing `install_optional` helper — non-fatal if declined;
`ufw_available()` degrades the UI to "not installed" rather than
erroring.

## Invariants

- Covers `ufw` on Debian/Ubuntu only, matching the installer's existing
  OS check. `ufw_available()` false on any other distro/OS just shows
  "not installed", no crash.
- `ufw_available()` must keep checking via `sudo`, and `get_status()`
  must keep reading `ufw show added` for the rule list — see the Backend
  section above for the two live-confirmed bugs this prevents from
  regressing.
- Never weaken the mandatory-rule/confirm-required enforcement described
  above; it's the specific mitigation for a real host-firewall self-
  lockout risk on a remotely-administered NAS.
- `dependencies.rs` models `ufw` as **presence-only** (`check_running: None`,
  2026-09-15): its "active" flag is the firewall state the admin controls from
  this app, not a daemon that must be up. Giving it a `running` check made
  every startup warn "installed but its service isn't running" on the intended
  default (firewall off). The regression is pinned by
  `dependencies.rs::tests::test_ufw_is_presence_only_not_a_running_daemon`.

## Verified

Audited live end-to-end on 2026-07-16, including — with the user's
explicit sign-off, since it's a live host-firewall change — a real `ufw`
enable and disable cycle (`sudo ufw --force enable` / `disable`) with the
sandbox remaining reachable throughout. See the "Security suite" section
in `CLAUDE.md` for the full bug list and verification narrative: real
rule add/delete, mandatory-rule deletion rejected via raw `curl` both
while active and inactive, enable-without-confirmation rejected, and the
mandatory SSH/app-port rules confirmed present via `sudo ufw show
added`/`status verbose` both before and after a real enable.

## Related

- [Settings](./settings.md), [Antivirus](./antivirus.md),
  [Anti-Ransomware](./ransomware.md), [VPN](./vpn.md), [Network](./network.md)
- Backend: `src/firewall.rs`, `src/storage/firewall_repo.rs`,
  `src/handlers/firewall.rs`
