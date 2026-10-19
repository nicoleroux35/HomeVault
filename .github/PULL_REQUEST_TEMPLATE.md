## Summary

<!-- What changes and why. Link the issue: Closes #… -->

## Checklist

- [ ] `cargo test` green (unit + integration)
- [ ] `cd frontend && npm run check && npm run build` green
- [ ] E2E updated/added for changed screens (`tests/e2e/<screen>/`)
- [ ] `TESTING.md` counts updated in the same change
- [ ] `docs/` updated in the same change, English only
- [ ] No scratch files; comments in English
- [ ] Accessibility invariants hold (44px, focus, contrast, keyboard, labels)
- [ ] AuthZ: `require_auth` reads, `require_admin` mutations; no secret leaks
- [ ] I have read the CLA in `CLA.md` and I agree to its terms

## Screens

<!-- Reference screenshots or Playwright evidence for UI changes. -->
