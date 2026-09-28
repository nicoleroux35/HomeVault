# UI Audit Tasklist — full-app design audit (2026-09-23)

Actionable backlog produced by the `ui-ux-audit` full pass over the whole
frontend: 241 `.svelte` files, 27 routes, shared layers (`components/app`,
`components/theme`, `components/ui`, `components/desktop`, `components/docker`).

- **Profile applied:** SaaS / dashboard (data-dense, keyboard-first, tables &
  charts first-class, dark mode expected).
- **Input type:** static code audit (no screenshots, no runtime pass) — anything
  that needs a running build is listed under [Verify in code](#verify-in-code-v1--v9).
- **Baseline score:** `100 - (0x12) - (2x8) - (18x4) - (1x1)` = **11/100**
  → verdict *Do not ship* (band < 40; no Blocker forced it).
- **Target after this backlog:** 100/100 (2 Critical → +16, 18 Warnings → +72,
  1 Polish → +1).

## How to use this list

1. Work top to bottom: Critical first, then the token/composite family (it
   clears the largest number of findings at once), then the rest.
2. One task = one commit-sized change. Check the box only when **both** hold:
   - `cd frontend && npm run check && npm run build` is green;
   - the touched screen's E2E suite still passes
     (`npx playwright test tests/e2e/<screen>/<screen>.spec.ts`), and
     `TESTING.md` is updated in the same change if a test was added/changed.
3. Update `docs/guides/design-system.md` in the same change whenever a task
   alters a token, a shared component, or a documented rule.
4. When a task fixes a *family*, also tick the matching [Durable fixes](#durable-fixes-d1--d4)
   item — the family is only closed once tooling prevents its return.

---

## Phase 0 — Critical (C1, C2) · +16 points

### [ ] C1 — Raise the 10px micro type scale above the profile floor (category 1)

- **Where:** `frontend/src/lib/styles/theme/tokens.css:614` (`--hv-font-size-micro: 10px`),
  consumed **58 times in 14 files**.
- **Impact:** sub-11px data/label text is a Critical under the SaaS profile
  ("Critical below ~11px"). Worst offenders carry real meaning:
  `routes/VirtualMachines.svelte:620-623` (`dt` spec labels),
  `routes/Storage.svelte:1090-1097` (disk detail labels),
  `routes/Rescue.svelte:619-636`, `routes/Raid.svelte:292-403`,
  `routes/Syncthing.svelte:429-431`, `routes/Terminal.svelte:650/671`,
  `components/desktop/WindowHeader.svelte:384`,
  `components/desktop/WindowSidebarItem.svelte:268`,
  `components/search/SearchItem.svelte:103`,
  `components/theme/FloatingPathPicker.svelte:553/581`,
  `components/explorer/ShareLinkModal.svelte:240`,
  `lib/styles/theme/components.css:244`.
- **Fix:** `--hv-font-size-micro: 11px;` then re-read the 14 consumers for
  `letter-spacing` / `line-height` / fixed heights that assumed 10px (badges
  with `px-2 py-0.5` may grow one pixel — that is intended).
- **Done when:** no rendered text below 11px anywhere
  (`grep -rn "hv-font-size-micro" frontend/src` → all values ≥ 11px).

### [ ] C2 — Move the 5 hand-rolled Docker modals onto `theme/Modal` (categories 9, 5)

- **Where:** `components/docker/ImagePullModal.svelte:70-71`,
  `ContainerCreateModal.svelte:100-101`, `LogsModal.svelte:65-66`,
  `DeleteConfirmModal.svelte:34-35`, `ComposeModal.svelte:69-70`.
- **Impact:** they render `<div class="modal" role="document">` + a
  `role="button" tabindex="0"` backdrop — never `role="dialog"` /
  `aria-modal="true"`, no focus trap, no focus restore, English-only labels.
  The SaaS profile escalates missing keyboard operability on a core flow.
- **Fix:** rebuild each modal on `components/theme/Modal.svelte` (bits-ui Dialog
  already provides trap + Escape + focus restore) and `ConfirmDialog` for
  `DeleteConfirmModal`; move the current skin into the `header`/`footer`
  snippets. Delete the `.modal-backdrop` keyboard handler once bits-ui owns it.
- **Done when:** opening/closing each Docker modal with mouse **and** keyboard
  keeps focus inside and returns it to the trigger; axe reports no
  `aria-dialog-name` / `aria-hidden-focus` violation.

---

## Phase 1 — Tokens & shared composites (W1–W6) · +24 points

Largest family: raw values and per-screen reimplementations.

### [ ] W1 — Unify the two colliding type vocabularies (categories 1, 10)

- **Where:** `frontend/src/app.css:59` `@theme inline` maps colors/radius but
  **no `--text-*`**, so `text-xs|sm|base` keep Tailwind defaults (12/14/16px)
  while `text-[length:var(--hv-text-*)]` uses the compact scale (11/12/13px).
  Counts: `text-xs` ×512 vs `text-[length:var(--hv-text-…)]` ×519.
  `routes/Desktop.svelte:5012` even hardcodes `var(--hv-text-sm, 13px)`
  (token is 12px).
- **Fix:** in `@theme inline` add
  `--text-xs: var(--hv-text-xs); --text-sm: var(--hv-text-sm); --text-base: var(--hv-text-base); --text-lg: var(--hv-text-lg); --text-xl: var(--hv-text-xl);`,
  then migrate `text-[length:var(--hv-text-…)]` → plain `text-*` and drop the
  wrong `13px` fallback in `Desktop.svelte:5012`.
- **Prevent:** covered by [D1](#durable-fixes-d1--d4) (grep gate on
  `text-[length:`).

### [ ] W2 — Replace hand-rolled cards/radii with `StatCard` / `SectionCard` / `EmptyState` (categories 10, 2)

- **Where:** off-scale radii `rounded-[18px]` ×5
  (`routes/Dashboard.svelte:194`, `routes/Storage.svelte:700`,
  `routes/Shares.svelte:542/546/747`) and `rounded-[20px]` ×8
  (`routes/Shares.svelte:564/598/617/634/654/671/678`,
  `routes/Groups.svelte:518`), plus `p-[18px]`, `py-[22px]`.
  Adoption: `StatCard` **0 uses**, `EmptyState` 2/27, `DataTable` 1, `FormRow` 1,
  `SectionCard` 6 — while Dashboard/Shares/Storage/CloudSync define their own
  `statCard` / `sectionCard` constants. `components/docker/*` uses
  `var(--hv-radius-md, 6px)` (wrong fallback: md = 8px).
- **Fix:** swap each local stat/section card for `<StatCard>` / `<SectionCard>`;
  map 18/20px to `var(--hv-radius-card)` (12px) or `var(--hv-radius-2xl)` (16px);
  correct the Docker fallback to `8px`; replace hand-rolled empty blocks in the
  ~19 routes with `<EmptyState>`.
- **Done when:** `grep -rn "rounded-\[1[789]px\]\|rounded-\[2[0-9]px\]" frontend/src`
  returns nothing, and `StatCard`/`EmptyState` are imported by the screens that
  render stats/empty lists.

### [ ] W3 — Put spacing back on the 4px rhythm (category 2)

- **Where:** `px-3.5` ×74 (14px), `gap-3.5` ×37, and 18 magic literals
  (`9px` ×5, `10px` ×9, `7px` ×3, `18px`, `22px`, `60px`), `p-[18px]`,
  `py-[22px]`, `pt-[140px]` (`routes/Login.svelte:387`).
- **Fix:** `px-3.5`→`px-3`/`px-4`, `gap-3.5`→`gap-3`/`gap-4`, literals →
  `var(--hv-space-*)`. Keep `Login`'s large optical offsets only if a comment in
  `tokens.css` documents them as intentional.
- **Prevent:** [D1](#durable-fixes-d1--d4).

### [ ] W4 — Kill raw hex / off-palette values (categories 4, 10)

- **Where:** 248 raw hex (77 unique) across `.svelte`. Live off-token values:
  `routes/Explorer.svelte:1231` `text-[#6366f1]`;
  `components/theme/FloatingPathPicker.svelte:111/113` and
  `components/theme/FolderPicker.svelte:85/86` `#6366f1`/`#0ea5e9`
  (tokens `--hv-app-*-c` already exist);
  `routes/Storage.svelte:623` raw donut palette `[… '#8b5cf6', '#ec4899', '#6366f1']`;
  `routes/Docker.svelte:1405/1455` `bg-[#1e1e1e] text-[#d4d4d4]`;
  `routes/Terminal.svelte` theme `#0f6cbd`;
  `components/docker/*` Google blue `rgba(26,115,232,…)` + `#1a73e8` fallbacks
  and `#e53935`;
  Material-ish fallbacks `#191c1e/#e5e7eb/#9ca3af` in `Path*` components;
  raw `z-index: 9000` (`FloatingPathPicker.svelte:463`) and `10000`
  (`FolderPicker.svelte:308`) vs the `--hv-z-*` scale.
- **Fix:** map each occurrence to its existing token (`--hv-app-*`, `--hv-primary`,
  `--hv-error`, `--hv-z-dropdown`, `--hv-shell-border-soft` /
  `--hv-shell-text-strong` for the Docker code viewer); delete stale
  `var(--hv-*, #hex)` fallbacks (all tokens are defined at `:root` — one of them,
  `#9CA3AF`, would render 2.54:1 if it ever did).
- **Prevent:** [D1](#durable-fixes-d1--d4).

### [ ] W5 — Collapse the three button systems into one (categories 10, 11)

- **Where:** 348 raw `<button>`; `theme/Button` imported ~24 times;
  `.hv-btn hv-btn-primary` documented in `lib/styles/theme/README.md:139` but
  used **once** (`FolderPicker.svelte:284/287`); per-screen copies:
  `primaryBtn`/`ghostBtn`/`iconBtn` (`routes/Users.svelte:666-696`),
  `iconBtn` (`routes/Groups.svelte:104`, `routes/Shares.svelte:482`,
  `routes/Docker.svelte:974`), `act`/`textInput` (`routes/Docker.svelte:966-974`),
  `vmBtn` (`routes/VirtualMachines.svelte:447`),
  `actionBtn` (`routes/Downloader.svelte:227`).
- **Fix:** add shared `iconBtn` / `actionBtn` class constants to
  `components/app/` (or migrate to `<Button>`), delete the local duplicates;
  decide in `docs/guides/design-system.md` whether `.hv-btn` stays (adopted) or
  is retired (documented as superseded by `theme/Button`).

### [ ] W6 — One icon family, no text glyphs (category 11)

- **Where:** `routes/Restic.svelte:928/1023` `✕` as button content,
  `routes/Docker.svelte:1226` `★ {r.star_count}`, `routes/Settings.svelte:935` `✓`,
  `components/theme/FolderPicker.svelte:199` `✕`; plus three families in
  parallel (sprite `<Icon>` in app code, `@lucide/svelte` ×35 inside
  `components/ui/**`, inline SVG in Dashboard/Desktop/SetupWizard/Login).
- **Fix:** replace the glyphs with `<Icon>` (`x`, `star`, `check`), and write the
  layer rule into `docs/guides/design-system.md`: sprite `<Icon>` for app code,
  Lucide only inside `components/ui` primitives, inline SVG only for artwork
  (wallpapers/orbs).
- **Prevent:** [D4](#durable-fixes-d1--d4) (no hardcoded glyph strings) is
  i18n-focused; for icons add the rule to the design-system checklist instead.

---

## Phase 2 — Accessibility, navigation, keyboard (W7–W10, W16–W18) · +28 points

### [ ] W7 — Route every visible/AT string through i18n (categories 14, 9)

- **Where:** `components/theme/Alert.svelte:79` `aria-label="Fermer"` (French
  hard-coded in a shared component while the app uses `$t('common.close')` in 16
  other places); `components/docker/*`: `aria-label="Close"` ×4
  (`ImagePullModal:77`, `ContainerCreateModal:107`, `LogsModal:79`,
  `ComposeModal:76`), `"Refresh logs"` (`LogsModal:76`), `"Remove port"`
  (`ContainerCreateModal:165`), untranslated titles/labels
  (`Container Name`, `Docker Image`, `Restart Policy`, `Memory Limit`,
  `CPU Limit`); `components/ui/pagination/*`: `Go to next page` /
  `Go to previous page` ×4.
- **Fix:** `$t('common.close')`, `$t('common.refresh')`, `$t('docker.*')` keys
  added to both locale files; pagination labels via the shared `ui` i18n hook
  used by the rest of `components/ui`.
- **Prevent:** [D4](#durable-fixes-d1--d4).

### [ ] W8 — Give every screen exactly one `<h1>` (categories 9, 3)

- **Where:** `<h1>` exists only in `routes/Account.svelte:385`,
  `routes/SetupWizard.svelte:807/830/882/997`, `routes/Login.svelte:401/444/492`,
  `routes/Users.svelte:851/1052`; `components/desktop/WindowHeader.svelte`
  renders the window title in `<div class="hv-winhdr-title">`; 23/27 routes have
  no `<h1>`; `routes/CloudSync.svelte` starts at `h2/h3`;
  `routes/Shares.svelte:750` forces `h2 text-base`.
- **Fix:** render the `WindowHeader` title as `<h1>` and demote in-route titles
  to `h2`/`h3` (the four routes that already render their own `<h1>` become the
  exception documented in the guide).
- **Done when:** each of the 27 screens exposes exactly one `<h1>` in the
  accessibility tree.

### [ ] W9 — Mark the active sidebar section (categories 9, 11)

- **Where:** `components/desktop/WindowSidebarItem.svelte` (and
  `HvLaunchpad.svelte` category list) never set `aria-current`; list rows in
  `routes/Downloader.svelte:341`, `routes/Users.svelte:807`,
  `routes/Syncthing.svelte:347` do it correctly.
- **Fix:** `aria-current={active ? 'page' : undefined}` on nav items — the SaaS
  profile requires the current section to be indicated beyond colour.
- **Prevent:** [D2](#durable-fixes-d1--d4) (axe rule `aria-current-allowed` /
  `landmark` checks catch the missing value once the screens are scanned).

### [ ] W10 — Add a render error boundary per window (category 5)

- **Where:** `grep -rn "svelte:boundary" frontend/src` → 0 matches.
- **Fix:** wrap each window's route content in `<svelte:boundary>` with a
  fallback snippet ("This panel failed to render" + reload action via i18n), so
  one throwing widget cannot blank the whole window.
- **Test:** E2E that forces a render error (throw in a dev-only branch) asserts
  the fallback and that the rest of the desktop still works.

### [ ] W16 — Restore the shared focus ring in Docker modal fields (categories 3, 9)

- **Where:** `.form-input:focus { outline: none; border-color: var(--hv-primary, #1a73e8);
  box-shadow: 0 0 0 3px rgba(26,115,232,0.1); }` in
  `ContainerCreateModal.svelte:326-331`, `ImagePullModal.svelte:237-241`,
  `ComposeModal.svelte:304-309`. Specificity (0,2,0) beats
  `base.css:269 input:focus-visible` (0,1,1), so the system ring
  `--hv-focus-ring` (2px `rgba(30,98,174,.9)` + halo, ≈6.15:1) never applies.
- **Fix:** delete those `:focus` blocks (or use `box-shadow: var(--hv-focus-ring)`);
  most of this task disappears anyway with [C2](#c2--move-the-5-hand-rolled-docker-modals-onto-themodal-c2--16-points).
- **Note:** severity stays Warning, not Blocker — the border-colour change keeps
  the indicator visible.

### [ ] W17 — Give every compact icon button the shared `.hv-ic` hit square (category 7)

- **Where:** missing `hv-ic`/`hv-ic-abs`: `routes/Terminal.svelte:629`
  (tab close, **20px**), `routes/Docker.svelte:1123` and
  `routes/Settings.svelte:980` (**28px**),
  `components/explorer/ShareLinkModal.svelte:249/258` (**32px**),
  the unsized `✕` buttons in `routes/Restic.svelte:928/1023`.
- **Fix:** add `hv-ic` to each (the `::after` 44px square plus the
  `any-pointer: coarse` rule in `base.css:636` do the rest); raise the Terminal
  close to `w-7 h-7` minimum.
- **Prevent:** [D2](#durable-fixes-d1--d4) — add a custom axe check or a Playwright
  assertion on `element.getBoundingClientRect()` ≥ 32px for these controls.

### [ ] W18 — Make `role="button"` containers activate on Space too (category 9)

- **Where:** Enter-only handlers: `routes/Explorer.svelte:1402` (file rows),
  `routes/VirtualMachines.svelte:515`, `routes/Downloader.svelte:304/306/344/389`,
  `routes/Restic.svelte:682/734/770/774`, `routes/Network.svelte:582`.
  Enter **and** Space already handled in `routes/Journal.svelte:284`,
  `routes/Terminal.svelte:629`, `components/theme/Select.svelte:126`,
  `components/docker/*` backdrops.
- **Fix:** add `activateOnEnterOrSpace(e, fn)` to `frontend/src/lib/utils/`
  and use it in all eight spots (Space must `preventDefault()` to avoid page
  scroll).
- **Test:** keyboard E2E step — focus a file row, press Space, row opens.

---

## Phase 3 — States & data UI (W11, W12) · +8 points

### [ ] W11 — Skeletons instead of a centred spinner for dense lists (categories 5, 15)

- **Where:** `.hv-skeleton*` (`lib/styles/theme/utilities.css:478-497`,
  `base.css:499`) and `components/ui/skeleton/` exist with **0 usages**;
  `components/app/DataTable.svelte:97-102` renders a single `<Spinner>` row;
  `animate-spin` appears ×18 (Explorer, Docker, Account…).
- **Fix:** give `DataTable` a `loadingRows` skeleton (N rows × 44px, matching the
  real column count) and use it in Explorer/Journal/Storage/Docker list loads;
  keep spinners only for in-control pending states (button spinners).
- **Why:** the SaaS profile forbids "one centred spinner replacing the table"
  (layout shift + no anatomy preview).

### [ ] W12 — Make `DataTable` power-user complete (category 15)

- **Where:** `components/app/DataTable.svelte` — `<thead>` not sticky, no sort
  affordance, no `aria-sort`, no pagination/virtualisation (backend caps ~200),
  `align: 'right'` opt-in only, no `tabular-nums`; 27 routes still hand-roll
  `<table>` (Docker/Users/Groups/Shares/Journal).
- **Fix:**
  - `sticky top-0` header (and sticky first column on wide tables);
  - clickable column headers → sort state + `aria-sort="ascending|descending"`;
  - `align: 'right'` by default for numeric columns and
    `font-variant-numeric: tabular-nums` on `.hv-datatable`;
  - pagination or virtualisation past ~300 rows;
  - migrate the hand-rolled tables onto `DataTable` route by route.
- **Test:** E2E asserting sort toggling, sticky header at 200 rows, and empty vs
  zero-state distinction.

---

## Phase 4 — System rules (W13–W15) · +12 points

### [ ] W13 — Ban `transition: all` (categories 12, 13)

- **Where:** 36 occurrences — `lib/styles/theme/components.css` ×9,
  `lib/styles/app-layout.css` ×5, `lib/styles/theme/base.css:146`,
  `lib/styles/theme/utilities.css` ×4, `components/docker/*` ×9,
  `routes/Desktop.svelte:4509/5157/5195/5246`,
  `components/theme/PathField.svelte:101`,
  `components/theme/FolderPicker.svelte:356/385/453`,
  `components/NotificationCenter.svelte:273`.
- **Fix:** name the properties actually varied
  (`color, background-color, border-color, box-shadow`) with
  `var(--hv-transition-base) var(--hv-ease-default)`.
- **Prevent:** [D1](#durable-fixes-d1--d4) (stylelint `property-no-vendor-prefix`
  companion rule / grep gate on `transition:\s*all`).

### [ ] W14 — Ship or explicitly decline dark mode (category 4)

- **Where:** no `prefers-color-scheme` / `data-theme` anywhere in
  `frontend/src` (only `assets/vite.svg`), while shadcn primitives ship dead
  `dark:` variants (`components/ui/switch`, `ui/select`, `ui/input`).
- **Fix (pick one, document it):**
  - **Ship:** add a dark pair for the shell tokens (surfaces, borders, all three
    text tiers, status colours, focus ring) + per-theme chart palette, re-check
    contrast at 4.5:1 in both themes;
  - **Decline:** strip the `dark:` variants and record "light-only by design"
    in `docs/guides/design-system.md` (the SaaS profile makes absence a Warning,
    so the shipped option is preferred).
- **Prevent:** [D2](#durable-fixes-d1--d4) (axe contrast run must execute in both
  themes if shipped).

### [ ] W15 — One responsive convention (categories 8, 2, 10)

- **Where:** three vocabularies — container variants `@max-[880px]:`
  (`routes/Account.svelte:369/421/451/509`, `routes/Dashboard.svelte:223+`,
  `routes/Shares.svelte:541`, `routes/Settings.svelte:782`), viewport variants
  `min-[880px]:`/`sm:`/`md:` (`routes/Restic.svelte:674/761`,
  `routes/Storage.svelte:732`, `routes/VPN.svelte:481`,
  `routes/Syncthing.svelte:261`, `routes/CloudSync.svelte:439`,
  `routes/Network.svelte:435/468`, `routes/Downloader.svelte:263`,
  `routes/Users.svelte:861/895/1075`), and the documented-but-unused
  `hv-grid-cols-*` / `hv-md:` system (`lib/styles/theme/utilities.css:75-91`,
  `README.md:283`, 0 usages). No breakpoint at all for detail grids
  (`routes/VirtualMachines.svelte:619/674/690/762/777/811`,
  Docker/Restic/VPN/SetupWizard `grid-cols-2`); only `routes/Desktop.svelte`
  has viewport `@media` rules and `routes/Groups.svelte` uses matchMedia(860px).
- **Fix:** adopt container queries (`@max-[…]`) as the one convention, add the
  missing collapse rules to the detail grids, and either adopt or delete
  `hv-grid-*` (deleting also fixes the README example).
- **Prevent:** [D3](#durable-fixes-d1--d4).

---

## Polish (P1) · +1 point

### [ ] P1 — `backdrop-filter` contradicts the documented "no blur" rule (categories 12, 10)

- **Where:** `components/ui/dialog/dialog-overlay.svelte:15`
  (`supports-backdrop-filter:backdrop-blur-xs`) vs
  `lib/styles/theme/tokens.css:1103` ("NO backdrop-filter anywhere — depth comes
  from shadow"; only `HvLaunchpad.svelte:398` is a documented exception).
- **Fix:** drop the blur from the overlay, or document it as a second exception
  right next to the launcher's in `tokens.css`.

---

## Verify in code (V1–V9)

Runtime/static checks the code audit could not settle. Each item states the
severity it becomes if it fails. Nothing here is deducted yet.

- [ ] **V1 — Muted text on the toolbar/chip fill.** Compute `--hv-shell-text
      #69747F` (tokens.css:1114) against `--hv-shell-fill #EFF1F4`
      (tokens.css:1108) **wherever it actually renders** (same element or a
      parent with that fill): 4.21:1. → **Critical** if a body/muted string sits
      on it (below 4.5:1).
- [ ] **V2 — 360px / 1280px layout.** Open the app at 360px and 1280px and check
      for horizontal scroll and squashed detail grids (most routes have no
      breakpoint; see W15). → **Critical** if horizontal scroll at 360px.
- [ ] **V3 — Docker modals keyboard contract.** Escape closes, focus enters on
      open, focus returns on close. → **Critical** if any fails (beyond C2's
      static findings).
- [ ] **V4 — Placeholder-as-label sweep.** VPN (`ovpn-name`), Docker and Users
      were sampled and do use `<label for>`; sweep the remaining forms. →
      **Critical** if found on a primary form.
- [ ] **V5 — axe-core runtime pass** on all 27 screens: focus order, dialog
      names, live regions (`aria-live` ×1 explicit but `role="alert"` ×13),
      in-situ contrast. → escalate per violation.
- [ ] **V6 — Keyboard shortcuts + discoverable map** (SaaS profile requires a
      `?` overlay/menu hints and a globally reachable command palette).
      → **Warning** if absent.
- [ ] **V7 — State in the URL.** Filters/selection/sort survive reload?
      (`URLSearchParams` appears only in `lib/services/api.ts`, routing uses
      bare `pushState`.) → **Warning** if state resets silently.
- [ ] **V8 — Performance budgets.** `sprite-unified.svg` ≈4MB fetched at startup
      (`frontend/src/main.ts`, trade-off documented) + `mapping-unified.ts`
      484KB. Measure CLS < 0.1 and LCP < 2.5s. → **Warning** if over budget.
- [ ] **V9 — Dead `var(--hv-*, #hex)` fallbacks.** All tokens verified defined at
      `:root`, so fallbacks should never render; confirm no theme path disables
      them. → no deduction expected (cleanup lives in W4).

---

## Durable fixes (D1–D4)

Tooling that keeps the families closed. Ordered by leverage. Stack note: this is
Svelte 5 + Tailwind 4 + Vite — `eslint-plugin-tailwindcss` does **not** apply;
use stylelint + grep gates + Playwright instead.

- [ ] **D1 — Token/magic-number gate** (closes W1, W3, W4, W13, part of W2).
  *Instruction for the coding assistant:* "Add stylelint
  (`stylelint-config-standard-scss`-free, plain CSS/Svelte via
  `postcss-html`) with rules that fail on raw hex colours in `.svelte`/CSS, on
  spacing/radius values outside the token scale, on `transition: all`, and on
  `text-[length:` — and wire it as a pre-commit hook."
- [ ] **D2 — axe-core on every screen** (closes C2 fallout, W9, W17, V5, and
  guards contrast if W14 ships). *Instruction:* "Add an automated accessibility
  test that runs axe-core via `@axe-core/playwright` against all 27 screens and
  fails the build on violations."
- [ ] **D3 — Visual regression at 3 widths** (closes W15, guards W2/W3, covers
  V2). *Instruction:* "Set up Playwright visual regression tests: screenshot
  each screen at 360px, 768px and 1280px and fail when pixels change
  unexpectedly."
- [ ] **D4 — i18n lint** (closes W7). *Instruction:* "Add a lint rule that flags
  hard-coded user-visible strings — including `aria-label`, `title` and visible
  text — in `.svelte` files so all copy goes through the translation keys."

---

## Re-audit checklist

- [ ] All phase boxes ticked; `cd frontend && npm run check && npm run build` green.
- [ ] `cargo test` + the 28 `tests/e2e/*/*.spec.ts` suites green; `TESTING.md`
      updated in the same changes.
- [ ] Re-run the `ui-ux-audit` full pass and record the new score above (target
      100/100, no Blocker).
- [ ] `docs/guides/design-system.md` reflects every token/component rule that
      changed; `docs/INDEX.md` still accurate.
