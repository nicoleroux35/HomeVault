# HomeVault — opencode project rules

HomeVault is a Rust (Axum) + Svelte 5 NAS appliance. The canonical guardrails
live in `CLAUDE.md` and the developer reference lives in `docs/`
(`INDEX.md` → `ARCHITECTURE.md`, `API.md`, `FRONTEND.md`, `guides/<area>.md`).
Read `CLAUDE.md` first; when `docs/` and `CLAUDE.md` disagree on a rule,
`CLAUDE.md` wins.

## Accessibility is a correctness bar — WCAG 2.2 AA + EN 301 549 + ISO 9241

Every new or edited screen must stay usable with **mouse, touch, pen and
keyboard**. The concrete invariants (full detail in `CLAUDE.md` →
"Accessibility baseline" and `docs/guides/design-system.md` → §7.1):

1. **44px hit areas.** Compact (28–40px) icon buttons get the shared
   `.hv-ic` / `.hv-ic-abs` transparent 44px hit square — never a bespoke
   `::after` on an element that already has one. Never revert
   `@media (any-pointer: coarse)` to `pointer: coarse`.
2. **Focus visible.** Never `outline: none` on `:focus-visible` without
   `box-shadow: var(--hv-focus-ring)` (2px solid `#1E62AE` + halo). A colour
   change alone is not a focus indicator.
3. **Contrast.** Text tokens ≥4.5:1 on every surface they render on; status
   dots / focus rings / control boundaries ≥3:1. Contrast-check any new
   page-scoped palette.
4. **Modals trap focus.** Use `theme/Modal` / `theme/Drawer` (bits-ui
   Dialog) or `use:focusTrap` from `lib/utils/focusTrap.ts` on hand-rolled
   overlays; restore focus to the trigger on close.
5. **No hover/right-click/double-click-only interactions.** Reveal on
   `group-focus-within` too; every contextual action needs a button or
   keyboard path.
6. **Accessible names.** Icon-only buttons carry `aria-label` from an i18n
   key (never `title` alone, never a hardcoded string). Every input has a
   label. `role="button"` containers must not nest `<button>` children.
   One `<h1>` per screen.

## Engineering guardrails (summary — see CLAUDE.md)

- Every feature ships with its tests: Rust unit (`#[cfg(test)]`), Rust
  integration (`tests/*_integration.rs`, in-memory SQLite + Axum Router +
  `tower::ServiceExt::oneshot`), Playwright E2E (`tests/e2e/<screen>/`).
  Keep `TESTING.md` counts in sync in the same change.
- Keep `docs/` current and English-only in the same change that touches
  code. No page is frozen.
- English-only code comments. No scratch/reference files committed.
- Secrets never leak; OWASP/ASVS review on every endpoint; least privilege
  (`require_auth` reads, `require_admin` mutations).
- RAG knowledge base: a new feature/app catalog entry gets a `FeatureDoc`
  in `src/rag_indexers.rs` + a `docs/guides/rag.md` "Document Sources" row.

## Verification

- Frontend: `cd frontend && npm run check && npm run build`
- Backend: `cargo test` (unit + integration)
- E2E: `npx playwright test tests/e2e/<screen>/<screen>.spec.ts`