# Testing Strategy Overview

This document provides a complete overview of HomeVault's testing strategy, covering all test types and their purpose.

## Test Pyramid

```
                    ┌─────────────┐
                    │   Manual    │  ← Production validation, exploratory testing
                    └─────────────┘
                  ┌───────────────────┐
                  │   E2E (77 specs)  │  ← Full user flows, real browser
                  └───────────────────┘
              ┌─────────────────────────────┐
              │  Integration (110 files)    │  ← API handlers, DB, real router
              └─────────────────────────────┘
          ┌───────────────────────────────────────┐
          │    Unit Tests (236 files)             │  ← Pure logic, validators
          └───────────────────────────────────────┘
```

## Test Types Matrix

| Type | Count | Runs On | Purpose | Frequency |
|------|-------|---------|---------|-----------|
| **Unit** | 235 modules | Every push | Pure logic, validators | Always |
| **Integration** | 109 files | Every push | API + DB contracts | Always |
| **E2E** | 77 specs | Push to main | Real user flows | Main only |
| **Property** | 28 tests | Every push | Invariant verification | Always |
| **Contract** | 7 tests | Every push | API stability | Always |
| **Mutation** | Full codebase | On demand | Test quality | Manual |
| **Chaos** | 10 scenarios | On-demand | Failure resilience | Manual |
| **i18n** | 16 tests | E2E run | Locale correctness | E2E |
| **Load (k6)** | 3 scenarios | Manual | Performance baseline | Manual |
| **Frontend (Vitest)** | 83 tests / 11 files | Every push | Stores, helpers, component snapshots | Always |
| **Soak** | 4 scenarios | Manual | Endurance testing | Manual |
| **Fuzz** | Deterministic | Every push | Input validation | Always |

## Coverage Goals

### Backend (Rust)
- **Line coverage**: ≥45% (enforced in CI)
- **Mutation score**: ≥80% target
- **Integration coverage**: Every DB-touching handler
- **Property coverage**: All critical validators

### Frontend (Svelte)
- **E2E coverage**: every screen listed in `SCREENS.md` has a spec directory
- **Snapshot coverage**: all reusable components (Vitest snapshots)
- **i18n coverage**: 4 locales (en, fr, es, de)
- **Visual coverage**: 3 widths (360 / 768 / 1280)

### Security
- **Auth paths**: 100% (login, 2FA, recovery, sessions)
- **Input validation**: Property-tested
- **Injection attacks**: Chaos-tested
- **XSS/CSRF**: E2E § Security mandatory

## Test Commands Quick Reference

```bash
# ── Backend Tests ──────────────────────────────────────────────────
cargo test --lib                          # Unit tests only
cargo test --test <module>_integration    # Integration tests
cargo test --test property_testing        # Property-based tests
cargo test --test contract_testing        # Contract tests
cargo test --test chaos_testing           # Chaos engineering
cargo test --test soak_integration        # Soak/endurance
cargo test --test fuzz_smoke              # Fuzz smoke tests
cargo mutants                             # Mutation testing (slow)

# ── Frontend Tests ─────────────────────────────────────────────────
cd frontend
npm test                                  # Vitest: stores, helpers, component snapshots
npm test -- -u                            # rewrite snapshots after an intentional change
npm run test:coverage                     # coverage (v8), thresholds enforced at 60 %

# ── E2E Tests ──────────────────────────────────────────────────────
npx playwright test                       # All E2E (Firefox default)
npx playwright test --project=chromium    # Chromium only
npx playwright test tests/e2e/login/      # Specific screen
npx playwright test -g "4.1"              # Specific test
npx playwright test tests/e2e/i18n/       # i18n tests

# ── Load Tests ─────────────────────────────────────────────────────
k6 run tests/load/homevault-load-test.js  # Basic load
k6 run tests/load/spike-test.js           # Spike test
k6 run tests/load/stress-test.js          # Stress test

# ── Coverage ───────────────────────────────────────────────────────
cargo llvm-cov --lib --lcov --output-path target/coverage/lcov.info
cd frontend && npm run test:coverage

# ── CI Commands ────────────────────────────────────────────────────
cargo fmt --all --check                   # Format check
cargo clippy --all-targets -- -D warnings # Lint
cargo audit                               # Security audit
cd frontend && npm audit --audit-level=high
```

## Test Data & Fixtures

### Seeded Test Data
- **E2E Admin**: `e2eadmin` / `E2eSecret123`
  - Override: `E2E_ADMIN_USER` / `E2E_ADMIN_PASS`
- **In-memory DB**: Fresh SQLite per integration test
- **Scratch ports**: Always pass `PORT=` + `DATA_DIRECTORY=`

### Test Isolation
- ✅ Each integration test: fresh database
- ✅ Each E2E test: detects and adapts to state
- ✅ Property tests: deterministic (seeded PRNG)
- ✅ Chaos tests: isolated instances

## CI Pipeline

```
┌─────────────────────────────────────────────────────────────┐
│ Push to any branch                                          │
└──────────────────┬──────────────────────────────────────────┘
                   │
      ┌────────────┴────────────┐
      │                         │
      ▼                         ▼
┌──────────┐            ┌──────────────┐
│ ci.yml   │            │ unit.yml     │
│          │            │              │
│ • Build  │            │ • cargo test │
│ • Clippy │            │   --lib      │
│ • Fmt    │            │              │
│ • Audit  │            └──────────────┘
└──────────┘
      │
      │  Push to main only
      ▼
┌────────────────────────┐
│ e2e-playwright.yml     │
│                        │
│ Stage A → Stage B →    │
│ Design → Visual →      │
│ Compat → Full suite    │
└────────────────────────┘
      │
      │  Weekly
      ▼
┌────────────────────────┐     ┌─────────────────┐
│ manual: cargo mutants  │     │ coverage.yml    │
│ (hours, opt-in)        │     │ (Mon 03:17 UTC) │
│                        │     │                 │
│ • mutation score       │     │ • Backend: ≥45% │
│ • per changed file     │     │ • Frontend: v8  │
└────────────────────────┘     └─────────────────┘
```

## Test Quality Metrics

### What We Measure
1. **Coverage** (quantitative)
   - Line coverage: ≥45%
   - Branch coverage: tracked
   - Function coverage: tracked

2. **Mutation Score** (qualitative)
   - Target: ≥80%
   - Measures: test effectiveness
   - Focus: critical paths first

3. **Performance** (non-functional)
   - p95 response time: <500ms
   - p99 response time: <1000ms
   - Error rate: <5%

4. **Stability** (reliability)
   - E2E flake rate: <1%
   - Snapshot drift: tracked
   - CI success rate: >95%

## Test Environment Requirements

### Minimal (CI)
- ✅ No sudo
- ✅ In-memory SQLite
- ✅ No privileged ops
- ✅ Runs in container

### Standard (E2E)
- Scratch `DATA_DIRECTORY`
- Isolated `PORT`
- Fresh admin seed
- No real system commands

### Full (Manual)
- Real sudo available
- Real disk operations
- Live network config
- Production-like environment
- Root-only suite: the 9 `#[ignore]`d `login_pam_integration` cases (PAM,
  `useradd`, `/etc/sudoers`), run with `--ignored` as root on a disposable host

## When to Run What

### Before Every Commit
```bash
cargo fmt --all
cargo clippy --all-targets
cargo test --lib
cd frontend && npm run check
```

### Before Every PR
```bash
cargo test --tests
npx playwright test tests/e2e/<your-screen>/
cd frontend && npm test
```

### Before Every Release
```bash
# Full backend suite
cargo test --tests

# Full E2E suite (push to main triggers this)
npx playwright test

# Mutation testing (manual: hours, one changed file at a time)
cargo mutants --file src/handlers/<changed>.rs

# Load testing (staging)
k6 run tests/load/homevault-load-test.js
```

### Weekly (Automated)
- Monday 03:17 UTC: coverage report (`coverage.yml`, backend ≥45 % lines)
- Dependency audits (cargo audit, npm audit)

### Manual (not scheduled)
- Mutation testing: `cargo mutants` takes hours, so it stays opt-in; run it on
  the file you changed rather than the whole crate.

## Troubleshooting Guide

### Tests are slow
1. Run specific suites: `cargo test --test auth_integration`
2. Use `--lib` for unit tests only
3. Run E2E in parallel: `npx playwright test --workers=4`

### Tests are flaky
1. Check E2E for timing issues (increase timeouts)
2. Verify test isolation (no shared state)
3. Review for race conditions (add proper waits)

### Coverage is dropping
1. Review new code paths
2. Add unit tests for pure logic
3. Add integration tests for handlers
4. Property-test new validators

### Mutation score is low
1. Focus on missed mutants first
2. Add negative test cases
3. Test edge cases explicitly
4. Verify error paths are tested

## Best Practices

### ✅ Do
- Write tests before fixing bugs
- Test the unhappy path
- Use property tests for validators
- Keep snapshots small and focused
- Run relevant tests locally
- Review coverage in PRs

### ❌ Don't
- Skip tests to make CI pass
- Mock excessively in E2E
- Hardcode test data
- Commit commented-out tests
- Update snapshots blindly
- Ignore mutation test results

## Resources

### Documentation
- `TESTING.md` - Core testing guide
- `TESTING_ADVANCED.md` - This file's detailed version
- `docs/guides/*.md` - Per-feature test requirements
- `CLAUDE.md` - Test invariants and rules

### External
- [Rust Testing Book](https://doc.rust-lang.org/book/ch11-00-testing.html)
- [Playwright Best Practices](https://playwright.dev/docs/best-practices)
- [Property Testing Guide](https://hypothesis.works/articles/what-is-property-based-testing/)
- [k6 Documentation](https://k6.io/docs/)

---

**Last Updated**: 2026-11-19  
**Test Count**: 4 658 Rust test functions (~7 600 executions), 83 frontend tests  
(11 files), ~4 400 E2E executions across 77 spec files  
**CI Runtime**: ~20 min (unit + component + integration), ~2.5 h (full E2E)
