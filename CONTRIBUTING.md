# Contributing to HomeVault

Thanks for contributing. HomeVault is a Rust (Axum) + Svelte 5 NAS appliance.
The canonical guardrails live in `CLAUDE.md`; the developer reference lives
in `docs/` (`INDEX.md` → `ARCHITECTURE.md`, `API.md`, `FRONTEND.md`,
`guides/<area>.md`). Read `CLAUDE.md` first — when `docs/` and `CLAUDE.md`
disagree on a rule, `CLAUDE.md` wins.

## Workflow

1. Create a feature branch from `main` (fork the repo first if you have access).
2. Keep changes focused; one feature or fix per pull request.
3. Follow the checklist below, then open a PR against `main` using the
   PR template. Link any related issue.

## Definition of done

- [ ] Backend: `cargo test` green (unit + integration).
- [ ] Frontend: `cd frontend && npm run check && npm run build` green.
- [ ] E2E (if a screen changed): `npx playwright test tests/e2e/<screen>/` green.
- [ ] `TESTING.md` counts updated in the same change (see `CLAUDE.md`).
- [ ] `docs/` updated in the same change, English only.
- [ ] Code comments in English; no scratch or reference files committed
      (see `CLAUDE.md` → "Keep the repository clean").
- [ ] Accessibility invariants hold (44px targets, visible focus,
      contrast, focus-trapped modals, keyboard paths, labelled controls).
- [ ] CLA accepted in the PR description (see `CLA.md`).
- [ ] Security: `require_auth` on reads, `require_admin` on mutations;
      no secrets in logs, errors, URLs, or client storage; inputs validated
      server-side; parameterised SQL only.

## Commit style

- Imperative scope prefix: `feat(auth): …`, `fix(shares): …`,
  `docs(rag): …`, `test(quotas): …`, `ci(github): …`, `chore(deps): …`.
- Keep the subject under 72 characters; explain the *why* in the body
  when it is not obvious.

## Reporting bugs / requesting features

Use the issue templates (`.github/ISSUE_TEMPLATE/`). Include the HomeVault
version or commit SHA, reproduction steps, expected vs actual behaviour,
and redacted logs. Security issues follow `SECURITY.md` — never a public
issue.

## Contributor License Agreement

HomeVault is source-available. Contributions are welcome and are accepted
under the [Contributor License Agreement](CLA.md). Add the acceptance line to
your PR description; a PR without it cannot be merged.

## Code of conduct

Participation is governed by our [Code of Conduct](CODE_OF_CONDUCT.md).
