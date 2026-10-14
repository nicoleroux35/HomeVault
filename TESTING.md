# HomeVault — Test tracking

Cross-references the 27 screens listed in `SCREENS.md` with three distinct
levels of testing, to answer "what is really tested, what remains to be
tested":

1. **Manual live audit** — real backend, real session, often
   Playwright driven by hand. Documented in dated prose/changelog in
   `CLAUDE.md` and `docs/guides/` (each documented invariant = one audit of
   this type). It is a
   one-off audit, not a guarantee against future regression.
2. **Automated Rust tests** — unit (`#[cfg(test)]` in the source
   file, pure logic) and integration (`tests/*.rs`, in-memory SQLite DB +
   `tower::ServiceExt::oneshot`).
3. **Playwright E2E tests** — `tests/e2e/<screen>/<screen>.spec.ts`, against
   a real server, no mocks.

Only levels 2 and 3 protect against future regression; level 1 is a snapshot
at a given date. Compiled on 2026-09-05, updated
on 2026-09-06 (addition of the **Network**, **VPN**,
**Shares/Protocols**, **Security suite**, **Downloader**, **Storage**,
**Backup**, **Cloud Sync**, **Syncthing**, **Chat**, **Explorer**
suites and the **Terminal** Rust tests), then on **2026-09-07** (filling the
gaps in the already partially tested screens: new
`tests/raid_integration.rs` (auth guards); `storage_integration.rs`
extended to the `virtual_disk_*` handlers; `explorer_integration.rs` extended
to `/roots` `/save` `/eject` `/snapshots` favorites and `?strategy=`; unit
tests `transfer_quota.rs::check_quota`, `handlers/storage.rs::vd_name`,
`browser.rs` collision-strategy, `handlers/explorer.rs`
`kind_for_root`/`resolve_dest`), by directly reading `CLAUDE.md`,
`tests/*.rs` and `tests/e2e/**` (real counting of the files and of the number
of `test(...)`/`#[test]`, not an estimate).

**2026-09-07 (real "host conditions" test — 4 areas, 10 bugs
fixed).** On explicit request, the 4 areas whose automated tests
stopped at "clean rejection" for lack of `sudo`/hardware access
— **Firewall (ufw)**, **Shares/SFTP**, **RAID/Storage (virtual disks)**
and **System journal** — were exercised end to end against the real
host (temporary NOPASSWD `sudoers` drop-in, 6 loop devices, a real
`ufw enable`/`disable` cycle with port 22 protected, a real
`homevault-sftp` `sshd`). **10 bugs found and fixed in frozen files**
(explicit authorization), plus the associated data-quality
fixes in `PoolService::scan_pools` (device_count / mount
point / mdadm size / ZFS mount point) — see the section
"## Live real test 2026-09-07" at the end of the file. Each fix was
re-verified live on the rebuilt binary.

**2026-09-07 (2nd pass "implement what remains" — same NOPASSWD
drop-in).** **de/es** i18n fixed (accents, values only);
**Rescue** exercised live for the 1st time (6 frozen bugs); **Storage/
LUKS** complete cycle live (2 frozen bugs); **Network**: reads
re-verified + application path `src/network.rs` fixed (`sudo` on
the `ip`/`dhclient` mutations, `resolv.conf` via `sudo tee` without clobber)
and verified live on a disposable `dummy` interface. Detail in the section
"## 2026-09-07 (continued)" and the dated addenda of `CLAUDE.md`. Tests:
rescue unit 8/8, rescue_integration 28/28, storage_integration 24/24,
network unit +3/3, network_integration 23/23.

**2026-09-13 (transfers v2).** Hardening of the Explorer transfer
methods and of share links: HTTP `Range` (206) + `X-Content-Sha256`
digest on downloads, short-lived tickets (single-use download,
multi-use thumbnail per directory) replacing the `?token=` in the
URLs, resumable chunked upload (session → chunks → complete), quotas
locked per-subject (reservation + refund, anti-race), canonical
`PathGuard` path in all handlers (TOCTOU symlink), `rename`
validates the destination, `/explorer/delete` admin-only, trash via
`copy_file_verified`, directory `fsync` after each atomic rename,
cleanup of the partial destination of a failed folder copy/move,
per-token rate-limit on `/share/{token}` + token without modulo bias. Detail
in `docs/guides/explorer.md` and `docs/guides/transfer-quotas.md`. Tests:
`explorer_integration.rs` 49/49, `share_links_integration.rs` 12/12,
unit `media_ticket.rs` 5/5, `handlers/explorer.rs` 17/17,
`browser.rs` 11/11, lib 592/592, `svelte-check` 0 errors. Explorer E2E to
re-run on a fresh server.

**2026-09-14 (desktop chrome — fidelity to the approved gateway mockup).** Dock and
topbar realigned to the reference mockup: each pill now has
**its own** height token (`--hv-chrome-dock-h: 84px` unlocked from
`--hv-chrome-topbar-h: 62px`), dedicated chrome material
(`--hv-chrome-surface-border` / `-shadow`, shared by the two pills and
their panels), dock tiles at 48px with a hover bounce and tooltip
(`.hv-dock-item::before`, on `--hv-tooltip-bg`), app gradients re-harmonized
on the Tailwind 500→700 scale with `--hv-app-<id>-c` = dark stop of the gradient.
No behavior change. Tests: `desktop.spec.ts` goes to
**51 tests / 14 categories** — 8.3 rewritten (the two heights are verified
separately, and "the dock is the tallest" only holds above the phone width),
new category 14 "Chrome geometry & material" (JS mirror `snapUtils.ts`,
badge = dark stop, opaque pills, tooltip). 1.5 finally covers
`dock-terminal`, which was rendered but never asserted. Count verified via
`npx playwright test --list --project=firefox` — the `grep -c 'test('`
method documented below over-counted (it caught the `.test(x)` of the
assertions); it is fixed to `grep -cE '^\s*test\('`.

**2026-09-14 (topbar buttons — icon + label chips).** The topbar adopts the
layout of the approved mockup (node `1:24`): the 4 buttons go from icon
only to **icon + label** from 901px, icon only below that (the target
stays 44×44 in both cases — only the width changes). Mockup order:
Network, Widgets, Notifications, Power. A decision formerly refused
(`docs/guides/desktop.md`) on three grounds, two of which were outdated: no
new i18n key is needed (the 4 labels reuse existing keys,
including `desktop.power`, until now orphaned — the only i18n edit: `es.power`
goes from "Apagar" to "Energía" to align with fr/de), and the 44px comes from
the mockup itself. **No token or keep-out moves** — the delta is
purely horizontal, the pill height (62px) still carried by the 48px user
chip. The notification badge is re-scaled to **16px**
anchored to the button (it previously shared a rule with `.hv-dock-badge`,
which keeps its 26px for its 48px tile). `desktop.spec.ts` goes from
**51 tests / 14 categories to 58 / 15**: category 15 "Topbar icon + label
chips" (7 tests — chip geometry, fallback ≤900px without a second markup
branch, DOM order, 16px badge vs dock 26px + non-overlap of the label,
WCAG 2.5.3, 901px in German), helper `topbarBtn`, and the 3 lines 7.1-7.3 that
targeted power via `.hv-topbar-btn').last()` rewritten as
`[data-testid="topbar-power"]`. `8.3`, `14.1`, `14.3` and `14.4` stay green
**without modification** — that is the proof of zero vertical ripple. Count
verified via `npx playwright test --list --project=firefox` (58) and
`grep -c '^test.describe('` (15).

**2026-09-14 (app window chrome — "AppWindow" mockup).** The app window is
realigned to the Figma mockup "Untitled" (node `3:76`),
measured to the pixel on its 2× export. The frame becomes **opaque
paper** (`--hv-window-bg` #FFFFFF, no more transparency), in a **slate-300**
hairline `--hv-window-border` (#CBD5E1 — deliberately darker than
`--hv-shell-border`, otherwise the frame dissolves into the wallpaper) and
the shadow specific to the mockup (`--hv-window-shadow`, two layers). The
header bar goes to a **constant height of 44px** (`--hv-winhdr-h` =
8 + 28 + 8), the icon tile to **24px / radius 8** (`--hv-winhdr-icon-*`,
app gradient kept), the title to **14px / 600 on 20px** in `#314158`
(`--hv-winhdr-title`). The min/max/close trio leaves its floating 48px
position for the bar itself: **28px**, radius 8, gap 6, at the exact stops
of the mockup `--hv-winctrl-min` #FFB900 / `--hv-winctrl-max` #2B7FFF /
`--hv-winctrl-close` #FF6467 — values **deliberately more vivid** than
the `--hv-warning`/`--hv-error` semantics, not to be "harmonized". The
glyphs are those of the mockup (14×14, `stroke-width: 1.458`, the value of the
SVG). Corollary: `WindowHeader` reserves `--hv-winhdr-btn-clear` **120px**
(3×28 + 2×6 + 12 inset + 12 gap; 108px at ≤480px, where the bar wraps),
and each button carries a transparent **34px** `::after` — the
maximum the 6px gap allows without two neighboring targets
overlapping — to stay usable by touch despite the 28px of the mockup.
No `data-testid` moves, so categories 3 and 15 stay green as
they are. `desktop.spec.ts` goes from **58 tests / 15 categories to 63 / 15**:
category 14 completed by 5 fidelity-to-the-mockup tests (14.5 opaque surface
+ slate-300 hairline + two-layer shadow + `backdrop-filter:none`;
14.6 44px bar, 24px/radius 8/gradient tile, 14px/600/#314158 title on
20px; 14.7 28×28/radius 8/gap 6 trio with measured inter-button gap and
12px inset in the bar — not overlaid; 14.8 the reserve really protects
the title, **measured** at 1440/768/400px rather than read in the
tokens, because the trio is absolutely positioned: the right padding of the
bar is the only thing preventing it from going underneath; 14.9 **a single
header per window**, on a sample of the registry). Three method points:
14.6 asserts the `min-height` of 44px and a render **≥** 44, not 44
hardcoded — **all** the shipped apps pass a `subtitle`, so their bar
is taller than the mockup (which is title-only); the bottom rule is
measured on `.hv-winhdr-wrap`, **not** on `.hv-winhdr` — the rule is carried
by the container (which also includes the optional tab row), so reading
`borderBottomColor` of the bar returns the inherited `currentColor` (the ink
`#1F2733`) and can never match — the first version of 14.6 failed
exactly on that; and 14.9 was written
**after finding a real bug**: `quotas`, added on 2026-09-14, was missing
from the `OWN_HEADER_APPS` `Set` of `WindowFrame.svelte`, so the Quotas
window stacked **two** bars (its own + the WindowFrame fallback, whose
sprite name has no glyph → empty bar). Fixed by adding
`'quotas'` to the `Set` + a comment that points to 14.9. It is the only app of the
registry that was missing; no `data-testid` or token is concerned. Count
verified via `npx playwright test --list --project=firefox` (63) and
`grep -cE '^\s*test\.describe\('` (15).

**2026-09-14 (standardization of window headers + dock tile gap).**
`WindowHeader` is now **icon + title only**: the `subtitle` prop is removed
from `WindowHeader.svelte` and from `PageLayout.svelte`, and
the 22 routes that passed one no longer do. The icon tile is worth
`--hv-winhdr-icon-size` = `--hv-winctrl-size` (28px, the size of the min/max/close
trio) and the three are centered on the same line; `.hv-dock-container`
goes to `--hv-chrome-dock-gap` (10px) because on hover a tile grows by
4.3px on each side and the old `gap` of 4px made them overlap.
Nine specs were realigned (desktop, account, health, journal, rescue,
dashboard, users, network, groups, shares): the subtitle
assertions become `toHaveCount(0)`, the removed decorative badges/dots are
removed from the expectations, one test removed with its object (account 1.3), one added
(desktop 14.10) and one rewritten (rescue 1.3, availability moves from the
header band to the tool's card). **None of these specs was re-run and nothing
was compiled** — the session instruction explicitly forbade it; the
per-screen counts of the next section were therefore produced by
`grep -cE '^\s*test\('` (the file's method) and not by a run. The lines
that announced an outdated number (account 76→72, groups 61→60, dashboard
39→37, desktop 63→64) are fixed.

**2026-09-16 — Terminal: the tab strip leaves the window header.**
`terminal-tabs` was rendered in the `center` snippet of `WindowHeader` — so
in the title bar, competing with the min/max/close trio pinned to its
right edge. It is now a **window content bar**, direct child
of the `terminal-page` container, between the header and `terminal-infobar`:
`WindowHeader` → `terminal-tabs` → `terminal-infobar` → xterm body. Immediate
corollary: the Terminal header passed **no** `title` (the tabs occupied the
space), it would no longer display anything but its icon tile — it therefore receives
`title={$t('terminal.title')}` (key already present in the 4 locales).
Geometry measured live: header 44px, strip 44px, infobar 32px, the strip
not being a descendant of `.hv-winhdr`. New test **2.8**, and four
tests repaired — **already red before this change**, selectors outdated since
the Tailwind-on-tokens migration of 2026-09-12: 3.2 (`page.locator('hv-tab-active')`,
a *tag* selector — it was missing the dot of `.hv-tab-active`) and 3.4 /
7.3 / 7.4 (they expected the semantic classes `hv-tab-dot` / `hv-info-badge`,
removed in favor of tokens; they now assert the **computed fill**, which is the real
state marker). Verified by a real run (Firefox,
fresh disposable instance on a scratch port, dedicated `BASE_URL`, no real
data): sections 1, 2, 3, 4, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18
green, `svelte-check` at 0 errors. Four failures remain, **all proven
pre-existing** by `git stash push -- frontend/src/routes/Terminal.svelte`
(the file restored to its state before, they fail identically):
**4.2** (monospace family of the xterm body — never touched here), **10.3**
("< 80 network requests": artifact of the Vite dev server, which serves
non-bundled ES modules — the assertion is about a production bundle),
**19.1 / 19.2** (completion popup, untouched area). The global run of the 79
tests in one pass was **discarded**: it was killed by the system for low memory,
and an HMR reload triggered by an ongoing edit had disturbed it — the retained
verification is therefore section by section, on frozen code.

**2026-09-17 — Terminal: no more vertical scroller in the strip, no more
black band under the terminal.** Two distinct display defects, both
measured in the DOM before the fix (no deduction):

1. **Vertical scroller in `terminal-tabs`.** `overflow-x: auto` alone computes
   `overflow-y` as `auto` as well; the `min-h-11` (44px) minus the bottom 1px
   border leaves the content box 1px too short, which is enough to make a
   vertical scrollbar appear. Reading: `clientHeight 43` /
   `scrollHeight 44`, `overflowY: "auto"`. What revealed it is the Tailwind
   migration itself: before it, `.hv-terminal-tabs` carried
   `scrollbar-width: none` (which hid both bars) — a declaration with no
   Tailwind equivalent, therefore lost. Fixed with `overflow-y-hidden`, which
   removes the axis at the source rather than hiding bars: the horizontal
   bar remains available when the tabs actually overflow.
2. **Black band at the bottom of the window.** `terminal-body` had stayed in
   `#1e1e1e` while the xterm theme had gone to opaque white
   (`theme.background: '#FFFFFF'`): `FitAddon` sizes the terminal in
   `floor(height / cellHeight)` lines, so there is always a remainder below
   one cell, which painted the dark background of the container
   under the last line. Reading: `terminal-body` 596px against 594px for the
   xterm, background `rgb(30,30,30)` versus `rgb(255,255,255)`. The drift dates from the
   ongoing Tailwind-on-tokens migration: before it, `.hv-terminal-content`
   carried `rgba(255, 255, 255, 0.55)` — light, therefore consistent with the theme of
   the time — and `#1e1e1e` did not exist anywhere in the file. Fixed by
   aligning the container with the theme; along the way, `bg-[#1e1e1e]` was a
   hardcoded color, forbidden by the "no hardcoded value" rule — it moves
   to the token `--hv-shell-glass`.

Two tests added. **2.9** locks the vertical axis (computed `overflow-y`
`hidden`, vertical overflow ≤ 1px) — the computed value is the only signal
that holds on all platforms, an overlay bar taking no
width. **2.10** compares live the color of the container to that of the
`xterm-viewport` (what the terminal actually paints) instead of a
constant: changing the theme drives the container, the drift is impossible.
These two assertions discriminate well — the values they reject
(`overflowY: "auto"`, `rgb(30,30,30)`) are exactly those read before the
fix. Verified by a real run (Firefox, fresh disposable instance on a scratch
port, dedicated `BASE_URL`, no real data): **81 tests**, sections 2, 4,
8, 12, 13 green, plus the complete suite; only **4.2** fails, pre-existing
(monospace family, already proven so by `git stash` the day before).

**2026-10-07 — the Quotas screen executed and audited live (the last screen
without a net).** The spec written on 2026-09-22 is **green end to end** for
the first time: **38/38** (`npx playwright test tests/e2e/quotas/quotas.spec.ts
--project=firefox`, reference count `--list` = **38**; `grep -cE
'^\s*test\('` = **36**, the difference coming from the responsive loop 360/768/1920, one
line for three tests), against a scratch server (`PORT=8099`,
isolated `DATA_DIRECTORY`, Firefox, no real data, `e2eadmin` seeded via
`POST /api/auth/setup` as in CI — the screen opens as admin). **No dead
selector**: the spec postdates the Appliance migration of
2026-09-12 and already targets the `data-testid` of `Quotas.svelte`, so **no**
assertion had to be fixed. Live audit (same scratch server, real Firefox,
screenshots as evidence): the window opens from the launcher, the header carries
only [ICON] [TITLE], the 3 filter chips (All/Users/Groups) form a
`role="group"` with an `aria-label`, mutually exclusive (`aria-pressed`), the
seeded row renders "USER", the name + "Updated on …", the Upload
`used/limit` bar (measured: 50 MB of 100 MB → `aria-valuenow="50"`, `aria-valuemax`
100, track filled at 50%) and "Unlimited" on the disabled Receive
direction; "Manage in Users" emits `hv-open-app`
(`detail.id = 'users'`), opens the Users window **without leaving**
`/desktop` or closing Quotas; console without errors. Detail in the 2026-10-07
addendum of `CLAUDE.md` (invariant "Explorer / transfers"). Consequence:
none of the 27 screens is now without a live audit **or** without an E2E spec.

**2026-10-12 — deleting a user left its share permissions behind (fixed).**
Found while auditing what was *actually* still open (the docs claimed several
fixed bugs as open, and vice versa). `handle_user_delete`
(`src/handlers/users.rs`) only walked the user's effective permissions to
`tracing::info!("Revoking permission for user …")` — **nothing was revoked**,
neither the POSIX ACL nor the `share_permissions` row. The table keys a grantee
by **name only** (no foreign key to `users`), so the rows outlived the account:
recreating a user with the same username silently inherited the deleted user's
access, and the audit log claimed a revoke that never happened. Fixed by a new
`PermissionService::revoke_all_user_permissions(username, pool)`: it removes
the real ACL for every share the user held a **direct** grant on
(best-effort, like every other revoke — an unresolvable share must not block
the cleanup) then deletes the rows with `delete_for_grantee("user", …)`.
Group grants are deliberately kept — the account was already removed from its
groups earlier in the handler, and the group rows belong to the *remaining*
members. Tests: `tests/users_integration.rs` goes from **60 to 61**
(`test_user_delete_revokes_its_direct_share_permissions_only`: two direct
grants — one on a parseable share type, one on a bogus one — plus another
user's grant and a same-named *group* grant; only the deleted user's direct
rows go). The test was **checked against the old code before shipping**
(`git stash` of the two source files): it fails with `left: 2, right: 0`, so it
really guards the regression. The ACL half needs root + a real share and stays
covered by `tests/e2e/shares/shares.spec.ts` per CLAUDE.md's "What NOT to test
in Rust". The stale "reported not fixed" notes in `docs/guides/shares.md`
(that revoke, plus the two ACL bugs already fixed in code — the `-d -m` order
of `set_default_acl` and the forced execute bit on a directory) were corrected
in the same change.

**2026-10-12 — the progress REST endpoint answered a placeholder, and no `Custom` job could be serialized at all (both fixed).** Two findings from the same subsystem, the second surfaced by the test written for the first.

**(a) `ProgressManager::get_progress` was a stub** (`src/sse/progress.rs`). It
returned a freshly built `Running` / `0 %` / `"Waiting..."` event for whatever
id happened to be in the map, throwing away the `last_event` the manager
already keeps in sync for the SSE stream and the activity feed. So
`GET /api/progress/{job_id}/status` reported "running, 0 %" for a job at 87 %,
for a **completed** job, and for a **failed** one. Nothing consumed it (the
frontend polls `/api/activity` for the taskbar and subscribes to the SSE stream
to follow a job), which is why it went unnoticed. Fixed: the handler returns the
job's real last event; `None` for an unknown or already-removed id.

**(b) `JobType::Custom` could not be serialized — every feature job was
unaffected in the activity feed but broken on the SSE wire.** `JobType` was
declared `#[serde(tag = "type")]` with a `Custom(String)` newtype variant, and serde
cannot internally tag a newtype variant wrapping a bare string: `to_string` fails
with *"cannot serialize tagged newtype variant JobType::Custom containing a
string"*. `Custom` is what **every** feature job uses — Explorer copy/move,
Docker install/update, antivirus scan, RAG reindex, VM create/ISO, ddrescue and
PhotoRec, cloud sync, Ollama model pull — while the built-in kinds
(`Backup`/`Restore`/`Verify`/`Upload`/`Download`/`Sync`/`CacheMover`) are the
ones almost nothing uses in practice. Two consequences, one per consumer:
`handle_progress_sse` does `serde_json::to_string(&event).unwrap_or_default()`,
so a `Custom` job emitted an **empty** `data:` payload — `JSON.parse` throws in
the browser, no `progress` and no `done` callback ever fires, the client falls
through to its `EventSource` error handler, and `Explorer.svelte`'s `watchJob`
therefore resolves **false** for a copy that in fact succeeded (progress bar
frozen, directory listing never refreshed — `Storage.svelte`'s cache mover, a
built-in kind, kept working, which is why the two SSE consumers behaved
differently); and `GET /api/progress/jobs` panicked inside the `json!` macro
for as long as any `Custom` job was tracked (the desktop taskbar was spared: the
activity handler maps `JobType` to a slug through `job_type_kind` and never
serializes the enum — that mapping is why the taskbar audited green all along).
Fixed by switching to **adjacent** tagging, `#[serde(tag = "type", content =
"name", rename_all = "snake_case")]`: the built-in kinds keep their exact
previous shape (`{"type":"backup"}`), and a feature job becomes
`{"type":"custom","name":"explorer_copy"}`. `frontend/src/lib/services/api.ts`
said `job_type: string` (wrong even before, nothing reads the field) and now
describes the real object shape.

Tests: `src/sse/progress.rs` **11 → 20** (5 for `get_progress`: seeded event
before the first emit, mid-flight `%`/message/byte counters, completed, failed,
unknown/removed → `None`; 4 for the serde shape: round-trip of every variant
through JSON, byte-identical wire shape for the built-in kinds, the `custom`
name field, and a whole `ProgressEvent` serializing non-empty) and
`tests/progress_integration.rs` **6 → 7**
(`job_status_reports_the_real_last_event` — over HTTP, through the real router:
mid-flight job reads its own `%`/message/`state`, completed reads
`completed`/100 %, unknown id is a 400; the failing assertion on the old code
was on the serialization half, which is how (b) was found). Both were **checked
against the old code** before shipping: reverting the two source changes makes
the 5 `get_progress` assertions fail (`left: Running` / `right: Completed`,
`left: 0.0` / `right: 87.5`, `left: "Waiting..."`) and the 3 serde ones fail on
`to_string`. `cargo fmt` clean, `npm run check` 0 error. Docs updated in the same
change: `docs/guides/explorer.md` (the "known pre-existing issue" section is
closed), `docs/API.md` (progress section), `CLAUDE.md` (Explorer/transfers
invariant) and `ROADMAP.md`.

**2026-10-12 — static-asset serving: validators, cache lifetimes, pre-compression and a subset icon sprite.** Four optimizations to the embedded `static/` tree (`src/handlers/spa.rs`), none of which changed a route or a payload — only the headers and the bytes behind them.

**(a) No `ETag`, so nothing was ever revalidated.** The `include_dir!`-embedded
files were served as raw bytes with no validator at all, which means a browser
could never issue `If-None-Match`; `/wallpapers/*.webp` (several MB) and the
3.6 MB icon sprite were re-downloaded on every visit. `spa` now builds a
`path → Asset` table once (lazily) and sends an `ETag` — a truncated SHA-256 of
the identity bytes **with a `-br` / `-gz` suffix per content coding**, because a
cache holding the identity body must not validate a `br` request. A matching
`If-None-Match` gets `304` with no body.

**(b) `Cache-Control` was only set on `/assets` and `/fonts`.** New
per-route policy (the blanket `SetResponseHeaderLayer` is gone, so a layer can
no longer accidentally pin the API or `index.html`): `immutable, max-age=1y`
for `/assets`, `/fonts`, `/wallpapers`, `/static`; `max-age=86400` for the two
sprites (their URL is fixed but their content is regenerated every build, so
`immutable` would pin a stale sprite — i.e. blank new icons — for a year); none
for `/manifest.json`, `/favicon.svg`, `/sw.js`, `index.html`.

**(c) Compression ran per request.** `frontend/scripts/precompress-dist.mjs`
(new, npm `postbuild`) writes a `.br` and a `.gz` sibling for every compressible
file in `static/dist`; the same `include_dir!` embeds them and `spa` serves them
on `Accept-Encoding`, setting `Content-Encoding` + `Vary: accept-encoding`. That
header is also what makes `CompressionLayer` skip the response (it never
re-compresses an encoded body), so the on-the-fly compressor is now a fallback.
93 files: 3 375 KB → 748 KB brotli (−78 %) / 846 KB gzip (−75 %). Dropping the
last `SetResponseHeaderLayer` also let the unused `tower-http` features `fs` and
`set-header` go (Cargo.lock loses `http-range-header`, `mime`, `mime-date`,
`futures-util`… from that crate).

**(d) The icon sprite shipped 7 750 glyphs to render ~110.**
`generate-sprite.mjs` now subsets to the files whose string literals can become
an icon name (components rendering `<Icon>`, components passing an `icon`-ish
prop, `$lib/stores` + `$lib/services`, the first-party catalog manifests):
3.6 MB → **55 KB** (11.7 KB brotli) and `mapping-unified.ts` 485 KB → 6 KB.
Scanning the whole tree instead removes almost nothing (7 659 of 7 750 kept)
because icon names are ordinary short words; a whole-tree first attempt was
measured and rejected for exactly that reason. A pre/post comparison script
verified **0** reachable glyph references lost and **0** mapping/sprite
inconsistencies. Regeneration is wired into `npm run build` (`prebuild`) and the
script aborts the build if the subset comes out empty; `HV_SPRITE_FULL=1`
restores the complete sprite.

Tests: `src/handlers/spa.rs` unit **21, new** (the module had none): ETag
shape, stability and per-coding distinctness; `Accept-Encoding` parsing incl.
`q=0`, `*`, case and other parameters; `If-None-Match` incl. `W/`, `*`, lists;
and the whole `asset_response` matrix — br beats gzip, gzip when br is absent, identity
fallback, `Vary`, 304 without a body, a stale validator still getting the body,
and identity-vs-br validator independence. New
`tests/spa_assets_integration.rs` **9** over a real router: the committed
`static/sounds/README.md` gives the deterministic half (validator + `immutable`
+ 304 + no body + a stale validator + 404 rather than the SPA fallback), and the
sprite/wallpaper half states its `static/dist` prerequisite and no-ops without
it (the integration CI job runs `cargo test` with no frontend build). Verified:
`cargo test --lib` **643** ok (622 before this change, `spa.rs` contributing the
21 new ones), `cargo test --test spa_assets_integration` 9 ok,
`svelte-check` 0 errors, `cargo fmt` clean, `cargo clippy` clean on the touched
files (the 3 remaining `manual_contains` warnings are pre-existing in
`src/dependencies.rs`). `npm run build` → 3375 KB raw / 748 KB br.

**2026-10-12 — the Cloud Sync editor read and wrote a different file than the one the remotes come from (fixed).** `handle_get_config`, `handle_save_config`, `handle_add_remote` and `handle_delete_remote` hardcoded `/etc/rclone.conf`, while `parse_rclone_config()` — which backs `GET /rclone/remotes` and every transfer — honoured the `RCLONE_CONFIG` override. Two symptoms for an admin whose config lives elsewhere (the documented way to run rclone as a non-root user): the remote list and the Settings editor disagreed about which file they were describing, and saving failed with `Permission denied` (os error 13) against a root-owned `/etc/rclone.conf`. Both sides now resolve through one function, `rclone_config_path()`: an explicit non-empty `RCLONE_CONFIG` wins outright (existing or not, since that is how a fresh config gets created outside `/etc`), then the first of rclone's fallback locations that exists, then `/etc/rclone.conf`. `parse_rclone_config` delegates to it instead of looping over candidates on its own. Tests: `handlers/rclone.rs` **11 → 14** (override wins even when the file does not exist yet; an exported-but-empty override is ignored rather than resolving to `""`; the fallback is always absolute; and the parser and the handlers resolve the same file) and `tests/rclone_integration.rs` **14 → 17** — the suite could never test the config CRUD because a writable `/etc/rclone.conf` was out of reach, so `add_remote → list → delete_remote` plus `GET`/`POST /config` and the `.backup.<timestamp>` sibling now run for real against a tempdir-selected config. Checked against the old code: the three integration tests fail with `Failed to read config /etc/rclone.conf: No such file or directory` and `Failed to save config: Permission denied`, i.e. exactly the reported inconsistency. Note the list endpoint itself was already correct — the bug was confined to the four read/write handlers. `cargo fmt` clean, `cargo clippy --lib -- -D warnings` clean. (The 3 `manual_contains` warnings in `src/dependencies.rs` — from the parallel `install.sh` refactor — were fixed in the same pass: `iter().any(|have| *have == want)` → `contains(&want)`. CI runs `cargo clippy --all-targets -- -D warnings`, so they were a hard failure, not a style note.)

**2026-10-12 — the RAG knowledge base was missing 25 of the 45 indexable guides (fixed).** `CLAUDE.md` has always required "a new feature gets a `FeatureDoc` + a `rag.md` Document Sources row", but the rule was **prose only** and was never applied retroactively: `src/rag_indexers.rs` carried **20** entries and `docs/guides/` had grown to 46 files. The consequence was user-visible — the assistant answered "I don't know about that" for shipped features such as Syncthing, the downloader, notifications, the desktop shell and app launcher, LUKS encryption, the RAID wizard, the cache tier, widgets, wallpapers, themes, mDNS, server control, avatars, the account page and the setup wizard. All 45 are now indexed (**20 → 45** entries). The change also **removes the class of bug** rather than just the instances: `test_every_guide_has_a_feature_doc` enumerates `docs/guides/*.md`, subtracts the explicit `DEVELOPER_ONLY_GUIDES` list (just `design-system.md` — a guide about tokens is not a feature a user can open), and requires the result to equal the guides claimed by the `FEATURE_GUIDES` table id-for-id, itself asserted against the real `feature_docs()`; `test_feature_docs_are_unique_and_substantive` rejects a duplicate id, a stub body (< 200 chars) and implementation details leaking into the text (`src/`, `.svelte`, backticks), since these entries are read by a user-facing assistant. Verified by mutation: deleting the `avatars` row fails the coverage test with the two guide lists side by side. Two tests replaced the previous `test_feature_docs_are_nonempty`, which asserted `features.len() >= 18` on a **hardcoded list unrelated to the shipped vector** — it could not have caught the drift, and hadn't. The Users entry was also refreshed: it still described account deletion without the share-permission revoke. `cargo test --lib rag_indexers` **3/3** (`index_all` still upserts on re-index without duplicating).
**2026-10-13 — the first-run state moved from marker files into the database, and the hardware-change dead-end became a recovery flow.** `/etc/homevault`-style marker files (`.setup_complete`, `.system_hash`) answered "is the appliance configured?" while the admin account lived in SQLite, so the two could disagree: losing the data directory while the DB survived re-opened the Setup Wizard with an admin still present, and a changed hardware fingerprint re-showed the Wizard only for `/auth/setup` to reject it with "Setup already complete". Now `SetupStateRepository::has_admin()` (a `users` row with `role_id = 2`) is the single source of truth, and the singleton `setup_state` row (migration **037**, `sqlx/migrations/037_setup_state.sql`) carries the fingerprint recorded at setup time — the account and the setup state travel together in `{DATA_DIRECTORY}/data/homevault.db`. A changed fingerprint with an admin present sets `needs_recovery`, which the new frontend route guard (`frontend/src/lib/stores/appGate.ts`, the standard route-guard pattern) routes to a `Login` `recoveryMode` screen; a successful admin sign-in (step-up auth) calls the new admin-only `POST /auth/setup/rebaseline`. The three unauthenticated bootstrap routes (`/auth/setup`, `/auth/setup/verify-totp`, `/auth/check-username`) are now restricted to loopback/private/link-local peers by `src/middleware/setup_guard.rs` (peer from `ConnectInfo`, never a forgeable `X-Forwarded-For`; `HOMEVAULT_SETUP_ALLOW_REMOTE=1` opts out). Kept: Argon2, the 8+/mixed-case policy, `check-username`, the one-time recovery code, TOTP at the password step, and the `O_EXCL` setup lock. The reference-design reboot-during-setup RAID flow is deliberately **not** implemented (the wizard has no storage step; pools are created after setup) — documented in `docs/guides/setup-wizard.md`. Tests: `setup.rs` **43 → 45** unit, `setup_state_repo.rs` **6**, `setup_guard.rs` **6**, `setup_integration.rs` **34 → 47**, `login_integration.rs` **53 → 54**; `cargo test --lib` **656/656**, `npm run check` 0 errors, `npm run build` OK. Frontend E2E specs updated but **not re-run** (no fresh first-run server in this session).

## Summary

| Level | Coverage |
|---|---|
| Manual live audit (CLAUDE.md, invariants) | **27 / 27** screens (Quotas audited live on 2026-10-07) |
| At least one relevant Rust unit test | **27 / 27** (Widgets and Quotas: not applicable — Quotas has no code of its own, it reads an existing endpoint; Desktop/shell covered since 2026-09-09 by `src/sse/progress.rs` + `src/handlers/activity.rs` — taskbar) |
| Rust integration test (`tests/*.rs`) | **42 files** `*_integration.rs`; one per screen except Quotas — Login, Setup Wizard, Account, Docker (+ `compose_integration.rs` — editing the compose of a container), Users, Groups, Rescue, Network, VPN, Shares/Protocols, Settings (security cards **+ core** — `settings_core_integration.rs` **+ built-in HTTPS/TLS — `tls_integration.rs`**), Downloader, Storage, RAID, Backup, Cloud Sync, Syncthing, Terminal, Chat (+ `mcp_integration.rs` — external-agent server + catalog phase-B endpoints), Explorer, Dashboard, System journal, Widgets, Desktop/shell (+ `activity_integration.rs` — taskbar), Health, **VirtualMachines**; **Quotas: no file of its own** — its only endpoint (`GET /transfer-quotas/{type}`) is covered by `transfer_quota.rs` + `explorer_integration.rs`. The remaining files are cross-cutting (`mdns`, `notifications`, `progress`, `snmp`, `ups`, `weather_location`, `app_catalog`, `app_proxy`, `compose`, `security`, `tls`, `share_links`, `spa_assets`, …) |
| Playwright E2E test (`tests/e2e/`) | **27 screen specs out of 28 files** — Login, Setup Wizard, Account, Users, Groups, Terminal, Settings, RAID, Docker, Rescue, Network, VPN, Shares/Protocols, Downloader, Storage, Backup, Cloud Sync, Syncthing, Chat, Explorer, Dashboard, System journal, Widgets, Desktop/shell, Health, **VirtualMachines**, **Quotas**, plus `security/` (sub-feature, not a section of `SCREENS.md`); the Quotas spec (`tests/e2e/quotas/quotas.spec.ts`, written on 2026-09-22) has been **green end to end since 2026-10-07** — **38/38** (Firefox, scratch server) — see the per-screen detail line 6 |

All 27 screens have been verified by hand **and** have at least one
automated test (Rust or Playwright) — none is without a non-regression
net (VirtualMachines: verified live against a real Incus —
containers, Linux ISO VM + noVNC screen, concurrent creation, ISO
library; only the boot of a real Windows ISO remains to be confirmed).

## Detail by screen

| # | Screen | Manual audit (CLAUDE.md) | Rust unit tests | Rust integration tests | Playwright E2E tests |
|---|---|---|---|---|---|
| 1 | Login | [x] 2026-07-13 (+07-15, 07-17, 09-03, 09-04) | [x] `auth/service.rs`, `auth/tests.rs` | [x] `login_integration.rs` (54 — +1 on 2026-10-13: the hardware-change tests were re-pointed from the removed `.setup_complete`/`.system_hash` marker files to the `setup_state` table + an admin row, and a third case was added — an admin with **no** recorded fingerprint is configured, not a first run. 53 before), `login_pam_integration.rs` (3), `auth_recovery_integration.rs` (7) | [x] `login.spec.ts` (91 — **+2026-09-23: 13.6 contrast guard of the Login tokens**: accent / `soft-border` / the 2 gradients ≥4.5:1 under white, read on the live `:root` — see "standardized contrast" l. 26; 90 before) ; **+2026-09-12: Appliance design migration — auth card + 2FA + code recovery in Tailwind-on-tokens (native inputs kept for autofill, native submission chain verified), selectors `data-testid="login-*"`, zero scoped CSS** ; **+2026-09-12: §12 anti-zoom iOS — form fields ≥16px on touch pointer** (`pointer: coarse`, cf. `docs/guides/design-system.md` §5.1) ; **+2026-10-13: the login screen no longer renders the wizard inline — `App.svelte` renders it from `$appGate.route`, and `Login` gained the `recoveryMode` hardware-change banner + `/auth/setup/rebaseline` call and the `connectionError` prop. Spec not re-run** |
| 2 | Setup Wizard | [x] 2026-07-13 (+07-17, 09-03) ; **+2026-10-13: verified live logic through the Rust suites — first-run state derived from the database, hardware-change recovery, local-network guard** ; **+2026-10-13 (b): the wizard grew from 4 to 8 steps (welcome, language, storage, RAID, restore, account, password, verify) and became reboot-safe — the step + non-secret choices persist server-side** | [x] `setup.rs` (45 — +2 on 2026-10-13: `system_hash_matches` accepts the current hash, rejects a stale/empty one), `storage/setup_state_repo.rs` (6 — `has_admin` true/false/admin-vs-user, `mark_complete` round-trip + upsert, `update_system_hash` inserts a missing row), `storage/setup_progress_repo.rs` (singleton `setup_progress` row — get/put round-trip, non-secret choices only), `middleware/setup_guard.rs` (6 — loopback, private, link-local, ULA, IPv4-mapped unwrapping, public rejected) | [x] `setup_integration.rs` (**52** — +5 on 2026-10-13 (b): `/auth/setup/progress` GET/PUT round-trips the step + choices, both 403 once an admin exists and are behind the local-network guard, `/auth/setup/storage-options` lists disks and 403s after setup. +13 on 2026-10-13: **DB-derived state** — an admin row with no marker files is not a first run, `/auth/setup` refuses it, `/auth/setup/verify-totp` 403s, and setup records the fingerprint in `setup_state`; **recovery** — `/auth/setup/rebaseline` requires a session, requires admin, and clears `needs_recovery`; **local-network guard** via `create_app` — a public peer is 403 on `/auth/setup`, `/auth/check-username` and `/auth/setup/verify-totp`, loopback and a LAN peer are allowed, `HOMEVAULT_SETUP_ALLOW_REMOTE` lifts it. +6 on 2026-09-22: TOTP `verify-totp` accepts/rejects + gated post-setup, setup rejects an invalid TOTP code without creating a user, setup with a valid TOTP = 2FA enabled + backup codes, secret alone ignored) | [x] `setup.spec.ts` (90 — +19.0 on 2026-09-22: TOTP toggle at the password step → QR + secret + compatible-apps message, non-destructive) ; **+2026-09-12: Appliance design migration — stepper + steps + admin creation + recovery + 2FA in Tailwind-on-tokens, selectors `data-testid="setup-*"`, zero scoped CSS** ; **+2026-09-22: TOTP moved INTO the password step (toggle → client `qrcode` QR → code verified via stateless `POST /auth/setup/verify-totp` then re-verified in `POST /auth/setup`), recovery code generated+displayed at step 4 after creation, post-creation 2FA screen removed — backup codes recap after recovery, section 19 rewritten (19.0 non-destructive + 19.1 full flow)** ; **+2026-10-13: the wizard is now rendered by `App.svelte` from `$appGate.route`, not by `Login.svelte`; the setup E2E still detects it via `[data-testid="setup-container"]`. Spec not re-run** ; **+2026-10-13 (b): the 8-step flow adds selectors `setup-stepper-label` (8 dots), `setup-language-{en,fr,de,es}`, `setup-storage-list`/`setup-storage-system`, `setup-raid-modes`/`setup-raid-{none,mirror,stripe}`, `setup-restore-choice`/`setup-restore-skip`/`setup-restore-enable`, and the `setup-resumed` banner. Spec not re-run** |
| 3 | Desktop (desktop/shell) | [x] 2026-07-17 (+ ~15 follow-ups up to 09-04, incl. the glassmorphism fix of 09-05) | [x] 2026-09-09 — `src/sse/progress.rs::tests` (20 — last event retained, `owner`, `list_activity`, cleanup; **+2026-10-12, 9 new**: 5 on `get_progress` now returning the job's real last event instead of the hardcoded `Running` / 0 % / `"Waiting..."` placeholder — seeded event before the first emit, mid-flight `%`/message/byte counters, completed at 100 %, failed, and unknown-or-removed id → `None`; 4 on the `JobType` serde shape — round-trip of every variant through JSON, byte-identical wire shape for the built-in kinds, `{"type":"custom","name":"explorer_copy"}` for a feature slug, and a whole `ProgressEvent` of a `Custom` job serializing non-empty) + `src/handlers/activity.rs::tests` (14 — `kind` mapping, aria2 name derivation, %, serialization) for the **taskbar** | [x] `desktop_integration.rs` (3 — auth guards on `/api/server/restart` + `/shutdown`) + `activity_integration.rs` (11 — `GET /api/activity`: `require_auth` 400/401, pending/running/completed/failed lifecycle, indeterminate `%`, partitioning by `owner`, never spawns aria2) + `progress_integration.rs` (7 — `require_auth` 400/401 on `/api/progress/jobs` and `/{job_id}/status` and the SSE route, security-hardening headers, and **+2026-10-12** `job_status_reports_the_real_last_event`: a real tracked job reads back its own `%`/message/state over HTTP, a completed one reads `completed`/100 %, an unknown id is a clean 400 — this test is what exposed the `JobType::Custom` serialization failure below) | [x] `desktop.spec.ts` (**74 runtime tests, 15 categories** — recounted on 2026-09-24: +11.3 "a crashing window body shows the alert fallback and Reload recovers it" (`svelte:boundary` guard, W10 — written with the rest of the pass, **not executed**; `grep -cE '^\s*test\('` **and** `npx playwright test --list` give 74); 73 on 2026-09-17: +14.11 "every app header bar carries the icon + title only", which iterates the 24 app ids of the registry and requires icon + title only in the bar; 72 on 2026-09-16 via `grep -cE '^\s*test\('` **and** `npx playwright test --list`, both give 72 (the file's method); +cat. 13 on 2026-09-09: taskbar (dock button + badge, panel width = dock, name/%/track row, row click = nothing, close via Escape/outside/re-click, responsive, localStorage/URL safety, poll `/api/activity` never `/downloader/tasks`); **never confirms** a restart/shutdown/logout; +2026-09-12: chrome geometry tests — 8.5 dock/topbar pills same radius at each width, 8.6 maximized window does not overlap the dock on tablet; selector fix 5.3 (`.jr-page` renamed → dialog `aria-label="System Journal"`); **+2026-09-14: dock/topbar fidelity to the approved gateway mockup — 8.3 rewritten** (each pill is worth **its own** token, `--hv-chrome-topbar-h` / `--hv-chrome-dock-h`, and the dock is strictly taller: the old "same height" invariant is replaced by its opposite, otherwise the test would no longer protect anything) **+ cat. 14 "Chrome geometry & material"** (14.1 the JS mirror `snapUtils.ts` exactly copies the CSS keep-outs; 14.2 the `--hv-app-<id>-c` badge of each tile is indeed the dark stop of its gradient; 14.3 the two pills are opaque — `backdrop-filter:none` + `rgb()` without alpha; 14.4 the hover tooltip, until now uncovered: content = `aria-label`, background `--hv-tooltip-bg`, radius and font size); 8.4 and 8.5 stay green without modification — the avatar shrinks with its frame, the pill radius has not moved; **+2026-09-14: topbar buttons as icon + label chips (approved mockup, node `1:24`) — cat. 15 "Topbar icon + label chips"**: 15.1 at 1440px each button shows icon **and** label (width > 44, height = 44, label to the right of the icon and centered to ±2px); 15.2 below 901px the label is `toHaveCount(1)` **then** hidden (present-but-hidden, a single markup branch) and the button becomes a 44×44 square again; 15.3 real DOM order `[network, widgets, notifications, power]` **and** Power stays the last `.hv-topbar-btn` (the positional hypothesis of 7.1-7.3 is kept as an assertion, even though those three tests now use `[data-testid="topbar-power"]`); 15.4 the notification badge is **16px** (the probe is a clone of a template element — Svelte CSS is scoped by hash, a `createElement` would measure an unstyled span), `.hv-dock-badge` keeps its **26px**, and the badge lives in the band **above** the label line (`badge.bottom <= label.top`) so a "99+" widens into empty space instead of covering the text; 15.5 translated accessible name (never a raw key) and, at 1440px, containing the visible label — WCAG 2.5.3, which validates the `power_options`/`power` pair; 15.6 at **901px** (the blind spot of 8.2, which only samples 360/768/1920) the right-anchored pill stays at `left >= 0` and without horizontal scroll; 15.7 the 4 locales render 4 non-empty and non-key labels, then the **German** pill (the worst width case) still fits at 901px; 8.3 / 14.1 / 14.3 / 14.4 stay green **without modification** — proof that the delta is purely horizontal; **+2026-09-14 (same day): app window chrome = "AppWindow" mockup (Figma `4pwNRZU5wQjSWQBODFViKz` node `3:76`) — cat. 14 completed by 14.5 opaque surface `#FFFFFF` + slate-300 hairline + two-layer shadow + `backdrop-filter:none`, 14.6 44px bar / 24px radius 8 tile / 14px 600 `#314158` title on 20px, 14.7 28×28 radius 8 gap 6 trio at the stops `#FFB900`/`#2B7FFF`/`#FF6467` **in the bar** (inter-button gap and 12px inset measured, not read in the tokens) and 14.8 the `--hv-winhdr-btn-clear` reserve really protects the title — **measured** at 1440/768/400px, because the trio is absolutely positioned: the right padding of the bar is the only thing preventing it from going underneath. **14.9 (a single header per window) was written after finding a real bug**: `quotas`, added on 2026-09-14, was missing from the `OWN_HEADER_APPS` `Set` of `WindowFrame.svelte` → the Quotas window stacked two bars (its own + the empty WindowFrame fallback); fixed by adding `'quotas'` to the `Set`. Categories 3 and 15 stay green **as they are**: no `data-testid` moved, only the size, color and position of the trio changed; **+2026-09-14 (same day): standardization of headers + dock tile gap — 14.6 hardened** (the bar is now title-only: the icon tile is worth `--hv-winhdr-icon-size` = `--hv-winctrl-size`, i.e. the same 28px as the trio, and the measured height is **exactly 44px** — the test goes from a floor `>= 44` to an equality, which turns any element that would come back into the bar into a failure); **14.7 completed** (the median lines of the tile, the trio and the title are compared to that of the bar, not just their sizes); **14.10 new** (the `gap` of `.hv-dock-container` goes to `--hv-chrome-dock-gap`, 10px: on hover a tile grows by `48 × (1.18 − 1) / 2 ≈ 4.3px` on each side, so the old `gap` of 4px made two neighbors **actually** overlap; the test hovers a middle tile and requires the measured clearance at the two neighbors to exceed the growth actually produced — it reads no token); 14.1 to 14.5 and 14.8 to 14.9 unchanged. **Specs modified but NOT re-run** — the session instruction was explicitly to neither compile nor test) ; **+2026-09-16: redesign of the app launcher on the Figma mockup `4pwNRZU5wQjSWQBODFViKz` node `23:6646` ("Desktop launcher") — `HvLaunchpad.svelte` goes from an opaque centered modal (1000×720, vertical category rail, wide cards with description) to a **darkened full-screen veil** (`.lp-overlay`, `rgba(15,23,42,0.72)` + `backdrop-filter`, content column aligned at the top (704px at the redesign; **828px** since the "6 columns" request of the same day)), a **row of category chips** (`aria-pressed` in a `role="group"`, not a `tablist`: they filter the same grid in place) and a **compact tile grid** (`repeat(auto-fill, minmax(124px, 1fr))`, 56px icon above the label on 2 lines max). New page-scoped palette `--hv-launcher-*`; **two values of the mockup were corrected at the WCAG contrast threshold** — the veil `0.55` → **`0.72`** (the white labels fell to 3.39:1 on a light wallpaper) and the inactive chip text 85% alpha → **opaque white** (4.28:1); the placeholder `#90A1B9` (2.6:1 on the white bar) goes to `#64748B` and the secondary ink 72% → 88% alpha (3.8 → 4.7:1). The tiles keep the **per-app gradient** (`--hv-app-<id>-grad`) and not the white square of the mockup — the "one color per app" rule is binding; **no invented counter badge** (the mockup shows some, the data does not exist): the only corner badge is green and **is rendered only if the container is running** (a gray "stopped" dot is undetectable on the tile background, ~1:1; the stopped state is already carried by `aria-describedby`). The 8 `desktop.app_<id>_info` warnings (Rescue, RAID, Storage, Network, VPN, Backup, Users, Docker) survive as an **accessible description** (`.lp-sr` + `aria-describedby`) instead of being orphaned. The scrim becomes a real `<button>` sibling of the dialog (no more `role="button"` nesting `<button>`s). **E2E contract unchanged**: `.lp-overlay`, `.lp-panel`, `.lp-card[title]`, `.lp-search-field`, the external Docker menu (`.lp-ext-*`), `.lp-empty`/`.lp-reset`, `.lp-count` keep their selectors — 5.1 to 5.4 pass as is; **category 5 extended from 4 to 12 tests**: 5.5 the chips filter the grid in place + exactly one `aria-pressed="true"`, "All apps" restores; 5.6 search with no result → `.lp-empty` → `.lp-reset` restores the grid and clears the field; 5.7 the launcher is a `fixed` **dark translucent** veil and the content column paints no surface; 5.8 **measured contrast** — the composite tile label tile→veil→white paper (the worst case of a wallpaper) and requires ≥ 4.5:1, opaque ink; 5.9 the chip paints < 44px but carries a 44px `::after`, the search field is 44px in the 56px bar of the mockup, the tile ≥ 44px; 5.10 the focus trap holds 30 Tab and Escape **returns focus to the dock tile** (2.1.1 / 2.4.3); **5.11 (phone)** 3-column grid, `nowrap`/`overflow-x:auto` chips with `flex: none` (**the desktop growth is cancelled** — a stretched chip would fill the row and there would be nothing left to scroll) and a row **actually overflowing**, the panel **scrolls internally** (neither the page nor the horizontal axis moves) and the last tile remains reachable; **5.12 (6 columns — request of 2026-09-16)** the grid makes exactly 6 tracks of ≥ 124px at 1440px, **and** the search bar, the chip row and the grid share the same width **and** the same `x` (1px sub-pixel tolerance: it is a layout misalignment that must fail, not a rounding), **and** the content of the chip row reaches the right edge of the grid (**request "not enough categories → enlarge the module" of the same day**: the box of the row already measured 828px, but a chip is only as wide as its label, so with few categories the chips stopped at two thirds and the module *appeared* narrower than the tiles; `flex: 1 1 auto` shares the free space of each line among its chips, and the assertion is made **per rendered line** so that it also holds when the row wraps — a complete catalog goes up to 13 chips; `grow` rather than `space-between`: both spread the last line, but `grow` spreads it into slightly wider chips whereas `space-between` opens a gap between two distant chips) — `--hv-launcher-content-w` goes from 704 to **828px** (6 × 124 + 5 × 12 = 804 minimum; 828 lands on round 128px columns). The three modules being children of the same column, they widen together — it is the test that prevents it from drifting. The scrollable container moved from `.lp-content` to **`.lp-panel`** (with `scrollbar-gutter: stable` and a `sticky` search bar) precisely for that: a scrollbar that takes width (Chrome Windows/Linux, GTK without overlay) took it away from the grid alone, the search bar and the chips being outside that container; 5.12 therefore measures the three widths **a second time at 1440×520**, with overflowing tiles — the only regime where they can diverge (the E2E engine's scrollbar is an overlay, 0px, and hides the drift). **Two real defects found by these tests, not by re-reading**: (1) 5.5 and 5.6 failed because the `<svelte:window onclick>` of `Desktop.svelte` closes the launcher on every click — `.lp-panel` took back its stopPropagation `onclick` (and above all no `onkeydown`, which swallowed Escape: it is a bug already seen); (2) the measurement of the `backdrop-filter` of 5.7 showed that declaring the pair `backdrop-filter` + `-webkit-backdrop-filter` makes the minifier keep the **only** prefixed form, which Firefox ignores (`CSS.supports('-webkit-backdrop-filter', …)` = false) — the `blur(2px)` of the mockup was therefore worth `none`. Written as a standard property alone, the minifier itself emits both forms. **72 tests / 15 categories** |
| 4 | Account | [x] 2026-07-13 (+09-03, 09-04) | [ ] shared with `auth/` only, nothing specific to Account | [x] `account_integration.rs` (25) | [x] `account.spec.ts` (72 — recounted on 2026-09-14 via `grep -cE '^\s*test\('`, the file's method; the entry said 76, an outdated number) ; **+2026-09-12: Appliance design migration — `PageLayout` + `SectionCard`, 2-col grid via Tailwind container queries (`@container`/`@max-*`), selectors `data-testid="account-*"`, zero scoped CSS** ; **+2026-09-14: standardization of headers — 1.2 now verifies `.hv-winhdr-title` alone + `.hv-winhdr-subtitle` `toHaveCount(0)`; test 1.3 "the admin shows the administrator subtitle" is DELETED (the subtitle no longer exists) and the following tests of category 1 renumbered 1.3/1.4 so as not to leave a hole — that is the cause of the −1; 17.1 verifies the same invariant. Specs modified but NOT re-run** ; **+2026-09-22: self-service session management — two `SectionCard` "Active sessions" (list IP/created/expires, "This device" badge, per-session revocation via `POST /api/security/sessions/revoke`, "Log out all others" via `/revoke-all` with `ConfirmDialog`, refresh button) and "Recent logins" (`GET /api/security/login-history/{me}`: IP, success/failure, timestamp, user-agent); 15 i18n keys ×4 locales under `[account]`; existing Account specs not re-run** |
| 5 | Users | [x] 2026-07-13 (+ 3 bugs found by E2E on 09-04, +1 bug found/fixed by static review/tests on 09-05; +2026-09-08: home directory size slider = `setquota -u` disk quota; +2026-09-08: single-view design redesign in the approved template style — thumbnail grid → settings-list sheet → creation form, markup/CSS only; +2026-09-08: folder picker for the home directory (server validation `validate_custom_home_path`), size slider for all roles, explicit error state when the group list fails, `homevault_admin` displayed as a read-only chip derived from the role) | [x] `groups.rs` (5) + `handlers/users.rs` (13, `is_valid_email`) + `quota.rs` (+2 — `bytes_to_quota_kb`) + `handlers/user_homes.rs` (7 — `validate_custom_home_path`: rejects relative/traversal/control characters/root/system dirs/missing parent, `containing_partition` longest-prefix) | [x] `users_integration.rs` (61 — +2026-10-12: `test_user_delete_revokes_its_direct_share_permissions_only`, the regression guard for the direct grants left behind by a deleted account; +2 before: `PUT .../full` with `home_path` rejects a path under a system folder + relative/traversal/control-character paths) | [x] `users.spec.ts` (**81** — recounted on 2026-09-14 via `grep -cE '^\s*test\('`, the file's method; the entry said 82, an outdated number; `npx playwright test --list` remains the reference and has not been re-run), incl. transfer quotas + §16: slider offered for an admin too (16.4 inverted), folder picker opens/closes (16.6) + 4.11 `homevault_admin` chip read-only derived from the role — §16 verified live via Playwright MCP on 2026-09-08; the `beforeAll` of the whole file requires a NOPASSWD sudo for `groupadd`/`useradd`, absent here) ; **+2026-09-12: Appliance design migration — `PageLayout`, grid/thumbnail view + settings sheet + creation in Tailwind-on-tokens, `FolderPicker`/`Modal` kept, selectors `data-testid="users-*"`, zero scoped CSS; the pre-existing TS error (`selectedUser`→`never` reduction in `{:else}`) fixed** ; **+2026-09-13: persistent 2-pane master-detail (Aurora Mail pattern) — grid/thumbnails on the left (360px column), sheet/creation on the right; the list is removed from the DOM when the container goes below 760px AND a detail is open (ResizeObserver on the split, not a viewport media query — the window floats), test 7.4 "one view at a time" preserved** ; **+2026-09-14: Figma mockup redesign (`4pwNRZU5wQjSWQBODFViKz` node `3:99`) — account rail on the left (New user button, search, 4 filters, counter, compact rows avatar/name/badge/state, footer "Quotas view" → Quotas app via `hv-open-app`) + detail pane in **6 `SectionCard divider` cards** at content height (Identity / Role and groups / Home folder / Transfer quotas / Security / Actions); the fixed heights of the mockup (152.69 / 216.23 / 173.09 / 887.26 px) are not copied; the switch threshold goes to ≈ 1065px window width (760px measured on the content column: rail 257px + `p-6` 48px already deducted); **pre-existing bug `canBack` FIXED** — the variable was declared nowhere and the shipped bundle raised a `ReferenceError` in strict mode when opening the creation pane (the "New user" button was broken in production); the notion is removed, not declared (the back bar is always rendered when a pane is open). **Spec not modified and not re-run** ; **2026-09-14 (same day): `npm run check` → 7 errors, none in `Users.svelte`/`Quotas.svelte` (the 7 are pre-existing: `Restic.svelte` 2, `Syncthing.svelte` 2, `Downloader.svelte` 3); `npm run build` OK (32 s, `Users-CCVDHXRe.js` + `Quotas-DbeDd7_t.js` emitted); `cargo build` OK (9 min 55, no warning). The check found and had 2 real defects of `Quotas.svelte` fixed: `t` never imported from `$lib/stores/i18n` (22 cascading errors) and an undeclared `{subtitle}`. The E2E pane remains not run** ; **+2026-09-14: standardization of headers — 1.2 no longer expects the recap "%accounts% accounts · %locked% locked" (`users.summary`) in the subtitle but `.hv-winhdr-title` alone + `.hv-winhdr-subtitle` `toHaveCount(0)`; the counters remain visible, in the filter chips (the test verifies the 4 `[data-testid="users-filter"]` so that the reading does not disappear silently). Number of tests unchanged (81, recounted on 2026-09-14); spec modified but NOT re-run** |
| 6 | Quotas | [x] **2026-10-07 — verified live** (Firefox scratch server, `PORT=8099` + isolated `DATA_DIRECTORY`, `e2eadmin` seeded via `POST /api/auth/setup` as in CI, no real data): opening from the launcher, header [ICON] [TITLE] only, the 3 filter chips (All/Users/Groups) `role="group"` + mutually exclusive `aria-label` (`aria-pressed`), seeded row "USER" + name + "Updated on …", measured `used/limit` bar (50 MB of 100 MB → `aria-valuenow="50"`, `aria-valuemax` 100, track filled at 50%) and "Unlimited" on the disabled direction, "Manage in Users" emits `hv-open-app` (`detail.id='users'`) without leaving `/desktop` or closing Quotas, console without errors. **Compilation acquired on 2026-09-14**: `npm run check` 0 errors (2 real defects fixed: `t` not imported, `{subtitle}` not declared), `npm run build` emits `static/dist/assets/Quotas-*.js`, `cargo build` OK. Detail in the 2026-10-07 addendum of `CLAUDE.md`. | n/a (screen with no backend of its own — reads `GET /transfer-quotas/{type}`) | [ ] **still no integration file of its own** — the endpoint is covered by `transfer_quota.rs` (unit) and `explorer_integration.rs` (quota guards) | [x] `tests/e2e/quotas/quotas.spec.ts` — **38/38 green 2026-10-07** (Firefox, scratch server; reference count `npx playwright test --list --project=firefox` = **38**, `grep -cE '^\s*test\('` = **36**, the difference coming from the responsive loop 360/768/1920, 1 line → 3 tests). Read-only: loading, all/users/groups filters, "Unlimited" row when the direction is disabled, progress bar = `used/limit`, "Manage in Users" (`hv-open-app`), §10 security (no write, no token in the DOM/URL), edge cases (500 → error state, empty backend → empty state), console without errors, memory regression. **No dead selector found** — the spec postdates the Appliance migration of 2026-09-12 and already targets the `data-testid`, it passed as is. |
| 7 | Groups | [x] 2026-07-13 (+07-18, 07-21) | [x] `groups.rs` (5) | [x] `groups_integration.rs` (26) | [x] `groups.spec.ts` (61 — **+2026-09-23: 16.1 avatar contrast guard** (white initials ≥4.5:1 on the 6 stops of `$lib/utils/avatars.ts`, row tiles + detail); 60 before this addition to the grep, **63 at `--list`** (2 tests generated in a loop); recounted on 2026-09-14 via `grep -cE '^\s*test\('`, the file's method; incl. transfer quotas) ; **+2026-09-12: Appliance design migration — `PageLayout` (list rail / detail panel), `SectionCard`, `FormRow`, Tailwind-on-tokens, zero scoped CSS, selectors `data-testid="groups-*"`; the 860px breakpoint (stacking of the panels) is driven by `matchMedia` → `sidebarHidden`** |
| 8 | Dashboard | [x] 2026-07-13 (+07-15, 09-02) | [x] `handlers/dashboard.rs` (23 — `format_uptime` (6) + **+2026-09-07** `format_bytes`, `parse_size` (incl. fraction + locale comma, **bug fixed**), `calculate_individual_disk_score`, `calculate_disk_summary`, `calculate_disk_score`) | [x] `dashboard_integration.rs` (10, +2026-09-07 — **10/10** — auth split on the 15 read endpoints + widget-CRUD + widget-preferences, widget add/list/reorder/remove cycle, per-user preferences upsert + partitioning) | [x] `dashboard.spec.ts` (37 runtime tests, 12 categories — recounted on 2026-09-14 via `grep -cE '^\s*test\('`, the file's method; the entry said 39, an outdated number, and the "39/39 green" of 2026-09-07 therefore concerns an earlier state; **+2026-09-14: standardization of headers — 1.7 and 7.1 no longer expect the "Live" badge (decorative element removed with the subtitle) but verify `.hv-winhdr-title` alone, `.hv-winhdr-subtitle` and `[data-testid="dashboard-live"]` at `toHaveCount(0)`**; read-only, the screen has no mutating action) ; **+2026-09-12: Appliance design migration — `PageLayout`, 12-col bento-grid via Tailwind container queries, SVG (gauge/core/mem) kept, selectors `data-testid="dashboard-*"`, zero scoped CSS** |
| 9 | Storage | [x] 2026-07-17 (+09-06 test suites + "Virtual disks" re-scope; +09-07 `impeccable` layout redesign; **+2026-09-07 real test: (a) list/scrub/destroy a real mdadm + ZFS virtual disk — `PoolService::scan_pools` fixed (device_count + mount_point + usage via `df`); (b) full LUKS cycle create/lock/unlock/delete on `/dev/loopN` + real `cryptsetup` — 2 frozen bugs fixed: `handle_encryption_create` did not validate `req.device` (now guarded on `get_available_devices` `!in_use`), `encryption::erase` left the LUKS header (+ `wipefs -a`)**) | [x] `pools.rs` (24), `cache.rs` (13), `encryption.rs` (2), `snapshots.rs` (1), `handlers/storage.rs` (3 — `vd_name`, +2026-09-07) | [x] `storage_integration.rs` (24, +2026-09-07) — includes `GET/DELETE /api/storage/virtual-disks[/{name}]` + `/scrub` (auth split) + `test_encryption_create_rejects_a_device_that_is_not_available` (guards `get_available_devices` before `cryptsetup`); still no real virtual disk / LUKS volume via integration; **+2026-10-10: auth guard strengthened on `POST /storage/disks/{name}/smart/test` — `require_auth` → `require_admin` (launches a real `smartctl -t` on the host) + trace in the event journal; read/write matrix extended to the 3 SMART endpoints** | [x] `storage.spec.ts` (29 `test(` / 31 runtime) — the "Virtual disks" tab is covered (nav + count of 6 tabs); **34/34 green 2026-09-07**; no listing/destruction of a real virtual disk; **+2026-09-12: Appliance design migration — `PageLayout` (6 tabs), Tailwind-on-tokens cards, `Modal`/`Toggle`/`Alert` kept, selectors `data-testid="storage-*"`, zero scoped CSS** |
| 10 | RAID | [x] 2026-07-13 (+09-05 creation-only re-scope; **+09-07 layout redesign via `impeccable` — 3-step wizard re-skinned mono + `--hv-*` tokens, 3-dot stepper, per-level disk diagram, touch targets; markup/CSS only; `svelte-check`/`vite`/`cargo` OK; verified live step 1**; **+2026-09-07 real test on 6 loop devices: end-to-end mdadm + native Btrfs + ZFS creation/scrub/destruction via the API; 4 bugs fixed — non-`sudo` `mount`/`umount`/`blkid` in `disk.rs`, argument order `zpool create` (name before the vdev type → all ZFS creation failed), and the wrong disk count / mount point in `scan_pools` for mdadm+ZFS**) | [x] `disk.rs` (12 — create_raid validation + `parse_size`) | [x] `raid_integration.rs` (5, +2026-09-07 — auth guards on the 5 `/api/storage/raid*` endpoints: read = session, create/delete/rebuild = admin; `require_admin` rejects **before** the handler body so `mdadm` is never reached); still no real creation/destruction (host required) | [x] `raid.spec.ts` (41 `test(` / 43 runtime) **rewritten for the creation wizard** — **35 passed / 8 skipped 2026-09-07** (the 8 skips require ≥2 free disks on the host to reach step 2/3); real opt-in cycle `RAID_E2E_DEVICES` (create in RAID → see in Storage → destroy); **+2026-09-12: Appliance design migration — `PageLayout` + 3-step wizard in Tailwind-on-tokens (stepper, disk diagrams, capacity, recap), selectors `data-testid="raid-*"`, zero scoped CSS** |
| 11 | Health | [x] 2026-07-16 (**+2026-09-07 complete test suites — the last screen without coverage**) | [x] `health.rs` (13, +2026-09-07 — `determine_overall_status` precedence (incl. the empty case vacuously-`all()`), PascalCase serde of `HealthStatus` (the handler's lowercase mapping depends on it), `run_all_checks` → 8 checks in order + consistent aggregate, `run_check` dispatch + unknown name → `Err`, `check_storage` Healthy/Warning depending on `DATA_DIRECTORY`, bounded `check_memory` %, `check_network_interfaces` always lists `lo`) | [x] `health_integration.rs` (10, +2026-09-07 — `GET /api/health` **always responds 200** and reports the auth failure in the body (`success:false`, no `data`); valid **non-admin** session → `success:true`; 8 named checks + valid lowercase statuses + non-empty messages; `overall_status` = `status`; no leak of PascalCase status; aggregate consistent with its checks; `cpu_usage`/`memory_usage` ∈ 0..=100; never a 500; no credential/hash/token returned) | [x] `health.spec.ts` (34 `test(` / 36 runtime tests, 12 categories — **36/36 green 2026-09-07**; read-only; +2026-09-08: manual Refresh button removed, auto revalidation on focus (`onAppVisible`) — specs 1.6/3.1/3.2/7.1/8.2/10.2 rewritten; opening via the launcher + via the dock "Security" icon (`handleDockAppClick('health')`, historical mislabel, asserted); 8 cards; the global badge follows the worst check (read from the raw API); Refresh is GET-only without its own polling; 500/malformed payload → empty state, never a white window) ; **+2026-09-12: Appliance design migration — `PageLayout` + `EmptyState`, Tailwind-on-tokens, zero scoped CSS, selectors on `data-testid="health-*"`** ; **+2026-09-14: standardization of headers + moving a real reading** — 1.3 no longer expects a subtitle but `.hv-winhdr-title` alone + `.hv-winhdr-subtitle` `toHaveCount(0)`; the global status is no longer a header badge but the `data-testid="health-overall"` band at the top of the body, which keeps the former `data-testid="health-badge"`: 1.5 therefore verifies the band **and** the badge. The label goes through the renamed i18n key `health.overall_status` (ex-`health.subtitle`, now orphaned) — renaming rather than deletion to keep the parity of the 4 locales. Number of tests unchanged; **spec modified but NOT re-run** |
| 12 | Terminal | [x] 2026-07-13 (+ ~6 follow-ups up to 09-02, +09-06 Rust tests) | [x] `handlers/terminal_pty.rs` (13 — `is_valid_tab_id`, `resolve_tab_id`, `generate_ticket`, `PtyManager` ticket cycle, `kill_session` purges the tickets, cleanup no-ops) | [x] `terminal_integration.rs` (10 — admin auth matrix on `/ticket` + `/close` + `/cwd` + `/ls`, tab id validation, `close_tab` no-op) | [x] `terminal.spec.ts` (**81 runtime tests**, recounted on 2026-09-17 via `npx playwright test tests/e2e/terminal/terminal.spec.ts --list --project=firefox` — 77 by `grep -cE '^\s*test\('`, the difference of 4 coming from the tests parameterized in a loop; the entry said 74 before 2026-09-16) ; **+2026-09-17: `terminal-tabs` no longer scrolls vertically (`overflow-y-hidden` — a lone `overflow-x: auto` computes `overflow-y` as `auto`, and the 1px that the `min-h-11` leaves under its content box was enough; test 2.9) and `terminal-body` paints the same background as the xterm theme (test 2.10 — the container had stayed in `#1e1e1e`, so the sub-cell remainder of the `FitAddon` painted a black band under the last line)** ; **+2026-09-12: Appliance design migration — `WindowHeader` header (and not `PageLayout`, the entry was wrong) + xterm tabs + infobar in Tailwind-on-tokens, selectors `data-testid="terminal-*"`, zero scoped CSS** ; **+2026-09-12: client completion (ghost text + clickable Tab popup, `section 19`)** ; **+2026-09-16: the tab strip leaves the `WindowHeader` — `terminal-tabs` is now a window content bar, between the header and `terminal-infobar` (new test 2.8: the strip is not a descendant of `.hv-winhdr` and its vertical order places it between the two bars); the header receives its `title`; 3.2/3.4/7.3/7.4 repaired (outdated selectors, **already red before**) ; sections 1/2/3/4/7/8/9/10/11/12/13/14/15/16/17/18 re-run on a fresh instance — all green except 4.2/10.3/19.1/19.2, **proven pre-existing by `git stash` of the modified file** (4.2 monospace family; 10.3 "< 80 requests" is an artifact of the Vite dev server, which serves non-bundled ES modules; 19.1/19.2 = completion popup, untouched area)** |
| 13 | System journal | [x] 2026-07-18 (**+2026-09-07 real test: `system`/`syslog`/`auth`/`kern` source via real `sudo journalctl`/`tail`, unit/priority/search filters OK; 1 major bug fixed — the auto-cleanup scheduler wiped the active application log at each startup → "app" source permanently empty**) | [x] `handlers/journal.rs` (9, +2026-09-07 — `priority_text`, `is_app_log_filename`, `text_log_path` (whitelist / anti-traversal), `strip_target_prefix`, `parse_app_log_line` (timestamp+level+target / fallback), `seconds_to_iso`) | [x] `journal_integration.rs` (11, +2026-09-07 — **11/11** — auth split (read=session, `clear`+`cleanup-settings` PUT=admin), `/sources` lists `app`+`system`, `app` source read **for real** from `DATA_DIRECTORY/logs` (parsing + `search` filter + empty), unknown source → clean `{success:false}`, `clear` deletes only `homevault.log*`, default cleanup settings (on/90d) + round-trip + `interval_days≥1` clamp) | [x] `journal.spec.ts` (27 `test(` / 29 runtime tests, 12 categories — **28/28 green 2026-09-07**; read-only, never empties the real logs nor durably modifies the cleanup setting) ; **+2026-09-12: Appliance design migration — `PageLayout` (filter toolbar + status footer), priorities → tokens, selectors `data-testid="journal-*"`, zero scoped CSS** ; **+2026-09-14: standardization of headers — 1.2 no longer expects a subtitle but `.hv-winhdr-title` alone + `.hv-winhdr-subtitle` `toHaveCount(0)`; the real actions of the bar (auto-refresh, Clear, Cleanup settings) remain, only the decorative elements were removed. Number of tests unchanged; spec modified but NOT re-run** |
| 14 | Network | [x] 2026-07-14 (+07-15 ; **+2026-09-07: reads re-verified live; frozen `src/network.rs` fixed — *mutation* `ip`/`dhclient` wrapped in `sudo` (`ip_sudo`), `/etc/resolv.conf` + `/proc/sys/net/ipv6/*` toggles via `sudo tee` (`write_as_root`), DNS only rewrites the `nameserver` lines (`build_resolv_conf`, keeps `search`/`options`). netplan/interfaces persistence = deliberate follow-up**) | [x] `network.rs` (32 — +3 `build_resolv_conf`) | [x] `network_integration.rs` (23) | [x] `network.spec.ts` (43) ; **+2026-09-14: standardization of headers — 1.2 no longer expects a subtitle but `.hv-winhdr-title` alone + `.hv-winhdr-subtitle` `toHaveCount(0)`; the Diagnostics/Scan buttons remain (real actions), only the description line (`network.subtitle`) disappeared. Number of tests unchanged; spec modified but NOT re-run** ; **+2026-09-12: Appliance design migration — `PageLayout` (ethernet/wifi tabs), selectors `data-testid="network-*"`, zero scoped CSS** |
| 15 | VPN | [x] 2026-07-16 | [x] `vpn.rs` (14) | [x] `vpn_integration.rs` (22) | [x] `vpn.spec.ts` (39) ; **+2026-09-12: Appliance design migration — `PageLayout` (WireGuard/OpenVPN), selectors `data-testid="vpn-*"`, zero scoped CSS** |
| 16 | Explorer | [x] 2026-07-14 (+07-17, +09-06: **5 robustness bugs fixed** + 8 Explorer-extra features; compiled & tested **2026-09-07**) ; **+2026-10-13 (b): Rewind (per-file ZFS snapshot timeline), server-side recents, fuzzy search, marquee-select, virtualized listing, internal drag & drop, SMB/network discovery, ext4/exfat formatting** | [x] `browser.rs` (11 — +6 collision-strategy on 2026-09-07, +1 on 2026-09-13: `copy_dir_all_tracked` records only the created paths ; **+4 on 2026-10-13 (b): `fuzzy_score` — exact/prefix, non-contiguous subsequence, rejection of non-subsequence/empty, contiguous-and-short preference**), `config/explorer_config.rs` (4), `handlers/explorer.rs` (17 — +5 on 2026-09-07: `kind_for_root`, `resolve_dest`; +5 on 2026-09-13: `parse_range` — absent/suffix/clamping/416/malformed ; **+2026-10-13 (b): `smbtree` + Avahi/mDNS parsers and the `ext4`/`exfat` allow-list**), `media_ticket.rs` (4, 2026-09-13) | [x] `explorer_integration.rs` (**55** — **+6 on 2026-10-13 (b): `/explorer/recents` list/touch/clear, `/explorer/history`, `/explorer/network-discovery`, `/explorer/format` (auth split + `fstype` allow-list + non-available device refused)**; +11 on 2026-09-07 — `/roots` (roots + `kind`), `/save` (write/overwrite/outside-root/folder/giant payload), `/eject` (admin only + refusal of a non-mount-point), `/snapshots` (empty outside ZFS), favorites (add/list/remove), `?strategy=keep_both\|skip` on `/copy`) ; **+9 on 2026-09-13 — transfers v2: `Range` (206/416/suffix), `X-Content-Sha256` digest, download ticket (single-use, without session), thumbnail ticket (multi-use, without session), resumable upload (chunks round-trip → complete, offset mismatch 409, abort without debris), outside-root rename refused, admin-only delete — **49/49 green 2026-09-13** ; **+2026-09-12: `share_links_integration.rs` (11 — auth matrix, create/list, refusal of folder/missing file/invalid mode, non-admin `PathGuard` guard, public download + unknown/revoked/expired token, `signed_in` without/with session, owner-only + admin revoke, `/share/{token}` reachable via the real `create_app` router)** ; **+2026-09-13: `share_links_integration.rs` (12) — `Range` on `/share/{token}`** | [x] `explorer.spec.ts` (62 `test(` at the grep; **64 at `--list` on 2026-09-24** — +8.5 "Space activates a focused folder row, not just Enter" (W18), written but **not executed**) — **49/49 green 2026-09-07** ; **+2026-09-12: Appliance design migration — `PageLayout` (locations sidebar + breadcrumb + list/grid view + trash), selectors `data-testid="explorer-*"`, zero scoped CSS** + **section 15 "Explorer extras" added on 2026-09-08 (9 tests, 3 isolated runs `-g "15"` green on a fresh server)**: inline editor (the modal exposes Edit → pre-filled textarea), `POST /api/explorer/save` (writes / overwrites / rejects > 5 MiB), favorites (toolbar star ⇄ sidebar ⇄ server, opening from a new window), collision strategies `skip`/`keep_both`/`overwrite` on `/copy` and `/move` (via in-page `fetch`), Snapshots menu ("no snapshot" outside ZFS + endpoint), `/eject` (admin only + refusal of a non-mount-point), "Share this folder" hand-off → the Shares app opens with the pre-filled form. `apiDownloadLen` goes to `cache: 'no-store'` (the only change of a shared helper). **+2026-09-12: section 16 — link sharing (create → anonymous public GET → revoke → 401/400), `signed_in` link (anonymous 401 / session header 200), editor close guard (unsaved changes → confirm "Discard/Save")**. **+2026-09-13: standardization — sidebar refactored onto `WindowSidebarItem`/`WindowSidebarSection` (`.disk-btn`/`.disk-name` kept as test hooks), **icon-rail collapse** (`sidebarCollapseable` + `bind:sidebarCollapsed` → 64px rail instead of disappearing), status footer via `WindowFooter mode="status"` (folder summary + total size + live selection) + **drop of an OS file onto a sidebar destination (root/favorite/mounted disk → upload in place, `is-drop-active` ring)** ; tests **1.6** (collapse ⇄ expanded: `hv-winsidebar--collapsed` class, hidden labels, tooltips, width) and **1.7** (status footer + selection counter) added** ; **the same status footer (Puter `explorer-footer` pattern) is generalized to the data screens: `Syncthing` (folders/devices/connected), `Storage` (disks/virtual/partitions/removable), `Shares` (accounts per protocol), `Downloader` (active/completed), `Network` (interfaces/links/IP)**.** **+2026-09-13 (transfers v2, to re-run live)** : download via ticket → anchor (no more `fetch`→`blob`), thumbnails authenticated by directory ticket (no more `?token=`), resumable chunked upload. **Not covered**: navigation *inside* a real ZFS snapshot (host required). **Pre-existing flakiness/stale locators (not related to this pass)**: `navigateTo`/`currentPath` rely on `.bc-item` (class absent from the current breadcrumb → intermittent sections 2/3/6/8/10/11) and 1.3/1.4 use obsolete selectors (nested `[data-testid="explorer-page"]`, `…"]2`). Re-run on a fresh server or with `-g`. |
| 17 | Shares/Protocols | [x] 2026-07-13 (+07-18 real SFTP ; **+2026-09-07 end-to-end real test: real SMB share create/toggle/update/delete + service start/stop ; SFTP key → Linux account + group, `setfacl` ACL grant on the root dir, `rebuild_jails` → bind-mount + `/etc/fstab`, revoke, `purge_user`. 5 bugs fixed — SMB toggle no-op on the last section of `smb.conf`, writing `/etc/homevault/*.json` without `sudo`, all `setfacl` without `sudo`, revoke that left the default ACL, `set key` that did `useradd` for a nonexistent name**) | [x] `permissions.rs` (10), `shares.rs` (12), `sftp.rs` (6) | [x] `shares_integration.rs` (18) | [x] `shares.spec.ts` (51) ; **+2026-09-14: standardization of headers — 1.2 no longer expects a subtitle but `.hv-winhdr-title` alone + `.hv-winhdr-subtitle` `toHaveCount(0)`; the recap "%total% shares · %active% active" (`shares.summary`) disappeared with the subtitle — the protocol tabs remain. Number of tests unchanged; spec modified but NOT re-run** ; **+2026-09-12: Appliance design migration — `PageLayout` + protocol tabs, `SectionCard`-like forms, `PathField`/`Toggle`/`Modal` kept, selectors `data-testid="shares-*"` (modal → `[data-slot="dialog-content"]`), zero scoped CSS** |
| 18 | Cloud Sync (rclone) | [x] 2026-07-14 (+09-06 test suites) | [x] `handlers/rclone.rs` (14 — `build_rclone_path`, `get_provider_description`, `rclone.conf` parser, `MountRequest` defaults; **+2026-10-12**: 3 on `rclone_config_path()` — honours a non-empty `RCLONE_CONFIG`, ignores an empty one, falls back to the default when none exists — and 1 proving the parser and the handlers resolve the same file) | [x] `rclone_integration.rs` (17 — +2026-10-12: `config_endpoints_use_the_rclone_config_override`, `save_config_writes_the_overridden_file_and_keeps_a_backup` and `add_list_delete_remote_round_trip_on_the_overridden_file`: the full admin cycle now runs for real against `RCLONE_CONFIG` pointed at a tempdir, which the hardcoded `/etc/rclone.conf` made impossible) | [x] `cloudsync.spec.ts` (24 — "credentials leak" guard on `GET /config` ; real remote cycle **opt-in** `CLOUDSYNC_E2E_WRITABLE_CONF=1`) ; **+2026-09-12: Appliance design migration — `PageLayout` (3 tabs + sync/config modals), selectors `data-testid="cloudsync-*"`, zero scoped CSS** ; **+2026-09-22: sync jobs persisted server-side (`sync_jobs`, migration 036) — the frontend abandoned localStorage (`api.listSyncJobs/createSyncJob/deleteSyncJob/runSyncJob`)** |
| 19 | Docker | [x] 2026-07-16 (+ ~8 follow-ups up to 09-04) | [x] `docker.rs` (9), `compose.rs` (6), `app_catalog/*` (22 — mod 7, manifest 8, store 3, updates 4), `app_proxy/*` (2) | [x] `app_catalog_integration.rs` (10), `app_proxy_integration.rs` (2), `compose_integration.rs` (6) | [x] `docker.spec.ts` (51, incl. real opt-in cycle §13 on a disposable `hello-world` container, + "Edit" compose button §4.8) ; **+2026-09-12: Appliance design migration — nav rail + metrics + 6 tabs (incl. catalog + compose) + inline modals, selectors `data-testid="docker-*"` (modal → `[data-slot="dialog-content"]`, toggle → `[role="switch"]`), zero scoped CSS** ; **+2026-09-13: nav rail + metrics section refactored onto `WindowSidebarItem`/`WindowSidebarSection` (`role="tab"`, `aria-selected`, roving `tabindex`, `docker-nav-item` kept)** ; **+2026-10-13: catalog phase B (feature parity with the reference design — `implements` alternatives, `[[environment]]` settings, `min_homevault_version`, install-review, update-all queue, per-app logs) + MCP `apps.*` sharing the install core — `docker.spec.ts` 51 → 56 `test(` / 58 runtime (§15: review shape, badge/button states, dialog resolve-and-cancel, update-all + logs gates, bad-alternative rejection), full suite 55 passed / 3 conditional skips / 0 failed (Firefox, scratch `:8099`). Two adjacent fixes in the same run: 4.5 was red on a stale selector (the stores toggle never exposed `aria-expanded`, missing even at HEAD — now a native `hv-btn-secondary` button with `aria-expanded`, same skin) and 5.1 raced the async `compose/available` probe (now waits for its response before reading `disabled`)** |
| 20 | Syncthing | [x] 2026-07-16 (+09-06 test suites) | [x] `handlers/syncthing.rs` (11 — `extract_api_key`, deserialization of the REST responses + `certName`/`deviceID`/`myID` aliases) | [x] `syncthing_integration.rs` (5 — **admin** guard on the 10 endpoints, 422 body, `status` shape) | [x] `syncthing.spec.ts` (21 — "unavailable" state, guards, daemon-off error path ; live read **opt-in** `SYNCTHING_E2E=1`) ; **+2026-09-12: Appliance design migration — `PageLayout` (3 tabs), selectors `data-testid="syncthing-*"`, zero scoped CSS** ; **+2026-09-13: 2-pane master-detail for the Devices/Folders tabs (compact list on the left, detail pane on the right; `syncthing-device-list-row`/`syncthing-folder-list-row`, `ResizeObserver` split < 720px → a single pane) ; the Overview tab remains unchanged ; `syncthing-btn-primary`/`syncthing-empty`/`syncthing-stats` kept (E2E unchanged)** |
| 21 | Downloader | [x] 2026-07-16 (+07-17, +09-06 test suites) | [x] `handlers/downloader.rs` (15 — serde mapping of the real aria2 camelCase format ↔ snake_case JSON returned to the frontend ; request bodies ; `ApiResponse` envelope) | [x] `downloader_integration.rs` (8 — admin guard on the 10 endpoints, 422 body validation, safe `GET /status` path) | [x] `downloader.spec.ts` (40 `test(` / 42 runtime — incl. a **real download** of a file served by the app, through to `complete` on disk, then deletion) ; **+2026-09-12: Appliance design migration — `PageLayout`, selectors `data-testid="downloader-*"`, zero scoped CSS** ; **+2026-09-13: 2-pane master-detail — compact list of the downloads (active + completed) on the left, detail pane of the selected task on the right (re-resolved by gid at each poll to follow live progress ; `ResizeObserver` split < 720px → a single pane) ; `downloader-card`/`downloader-card-name`/`downloader-action-danger`/`downloader-section-title` kept on the rows (E2E 10.x unchanged)** |
| 22 | Backup (Restic) | [x] 2026-07-14 (+07-17, +09-06 test suites ; **3 bugs found and fixed** incl. the entire page dead in the browser, cf. below) ; **+2026-10-13 (b): backup reminders banner, split-view restore dialog with a debounced pre-flight space check, floating progress island** | [x] `restic_scheduler.rs` (12 — `compute_next_run` + `normalize_crontab_dow`), `restic.rs` (8 — `parse_backup_summary` NDJSON, snapshot deserialization, `restore_size`), `handlers/restic.rs` (**+3 on 2026-10-13 (b): `days_since`, `parse_df_available`, and the restore-plan size/space comparison**) | [x] `restic_integration.rs` (**17** — **+3 on 2026-10-13 (b): `GET /restic/reminders` (per-repo `last_backup_at`/`days_since`/`stale`, auth-guarded), `GET /restic/restore-plan` (size vs. available, auth-guarded), and the `include_password` guard still holds**; auth split, schedule + retention cycles 100% DB, never `restic`) | [x] `backup.spec.ts` (25 — incl. a **real restic backup** in a disposable repository + verification of the cron day of the week ; **+2026-09-22: 14.3 — "Restore" regression audit: the modal acts on the snapshot whose button was clicked (2nd row), never the first anymore — restic API entirely mocked in the browser, runs without restic**) ; **+2026-09-12: Appliance design migration — `PageLayout` (repos / creation / detail, snapshots-jobs-schedules, retention policies), selectors `data-testid="restic-*"`, zero scoped CSS** ; **+2026-09-13: the snapshots/jobs/schedules tabs go from the hand-made inline bar to the single tab row of the `WindowHeader` (`tabs`/`bind:activeTab` on `PageLayout`, counters kept)** ; **+2026-09-13: 2-pane master-detail — repository cards on the left (420px column, stats at the top), repository detail / creation form on the right (`ResizeObserver` split < 720px → a single pane), `restic-repo-name` still rendered on the list pane** ; **+2026-10-13 (b): the restore dialog is now split-view with a pre-flight space check, and the reminders banner + floating progress island are new. Spec not re-run** |
| 23 | Rescue | [x] 2026-07-17 (+07-17, +09-05 ; **+2026-09-07: 1st real run on 4 loop devices + disposable RAID1 — ddrescue clone / testdisk analyze / photorec / mdadm diagnose+repair end to end ; 6 frozen bugs fixed — sudo-wrap ddrescue/testdisk/photorec, `lsblk -P -b` (model/transport/removable/size), `/dev/md127:` header in `mdadm_diagnose`, "clean, degraded" health→healthy, `parse_testdisk_output` offsets, `testdisk_rebuild` no-op + anti-hang**) | [x] `rescue/validator.rs` (8) + `rescue/service.rs` (8 — `parse_ddrescue_mapfile` ×5, `parse_testdisk_output` ×3) | [x] `rescue_integration.rs` (28) | [x] `rescue.spec.ts` (53) ; **+2026-09-14: standardization of headers + moving of the availability badges** — 1.2 no longer expects a subtitle but `.hv-winhdr-title` alone + `.hv-winhdr-subtitle` `toHaveCount(0)` ; 1.3 is REWRITTEN: the band of 4 badges that lived in the header disappeared (decorative element), availability is now on the tool's card (`SectionCard`'s `actions`), one per tab — the `data-testid="rescue-tool-badge"` is kept but moved to a wrapping `<span>`, without which `Badge` could not guarantee its transfer via `$$restProps`. The test therefore verifies: no badge in `.hv-winhdr`, exactly one on the active tab. Number of tests unchanged; spec modified but NOT re-run** ; **+2026-09-12: Appliance design migration — `PageLayout` (4 tabs) + `SectionCard`, `PathField` kept, selectors `data-testid="rescue-*"`, zero scoped CSS ; the legacy components `Card`/`CardHeader`/`CardContent`/`ProgressBar`/`Divider` (last consumer) removed from `theme/`** |
| 24 | AI Assistant (Chat) | [x] 2026-07-13 (+09-02, +09-06 test suites ; **3 "working chat" fixes** ; **verified live with a real Ollama 0.33.3 + gemma3:1b**) | [x] `handlers/chat.rs` (13 — localized `system_prompt`, `session_error_message`, `reset_ai_model`, `recommend_models` (extracted, pure), Ollama deserialization) + `mcp.rs` (7 — scopes, permission parsing, file guard incl. denylist, token mint/verify, tool filtering) | [x] `chat_integration.rs` (9 — session guard on the 7 endpoints, admin guard on pull/model-set, model-set cycle) ; `dependencies.rs` (+1 — `install.sh` does install Ollama, **strengthened on 2026-09-23**: the test also requires `systemctl is-active --quiet ollama` and the probe `http://127.0.0.1:11434/`, the installer's final check controlling binary + active unit + API as the Ollama Linux doc requires) ; **+2026-10-13: `mcp_integration.rs` (11 — `/api/mcp/*` admin matrix, token lifecycle incl. disabled-server 401, permission round-trip + widening rejections, RPC auth/protocol errors, granted-tools calls incl. file-scope allow/deny, catalog install-review/update-all/logs, bad-alternative install rejection)** | [x] `chat.spec.ts` (20 `test(` / 22 runtime — Ollama off → clean fallback + "AI offline" badge ; **+2026-09-12: Appliance design migration — side history + message thread in Tailwind-on-tokens, selectors `data-testid="chat-*"`, zero scoped CSS** ; **suite `CHAT_E2E=1` 22/22 with real Ollama**, incl. §13.1 real streamed conversation) ; **+2026-10-13: §15 External agents (6 tests — admin toggle, panel content, kill-switch flip + restore, token create-list-revoke, non-admin invisibility + bearer gate, secret hygiene), full suite 27 passed / 1 opt-in skip (Firefox, scratch `:8099`). One real UI bug found by 15.4: `api.mcpStatus()` used `{ttl: 0}`, which is falsy and fell back to the 5 s default TTL — the token list stayed stale after create/revoke; now `{cache: false}`** |
| 25 | Widgets | [x] 2026-07-16 (+07-18, 07-19) | n/a — DB-only handlers (`monitor_layout`, `active-sessions-count`) | [x] `widgets_integration.rs` (6, +2026-09-07 — **6/6** — `GET/POST /api/monitor-layout`: session guard, `null` before saving, blob round-trip, upsert = 1 row, partitioning by account ; `GET /api/security/active-sessions-count`: admin guard, deduplication per user + expired sessions ignored) ; **+2026-09-14: 6 → 8 — `refreshMs` lives in the same `monitor-layout` blob (no migration, the handler stores the JSON verbatim) ; a legacy blob without `refreshMs` round-trips to the byte without the server inventing one, and a new value persists while leaving its neighboring keys intact** ; **+2026-09-22: 8 → 9 — `GET /api/widgets/update-info`: session guard + without `UPDATE_CHECK_URL` the widget does not fabricate an update (`has_app_update=false`), version comparison unit-tested (`is_newer_version`)** | [x] `widgets.spec.ts` (21 `test(` / 23 runtime, 9 categories — **23/23 green 2026-09-12** ; snapshot+restore of the layout ; toggle of a tile → desktop monitor panel appears/disappears, footer, Reset, `POST /monitor-layout` persistence) ; **+2026-09-12: Appliance design migration — `PageLayout` + `WindowFooter`, Tailwind-on-tokens tiles, zero scoped CSS, selectors `data-testid="widgets-*"`** ; **+2026-09-12: 2.2 consistent geometry of the 11 panels (card radius 12px, header 48px, close button 44px not clipped, widths 280/300/320 via `--hv-monitor-w*`)** ; **+2026-09-15: panel header = app bar — `WidgetHeader.svelte` goes from "blue icon + label + action/close" to the `WindowHeader` anatomy: 44px white bar (`--hv-widget-header-h` 48 → 44), 28px accent badge (`--hv-winhdr-*`, color per monitor = that of `WIDGET_DEFS`, white glyph) + 14px/600 title, **without a close/action button** (visibility is managed in Settings > Widgets) ; the 11 `desktop.close_*_monitor` i18n keys removed ×4 locales ; 2.2 rewritten (header 44, badge 28 centered, no more close button) — 23/23 green** |
| 26 | Settings (core) | [x] 2026-07-16 (+ ~5 follow-ups up to 09-04 ; **+2026-09-07 real Firewall test: real `ufw enable`/`disable` cycle (port 22 protected), `{confirmed}` lock, 403 on mandatory rule, rule add/delete. 1 bug fixed — `firewall.rs::get_status()` read the English string `Status: active` → on a non-English locale (`fr_BE` → `État : actif`) the firewall always displayed "disabled"**) | [x] security cards (`firewall.rs` 9, `antivirus.rs` 8, `ransomware_watcher.rs` 7, `ransomware.rs` 3) ; `snmp.rs` (**5** — +2026-09-15: parser rewritten on the **numeric** OIDs (`snmpwalk -On`) ; the old one matched the symbolic names `IF-MIB::ifDescr` which do not exist without the MIB files → interfaces/storage always empty ; new tests `parse_interface_stats`/`parse_storage_stats`/`split_oid_line`/`normalize_mac` on real numeric outputs), `ups.rs` (**7** — +5 on 2026-09-15: `parse_ups_conf`/`upsert_ups_section`/`remove_ups_section`/`validate_ups_entry` for the device sections of `ups.conf`), `quota.rs` (7 — `bytes_to_quota_kb`/`format_size_kb` only ; the group quota tests removed with the feature) ; **core (2026-09-08): `handlers/settings.rs` (13 — `parse_version`/`is_newer_version`, `sanitize_hostname`, `UpdateFrequency` default = `Manual` (G2), appearance defaults, `normalize_accent` which rewrites the legacy accent `#4a90d9` → `#1E62AE` §1.4.3) + `settings_persistence.rs` (7 — `export()` masks `smtp_password`, `import()` round-trip, re-import of a masked export does not lose the password)** | [x] `security_integration.rs` (18) ; `ups_integration.rs` (**4**, 2026-09-15 — `/ups/config` GET/POST/DELETE: without token → 401, invalid token → 401, non-admin → 403, admin GET → 200 ; the admin write path is covered live only) ; **core: `settings_core_integration.rs` (13, 2026-09-08 — admin split on reads+writes, `/settings/language` any session, `/settings/public` without auth, appearance round-trip (accent/density/wallpaper), `GET /settings/export` masks `smtp_password`, export→import, `reset` restores the defaults (incl. G2), notification threshold clamp)** | [x] `settings.spec.ts` (94 `test(` / 97 runtime — 77/77 green on 2026-09-09 ; **+2026-09-12: Appliance design migration — `PageLayout` (category rail) + auto-save cards in Tailwind-on-tokens, embedded security/TLS/monitoring cards kept, selectors `data-testid="settings-*"` (16.5 rewritten for the standard PageLayout anatomy), zero scoped CSS** (full run) ; new 2.5 + 12.2 and sections 1/2/12/16 re-verified green against the rebuilt binary ; **(a) sidebar grouped 12 → 7 categories** (email→Notifications, weather→Appearance, updates/info/data/privacy→System) ; `openCategory()` selects by label, not by index ; **(b) removal of the "Save" button — auto-save per group (700 ms debounce)**: section 15 "Save All" → "Auto-save" (15.1 a single PUT of the modified group, 15.2 persistence after reopening, 15.3 header status, 15.4 no change = no PUT), 21.3 → coalescence of rapid edits, 1.2/1.3/4.6/9.4 adapted ; test 11.2 removed — `.settings-privacy-link` = dead CSS in the frozen `Settings.svelte` ; 4.3 `loading=lazy`→`decoding=async`, 4.2 16→15 thumbnails ; **4.4/4.5/18.3: wallpaper thumbnail reactivity bug FIXED on 2026-09-09** — `isWallpaperSelected()` (helper reading `$wallpaperStore`) only refreshed `class:is-selected`, not `aria-checked` nor the ✓ badge ; replaced by a reactive `$: selectedWpUrl` — the 3 update on click) ; **+2026-09-09: test 2.5 added (78) — no card truncates its content ; layout bug FIXED — `.set-content` is a flex column and each card has `overflow:hidden`, which brings its minimum flex size to 0 → flexbox crushed the tall cards and their content was cut ; `.set-content > :global(.settings-card) { flex-shrink: 0 }`** ; **+2026-09-09: test 12.2 added (79) — in Security, no more manual path entry: each folder field is a read-only display + "Select a folder" button that opens `FloatingPathPicker` (floating movable panel, non-modal, `use:portal` to `<body>`, closes on Escape)** ; **+2026-09-09: wallpaper display bug FIXED — `.settings-wallpaper-thumb` did not have `height:auto`, the `<img height="200">` attribute fixed the height → thumbnails in distorted vertical bands on a narrow grid ; **+ box-shadow removed from the thumbnails** (the `--hv-shell-shadow-card` bled ~24px and made the rows look stuck/overlapping ; selection = `inset` ring that does not overflow) ; 2-column grid below 560px, gap 12px everywhere ; test 4.7 extended (column gap == row gap, no external shadow) ; **8th category "Widgets"** added, the Weather widget moved from `appearance` to `widgets` (`settings.widgets_title`/`_subtitle` ×4 locales) ; `settings.spec.ts` CATEGORIES/GROUP_OF/GROUP_LABEL + test 1.4 "8 entries" adapted** ; **+2026-09-09: "Disk quotas" card (per group) + the entire `/api/quotas` backend REMOVED** — `QuotaCard.svelte`, `handlers/quotas.rs`, `storage/quota_repo.rs`, `tests/quotas_integration.rs`, the group methods of `QuotaService`, `create_group_with_quota`/`delete_group_cascade` (0 caller) and the i18n keys `settings.disk_quota_*`/`quota_*` ; category renamed "Monitoring and storage" → "Monitoring" ; test 13.1 asserts the absence of the quota card ; quotas are managed from Users/Groups**), `security.spec.ts` (30) ; **+2026-09-09: "HTTPS / TLS" card (Security) — built-in opt-in HTTPS (self-signed `rcgen` + provided certificate, HTTP→HTTPS redirect, conditional HSTS). Rust: `src/tls.rs` (7 — effective port, port/path validation, self-signed generation + introspection, idempotency) + `src/handlers/tls.rs` (1) + `tests/tls_integration.rs` (8 — admin guards 401/403/200, rejection of port==HTTP, manual mode missing paths/`..`, self-signed persists + `requires_restart`, `/regenerate` writes the cert, real PEM pair accepted, no key leak). Live verified: `https://:8443` → 200, HSTS present in HTTPS / absent in HTTP loopback, 301 redirect outside loopback, SAN = localhost/hostname/homevault.local/LAN IP. E2E `settings.spec.ts` 12.3–12.6 added (card loading, field reveal, port==HTTP → Save disabled, no key in URL/localStorage)** ; **+2026-09-13: category rail refactored onto `WindowSidebarItem` (icon-rail collapse `sidebarCollapseable` + `bind:sidebarCollapsed`, `settings-nav-item`/`set-nav-active` kept)** ; **+2026-09-14: complete re-skin on the Figma mockup `Untitled` node `16-6187` — 2-line 208px rail (`WindowSidebarItem variant="rich"`: 28px badge tinted at 9%, flat fill + white glyph + 4×20 bar on the right when active), content header (36px badge + title + subtitle), and shared vocabulary section + card + row (`components/settings/vocabulary.ts`) applied to the **8 categories** ; the 6 embedded cards (TLS, Antivirus, Anti-Ransomware, Firewall, UPS, SNMP) **rebuilt** on this vocabulary with frozen behavior (class/id hooks kept) ; new `theme/Switch` (real `input[type=checkbox]` in `.settings-toggle-label`, 40×22 track, 16px knob) which replaces `Toggle` on the **whole** screen, TLS card included (its 2 toggles were the only 2 callers of `Toggle` in Settings ; `theme/Toggle` remains the `role="switch"` button primitive of the other apps — Shares, Docker, Restic, Storage) — the `.hv-toggle-switch` (44×24, 20px knob, brand blue) that they targeted no longer had a producer, hence the pre-existing failure of 12.3/12.4/12.5, rewritten on `flipSwitch()` ; page-scoped palette `--hv-settings-*` + 8 accents `--hv-settings-cat-*` in `tokens.css` ; **Widgets** category wired to the real thing — the 11 monitors of `WIDGET_DEFS` and a refresh interval persisted in the `monitor-layout` blob (`refreshMs`, **zero migration**: `POST /api/monitor-layout` stores the JSON verbatim) ; `settings.spec.ts` **80 → 95 tests** (11 new in §23, `--list` = 95 — `grep -cE '^\s*test\('` counts 92 because line 1128 generates 4 tests per loop ; the 11 groups of dead selectors of §16 and 2.5/12.2 updated for the new shell), `widgets_integration.rs` 6 → **8** (`refresh_ms` round-trip of a legacy blob + persistence of a new value)** ; **+2026-09-15: removal of ALL auto-save — a single "Save" button in the rail footer (`[data-testid="settings-save"]`) empties each modified group via the shared store `stores/settingsDraft.svelte.ts` (`draft.*` draft + JSON baseline per group + sequential `saveAll()`) ; the 6 embedded cards lose their individual save buttons (their editable fields are in the same draft, the *actions* — scan, cert regeneration, rule add/delete — remain immediate) ; autostart/AI/widgets/weather go to the draft, pushed only on clicking Save ; header pill `settings-save-status` (unsaved / in progress / failure) ; E2E rewritten: 1.3, 3.4, 4.6, 5.3, 9.4 (comment), 12.3, 12.5, §15 "Auto-save" → "Save button" (15.1 the change alone does not PUT, 15.2 persistence via button + reopening, 15.3 in-flight status, 15.4 no PUT without editing, **+15.5 button disabled during saving**), 21.3 (no PUT before Save, one after), 23.7/23.8 (widget/interval toggles: staged then POST on click) ; +1 test (95 → 96)** ; **+2026-09-15 (SNMP/UPS monitoring): 3 startup WARN removed (`snmp.rs`/`ups.rs`: the `is_available()` predicates no longer log at warn — `debug!` ; `ufw` modeled presence-only in `dependencies.rs`, cf. Firewall guide). SNMP parser rewritten (see unit tests) + `snmpd` view extended to interfaces/storage ; `SnmpCard` card aligned on the backend fields (`bytes_in`/`bytes_out`, `mount_point`, `total`/`used`/`usage_percent`) + `formatBytes`. UPS: NUT configured in auto-detection (`MODE=standalone` + `usbhid-ups port=auto`, no model id hardcoded) by `install.sh` ; a UPS configured but not connected logs at `debug!`. Verified live on the server: `snmpd`+`nut-server` active, `/api/snmp/{availability,info,interfaces,storage}` and `/api/ups/{availability,list}` OK.** ; **+2026-09-17: `settings.spec.ts` **96/96 green** against the rebuilt binary (Firefox, `--project=firefox`) ; the only remaining red, **23.5** (section labels as micro-labels, `<= 12px`), is fixed — three sizes written `text-[var(--hv-text-xs)]` without the `length:` type hint compiled to `color:` and the labels inherited the 14px of their parent (detail, scope and verification pitfalls in the section "third bug of the same family" below). `npm run check`: **7793 files, 0 errors, 0 warnings**. The full run was killed by the kernel at 91/96 (machine memory pressure, not a failure) ; the last 5 (23.7–23.11) were re-run alone and pass — **5 passed (1.0m)**. **+2026-09-23: the "Save" button becomes the single save indicator** — the status pill visible above the button and the caption "N unsaved change(s)" below it are removed (component `SettingsSaveStatus.svelte` removed, import + footer snippet cleaned in `Settings.svelte`) ; the button exposes `data-state` = `clean` (blue `--hv-primary`, "Save") → `dirty` (red `--hv-error`, "Save changes") → `saving` (blue, disabled, "Saving…") → `saved` (green `--hv-success`, "Saved", held 2.5 s then back to blue) / `error` (red, "Not saved") ; the label and the icon **always** change with the color (WCAG 1.4.1 ; white on the 3 backgrounds: blue 6.15:1 / red 5.73:1 / green 5.14:1 — blue follows the `--hv-primary` accent re-hydrated at boot, whose shipped default is `#1E62AE` since 2026-09-23) ; an edit made during the green flash cancels it, a failing group parks the button on `error` (never "Saved"). `settingsBtnSuccess` added to `vocabulary.ts` (one skin per state, never two `bg-` on the same element) ; keys `settings.save_changes` / `settings.save_error` ×4 locales replacing `settings.unsaved_changes` ; `settings-save-status` kept but as **sr-only `role="status"`** (screen readers hear each state change, the rail displays only one element). E2E rewritten: helper `saveSettings` waits for `data-state = saved|clean` (a failure parks on `error` and therefore fails the wait), **1.3** on `data-state` + presence of the sr-only line, **15.3** verifies the 3 colors (`toHaveCSS` on RGB **read in the page** — `--hv-primary` is re-hydrated from the accent, the default token does not match the render) **and** the labels in blue → red → in flight → green → blue (pointer moved away before each background assertion, otherwise the hover reads `-*-hover`), **14.5** targets `.settings-data-actions .settings-btn-danger` (the rail button carries the same skin when a change is pending — strict mode otherwise). **96 tests unchanged** ; `npm run check` 0 errors. **2026-09-23 result: 96/96 green (9.4 min, Firefox, scratch instance `:8099`)** — the first shot was entirely red (96/96) because the Playwright binaries were absent from the machine (`npx playwright install firefox` required), the second 94/96 with two reds **outside the change**: **15.2** revealed a real cache bug (`put('/settings/<group>')` did not evict the `GET:/settings` aggregate with a 5 s TTL that the window re-seeds on reopening → fix `invalidateForMutation()` in `api.ts`, now covered by 15.2) and **15.3** compared against the hardcoded RGB of `tokens.css` whereas `App.svelte` re-hydrates `--hv-primary` from the accent (`#4a90d9`) → the RGB are now read in the page. **+2026-09-23 (standardized contrast)**: the default accent goes from `#4a90d9` (3.34:1 behind white **and** ≤3.34:1 as text on light backgrounds → §1.4.3 failure in both directions) to `#1E62AE` = `--hv-primary` (6.15:1 / 4.99–5.68:1), rewritten at each entry by `DEFAULT_ACCENT` + `normalize_accent` (default, persisted load, import, `PUT /settings/appearance` ; the accent has no editing UI, no operator choice is overwritten) ; brand gradient `--hv-app-accent-grad`: light stop `#4b8ad4` (3.57:1) → `#3275C4` (4.69:1) ; `theme-color` + manifest aligned ; **+1 unit test** (`normalize_accent`) and **+1 integration test** (legacy PUT → `#1E62AE`, aggregate included) → `settings.rs` 12→13, `settings_core_integration.rs` 12→13, **E2E settings 96 → 97 —
executed on 2026-09-23: 97/97 green (new guard **17.5**: the 8 category tiles of the rail
measured in both states — glyph/tint at rest, white glyph/flat fill when active — ≥3:1 §1.4.11,
RGB read in the page). **Palettes extended in the same pass**: avatars centralized in `$lib/utils/avatars.ts` (single source — `#4B8AD4`/`#8b5cf6` → `#3275C4`/`#7C3AED`, **white** initials also in Groups which painted the dark ink → 6/6 stops ≥4.5:1 ; guard `groups.spec.ts` **16.1**), disk ring + legend Storage (`#14b8a6`/`#f97316` → `#0F766E`/`#C2410C`, ≥3:1 §1.4.11 on white ; the "unpartitioned" gray `#e5e7eb` remains the *empty* value, next to its text label), Chat tile (`#14b8a6` → `#0d9488`, white glyph 2.49→3.74:1), **category accents** (the flat tile = state marker of the control, §1.4.11): widgets `#A78BFA`→`#7C3AED` (2.72→5.70), notifications `#F59E0B`→`#B45309` (2.15→5.02), ai `#06B6D4`→`#0E7490` (2.43→5.36), monitoring `#10B981`→`#047857` (2.54→5.48) — the other 4 (\\*-500) already passed (≥3.32:1), and **Login/Wizard scale** (l. 1): `--hv-login-accent` `#6366F1`→`#4F46E5` (4.47→6.29 as text), `accent-deep`→`#4338CA` (6.29→7.90), `soft-border` `#A78BFA`→`#7C3AED` (2.72→5.70 — it is also the text of the completed steps), logo gradients (stops 4.47/2.72 → 6.29/5.70 under the white digit) and button (4.47→6.29), focus ring following the new hue, guard `login.spec.ts` **13.6** → suite 94→**95** (`--list`) / 90→**91** (grep). |
| 27 | VirtualMachines | [x] **2026-09-10 — verified live** against a real Incus 6.0.0 (host with `qemu-system-modules-spice` + `incus admin init`) ; the only uncovered point: boot of a real Windows ISO (no ISO at hand). **2026-09-09: app rewritten from the ground up** (frontend↔backend contract realigned ; general-public wizard with **2 modes**: (a) curated OS catalog + presets + account seeded by cloud-init (SHA-512 hashed password, never in clear), (b) **installation from a Linux/Windows ISO** — file picker, empty VM, noVNC graphical screen via `raw.qemu -vnc 127.0.0.1:…`, Windows = NVMe disk + TPM 2.0 + Secure Boot + optional VirtIO drivers ISO) ; real waiting for the Incus operations ; tracked creation job ; xterm shell console (`tokio-tungstenite`) ; Images/Networks tabs removed. **Verified live end to end** against a real Incus (after `apt install qemu-system-modules-spice`): (a) containers — creation via the wizard/API, real xterm shell console, create/restore/delete snapshots, resize, deletion ; (b) **Linux ISO VM** — wizard → `create` → Incus creates the empty VM, `raw.qemu -vnc 127.0.0.1:20` + TPM/NVMe/SecureBoot applied, **the VM starts**, the Alpine ISO boots ; **the noVNC screen displays the Alpine boot console live in the browser** (canvas 1280×800, relayed RFB handshake, Ctrl+Alt+Del handle) ; `delete` stops then removes the VM. **1 bug found and fixed live**: Incus sends `state.disk: null` on a freshly started VM → `#[serde(default)]` failed (`invalid type: null, expected a map`) and broke the whole `/api/vm/list` ; helper `null_default` added (covers `state.disk` + `config`/`devices`/`expanded_devices`). Fixes to `install.sh` + `dependencies.rs` (spice package) added. **2026-09-10 — "fix everything" hardening**: (A) OVMF Secure Boot check before creation (`OVMF_VARS_4M.ms.fd` **confirmed present** on Ubuntu) + **TPM auto-repair** — if a VM with `vtpm` fails to start (Incus 6.0.0 does not wire swtpm), the job deletes the failed VM (waits for the op) then recreates it without TPM with a fresh VNC display → Windows-10-capable VM that starts ; (B) creation job turned into generic "preparation" ; (C) `ensure_readable_by_qemu` — `chmod o+r` on the ISO + `o+x` on each parent folder, error naming the blocking folder ; (D) `RESERVED_DISPLAYS` (mutex, 60 s reservation) against display collision on concurrent creation + auto-repair of the VNC port at `start` + 45 s timeout on the Incus requests (`/api/vm/list` can no longer freeze) ; (F) **ISO library** — download from a link (`POST /api/vm/isos/download`, progress job), "VirtIO drivers" button (`fedorapeople.org/.../virtio-win.iso`), `safe_iso_name`, deletion. **Verified live**: concurrent Linux + Windows ISO creation — both VMs RUNNING, displays 21/22 without collision, `win1` auto-repaired without TPM ; ISO download (67 MB) OK ; library chips + VirtIO button rendered in the wizard. Remains unverified (no Windows ISO at hand): real boot of a Windows ISO + 2nd VirtIO drivers CD-ROM ; real TPM 2.0 requires an Incus newer than 6.0.0 | [x] `incus.rs` (**14** — + `null_default`: `state.disk`/`config`/`devices` as explicit `null` tolerated ; **+3 on 2026-10-13 (b): the catalogue grew Windows 11/10 + minimal Ubuntu/Debian entries with the `iso_only` flag, and `audio_device()`**) + `handlers/vm.rs` (**17** — +`vnc_display`/`has_screen`, `validate_local_iso` (absolute/traversal/extension/size/`chmod o+r`), `alloc_vnc_display` in the range 20–119, `safe_iso_name`, `human_bytes`, capacity probes (`swtpm`/OVMF) without panic ; `valid_instance_name`/`valid_unix_username`/`valid_snapshot_name`, `size_preset`, `normalise_status`, `parse_size_to_mib`, `instance_to_info` maps the wire contract, `pretty_os` fallback ; **+3 on 2026-10-13 (b): `apply_audio_device` attaches `audio0` only for a VM, and an `iso_only` catalogue entry is rejected on the ready-made path**) | [x] `vm_integration.rs` (**24** — **+2 on 2026-10-13 (b): `create` with `audio: true` attaches the device, and a Windows `iso_only` OS is refused on the ready-made path**; missing auth (400)/bogus (401)/non-admin (403) on **all** the endpoints (incl. `screen/ticket`) ; admin reaches each handler without 404/405 ; `available`/`os-options` without Incus ; `list` = clean message without socket leak ; `create` validation **before** Incus: name, unknown OS, unknown kind, short/absent password, invalid login, alpine as VM, **+ ISO: relative path/wrong extension/missing file, mutually exclusive ISO+OS, neither one, valid ISO → reaches Incus** ; `config` (bounds, empty body) ; snapshot name ; console **and** screen ticket require a running machine) | [x] `vm.spec.ts` (43 tests, 14 categories ; **+2026-09-12: Appliance design migration — `PageLayout` (machine list) + detail drawer + catalog/ISO wizard in Tailwind-on-tokens, console/screen overlays kept, selectors `data-testid="vm-*"` (wizard → `[data-slot="dialog-content"]`), zero scoped CSS** — 3 initial states ; catalog **and ISO** wizard (mode toggle, Linux/Windows toggle, 1 vs 2 file pickers, no password field in ISO, blocking without ISO) through to "Cancel" ; backend contract by `fetch` (`os-options`, `list` shape incl. `has_screen`, `create` rejections catalog **and ISO**, screen ticket) ; detail drawer ; xterm console ; responsive 360/768/1920 ; a11y ; security (no secret exposed, non-admin rejected on all endpoints incl. `screen/ticket`, XSS name not reflected) ; keyboard ; network ; **real opt-in cycle `VM_E2E=1`** ; memory regression) ; **+2026-09-13: 2-pane master-detail — the detail drawer (formerly `fixed` portaled) becomes an inline 420px column to the right of the list (`ResizeObserver` split < 720px → a single pane), `vm-drawer`/`vm-d-specs`/`vm-d-danger` kept, Escape/X close preserved (`vm.spec.ts` 5.1/5.2 unchanged)** ; **+2026-10-13 (b): the ISO-only catalogue entries, the audio checkbox and the "open screen in a new tab" action are new. Spec not re-run** |

### Notable sub-features (integrated into a screen above)

| Sub-feature | Manual audit | Rust unit tests | Rust integration tests | Playwright E2E tests |
|---|---|---|---|---|
| Security suite — Antivirus/Anti-Ransomware/Firewall (in Settings) | [x] 2026-07-16 (+ 09-06: real EICAR scan + real honeypot ; **+2026-09-09: folder paths = choice only, no more manual entry — new `theme/PathPickerField` + `theme/FloatingPathPicker` (floating non-modal movable panel, `use:portal` to `<body>`), standard reusable component (`mode="folder"\|"file"`, sidebar of locations — physical/USB/RAID/cloud/network disks, each displayed by its FS label or brand+model+serial no. + size — + list/grid toggle persisted in `localStorage`) ; `handle_explorer_disks` (frozen) enriched with `label`/`vendor`/`model`/`serial` (1 `lsblk -P` pass) ; `PathField`/`FolderPicker` (modal) unchanged**) | [x] `firewall.rs` (9), `ransomware_watcher.rs` (7), `antivirus.rs` (8), `ransomware.rs` (3) | [x] `security_integration.rs` (18) | [x] `security.spec.ts` (30 — 2.4: the "Select a folder" button opens a `.hv-fpp` non-fullscreen panel, no `.hv-fp-overlay`, drag-handle, sidebar of locations (disk row = size/serial no.), list/grid toggle (persisted), closes on Escape ; 2.2 renamed: `#avScanPath` field `readonly`) |
| Transfer quotas (in Users/Groups/Explorer) | [x] 2026-07-18 (+07-21 overview) | [x] `transfer_quota.rs` (9, +2026-09-07 — `check_quota`: bounds, first exceeded quota, multi-TiB values without i64 overflow, `Display`) | [x] covered indirectly by `explorer_integration.rs` (quota guards on download 429 / on upload) | [x] covered by `users.spec.ts`/`groups.spec.ts` |

**2026-09-13 (transfers v2)** — download: reservation under per-subject lock + refund if aborted, download ticket (check quota before the stream), `Range` (only the bytes of the range are reserved) ; multipart and chunked upload: per-subject lock held throughout the transfer + re-check under lock at completion (TOCTOU anti-race).

| System dependency inventory ↔ `install.sh` | [x] 2026-09-06 (missing packages added: ddrescue, testdisk, wireguard-tools, openvpn, restic, syncthing, cryptsetup, dmidecode, ethtool, snmp) ; **+2026-09-23: missing packages added — `git` (Docker app catalog), `unzip`/`zip`/`7z` (Explorer-Browser compression/extraction), `pciutils` (GPU card), `hostname`, `python3` (PAM login)** ; test parser extended to single-line `install_packages` (packages after the closing quote of the label — e.g. `ffmpeg`), 7 `System` entries added to `specs()` ; **+2026-09-23: `7z` installed adaptively (`7zip` on Debian 12+/Ubuntu 23.04+, `p7zip-full` otherwise) — `apt-get install` fails all-or-nothing if ONE package of the batch is unavailable, so the archive group no longer crashed as a whole ; `install_packages` now has a package-by-package fallback + `apt-cache policy` queries with `LC_ALL=C` (localized label `Candidat` ≠ `Candidate`)** ; **+2026-09-23: `install.sh` restructured into 5 "curl-pipe" style steps — the application archive is now downloaded by the script (`HV_RELEASE_URL`, or `$1` for a local file, no more mandatory argument), the systemd service and udiskie automount are **embedded** (a piped `curl | sudo bash` has no checkout on disk) with fallback to `installer/homevault.service` / `installer/setup-automount.sh` when the repository is present, Step 4 = global verification (packages + service + HTTP response + binary location), Step 5 = final message in English + local/internet addresses** ; **+2026-10-12: `installer/` reduced to ONE single file, `install.sh`, self-contained — `homevault.service`, `setup-automount.sh`, `optional-packages.conf` and the packaging `Makefile` removed (the systemd unit and the udiskie config are written in clear in the script, no more dependency on a neighboring file or on `$0`) ; the binary is downloaded per architecture from the GitHub Releases (`homevault-<arch>.tar.gz`, `HV_RELEASE_URL`/`HV_RELEASE_SHA256`/`$1` overridable, optional SHA-256 check) ; new maintainer mode `install.sh --package` (frontend + backend build → `installer/release/homevault-<arch>.tar.gz` + `.sha256`) which replaces the `Makefile` ; corrected data tree (no more `wallpapers/`, addition of `logs/`, `quarantine/`, `vm/isos/`)** | [x] `dependencies.rs` (**10** — each package of the inventory must appear in `installer/install.sh` ; `test_install_sh_embeds_a_complete_systemd_unit` and `test_install_sh_embeds_the_automount_config` lock the useful directives of the systemd unit and of the udiskie config written by the script ; `test_install_sh_is_self_contained` forbids any reference to a removed neighboring file) | n/a | n/a |

## Remaining to test as a priority (no automated test at all)

**2026-10-12 — the filesystem half of the user-deletion revoke has no Rust
net** (the database half does, see the 2026-10-12 addendum above).
`PermissionService::revoke_all_user_permissions` really shells out to
`sudo setfacl` on the share path, which CLAUDE.md's "What NOT to test in
Rust" keeps out of the suite: `test_user_delete_revokes_its_direct_share
_permissions_only` deliberately uses share names that do not exist on the
host, so the ACL branch fails cleanly and only the DB cleanup is asserted.
**To check on the next live-host pass:** grant a user a real ACL on a real
share (`getfacl` shows `user:<name>:rwx` + its `default:` twin), delete the
account, and confirm `getfacl` no longer lists it **and** that
`share_permissions` has no row left for the name. Also confirm the intended
non-effect: a **group** grant on the same share still lists the group and
still works for its remaining members.

**2026-10-12 — the Explorer directory copy/move needs a live re-run** (the SSE
half of the `JobType` fix, see the 2026-10-12 addendum above). The serialization
failure is unit-tested (a `Custom`-job `ProgressEvent` used to fail
`serde_json::to_string`, so the SSE `data:` line came out empty), but the
user-visible consequence — the progress bar animating during a copy, and the
directory listing refreshing once it finishes (`watchJob` resolving `true`) —
has not been observed on a browser since the fix. **To check on the next live
pass:** copy a directory (cross-filesystem, so a tracked job actually starts),
and confirm the progress bar moves, the copy ends on success rather than the
false "failed" it used to report, and the destination listing refreshes. The
Rescue ddrescue/PhotoRec progress display (also `Custom` jobs) deserves the same
check.


**2026-10-07 — Quotas removed from this list** (explicit request "implement
[the] task 01"). It was the **last screen** that had neither a live audit nor a
spec run: the spec written on 2026-09-22
(`tests/e2e/quotas/quotas.spec.ts`) is **38/38 green** on a scratch server
(Firefox, isolated `PORT=` + `DATA_DIRECTORY=`) and the screen was audited live
(detail in the 2026-10-07 addendum of `CLAUDE.md` and line 6 of the detail by
screen). **No dead selector fixed** — the spec had been written after the
Appliance migration of 2026-09-12 and already targets the `data-testid`, it passed
as is. Compilation had been acquired since 2026-09-14 (`npm run check`
0 errors after fixing 2 real defects in `Quotas.svelte`, chunk
`Quotas-*` emitted, `cargo build` OK). **Remains open, outside the scope of this
pass:** no integration file of its own — the single endpoint
`GET /transfer-quotas/{type}` remains covered by `transfer_quota.rs` (unit)
+ `explorer_integration.rs` (quota guards). **Consequence:** none of the
27 screens is now without an automated test of at least one category.

**2026-09-07 — Health removed from this list** (explicit request "implement
the complete tests for Health"): `#[cfg(test)] mod tests` in
`src/health.rs` (13), `tests/health_integration.rs` (10) and
`tests/e2e/health/health.spec.ts` (36 runtime tests, 12 categories,
read-only — **36/36 green** against the live server).
`handle_health_endpoint` was made `pub` (in the
frozen file `src/web/mod.rs`) to allow the integration test to reach a route
— behavior, session check and field names `checks`/
`overall_status`/`status` unchanged. No bug found, the audit of
2026-07-16 still holds. **What is not done:** forcing a check into
`warning`/`unhealthy` state (depends on the host — the aggregate is verified by
consistency with what the host returns).

**2026-09-07 — Desktop/shell removed from this list** (explicit request
"continue Desktop/shell"): `tests/desktop_integration.rs` (3 — **3/3** —
auth guards on `POST /api/server/restart` + `/shutdown`, never with an
admin token because that `spawn`s a real `shutdown -r/-h now`) and
`tests/e2e/desktop/desktop.spec.ts` (35 tests, 12 categories — **35/35
green** on 2026-09-08 against the rebuilt binary ; test 4.4 added with
the standardization of the topbar popovers: the 3 (network / account /
notifications) now share all the card chrome via
`.hv-topbar-panel` ; 2 shell bugs found+fixed:
the dock activity dot never showed for a freshly opened app — non-reactive
`{#if}` condition in `Desktop.svelte` — and Escape
did not close the launcher when the search field had focus —
`stopPropagation` too broad on `.lp-panel` in `HvLaunchpad.svelte`).
The logic/unit is **not applicable**. E2E: rendering of the desktop/dock/topbar pill/clock/user
pill ; the dock opens an app + activity dot + deduplication on
re-click ; windows (close / minimize→dock / maximize→restore) ;
mutually exclusive topbar panels (account menu ↔ notifications, outside click
closes) ; app launcher (opening, search filter,
clicking a tile opens the app + closes the launcher, Escape) ; global
search (Ctrl+K → overlay → result opens the app + closes) ; Power menu
(opening, the Restart option shows a confirmation that is **cancelled** —
`/api/server/*` **never** called) ; responsive (< 480 px hides
date/username), a11y (dock = labeled buttons, no duplicated `id`),
console without errors, edge cases, memory regression. **Never done:**
confirming a restart/shutdown/logout (the former reboots the host, the
latter would end the ongoing session mid-suite).

**2026-09-16 — app launcher redone on the Figma mockup `23:6646`.**
`HvLaunchpad.svelte` is no longer an opaque modal but a darkened full-screen
veil with category chips and a tile grid ; full detail
(contrast fixes, state badge, survival of the `desktop.app_*_info` warnings
as an accessible description, E2E contract preserved) in
the Desktop line of the per-screen table. Category 5 of
`desktop.spec.ts` goes from 4 to 12 tests — 5.5 the chips filter the grid
in place (`aria-pressed`, only one active), 5.6 empty state + `Reset`,
5.7 `fixed` dark translucent veil and content column without surface,
5.8 **composite and measured** label contrast (tile → veil → white
paper, worst case) ≥ 4.5:1, 5.9 44px targets (chip 32px + `::after` 44px,
field 44px in a 56px bar, tile ≥ 44px), 5.10 focus trap
(30 Tab) + Escape returns focus to the dock tile, 5.11 phone — 3-column
grid, chips in `nowrap`/`overflow-x:auto` (`flex: none`, the desktop
growth being cancelled) and **actually overflowing**, the
panel **scrolls internally** (no page scroll, no horizontal scroll)
and the last tile remains reachable, 5.12 six columns at
1440px **and** search bar / chip row / grid at the same width
and same `x` (explicit request: the three modules must never
misalign), **and** the content of the chip row — not just its
box — goes to the right edge of the grid, line by line (request
"not enough categories → enlarge the module": a chip is only as
wide as its label, `flex: 1 1 auto` makes it grow ; the per-line
assertion also holds when the row wraps),
then **the same measurement repeated at 1440×520** — the scrollable container is
the panel and not the grid wrapper, otherwise a scrollbar
that takes width would remove it from the grid alone.
**Two real defects were found while writing
these tests, not by re-reading**: (1) 5.5/5.6 revealed that the `<svelte:window onclick>` of
`Desktop.svelte` closed the launcher on each internal click — `.lp-panel` took
back its stopPropagation `onclick` (without `onkeydown`, which swallowed Escape) ;
(2) the measurement of the `backdrop-filter` showed that the pair
`backdrop-filter` + `-webkit-backdrop-filter` is reduced by the minifier to
the **only** prefixed form, which Firefox ignores → the blur of the mockup
was worth `none`. **Third request of the same day — "if the categories module
is smaller than the width of the 6 columns because not enough categories =
enlarge the category module"**: the box of `.lp-cats` already measured 828px
(5.12 measured it), but a chip is only as wide as its label, so
with few categories the row stopped two thirds of the way and the
module *appeared* narrower than the tiles. `flex: 1 1 auto` on
`.lp-cat` (emitted `flex: auto` by the minifier) shares the free space of
each line among its chips — growth in equal *delta*, so a longer
label remains the wider chip — and not `space-between`, which would have opened
a gap on a short last line. The `≤640px` block takes back `flex: none`
(otherwise a stretched chip would fill the row and the phone's lateral
scrolling would have nothing left to scroll). 5.12 gained the "per line"
assertions and 5.11 the `flexGrow === '0'` check + a row actually
overflowing. Desktop suite:
**73 tests / 15 categories** (`grep -cE '^\s*test\('` = 73 =
`npx playwright test --list` (146 = 73 × 2 projects)). On 2026-09-17, **the desktop
suite re-run in full** against the rebuilt binary: **65 green +
6 skipped + 2 failures**, the two failures both being defects of the **test**,
not of the product, and both fixed then revalidated (cf. below) — the
6 skips being §13 ("no observable background task on this host",
pre-existing). Neither failure is a product regression:
(1) **14.11** compared the raw `class` attribute and hit the Svelte scope
class (fixed: filtering of the `svelte-*` tokens) ; re-run alone, it is
**green** in 1.7 min, its two additional checks included. (2)
**5.6** failed on `browserContext.close: ENOENT … recording41.trace`, a
**teardown** error and not an assertion: two Playwright processes launched
in parallel write into the **same** `test-results/.playwright-artifacts-0/`
(they share the repository root), and one cleans the artifact while the
other closes it. Proof: re-run **alone**, 5.6 is **green**. Rule that follows —
**never launch two Playwright suites in parallel on this repository**,
the second corrupts the teardown of the first (the error looks like a test
failure although the test succeeded).

**2026-09-17 — app headers: the bar carries only [ICON] [TITLE]
(explicit request "verify in all apps that in the windowHeader only
[APP ICON] [WINDOW TITLE] [—] [□] [×] is present, nothing else").**
Registry audit, **recounted by script on `HEAD`**: **17** of the 24 apps
still filled the bar via the props of `WindowHeader`. The counting must
cover **both forms** of passing a snippet — the `actionLabel={…}` attribute
(CloudSync, Downloader, Explorer, Restic) **and** the child `{#snippet …}` block
(the others) ; a `grep` on the attribute form alone misses 13, and a `grep`
that does not bound itself to the `<WindowHeader>…</WindowHeader>` block
over-counts the `{#snippet actions()}` of `SectionCard` (Rescue ×3, Users).
The 17: Chat (Ollama state badge), CloudSync, Dashboard ("Live" badge),
Downloader, Explorer (`extra` + `actionLabel`), Groups, Health, Journal ("auto
refresh" checkbox + 2 buttons), Network, Rescue (tool status row), Restic
(`extra` + `actionLabel`), Settings (save status), Shares, Syncthing, Terminal
(tab row via `center`), Users ("new user" button), VPN. All emptied, and each
control has a decided location:
- `WindowToolbar` (`toolbarRight` / `toolbarLeft` of `PageLayout`) for 10
  of them — Chat, CloudSync, Downloader, Groups, Journal, Network,
  Restic, Shares, Syncthing, VPN ;
- their own file toolbar, already in the page body, for Explorer ;
- the rail footer for `SettingsSaveButton`, which itself carries the save
  state (color + label + sr-only line `settings-save-status`,
  2026-09-23 — the visible pill above and the caption below were
  removed, only a single element remains) ;
- the side rail for the Users "new user" button
  (`data-testid="users-tile-new"`) ;
- the window content for the Terminal tab row (`terminal-tabs`,
  between the header and `terminal-infobar`) ;
- the page body for the Health global status badge (always rendered) and
  for the Rescue tool availability, now a badge **per tool**
  in each tool's card (`data-testid="rescue-tool-badge"`) ;
- **removed** for the Dashboard "Live" badge: decorative indicator
  (the app revalidates itself), not a piece of data.

Not to be confused with the **12 screens that render a `WindowToolbar`**: two
apps that did **not** fill the bar (`Quotas`, `VirtualMachines`) also render one
for their page actions, hence 10 + 2 = 12.

**New test 14.11 "every app header bar carries the icon + title only"**:
it iterates **the 24 app ids** of `OWN_HEADER_APPS` and requires, for each,
exactly one icon tile + one title, **zero** `.hv-winhdr-actions` and
**zero** `.hv-winhdr-center`, and no interactive element
(`button`/`input`/`select`/`textarea`/`a`/`label`/`[role=button]`) in
`.hv-winhdr` — the tab row lives in `.hv-winhdr-wrap`, **outside** the
bar, and the min/max/close trio is a **sibling** of the bar (it belongs to
`WindowFrame`), so both remain allowed. Two more checks close
the blind spot of the first three, which only look for the *slots* and the
controls: **(4)** any **non**-interactive element added in the bar (a
badge, a state dot, a superfluous wrapper) is caught by pinning the
markup shape — apart from the icon tile, `.hv-winhdr` must contain **exactly**
`.hv-winhdr-text` then `.hv-winhdr-title`, in that order ; **(5)** the icon
tile, the only place where an app's markup is free, is pinned to **a
single** glyph (`.hv-winhdr-icon > *` = 1), which catches the "extra badge"
case. **Lesson from the first pass of 14.11**: check (4) compared the **raw**
`class` attribute, but the Svelte compiler adds its scope class
(`svelte-19iy2no`) to any element targeted by the component's `<style>` — and
`WindowHeader.svelte` styles precisely these two classes. The test therefore failed
on **all** the apps, not just the first ; it now filters the
`svelte-*` tokens. **All the assertions of the loop are `expect.soft`**:
a registry guard that stops at the first faulty app hides the 23 others,
whereas the goal is precisely to name **all** the non-compliant bars in
a single pass (the *web-first* assertions in soft mode keep their
auto-retry). Three opening mechanisms
are needed and the test documents why: `hv-open-app` only resolves
in the `apps` registry, which **does not contain** `terminal` or `settings`
(added separately as `terminalApp` / `settingsApp` in
`Desktop.svelte`) — those open via the dock (`dock-<id>`) — and `account`
has **no** launcher tile: it opens via the user pill menu. The test's
timeout is raised to 300 s (24 apps × ~5 s). Two real
findings of this pass: (1) the E2E locators of downloader (15),
vm (2), backup (1), settings (3) and health (2) targeted
`.hv-winhdr-action-btn` / `.hv-winhdr [data-testid=…]`, therefore **empty and
silently green** after the move — rewritten toward the new
`data-testid` (`downloader-add-toggle`, `vm-new`, `restic-add-repo`,
`settings-save-status`) ; (2) `handleOpenAppRequest` cannot open
`terminal` / `settings` (cf. above) — the flaw of the `hv-open-app`
handle that the doc describes as the inter-app app-id passage is real, but
**not fixed** in this pass (outside scope, no caller uses it today) ; to fix
if one day an app must open Settings or the Terminal from its window.

**2026-09-17 (continued) — repair of the cluster of dead E2E selectors.** On
re-running the suites touched by the emptying of the bars, a cluster of
selectors **dead since the "Appliance" migration of 2026-09-12** appeared:
that migration replaced the class hooks with
`data-testid`, and the specs did not follow. Families concerned:
`.ef-file-name`, `.bc-item`, `.modal-input`, `.ef-row-action`,
`.toolbar-btn`, `.toolbar-left`, `.modal-dialog`, `.modal-wide`,
`.preview-editor`, `.preview-text`, `.hv-shares-modal`, `.hv-shares-inp` and
`.hv-modal-dialog`. Two distinct causes, to know before concluding
that a selector is alive: (1) **the Svelte const trap** —
`class={efRowAction}` / `class={toolbarBtn}` put in the DOM the *Tailwind
utility string* that the const contains ; `.ef-row-action` /
`.toolbar-btn` are the **names** of the consts, never class tokens, and
a literal `grep` finds the name, making the selector appear alive ;
(2) **hooks composed at runtime** — `theme/Alert.svelte` renders
`hv-alert hv-alert-{variant}`, and `sidebarTestId="explorer-sidebar"` is a
**prop** that `PageLayout` passes to `WindowSidebar` (a
`grep 'data-testid="explorer-sidebar"'` in `Explorer.svelte` therefore finds
nothing although the attribute is rendered). Only a DOM-level check
decides. `.hv-modal-dialog` is separate: the CSS rule exists
(`lib/styles/theme/components.css:323`) but is attached to **nothing** —
the orphan is now documented rather than assumed alive.
**A real hole found by this pass**: the Explorer E2E guard 14.3
("no manual Refresh button") targeted the dead class, therefore matched
**nothing** and was vacuously green — it no longer guarded the decision of
2026-09-08. Rewritten toward
`button[title*="efresh" i], button[title*="ctualiser" i]`, it becomes a
guard again. The 5 "click Refresh to re-list" calls are removed: the
automatic re-list mechanism was **verified before removing the calls**
(`connectWatcher()` opens an SSE `EventSource` on
`GET /api/explorer/watch`, re-lists on `change`, + `visibilitychange`), and
not assumed from the test comment. The root of the modals is
now `[data-slot="dialog-content"]`
(`ui/dialog/dialog-content.svelte:29`) — `journal.spec.ts` and
`users.spec.ts` (4 sites) are repointed ; three were real
assertions already broken, the fourth (`toHaveCount(0)`) passed vacuously.
`Explorer.svelte` gains 17 `data-testid`, `Shares.svelte`
`shares-name-input`.
**Bug of this pass, fixed**: 10 of these attributes had been inserted
in the body of the arrow function of `onclick` (or **before** the `<button`
tag), therefore **outside** the tag — the frontend compilation failed
(`Assigning to rvalue`). Fixed ; a tag-aware scan, run on
the **242 `.svelte`** of the frontend, no longer reports any `data-testid` outside
a tag (the only remaining hit, `SettingsCard.svelte:51`, is in a
documentation comment).
**14.11 hardened**: two checks added, which close the hole reported
by the audit — a **non-interactive** element (badge, state dot, parasitic
wrapper) slipped between the meshes of checks 2 and 3. Now the
elements of the bar **apart from** the icon tile must be exactly
`['hv-winhdr-text', 'hv-winhdr-title']`, and the icon tile must contain
**exactly one** glyph. Prerequisite verified in the source for the 24 apps: the
inline `<svg>` of Dashboard is **one** root (three `<path>` as children),
and Terminal passes `<TerminalIcon>` as `{#snippet icon()}` **child of
`WindowHeader`** — it does not use `PageLayout`, unlike the 23 others.
**Counts (2026-09-24)**: `npx playwright test --list --project=firefox` = **1436 tests /
28 `.spec.ts` files** — that is the reference ; the
`grep -cE '^\s*test\('` documented below gives **1367**, the difference (69)
being the tests **generated by loop**, which that grep does not count. Per suite
(`--list`): desktop **74**, users **83**, explorer **64**, vm **45**,
journal **29**.
**Environment limit (groups suite)**: the `beforeAll` of `groups.spec.ts`
creates a throwaway group and fails on this non-root dev machine —
`could not create throwaway group: 400 … sudo: a terminal is required to read
the password`. This is not an application regression but the absence of
passwordless `sudo` for the dev user. In cascade, Playwright
reports **1 failure (1.1) + 62 "did not run"**: the whole file (63 tests)
remains a dead letter, **which includes the 16.1 guard added on 2026-09-23**
(written, counted, but not executed here). To run the suite in full:
NOPASSWD `sudoers` drop-in for `groupadd`/`groupdel` — the same execution
condition as the `beforeAll` of `users.spec` ("absent here", Users line).

**2026-09-17 (third pass) — selectors corrupted by the bulk substitution, and
last vacant guards.** The global replacement of the dead classes left
**three syntactically broken selectors**, all of the same mold:
an attribute selector followed by the **rest of the class name** it
replaced. None was visible on re-reading, only at execution (or never, for
the third).
- `login.spec.ts` (new password): `[data-testid="dialog-content"]`
  — the root hook of a modal is `data-slot`, not `data-testid`. The
  scope remains necessary: `login-recovery-submit` exists **twice**
  (`Login.svelte:432` the panel, `:614` the modal), and only the one of
  the modal validates the new password.
- `shares.spec.ts` 4.5: `'[data-slot="dialog-content"]2 button'` — the orphan
  `2` made the selector **invalid** (CSS: unexpected token). Real
  hook: `[data-testid="shares-mode-btn"]` (`Shares.svelte:762-763`).
  Reminder: `segBtn` is a Tailwind **const** (`:490`), therefore never a class
  token — the already documented trap.
- `docker.spec.ts` (Compose editor): `[data-slot="dialog-content"]-close`,
  same invalidation. The live hook of the cross is
  `[data-slot="dialog-close"]` (`ui/dialog/dialog-content.svelte:38`).
- `users.spec.ts` 4.9 clicked `.hv-modal-close`: **class declared but
  applied by no one** (`components.css:353`) — the click timed out. Repointed
  to `[data-slot="dialog-close"]`.
**Two orphan CSS rules** in `lib/styles/theme/components.css`:
`.hv-modal-dialog` (`:323`) and `.hv-modal-close` (`:353`). Their neighbors
`.hv-modal-title` / `.hv-modal-icon` **are** alive (`theme/Modal.svelte`
`:63,65`) — the family is only half orphaned, hence the verification by
reading the component and not by `grep`. **Left in place** (removing from the
shared theme CSS without visual control is not verifiable) ; reported.
**Three more vacant guards** converted into real assertions, same family
as the 10 `.hv-winhdr-subtitle` guards: `dashboard.spec.ts` 1.7 and 7.1
(`.db-refresh-btn`) and `settings.spec.ts` 1.2 (`.ah-wrap, .ah-top` — `AppHeader`
disappeared from the entire repository). For Dashboard, the rule is now asserted on
the **accessible name** (`getByRole('button', { name: /refresh|actualiser/i })`),
so a reintroduced Refresh button fails whatever its class ; the
Dashboard today renders **no** button (0 `<button>`, 0 `role="button"`).
**Cross-check of the `data-testid`**: **362** used by the specs against
**454** declared in `frontend/src`. The 7 used-but-non-literal are
all explained: `explorer-sidebar` / `groups-list` / `settings-sidebar`
come from the `sidebarTestId` prop, `docker-nav-item` / `settings-nav-item` from
the `testId` prop of a rail item, `dashboard-live` is the **removed**
badge (deliberate negative assertion) and `dialog-content` was the
misspelled hook, fixed above. No dead `data-testid` remains.
**A third missing hook, this one on the product side** — found on re-running
the security suite after the fix of `openSecurity()`: **2.1 remained
red**, and for an independent reason. `security.spec.ts:302` asserts
`.settings-info-row` in the Antivirus card ; but `AntivirusCard.svelte`
did render this class **at HEAD** (markup + scoped rule), but the migration
to the shared vocabulary lost it: its rows now use
`settingsRow` of `lib/components/settings/vocabulary.ts`, which was **the only
const of the module without its `settings-*` hook** (all the others carry
one — `settings-hint`, `settings-input`, `settings-select`, `settings-btn`…, the
module itself documenting that "the `settings-*` names are the E2E contract, not
of style"). It is therefore not an outdated test selector but a
**hook removed from the product**: the test was right, the vocabulary was wrong.

**First fix — wrong, and it is the mistake to remember.** Adding
`settings-info-row` to `settingsRow` made 2.1 green but **broke
`settings.spec.ts` 10.1** (`toHaveCount(3)` → 4). My justification at the time —
"the 3 lines of `system` that 10.1 counts do not use this const,
verified in the source" — was false: I had read the list of usages of
`settingsRow` truncated by a `head -20`, and **`Settings.svelte:868`** (the line
"update check frequency", a `<select>`, therefore a *command* row) does fall
into the `{#if activeCategory === 'system'}` block opened at
865. The very name of the hook said the error: `settings-info-row` designates a
**read-only** row (label + value), and `settingsRow` is also used for the
command rows.

**Retained fix**: `settingsRow` returns to its form without a hook, and a
dedicated const `settingsInfoRow` (which prefixes `settings-info-row` to `settingsRow`)
carries the hook. `AntivirusCard.svelte` uses it on its **two only** read
rows (`definitions_status`, `last_scan`) — the other rows of the card
(scan path, scheduled scan, quarantine folder) are command rows and keep
`settingsRow`/`settingsRowStacked`. `settings-info-row`
therefore has only one producer, and the 3 literals of `system` remain alone
in their category. Verified: `security.spec.ts` 2.1 **and**
`settings.spec.ts` 10.1 **both green** (2 passed, 27.6s, serial).
To remember: when a card migrates from scoped CSS to the shared vocabulary,
**the E2E hook follows only if it is in the const** — but a shared const
is the right place only if **all** its rows have the semantics of the hook.

**A third bug of the same family, found by E2E and not by grep
(2026-09-17).** After the fix above, `settings.spec.ts` went to
**95/96**: only **23.5** remained red, at `:1807`, `Expected: <= 12`,
`Received: 14`. 23.5 is the only test of the suite that reads a font
size (`parseFloat(cs.fontSize)`) ; it verifies that the section labels are
micro-labels. They rendered at 14px.

Cause: `--hv-text-*` is an **overloaded** prefix — it carries colors
(`--hv-text-primary` = `#1f2733`) *and* sizes (`--hv-text-xs` = `11px`).
Tailwind v4 cannot guess the property targeted by an untyped `var()` and
decides for the color: `text-[var(--hv-text-xs)]` compiles to
`color: var(--hv-text-xs)`. The value not being a valid color, the
declaration is discarded and **no `font-size` is emitted** — the label therefore
inherits the 14px of its parent. The repository convention is the typed
form `text-[length:var(--hv-text-xs)]`, which compiles to `font-size:`
(18 rules `font-size:var(--hv-text-xs` in the 10 CSS chunks).

Scope: three usages, all **sizes** written without the hint — `secLabel` and
`secLabelFirst` of `components/settings/vocabulary.ts`, and the rail header
in `routes/Settings.svelte:657` (without a test hook, therefore invisible to the
suite). Fixed by adding `length:`.

**What this teaches about the documentation.** The guide
`docs/guides/design-system.md` already documented the rule *and* asserted that
"the typographic audit of 2026-09-12 fixed all the existing occurrences".
That was false — three had survived, and it is an E2E test, not a
grep, that found them. The guide is fixed (2026-09-17) with the explanation
of the overloaded prefix and the command that really decides: read the **compiled**
CSS, not the source.

**Method trap, to remember.** My first explanatory comment in
`Settings.svelte` literally quoted `text-[var(--hv-text-xs)]` — and Tailwind
extracts class candidates **also in the comments** of a `.svelte`
(`@import "tailwindcss"` = automatic content detection). Writing
the spelling of the bug in a comment therefore regenerated the dead rule that
had just been removed, and made the source diverge from the CSS actually shipped. The
comment now names only the token.

**Two verification traps encountered in the same pass.** (a) A `grep`
with `\.` at the head cannot match a selector **minified into a list**
(`.a,.text-\[…\]{…}`): several searches wrongly returned "clean" ; the
final checks are done with a literal string (`grep -F`). (b) `grep -c` on
`Settings.svelte` returns 2 for the typed form — the fixed line 657 **plus**
a legitimate pre-existing usage (line 950), not a duplicate of the fix.

**2026-09-07 — Widgets removed from this list** (explicit request "continue
with Widgets"): `tests/widgets_integration.rs` (6 — **6/6**) and
`tests/e2e/widgets/widgets.spec.ts` (22 runtime tests, 9 categories —
**22/22 green**). The logic/unit is
**not applicable** — the only endpoints of the feature (`GET|POST
/api/monitor-layout`, `GET /api/security/active-sessions-count`) are
upserts / DB queries without pure logic. Integration: session guard on
`/monitor-layout` (per-account layout blob: `null` before saving,
round-trip, upsert = 1 row, alice/bob partitioning) ; **admin** guard on
`active-sessions-count` + count semantics (deduplication of a
user with several sessions, expired sessions ignored). E2E: the
catalog window (11 tiles), the active/inactive state of each tile
consistent with the corresponding monitor panel on the desktop, the toggle
that makes the panel appear/disappear + updates the counter of the
footer, the "Reset" button, persistence via `POST /monitor-layout` ;
snapshot+restore of the admin layout in `beforeAll`/`afterAll`.
**Never done:** nothing mutating on the host side (the feature has none).

**2026-09-07 — System journal removed from this list** (explicit request
"continue with System journal"): `#[cfg(test)]` in
`src/handlers/journal.rs` (9 — `priority_text`, `is_app_log_filename`,
`text_log_path` (whitelist + anti-traversal), `strip_target_prefix`,
`parse_app_log_line`, `seconds_to_iso`), `tests/journal_integration.rs`
(11) and `tests/e2e/journal/journal.spec.ts` (28 runtime tests, 12 categories —
**28/28 green 2026-09-07**). **What is
really exercised:** the `app` source (the `homevault.log*` files of
`DATA_DIRECTORY/logs`, which the process owns — no `sudo`) — parsing
of the `tracing` lines, `search` filter, empty state ; `DELETE /clear` which
deletes only `homevault.log*` (never `/var/log` nor journald) ; the
auto-cleanup settings (default on/90 d, round-trip, `interval_days ≥ 1`
clamp). **Never done:** a real `journalctl` / `tail
/var/log/*` (goes through `sudo`) — only the clean fallback `{success:false}`
is verified when `sudo` asks for a password ; the real emptying of the
app logs ; a real triggering of the auto-cleanup scheduler.

**2026-09-07 — Dashboard removed from this list** (explicit request
"start with DASHBOARD"): `#[cfg(test)] mod tests` in `dashboard.rs`
(17 — `format_bytes`, `parse_size`, `calculate_individual_disk_score`,
`calculate_disk_summary`, `calculate_disk_score`),
`tests/dashboard_integration.rs` (10 — **10/10 green** — auth split on
the 15 read endpoints + widget-CRUD + widget-preferences, add/list/reorder/remove
cycle of a widget, per-user preferences upsert + their partitioning
by account) and
`tests/e2e/dashboard/dashboard.spec.ts` (37 `test(` / **39 runtime tests**,
12 categories — **39/39 green 2026-09-07** against the prod binary ;
read-only — the screen has no mutating action ; +2026-09-08: the manual
Refresh button removed (5 s auto-refresh kept), specs aligned). **1 bug
fixed (frozen file, same as
`disk.rs`/`pools.rs`)**: `parse_size` cast the `f64` to `u64` *before*
multiplying by the unit → "9.8G" of `df -h` truncated to "9G"
(under-counting up to ~1 TiB on a size in T), and "465,8G" (locale
comma) parsed to 0. Fixed (`,`→`.`, multiplication in `f64`, cast last) ;
tests `test_parse_size_keeps_the_fraction` /
`_accepts_a_locale_decimal_comma`.

**2026-09-07 — gaps filled in the already partially tested screens**
(explicit request "finish the functions left to test"):
- **Storage / virtual disks**: the handlers `handle_virtual_disks_list`
  / `_delete` / `_scrub` (routes `/api/storage/virtual-disks*`) had
  **no** Rust test. Addition of `handlers/storage.rs::vd_name` (3 unit)
  and of 2 integration tests + the addition of these endpoints to the
  auth matrix loops of `storage_integration.rs` (23 in total). Still no
  listing/destruction of a real virtual disk (real mdadm/ZFS).
- **RAID**: `tests/raid_integration.rs` created (5) — the 5 endpoints
  `/api/storage/raid*` are exercised only up to the auth rejection
  (`require_admin` runs **before** the handler body → `mdadm` never
  reached). Real creation/destruction remains `raid.spec.ts` opt-in.
- **Explorer / Explorer-extra features**: `/roots`, `/save`, `/eject`,
  `/snapshots`, favorites (server-side) and the `?strategy=` parameter on
  `/copy` were without coverage. +11 tests in `explorer_integration.rs`
  (40 in total) + `kind_for_root`/`resolve_dest` unit (5) +
  `CollisionStrategy::parse`/`non_colliding_path` in `browser.rs` (6).
  **2026-09-08 — `explorer.spec.ts` section 15 added** (9 tests): inline
  editor (UI), `/save`, favorites (star ⇄ sidebar ⇄ server), collision
  strategies `/copy`+`/move`, Snapshots menu, `/eject`, Explorer→Shares
  hand-off. **Remains uncovered**: navigation *inside* a real
  ZFS snapshot, a real USB `umount` (host required).
- **Transfer quotas**: `transfer_quota.rs::check_quota` (8 unit).
  The rest (`applicable_*_quotas`, `record_usage`) touches the DB + `id -nG`
  and remains covered end to end by `explorer_integration.rs` (429 on
  download / blocking on upload) + `users.spec.ts`/`groups.spec.ts`.
- **1 bug fixed (explicit authorization, frozen file)**: `check_quota`
  did `additional_bytes as i64` — a `u64` beyond `i64::MAX`
  became negative and wrongly passed under the limit. Fixed: comparison
  in `u128`. Non-regression test
  `test_check_quota_huge_transfer_saturates_instead_of_overflowing`. See
  `docs/guides/transfer-quotas.md` and the invariant "Explorer / transfers"
  of `CLAUDE.md`.
**2026-09-08 — Settings (core) removed from "without the slightest test"**
(explicit request "finish points 1 and 2"):
- `#[cfg(test)] mod tests` in `src/handlers/settings.rs` (13 — pure logic:
  `parse_version` / `is_newer_version` (an unreadable version never claims an
  update), `sanitize_hostname` (collapse/trim of the hyphens, fallback
  `homevault`, RFC 1123 limit at 63), `UpdateFrequency` default = `Manual` +
  `as_seconds` (G2 — no automatic GitHub call), appearance defaults,
  `normalize_accent` (2026-09-23 — rewrites the legacy accent `#4a90d9` to
  `#1E62AE` §1.4.3, case-insensitive, any other value is kept)).
- `#[cfg(test)] mod tests` in `src/settings_persistence.rs` (7 — `export()`
  masks `smtp_password` (never in clear, `null` in the JSON) without mutating the
  file on disk, `import()` round-trip of the appearance, `apply_loaded_
  settings`: a `None` password (masked export) re-imported **does not replace**
  the configured password).
- `tests/settings_core_integration.rs` (13 — Axum router + in-memory SQLite +
  `init_persistence(tempdir)`: **admin** split on all the reads
  (`/settings`, `/appearance`, `/email`, `/notifications`, `/update`,
  `/export`) and writes ; `GET /settings/language` = any
  session ; `GET /settings/public` = without auth and without leak ; round-trip
  PUT→GET of the appearance (accent / density / wallpaper) + reflection in
  the aggregate ; partial update that preserves the other fields ;
  `GET /settings/export` masks `smtp_password` ; export→import ;
  `POST /settings/reset` restores the defaults (accent `#1E62AE`,
  `check_frequency` = `manual`) ; `set-language` accepts en/fr/de/es and
  rejects the unknown ; notification threshold clamped to 100) ; +
  2026-09-23: `PUT /settings/appearance` with the legacy accent `#4a90d9`
  → re-read `#1E62AE` (and reflection in the `GET /settings` aggregate).
- **Still not covered**: the *runtime* application of the accent
  (`--hv-primary`) and of the density — purely front, within the remit of
  `settings.spec.ts` ; `POST /settings/general` (→ `hostnamectl`) is not
  exercised in integration (the shell-out) — `sanitize_hostname` is in
  unit.

**Syncthing** left this list on 2026-09-06: unit tests
`handlers/syncthing.rs` (11 — `extract_api_key` on the `config.xml`, +
deserialization of the REST responses of syncthing v1.27/1.29 with all the
`#[serde(alias)]`: `deviceID`, `certName`, `myID`, `globalBytes`, `at`,
`clientVersion`… — several of these aliases exist because a missing alias
silently broke the parsing against a real daemon),
`tests/syncthing_integration.rs` (5), `tests/e2e/syncthing/syncthing.spec.ts`
(21). **What is locked:** the **10** endpoints require an
**admin** session — that was THE flaw of the 2026-07-16 audit: zero
auth check on `restart` (daemon restart), pause/resume
(disturbs any paired device), and reading the IDs/IPs of
all the devices. `pause`/`resume`/`scan` reject a malformed body
(422) before any HTTP call ; when the daemon is off (case of the
current machine), these endpoints return a clean error "Failed to
connect to Syncthing" (never a 500). **Never done:** a real
`restart`/`pause`/`resume`/`scan` against a running daemon, nor a real
device/folder cycle (requires a running Syncthing + a paired peer —
like the initial audit, dedicated session) → live read left in
**opt-in** `SYNCTHING_E2E=1`. No application bug found. `Syncthing.svelte`
is in *legacy* Svelte mode (no runes) → not touched by the reactivity bug of the
Backup page.

**AI Assistant (Chat)** left this list on 2026-09-06: unit tests
`handlers/chat.rs` (11 — `system_prompt` per language fr/de/es/en
+ English fallback, `session_error_message`, `reset_ai_model`,
`MODEL_CATALOG` + the recommendation scorer that runs against the real
`/proc/meminfo` of the host, deserialization of the Ollama responses),
`tests/chat_integration.rs` (9), `tests/e2e/chat/chat.spec.ts` (22).
**What is locked:** `send` / `send/stream` / `models` / `model`
GET require a **session** (any role — chatting is not
privileged, that was the meaning of the audit fix) ; `models/pull` and
`model` PUT require **admin**. The clean fallback "AI unreachable" (never
a 500) when Ollama is off is tested as a first-class case.
Real conversation in **opt-in** `CHAT_E2E=1` (Ollama running + a model
downloaded). **Verified end to end on 2026-09-06 with a real Ollama
0.33.3 + `gemma3:1b` (CPU inference):** complete suite `CHAT_E2E=1`
**22/22** — incl. §13.1 (real prompt → non-empty streamed response) and §5.1
(the language is indeed transmitted, real response received). In the browser:
**"Online"** badge (green), offline advice hidden, a real prompt
gets a streamed response. `/api/chat/send` → `{"reply":"Pong.",
"model":"gemma3:1b"}`, `/api/chat/send/stream` → SSE `chunk`×N + `done`.
**3 fixes on 2026-09-06 to make Chat functional (explicit
request):**
1. `GET /api/chat/models/recommend` — the **only** Chat endpoint without a
   session check (leaked the host RAM+GPU to any caller). Fixed:
   takes `State`+`headers`, same `get_session` as its siblings. The scorer
   is extracted into `recommend_models(&specs)` (pure, testable).
2. `Chat.svelte` — the status badge displayed a **hardcoded green
   "Online"**, never verified: it stayed green while every
   message failed. Fixed: `onMount` calls `checkOllama()`, the badge
   shows **online** / **no model** / **AI offline** / **check**,
   and the empty state displays actionable advice when Ollama is absent.
3. `installer/install.sh` — Ollama was **never installed**, the Chat
   app was dead on delivery. Addition of `install_ollama` (official
   script, not apt, then `systemctl enable --now ollama`). **No
   model is downloaded automatically** — the choice of the model is up
   to the user (Settings > AI, recommendations based on RAM/GPU).
   `ollama` entry in `dependencies.rs` (`AI` category) + test
   `test_install_sh_installs_ollama_for_the_chat_app` (which also verifies
   the absence of `ollama pull` in `install.sh`).
   **+2026-09-23, strengthened verification against the Ollama Linux doc**
   (`docs.ollama.com/linux`): install *and* Step 4 of verification
   now control the three levels — binary present, `ollama` unit
   active (`systemctl is-active --quiet ollama`, with a 10 s wait
   loop after `enable --now` to let the daemon bind the port) and
   HTTP response on `http://127.0.0.1:11434/`. The
   Rust test requires the three patterns ; a binary installed with a dead
   service now fails the verification instead of displaying a green install
   log with Chat offline.

**Terminal** completed its coverage on 2026-09-06 (it already had 71
E2E tests): addition of the **Rust** tests — `handlers/terminal_pty.rs` (13 —
`is_valid_tab_id` / `resolve_tab_id` / `generate_ticket`, and the cycle of the
single-use connection ticket of the `PtyManager`: issuance → single
consumption, `kill_session` which also purges the pending tickets of the deleted
account, no-ops of `close_tab`/`cleanup_all`/`reap_stale`) and
`tests/terminal_integration.rs` (8 — **admin** auth matrix on
`/api/terminal/ticket` and `/close`, tab id validation → 400, `close_tab`
of an unknown tab = clean success). **Not tested in Rust:** the spawn
of a real `sudo -u <user> -i` shell, therefore the `MAX_TABS_PER_USER`
ceiling and the kill of the inactivity reaper — that is the role of
`terminal.spec.ts` (real PTY). No bug found.

**Cloud Sync (rclone)** left this list on 2026-09-06: unit tests
`handlers/rclone.rs` (14 on 2026-10-12), `tests/rclone_integration.rs` (17 on
2026-10-12 — the config CRUD moved here from the E2E spec once the path stopped
being hardcoded),
`tests/e2e/cloudsync/cloudsync.spec.ts` (24). **What is locked:**
the 12 endpoints require a session ; all the mutations **and the
reading of the raw config file** (`GET /rclone/config`, which returns
the "obscured" — therefore in clear — credentials of each remote)
require admin ; the anti-injection guards (`config` containing a `[section]`
line), invalid remote name and empty source/destination of
`sync`/`copy` reject cleanly without launching `rclone` nor writing the
file. **Never done:** a real `sync`/`copy`/`mount` `rclone`.
**+2026-10-12:** the real `add remote → list → delete` cycle is now covered in
Rust instead of being left to the opt-in E2E section — the config path stopped
being hardcoded, so the integration suite points `RCLONE_CONFIG` at a tempdir and
drives the real handlers (`rclone_integration.rs`, 14 → 17, incl. the
`.backup.<timestamp>` sibling). A real *`rclone` binary* transfer remains E2E-only
(`CLOUDSYNC_E2E_WRITABLE_CONF=1`, or a server launched with
`RCLONE_CONFIG=/tmp/…/rclone.conf`). No application bug found in the sync paths
(the fixes of the 2026-07-14 audit hold).

**Backup (Restic)** left this list on 2026-09-06: unit tests
`restic_scheduler.rs` (12), `restic.rs` (8),
`tests/restic_integration.rs` (14), `tests/e2e/backup/backup.spec.ts`
(24). **Real conditions:** a real `restic init` + `restic backup` of
`/etc/hostname` in a disposable repository `/tmp/hv-backup-e2e-*`, the list of
snapshots (the snapshot really exists), then deletion of the repository. The
**schedule** and **retention-policy** cycles are 100% DB + `compute_next_
run` — tested end to end in integration without ever launching `restic`.
**Never done:** real `restore`, real `forget`/`prune`, scheduled backup
triggered by the scheduler (30 s loop).
**3 bugs found and fixed (frozen files, explicit request
"fix all the bugs"):**
0. **The Backup page was entirely dead in the browser** —
   permanent loading spinner. `Restic.svelte` is a Svelte 5 *runes mode*
   component but ~35 reactive variables (`resticLoading`,
   `resticAvailable`, `repos`, all the modal flags…) were
   simple `let`: in runes mode a reassignment of `let` does not
   re-render, so `resticLoading = false` in `onMount` had
   no effect. Same class of bug as Shares/Downloader/Rescue;
   audited on 2026-07-14 under Svelte 4, missed by the Svelte 5 migration,
   never detected for lack of tests. Fixed: each reactive `let` →
   `$state(...)`. `backup.spec.ts` fails en masse (24/26 timeouts on
   the spinner) without this fix.
1. `create_backup` did `serde_json::from_str` on **all** the output
   of `restic backup --json`, which is **NDJSON** (`status` lines
   then a `summary` line) → "trailing characters at line 2" → **every
   manual and scheduled backup was reported as failed** whereas the
   snapshot was indeed created. Fixed: `parse_backup_summary` takes the
   last non-`status` JSON line. (Field `total_bytes` → `total_bytes_
   processed` along the way — the real restic name.)
2. `compute_next_run` passed the cron day-of-week field as is to
   the `cron` crate, whose convention differs from crontab: `0` was
   **rejected** (crontab = Sunday) and `1..6` were **shifted by one day**
   (a "every Monday" schedule `* * 1` ran on Sunday). Fixed
   by `normalize_crontab_dow` (crontab `0/7`→Sun, `1..6`→Mon..Sat) ;
   the names `SUN`/`MON`/… already passed correctly.

**Storage** left this list on 2026-09-06: unit tests
`pools.rs` (24), `cache.rs` (13, +2 existing), `encryption.rs` (2),
`tests/storage_integration.rs` (24, +3 in the 2026-09-07 passes) and `tests/e2e/storage/storage.spec.ts`
(30). **2026-09-07 (RAID↔Storage re-scope):** `storage.spec.ts` extended —
test 1.3 now expects **6** tabs ("Virtual disks" added),
the tab nav loop covers the new panel. Run 2026-09-07
against the rebuilt binary: **34 passed / 1 skipped** (LUKS
opt-in cycle). Still **no** test that lists or destroys a real virtual
disk (real mdadm/ZFS required) — only the opt-in §13 of `raid.spec.ts`
covers this path end to end. **What is really exercised:** the whole read-only inventory
(disks/partitions/removable, settings + pools + cache shares, encrypted
volumes) against the real host ; the auth split (read = any
session, write = admin) on the 18 endpoints ; and **each**
mutation path up to the clean rejection (400/401/403, never 500) —
cron/out-of-bounds ranges of the cache settings, invalid cache share
name/mode, too-short passphrase / filesystem / LUKS label
invalid, unknown volume id, deletion without confirmation. **Never
done:** create/destroy/scrub a real ZFS/Btrfs pool, nor format/
unlock/erase a real LUKS volume — all these paths go through
`sudo zpool`/`btrfs`/`cryptsetup`/`mkfs`/`mount`. A real LUKS cycle
(create → verify listed → delete) is planned in `storage.spec.ts`
§13, **disabled by default**: it requires `STORAGE_E2E_LOOP=1` +
`STORAGE_E2E_LOOP_DEV=/dev/loopN` + a passwordless `sudo` (impossible
on the current dev machine — `sudo` there asks for a password). The
unit tests never touch a disk: the invalid-input branches of
`create_pool`/`delete_pool`/`start_scrub` are verified
as returning `Err` **before** reaching a `Command`.
**2 latent bugs found then FIXED on 2026-09-06 (explicit request
of the user to fix all the bugs found):**
1. `PoolService::parse_size` cast the parsed `f64` to `u64` *before*
   multiplying by the unit, so any fractional size of
   `zpool list -H` ("1.81T", "9.06G") was truncated to the integer of
   the unit — a 1.81 TiB pool reported as 1 TiB — and the decimal
   comma of a FR/DE locale ("1,81T" → "181" → 181 TiB) was not
   handled. `disk.rs::parse_size` had **exactly** these two bugs and
   had already been fixed ; `pools.rs` now reproduces the same fix.
   Non-regression tests: `test_parse_size_keeps_the_
   fraction`, `test_parse_size_accepts_a_locale_decimal_comma`.
2. `cache.rs::parse_rsync_stats` split the byte count of
   `rsync --stats` on the spaces before removing the `,` separator —
   under a locale grouping by space ("1 234 567 bytes") it
   recorded only `1`. Fixed: keeps only the ASCII digits
   before the unit word. Test:
   `test_parse_rsync_stats_survives_a_locale_thousands_separator`.

**Downloader** left this list on 2026-09-06: unit tests
`handlers/downloader.rs` (15 — serde round-trips against fixtures
captured as is on a live `aria2c` 1.37.0: the real RPC
format in camelCase must parse, the JSON returned to the frontend must stay in
snake_case — exactly the bug that the `#[serde(rename(deserialize))]`
fixed), `tests/downloader_integration.rs` (8) and
`tests/e2e/downloader/downloader.spec.ts` (41). **Real condition
really exercised:** a real `aria2c` downloads a file that
the application serves itself (`/wallpapers/hv-01-thumb.webp`, ~25 KB,
without Internet) into a disposable folder under `/tmp`, the test waits for the state
`complete` with the right byte count and the file really present
via `task.files[].path`, then deletes the task (trash button of the UI
_and_ endpoint) and verifies that it disappears from the list. **Never done:**
a real torrent/magnet, nor a remote source requiring Internet.
The integration never triggers the spawn of the `aria2c` daemon (any
endpoint other than `status` would launch it and talk to the daemon of the prod
server on port 6800) — it only locks the admin guard on
the 10 endpoints, the body validation (422 before any logic) and the
only path without side effect (`GET /status`). `downloader.rs` is a
_frozen_ page: only a `#[cfg(test)]` block was added at the end of the
file, no production code modified.

**Explorer (File Explorer)** gained its integration + E2E tests on
2026-09-06 (it only had unit tests `browser.rs`/`config`):
`tests/explorer_integration.rs` (29), `tests/e2e/explorer/explorer.spec.ts`
(50, **49/49 green** against the prod binary), + a `#[cfg(test)]` block
in `src/handlers/explorer.rs` (7). **What is really exercised:**
all the Explorer handlers being simple filesystem calls (no
`sudo`, no service, nothing under `/etc`), the integration drives the real
success paths end to end in a `tempfile::tempdir()` —
mkdir/list/rename/delete, copy+move of a file, text preview, recursive
search, chmod get+set, compress→extract, multipart upload (+ rejection of a
traversing name), byte-exact download (+ folder→400, absent→404), the
transfer quota guard on download (429) and on upload, and the trash
cycle trash→list→restore→purge / empty. The E2E confines all mutation
to a single folder `/tmp/hv-fx-e2e-*` (never `DELETE /api/explorer/trash`
which would empty the trash of a real deployment) ; a disposable non-admin
account proves the `PathGuard` confinement.
**5 robustness bugs found then FIXED on 2026-09-06 (frozen
files, explicit authorization "fix"):**
1. `FileBrowser::restore_trash_item` had no cross-device fallback
   (unlike `trash_item`/`move_item`/`copy_item`) → restoring a
   trashed file from a RAID/pool mount failed with an opaque IO
   error. Addition of the same rename→copy+delete cascade on EXDEV.
2. `find_trash_entry` propagated the `read_dir` error of a nonexistent
   trash folder instead of treating it as `NotFound` → as soon as
   `/home/.homevault-trash` did not exist (normal case), `restore` **and**
   by-id `purge` returned "No such file or directory" and the by-id trash
   was **broken on a real deployment** (verified live).
   Fixed: `ErrorKind::NotFound` → move on to the next folder.
3. `list_directory` failed entirely (`entry?` / `metadata()?`) if a
   single entry posed a problem (broken link, permission, race with a
   concurrent delete) — a dubious file hid the whole folder.
   Fixed: faulty entry ignored, the list continues.
4. No ceiling on the number of entries (a listing of 100k files =
   100k DOM lines). Addition of `MAX_DIR_ENTRIES = 10_000` (after sorting, so
   stable slice) + `truncated` field on `DirectoryListing` + mention
   in the footer (i18n key `explorer.listing_truncated` ×4) +
   filtering of the junk files of other OSes (`.DS_Store`, `Thumbs.db`,
   `._*`, …).
5. Double-click on a folder row unreliable: the 1st click
   selects → inserts the action bar → shifts the list → the 2nd click
   (and the `dblclick` event) misses the row. Fixed in `Explorer.svelte`
   by a double-click detection in JS based on the **identity** of the
   entry (path) and not the screen position (`handleRowClick`), insensitive to
   reflow ; `ondblclick` removed from the rows/tiles.
**No tests added for these 5 fixes nor recompilation** (explicit
request of the user) — to do: `cargo test`, `npm run build`,
then re-run of `explorer.spec.ts` (test 13.2 can then re-include
restore + by-id purge, today bypassed because of bug no. 2).
**+2026-09-23: `cargo test --locked` executed for the first time on this
machine** (Rust toolchain installed via rustup — stable 1.98.1, the machine did
not have one): **green on all the suites, 0 failure** — the `cargo
test` part of this "to do" is therefore covered ; `npm run build` and the
re-run of `explorer.spec.ts` remain.

**2026-09-06 (8 features inspired by the reference design added in 2 batches,
explicit request, without compile or tests):**
Batch 1 — named roots in the sidebar (`GET /api/explorer/roots`) + breadcrumb
with labels ; inline text/code editor (`POST
/api/explorer/save`) ; auto-refresh watcher (8 s poll) ; management of the
copy/move/upload collisions (`?strategy=` : `overwrite`/`keep_both`/`skip`
+ 3-choice prompt).
Batch 2 — favorites pinned in the sidebar (localStorage) ; safe ejection
of USB disks (`POST /api/explorer/eject`, admin) ; "Share this
folder" action which opens the pre-filled Shares app (new store
`explorerShareIntent` + 1 hook in `Desktop.svelte` and `Shares.svelte`) ;
read-only navigation in a past ZFS snapshot (`GET
/api/explorer/snapshots` → `.zfs/snapshot/…`, `snapshotMode`).
**Coverage (caught up):** `explorer_integration.rs` +11 on 2026-09-07
(`/roots`, `/save`, `/eject`, `/snapshots`, favorites, `?strategy=`) ;
`explorer.spec.ts` **section 15 on 2026-09-08** (9 tests, 2 consecutive
green runs) — inline editor (UI), `/save` (writes / overwrites /
rejects > 5 MiB), favorites (toolbar star ⇄ sidebar ⇄
server), collision strategies `skip`/`keep_both`/`overwrite` on
`/copy` and `/move`, Snapshots menu ("no snapshot" outside ZFS),
`/eject` (admin only + refusal of a non-mount-point), "Share this folder"
hand-off → the Shares app opens pre-filled. **Remains not
covered**: navigation *inside* a real ZFS snapshot (`snapshotMode`
with real data), a real USB `umount` — host required.

**RAID** left this list on 2026-09-05: live audit with
disposable loop devices, 9 bugs found and fixed (incl. a total blockage
of the creation — see `CLAUDE.md`), unit tests `disk.rs` (12) and
`raid.spec.ts` added. **2026-09-07:** after the RAID↔Storage re-scope
(RAID = creation wizard only), `raid.spec.ts` was
entirely rewritten (42 `test(`, 43 runtime tests, 14 categories) for the
new wizard ; it never drives a real creation (each test stops before "Confirm
creation", the server validation is exercised by direct `fetch` which 400s before any
`mdadm`/`zpool`) except the opt-in cycle `RAID_E2E_DEVICES`. Run 2026-09-07
against the rebuilt binary: **35 passed / 8 skipped / 0 failure** (the 8 skips require
≥2 free disks on the host to reach step 2/3). One test fixed
after the first run (`10.1` — a POST without a session returns 400, not
401/403: that is the "no token" branch of `require_admin`).

**Rescue** left this list on 2026-09-05: `tests/rescue_integration.rs`
(28) + `tests/e2e/rescue/rescue.spec.ts` (53, never any mutation against a
real device — see the file header) added. Three real bugs found
and fixed (explicit authorization): (1) `get_ddrescue_log` called
`ddrescue --status`, an option that does not exist — the status of an
ongoing clone was therefore always wrong (running=true/0%) ; replaced by a real
parsing of the ddrescue mapfile. (2) 6 streams (ddrescue clone, TestDisk
analyze/rebuild, PhotoRec, mdadm diagnose/repair) displayed no
error on failure — empty `else` branches ; fixed via
`notificationsStore` (same i18n keys already present but never wired).
(3) the shared app header (then `AppHeader.svelte`, since merged into
`WindowHeader.svelte`, ~13 apps): keyboard arrow navigation in
the tabs compared `e.key` to `'IconArrowRight'` etc. instead of
`'ArrowRight'` — totally dead everywhere, fixed.

**Network** and **VPN** left this list on 2026-09-06:
`tests/network_integration.rs` (23) + `tests/e2e/network/network.spec.ts`
(43), `tests/vpn_integration.rs` (22) + `tests/e2e/vpn/vpn.spec.ts` (39),
plus the unit tests `network.rs` (32, +3 `build_resolv_conf` on
2026-09-07) / `vpn.rs` (14). The automated suites do not drive any
real mutation (change an IP, bring down an interface, connect to a WiFi,
bring up a tunnel) — only the access guards and the validation
rejections. **2026-09-07**: the application path of `src/network.rs` was
fixed (mutations `ip`/`dhclient` wrapped in `sudo`,
`/etc/resolv.conf` via `sudo tee` without clobber) and verified live on
a disposable `dummy` interface — see the "Network" invariant of `CLAUDE.md`
and `docs/guides/network.md`.

**Shares/Protocols (Shares)** left this list on 2026-09-06:
`tests/shares_integration.rs` (18) + `tests/e2e/shares/shares.spec.ts`
(51), plus the unit tests of the permission parser (`permissions.rs`
10), of `ShareType` + default configs (`shares.rs` 12) and of the
public SSH key validator (`sftp.rs` 6). The suite never creates / deletes
/ toggles a real share, never starts / stops a real share service,
never grants / revokes a real ACL and never provisions a real SFTP
Linux account nor a bind mount — all these paths go through `sudo` (`tee`/`systemctl`/`exportfs`/`useradd`/
`mount --bind`) or write a root file under `/etc`. The mutations are
exercised only up to confirming a clean rejection (400/401/403, never
a panic/500) on absent/non-admin session or invalid input. E2E suite
launched for real against the prod binary on 2026-09-06: **53/53
green** (Firefox), no real mutation triggered.
**One bug found and fixed (explicit authorization, frozen page):** the
keyboard arrow navigation in the tab bar of `Shares.svelte`
(`onTabKeydown`) compared `e.key` to `'IconArrowRight'`/`'IconArrowLeft'`/…
(icon component names) instead of `'ArrowRight'`/`'ArrowLeft'` —
exactly the bug fixed on 2026-09-05 in the shared app header
(then `AppHeader.svelte`, since merged into `WindowHeader.svelte`), but
here in the page's local copy. The arrow
navigation was totally dead (only Home/End worked) ; fixed and
verified by `shares.spec.ts` test 11.1 (arrows + wrap + Home/End).

**Security suite (Antivirus / Anti-Ransomware / Firewall)** left
this list on 2026-09-06: unit tests (`firewall.rs` 9 — parsing of
`ufw show added` + detection of the mandatory rules ; `antivirus.rs` 6 —
parsing of `clamscan -i` ; `ransomware_watcher.rs` 7 — event
classification + protected path matching ; `ransomware.rs` 3),
`tests/security_integration.rs` (18) and
`tests/e2e/security/security.spec.ts` (30). **Real conditions
really exercised:** a real `clamscan` of an EICAR file in a
disposable folder (detection → quarantine → threat line → resolution),
and real bait files (honeypots) written then deleted on disk
and verified. **Never done:** enabling/disabling the real firewall of the
host or deleting a real `ufw` rule — when `sudo ufw` asks for a
password (common case) `ufw_available()` is `false` and the firewall
card is verified only for the "not installed" state + the access
guards ; the `{confirmed:true}` lock and the 403 on a mandatory rule are
tested live (always without ever enabling) only if a real `ufw`
is reachable.
**A race found and fixed (explicit authorization, frozen page):**
`parse_clamscan_line` read `quarantine_path.exists()` at the exact
moment `clamscan` prints the `FOUND` line, which raced with the `--move`
that follows — on slow storage the threat line was recorded
`action:"left_in_place"` / `quarantine_path:null` whereas the file *is*
indeed moved a handful of milliseconds later (so a later "Resolve + delete"
did not clean the quarantined copy). Fixed: `parse_clamscan_line(&line)`
now only parses path+signature, and a new `resolve_move_status()` re-resolves
the disposition of each threat in a loop *after* `child.wait()`,
when all the moves are finished. Verified by
`security.spec.ts` test 3.1 (real EICAR scan end to end) +
2 unit tests `resolve_move_status`.

**Docker** left this list on 2026-09-05: complete E2E suite
`docker.spec.ts` (49 tests, 14 categories) written and debugged against a
real Docker Engine — no application bug found, only bugs of the
test file (locators badly scoped on the modal which is a DOM sibling
of `.dk-shell`, not a descendant ; close button selector too
broad which intercepted the "Close" button of the hidden Resource Monitor widget ;
`fetch` called in `page.evaluate` before any navigation).

## Live real test 2026-09-07 (Firewall, Shares/SFTP, RAID/Storage, Journal)

The 4 areas exercised end to end against the real host (temporary
NOPASSWD `sudoers` drop-in, 6 loop devices, real `ufw` cycle with
port 22 + the source SSH IP pre-authorized, `homevault-sftp` `sshd`). The whole
environment was restored afterwards (drop-in removed, loops detached,
test accounts/shares/rules deleted, `ufw` left inactive).

**10 bugs fixed (frozen files, explicit authorization), all
re-verified live on the rebuilt binary:**

| # | File(s) | Bug | Impact before fix |
|---|---|---|---|
| 1 | `journal_cleanup_scheduler.rs`, `storage/journal_cleanup_repo.rs`, `handlers/journal.rs` | The auto-cleanup scheduler (default `on`/90 d since G1) runs on the 1st immediate tick ; empty `journal_cleanup_settings` table → `last_cleanup_at=None` → `due=true` ; `set_last_cleanup` was a no-op `UPDATE … WHERE id=1` | `clear_app_logs()` deleted `homevault.log.<day>` just after the appender opened it → **"app" source of the Journal permanently empty, at each restart** ; all the application audit (login/2FA/user-group-share CRUD/Docker/security…) lost. Fixed: `None` branch = set a baseline without cleaning + `set_last_cleanup` UPSERT + `clear_app_logs` **truncates** the day's file instead of deleting it (keeps the inode open by the appender) |
| 2 | `disk.rs::mount_and_persist_raid` | Bare `Command::new("mount")` / `Command::new("blkid")` | mdadm RAID creation: "only the superuser can use mount" → array created but not mounted then tear-down. OK in prod (root), broken outside root, inconsistent with the `sudo mkdir`/`sudo mdadm` right next to it |
| 3 | `disk.rs::create_zfs_pool_from_devices`, `pools.rs::create_zfs_advanced_pool` | `zpool create -f <vdev-type> <name> …` — the pool name must precede the type | **All ZFS creation failed** (`cannot open '<name>': no such device`) for raid1/5/6/10 ; only raid0 passed. Breaks even as root. Also `Command::new("zpool")` without `sudo` |
| 3b | `pools.rs::scan_pools` (`get_mdadm_devices`, `get_mdadm_mount_point`, `get_mdadm_size`, `get_zfs_devices`, `get_zfs_mount_point`) | Storage "Virtual disks" view: wrong `device_count` (mdadm 2 disks → 1 ; ZFS mirror 2 disks → 5), `mount_point` always `null` (mdadm: match on the symlink not `/dev/md127` ; ZFS: invalid `zpool get mountpoint` → `zfs get` was needed), `used_bytes` mdadm = fabricated `size/2`, `size_bytes` mdadm under-counted ×1024 (KiB read as bytes) | Misleading metadata (never blocking). Fixed: parse of the `mdadm --detail` table, `canonicalize`, `zfs get`, real `df`, `blocks*1024` |
| 4 | `disk.rs::delete_raid` | Bare `Command::new("umount")` | Destruction of an mdadm virtual disk: `umount` fails silently → `mdadm --stop` "device busy" → **delete aborted, array + `/etc/fstab` + `mdadm.conf` left in place** |
| 5 | `shares.rs::set_share_enabled` (SMB) | End of section located via `after_section.find("\n[")` — `None` for the **last** section of `smb.conf` | The freshly created SMB share (always added at the end of the file) **can never be disabled** as long as another is not added after it. Lying `"Share disabled"` response. Breaks even as root |
| 6 | `shares.rs` (create/update/delete `_sftp`/`_ftp`/`_webdav`_share, `save_*_config`) | Bare `fs::write` / `fs::create_dir_all` to `/etc/homevault/*.json` (root dir) | Creation of an SFTP/FTP/WebDAV share: `Permission denied` outside root ; inconsistent with `write_file_as_root` everywhere else. Fixed + new helper `mkdir_as_root` |
| 7 | `acl.rs` (4 `setfacl` sites) | Bare `Command::new("setfacl")` | Any share ACL grant/revoke: `EPERM` outside root if the folder does not belong to the process |
| 8 | `acl.rs::remove_acl` | Removes the access entry but not the corresponding **default** ACL | A revoked user remained granted on any file created afterwards in the share (`default:user:X:rwx` persisted) |
| 9 | `handlers/shares.rs::handle_sftp_key_set` | No check that the `username` is a HomeVault user | `PUT /api/shares/sftp/users/key` with any name → real `useradd` → orphaned Linux accounts on a typo. Fixed via `state.auth.user_exists()` |
| 10 | `firewall.rs::get_status()` | Greps the English string `Status: active` | On a non-English locale host (`fr_BE` → `ufw` displays `État : actif`, and `LANGUAGE` overrides even `LC_ALL=C`) → `enabled` **always `false`**: the Firewall card shows "disabled" while `ufw` is active. Fixed: `ufw_command()` helper which forces `LC_ALL=C`/`LANGUAGE=` (kept by the default `env_keep` of `sudo`) |

**Verified OK live (no bug):** firewall `{confirmed:true}` guard,
403 on deletion of a mandatory rule, rules visible with `ufw`
disabled, `ensure_mandatory_rules` (SSH survives the `enable`) ;
`rebuild_jails` (bind-mount + `fstab`), `purge_user` on deletion of a
HomeVault user, default ACL (`-d`/`-m` order) ; native Btrfs complete
cycle ; `sudo journalctl`/`tail` + unit/priority/search filters of the
Journal ; rejection of traversal `source=../../etc/passwd`.

**Still not covered by automated test** (requires the real host — remains
E2E/manual): a real `ufw enable` (possible network restart), a
real SMB/NFS/SFTP share created, a real `mdadm`/`zpool create`. The
integration suites still stop at the "clean rejection".

## 2026-09-07 (continued) — de/es i18n + rest of the host tests

**de/es i18n (done).** `src/i18n/de.toml` and `es.toml` had the same
defect as `fr` (whole sections written without diacritics). Restoration
of the accents **values only** (keys + `%placeholders%` verified
intact, `tomllib` + `cargo build --release` clean): ~350 lines each,
via curated whole-word maps (`fix_de.py` / `fix_es.py`, scratch dir).
`de`: digraphs `ae/oe/ue/ss` → `ä/ö/ü/ß` (false positives `aktuell`,
`daemon`, `neue`, `manuell`, `quell`, `steuer`, `virtuelle`, … excluded).
`es`: singular `-ción/-sión`, `éxito/código/tamaño/batería/contraseña/…`,
a few future tenses, `¿?` on the obvious questions ; plurals `-ciones` and
`solo` left as is (correct). The embedded `de`/`es`/`fr` block of the
frontend (`i18n.ts`) was already correctly accented — untouched.

**2nd pass fr + es ("do another tour of the project").** The 1st fr pass had
wrongly classified `[settings]` `[shares]` `[users]` `[groups]` `[docker]`
(residuals), `[journal]` `[common]` `[cache]` `[account]` as "already
accented" — they were in fact largely bare. Fixed via `fix_fr.py`
(~155 lines, whole-word ; all the ambiguous verb/present forms —
`utilise`, `gere`, `affiche`, `chiffre`, `complete`, `limite`, `detecte`,
`synchronise`, `verifie`, `deploye`, `genere`, `execute`… — **excluded from
the map**). 5 imperative-vs-participle mis-accents ("Installe/Supprime/
Démarre…") + `À propos`/`À utiliser` + 2 `_synced` fixed by hand
after reviewing the diff. `es`: `máscara`, `estática`, `público`, futures
`aparecerán/aplicarán/formarán/moverán/perderán` (~11 lines).

**Complete linguistic verification + removal of the orphans
(2026-09-08).** Key parity now **identical** in the 4
languages (`en:common.browse_button` → `common.browse`) ; `%placeholders%`
consistent everywhere (0 discrepancy) ; no empty value ; no English leak.
Last accent residues fixed (26 lines: fr 9, de 7 "bare
vowel" — `konnte`/`wurde` = correct preterite, untouched —, es 9).
**825 orphan keys removed** from the 4 files (3048 → **2223**) —
analysis: all the enumerated i18n lookups (static + fixed list of
dynamic prefixes + `docker.${actionKey}` with 6 values), 0/825 cited
bounded in the source, 25/25 clean spot-checks, `cargo build` +
`svelte-check` 0 errors, E2E `dashboard`/`health`/`widgets`/`settings`/
`explorer`/`network` green.

**2026-09-14 — 2nd pass, orphans of the header standardization.**
19 more keys removed from the 4 files (2522 → **2503**): they are the
subtitles removed from the `WindowHeader`s — `docker.containers_summary`,
`users.summary`, `quotas.subtitle`, `groups.summary`, `shares.summary`,
`account.subtitle_{admin,user}`, `dashboard.overview_subtitle`,
`rclone.subtitle`, `raid.manager.header_subtitle`, `network.subtitle`,
`chat.subtitle_default`, `chat.model_via`, `vpn.subtitle`,
`downloader.subtitle`, `syncthing.subtitle`, `widgets.subtitle`,
`rescue.subtitle`, `journal.subtitle`. Removal **by (section, key)** and
not by leaf name: six other `subtitle` are alive and were kept after
verification one by one (`settings.` = rail label +
nav `aria-label`, `restic.` = `tabsLabel`, `login.` = login page,
`antivirus.`/`ransomware.`/`firewall.` = `SettingsCard` subtitles).
`docker.${actionKey}` re-checked — it only produces the 6
`{container,stack}_{started,stopped,restarted}`, `containers_summary` is
therefore safe. Verified afterwards by script: 2503 keys in each of the 4
languages, **identical** key sets, identical `%placeholders%` sets
(0 discrepancy), 19/19 keys absent everywhere, 6/6 kept keys present
everywhere, no emptied section. **Nothing was compiled or re-run.**

The test counters of this document do not move: this pass only touches
`src/i18n/*.toml` and `size=` attributes of icon snippets.

**Remains**: native review of `es` (verb tenses in long sentences) ;
a few scattered `a`↔`à` in `fr`.

**Real host tests (temporary `/etc/sudoers.d` NOPASSWD drop-in, removed
afterwards ; 4 loop devices + a disposable RAID1 ; environment restored).**

- **Rescue — DONE (never exercised live before). 6 bugs fixed** (all
  frozen `src/rescue/*`, re-verified live): ddrescue/testdisk/photorec
  never `sudo`-wrapped (only mdadm was) → on non-root everything failed
  silently (`success:true` + empty partitions) ; `list_block_devices`
  parsed `lsblk -o` positionally (MODEL has spaces → wrong model/
  transport/removable) + private `parse_size` with the
  cast-before-multiplication bug + locale comma ("465,8G" → `size:0`) —
  rewritten as `lsblk -P -b` ; `mdadm_diagnose` read the header
  line `/dev/md127:` instead of the member table ; health of a degraded
  mirror (`"clean, degraded"`) returned `healthy` (order of the `contains`) ;
  `parse_testdisk_output` offsets wrong for TestDisk 7.1 `/list` (never
  executed because no sudo) → rewritten + 3 unit tests ;
  `testdisk_rebuild` never worked (stdin script ignored, device
  missing) **and could now block the worker** (TestDisk 7.1
  `/cmd` `write` waits for a Y/N that stdin cannot give) → reduced to
  `analyze,search` + `stdin(null)` + 300 s timeout + `kill_on_drop`. See
  the "Storage / RAID / encryption" invariant of `CLAUDE.md` and
  `docs/guides/rescue.md`.
- **Storage / LUKS — DONE. Complete cycle** create → write a file →
  `/lock` → `/unlock` (wrong passphrase rejected, correct one brings it back, data
  intact) → `DELETE {confirm:true}` against a real `/dev/loopN` +
  `cryptsetup`. The lifecycle (sudo-wrap + `write_file_as_root` of
  2026-07-17) holds. **2 bugs fixed** (frozen): `handle_encryption_
  create` did **not** validate `req.device` → an admin could pass
  `/dev/sda` to `cryptsetup luksFormat` (now guarded by
  `get_available_devices` `!in_use`) ; `encryption::erase` left
  the LUKS header → device stuck `in_use` forever (addition of a
  best-effort `wipefs -a`). Invariant under "Storage / RAID / encryption"
  (`CLAUDE.md`) and `docs/guides/storage.md`.
- **Network — reads re-verified live + `sudo` bugs of the application path
  FIXED** (frozen `src/network.rs`, on "fix what is needed").
  Reads OK (`/interfaces`, `/diagnostics`, `/summary`, `/services`,
  `/public-ip`, `/wifi/status`, `POST /wifi/scan`), rejection of invalid body
  OK. Fixed: helper `ip_sudo(program)` — each *mutation* `ip`/`dhclient`
  (`addr add/flush`, `route replace`, `link set up/down`,
  `dhclient -6`) goes through `sudo` (the read `ip … show`/`ping`/`ethtool`
  remain bare) ; `/etc/resolv.conf` + the toggles
  `/proc/sys/net/ipv6/conf/<iface>/*` written via `sudo tee`
  (`write_as_root`) instead of a silent `std::fs::write` ; DNS no longer
  rewrites anything but the `nameserver` lines (`build_resolv_conf`, pure,
  3 unit tests — `search`/`options`/comments kept).
  **Real mutations still NOT exercised** (host over SSH) and the
  **persistence** (netplan / `/etc/network/interfaces` — `ip addr add` is
  runtime-only) remains a deliberate follow-up (backend choice + console
  session). Invariant under "Network" (`CLAUDE.md`) and
  `docs/guides/network.md`.

**2026-10-16 (four desktop/plumbing features).** Four features landed in one
pass, each with its own tests and doc updates:

1. **Single settings catalog** (`frontend/src/lib/stores/settingsCatalog.ts`)
   now feeds the Settings rail, the content header and the global ⌘K index
   (`settingsSearchItems`). E2E `desktop.spec.ts` 6.5–6.7 (keyword search,
   category activation, wallpaper shortcut). Doc: `docs/guides/settings.md`.
2. **Live usage by app** — `components/dashboard/AppUsageBreakdown.svelte` +
   `charts/AreaChart.svelte` + `lib/utils/appColor.ts`, embedded in the
   Dashboard. E2E `dashboard.spec.ts` 2.6/2.7 and 3.4/3.5 (menu freeze, menu
   contents, no mutation on open). Doc: `docs/guides/dashboard.md`.
3. **Per-member resource access** — migration `041_resource_shares.sql`,
   `storage/resource_share_repo.rs`, `handlers/resource_shares.rs`
   (`/shares/access` + `/shares/access/mine`, server-side filter helpers +
   WS whitelist). Tests: 5 unit + 10 integration
   (`tests/resource_shares_integration.rs`) + E2E `users.spec.ts` 17.1–17.4.
   Docs: `docs/guides/users.md`, `docs/API.md`.
4. **Web shortcuts + Edit Widgets** — SSRF-guarded
   `POST /shortcuts/fetch-metadata`, `stores/shortcuts.ts`,
   `DesktopShortcuts.svelte`; `routes/EditWidgets.svelte` +
   `stores/widgetLayout.ts` (drag & drop, max 3 featured). Tests: 7 unit + 3
   integration (`tests/shortcuts_integration.rs`) + E2E `shortcuts.spec.ts`
   and `edit-widgets.spec.ts`. Docs: `docs/guides/shortcuts.md`,
   `docs/guides/widgets.md`, `docs/INDEX.md`, `docs/API.md`.

`cargo test --lib` = **704** passing (was 689); the new guide is indexed by a
`FeatureDoc` (`shortcuts`) so `test_every_guide_has_a_feature_doc` stays green.

## 2026-10-17 — notification center, storage health, app-auth, client contract

Four areas, each with its own tests and docs:

1. **Persistent notification center + What's New.** Unit: `release_notes`
   (3), `api_contract` (5), `middleware::contract` (2). Integration:
   `tests/whats_new_integration.rs` (4 — auth guards, per-user `seen`,
   idempotence) and `tests/api_contract_integration.rs` (2 — unauthenticated
   `/api/meta` shape). E2E: `tests/e2e/desktop/desktop.spec.ts` 16.1–16.4.
   Docs: `docs/guides/notifications.md`, `docs/API.md`.
2. **Storage health — scrub status + foreign-pool import.** Unit:
   `handlers/storage.rs::scrub_in_progress` (2) and `pools::tests`
   `parse_zpool_import` / `parse_btrfs_show` / `import_pool` (8). Integration:
   `tests/storage_integration.rs` 28 (was 24) — the two new read/write
   endpoints join the auth-split tables, plus dedicated unknown-name and
   unsupported-type tests. E2E: `tests/e2e/storage/storage.spec.ts` 14.1–14.5.
   Docs: `docs/guides/storage.md`, `docs/API.md`.
3. **App-proxy auth hardening.** Unit: `app_proxy::app_auth` (5),
   `app_proxy::cookie` (3). Integration: `tests/app_proxy_integration.rs`
   (1 end-to-end flow asserting the API session is rejected, the app session
   is accepted, and both cookies are stripped upstream). Docs:
   `docs/guides/docker.md`, `docs/ARCHITECTURE.md`, `CLAUDE.md`.
4. **Versioned client contract + DX.** Covered by (1)'s unit/integration
   tests; `src/mdns.rs` gains a `mdns_hostname` unit test. Tooling:
   `scripts/dev-instance.sh`, `playwright.vm.config.ts` +
   `tests/vm/smoke.vm.test.ts` (opt-in real-VM suite). Docs:
   `docs/guides/development.md`, `docs/ARCHITECTURE.md`.

`cargo test --lib` = **734** passing (was 704). The new
`docs/guides/development.md` is registered as developer-only in
`rag_indexers.rs`, so `test_every_guide_has_a_feature_doc` stays green.

## Verification method

- `tests/*.rs` files listed via `ls`, counted via
  `grep -cE '#\[tokio::test\]|#\[test\]'` (number of test attributes =
  number of test functions).
- `tests/e2e/**/*.spec.ts` files listed via `find`, counted via
  `grep -cE '^\s*test\('` (the tests are indented in their `test.describe(` ;
  the pattern must be anchored at the start of the line). **Do not** use
  `grep -c 'test('`: it also catches the `.test(x)` of the regex
  assertions (`/…/.test(url)`) and over-counts. The number retained is
  that of Playwright — `npx playwright test <spec> --list --project=firefox`
  — which is the reference. **The two do not coincide, and that is normal**: a
  `test(` written **in a loop** is counted only once by the grep whereas
  Playwright expands it into N tests at collection (e.g. the responsive
  loop 360/768/1920 of `storage.spec.ts`: 1 line, 3 tests). The grep
  is therefore a **lower bound** ; the difference measures the tests generated by
  loop. Observed on 2026-09-24: **1367** at the grep against **1436** at
  `--list --project=firefox` (69 generated tests) on the 28 `.spec.ts` files.
- Unit tests per module: `grep -cE '^\s+fn test_'` in the source
  file, modules found via `grep -rl '#\[cfg(test)\]' src/`.
- The per-area statuses come from `CLAUDE.md` and from `docs/guides/` (not
  re-verified against the code in this pass — see the caveat of
  `ROADMAP.md` on the freshness of this type of information).

**2026-10-13 (b) — Explorer, Backup, VM and the 8-step Setup Wizard.** Four
feature sets landed together, each with its Rust tests; the Playwright specs
were **not re-run** (no fresh server in this session).

- **Explorer** — Rewind (per-file ZFS snapshot timeline, `GET /explorer/history`),
  server-side recents (`/explorer/recents` GET/POST/DELETE, migration
  `039_explorer_recents.sql`), fuzzy search (`?fuzzy=true`, `browser::fuzzy_score`),
  marquee-select, virtualized listing, internal drag & drop, SMB/network
  discovery (`/explorer/network-discovery`, `smbtree` + Avahi parsers), and
  ext4/exfat formatting (`/explorer/format`, two-value allow-list). `browser.rs`
  **11 → 15**, `handlers/explorer.rs` **17 → 22** (parsers + format allow-list),
  `explorer_integration.rs` **49 → 55**.
- **Backup** — reminders banner (`GET /restic/reminders`), split-view restore
  dialog with a debounced pre-flight space check (`GET /restic/restore-plan`),
  floating progress island (`backupJobs.svelte.ts` +
  `BackupProgressIsland.svelte`). `handlers/restic.rs` **+3** (`days_since`,
  `parse_df_available`, size/space comparison), `restic.rs` gains `restore_size`,
  `restic_integration.rs` **14 → 17**.
- **Virtual Machines** — Windows 11/10 + minimal Ubuntu/Debian catalogue entries
  with an `iso_only` flag, optional audio device, and "open the screen in a new
  tab". `incus.rs` **11 → 14**, `handlers/vm.rs` **14 → 17**,
  `vm_integration.rs` **22 → 24**.
- **Setup Wizard** — grew from 4 to 8 steps (welcome, language, storage, RAID,
  restore, account, password, verify) and became reboot-safe: the step + the
  non-secret choices persist in a singleton `setup_progress` row (migration
  `040_setup_progress.sql`) behind `GET/PUT /auth/setup/progress`, and
  `GET /auth/setup/storage-options` feeds the storage/RAID steps. Secrets are
  never part of the payload. `setup_integration.rs` **47 → 52**.

`cargo test` on 2026-10-13 (b): `--lib` **734**, `explorer_integration` **55**,
`restic_integration` **17**, `setup_integration` **52**, `vm_integration` **24**,
`rag_indexers` **3/3** (FeatureDoc coverage still green after the
explorer/setup/vm/backup entries were refreshed). `npm run check` 0 errors
(1 pre-existing a11y warning), `npm run build` OK.

To be updated on each new test file added or new manual audit
documented in `CLAUDE.md`.
