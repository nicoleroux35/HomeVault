# Advanced Testing Guide

This document covers the 7 advanced testing types implemented in HomeVault beyond the core unit/integration/E2E suite.

## 1. Mutation Testing 🧬

**Purpose**: Verify test quality by introducing bugs and checking if tests catch them.

### Setup
```bash
cargo install cargo-mutants --locked
```

### Running
```bash
# Full mutation testing (slow)
cargo mutants

# Specific file
cargo mutants --file src/handlers/auth.rs

# Not scheduled in CI: a full run takes hours, so it stays opt-in and manual.
```

### Configuration
See `mutants.toml` for:
- Files to exclude
- Timeout multipliers
- Focus directories

### Interpreting Results
- **Caught mutants**: Tests detected the introduced bug ✓
- **Missed mutants**: Bug went undetected (improve tests) ✗
- **Target**: ≥80% mutation score

### Example Output
```
Mutation score: 85.3% (256/300 mutants caught)
✓ src/handlers/auth.rs: 95% (19/20)
✗ src/shares.rs: 70% (14/20) - needs better validation tests
```

---

## 2. Contract Testing 📜

**Purpose**: Ensure API contracts remain stable and backward-compatible.

### Running
```bash
cargo test --test contract_testing
```

### What It Tests
- Endpoint existence (no 404)
- Response structure stability
- Backward compatibility
- Field presence/types
- No leaked sensitive data

### Key Tests
```rust
// Response structure must match spec
test_contract_appearance_settings_exactly_two_fields()
  → Ensures no smtp_password, theme, density leaks

// Auth status codes match documented contract
test_contract_auth_status_codes()
  → Missing token: 401
  → Invalid token: 401
  → Valid non-admin: 403

// Old clients still work
test_contract_backward_compatibility_auth_login()
```

### Adding New Contracts
When adding an endpoint:
1. Add to `build_contract_catalog()` in `tests/contract_testing.rs`
2. Specify request/response schemas
3. Document in `docs/API.md`

---

## 3. Property-Based Testing 🎲

**Purpose**: Test invariants across thousands of generated inputs.

### Setup
Already in `Cargo.toml`:
```toml
[dev-dependencies]
proptest = "1.5"
```

### Running
```bash
cargo test --test property_testing
```

### What It Tests
Properties that should **always** hold:
- Share names: length ≤64, no traversal, reserved names rejected
- Paths: absolute, no `..`, no control chars
- Usernames: valid format, system UIDs rejected
- Crypto: deterministic hashing, different inputs → different outputs
- Network: IPv4 validation, port ranges
- Quotas: no overflow, transitive comparison

### Example Property
```rust
proptest! {
    #[test]
    fn share_name_roundtrip(name in "[a-zA-Z0-9._-]{1,64}") {
        if is_valid_share_name(&name) {
            assert!(name.len() <= 64);
            assert!(!name.contains('/'));
        }
    }
}
```

### Benefits
- Finds edge cases unit tests miss
- Tests hundreds of generated inputs automatically (proptest default: 256 cases per property)
- Reproducible failures (seeded PRNG)

---

## 4. i18n Testing 🌍

**Purpose**: Verify all 4 locales (en, fr, es, de) render correctly.

### Running
```bash
npx playwright test tests/e2e/i18n/i18n.spec.ts
```

### What It Tests
- ✅ All locales load without errors
- ✅ Key translations present (no missing keys)
- ✅ Date formatting respects locale
- ✅ Number formatting (1,234.56 vs 1 234,56)
- ✅ Currency formatting ($ vs €)
- ✅ Special characters (é, ñ, ä, ß) render correctly
- ✅ Locale persists across reloads
- ✅ Error messages translated
- ✅ Unsupported locales fallback to English

### Adding New Translations
1. Add key to all 4 files: `src/i18n/{en,fr,es,de}.toml`
2. Verify parity: test `3.1` catches missing keys
3. Test special chars: test `3.8` verifies diacritics

### Locale Coverage
| Locale | Status | Special Chars | Coverage |
|--------|--------|---------------|----------|
| en 🇬🇧 | ✅ Full | None | Baseline |
| fr 🇫🇷 | ✅ Full | é à è ê ç | 100% |
| es 🇪🇸 | ✅ Full | ñ á é í ó ú ¿ ¡ | 100% |
| de 🇩🇪 | ✅ Full | ä ö ü ß | 100% |

---

## 5. Chaos Engineering 💥

**Purpose**: Verify graceful degradation under failures.

### Running
```bash
cargo test --test chaos_testing -- --nocapture

# With more iterations
CHAOS_ITERATIONS=100 cargo test --test chaos_testing
```

### What It Tests
- ✅ Concurrent requests with timeouts (≥70% success under chaos)
- ✅ Database contention (20 concurrent writes)
- ✅ Rapid auth/logout cycles
- ✅ Malformed requests (invalid JSON, wrong types)
- ✅ Extremely long inputs (10KB strings)
- ✅ Concurrent session invalidations
- ✅ Path traversal attempts (all rejected)
- ✅ SQL injection attempts (all blocked)
- ✅ XSS injection attempts (sanitized/rejected)

### Chaos Scenarios
```rust
// Concurrent requests with random timeouts
test_chaos_concurrent_requests_with_failures()
  → 50 requests, 100-600ms timeouts
  → Success rate must be ≥70%

// Security: injection attacks
test_chaos_sql_injection_attempts()
  → "admin' OR '1'='1"
  → "'; DROP TABLE users--"
  → Must return 401, never 500

test_chaos_xss_injection_attempts()
  → "<script>alert('xss')</script>"
  → Must reject or sanitize, never 500
```

### Success Criteria
- No panics
- Graceful 503/400 errors
- No 500 Internal Server Error
- No data corruption
- System recovers when load drops

---

## 6. Load Testing (k6) 📊

**Purpose**: Measure performance under realistic load, find breaking points.

### Setup
```bash
# macOS
brew install k6

# Linux
sudo apt-get install k6

# Windows
choco install k6
```

### Running

#### Basic Load Test
```bash
k6 run tests/load/homevault-load-test.js

# Against specific URL
k6 run --env BASE_URL=http://192.168.1.100:8080 tests/load/homevault-load-test.js
```

#### Spike Test
```bash
k6 run tests/load/spike-test.js
```

#### Stress Test
```bash
k6 run tests/load/stress-test.js
```

### Test Profiles

| Test | Duration | Users | Purpose |
|------|----------|-------|---------|
| **Load** | 5 min | 0→100→0 | Normal capacity |
| **Spike** | 30 sec | 5→500→5 | Sudden burst |
| **Stress** | 10 min | 0→400 | Find breaking point |

### Metrics

#### Good Results ✅
```
✓ http_req_duration: avg=250ms p95=450ms p99=800ms
✓ http_req_failed: 2.5%
✓ iterations: 15000
```

#### Warning Signs ⚠️
```
✗ http_req_duration: avg=1.2s p95=3s p99=8s
✗ http_req_failed: 15%
✗ errors: 20%
```

### Output Files
- `summary.html` - Visual report
- `summary.json` - Parseable results
- Console - Real-time metrics

### Thresholds
```javascript
thresholds: {
  http_req_duration: ['p(95)<500', 'p(99)<1000'],
  http_req_failed: ['rate<0.05'],
  login_duration: ['p(95)<1000'],
}
```

---

## 7. Snapshot Testing 📸

**Purpose**: Catch unintended UI changes, ensure visual stability.

### Setup
Already configured with Vitest.

### Running
```bash
cd frontend

# Run snapshot tests
npm test

# Update snapshots (after intentional changes)
npm test -- -u
```

### What It Tests
- ✅ Screen structure (Login, Desktop, Settings)
- ✅ Component rendering (Button, Badge, Alert, Modal)
- ✅ Widget layouts (CPU, Storage, Network)
- ✅ Consistency (all buttons use same classes)
- ✅ State variations (open/closed, error states)

### Snapshot Files
```
frontend/src/lib/components/__snapshots__/
  screens.test.ts.snap
  
frontend/src/lib/components/theme/__snapshots__/
  components.test.ts.snap
```

### Workflow
1. Make UI change
2. Run `npm test`
3. Review diff
4. If intentional: `npm test -- -u`
5. If unintended: fix the code

### Example
```typescript
it('renders login form', () => {
  const { container } = render(Login);
  expect(container.innerHTML).toMatchSnapshot();
});
```

---

## CI Integration

All 7 test types are integrated into CI:

```
.github/workflows/
  mutation.yml        # Weekly, on-demand
  ci.yml             # Every push (includes contract tests)
  coverage.yml       # Weekly
```

### When Each Runs

| Test Type | Frequency | Trigger |
|-----------|-----------|---------|
| Unit | Every push | CI |
| Integration | Every push | CI |
| E2E | Push to main | e2e-playwright.yml |
| Property | Every push | CI (via cargo test) |
| Contract | Every push | CI (via cargo test) |
| Mutation | Weekly Tue | Cron + manual |
| Chaos | On-demand | Manual |
| Load (k6) | Manual | Dev/staging only |
| Snapshot | Every PR | Frontend CI |

---

## Test Coverage Summary

```
Backend Tests:
  Unit tests (lib):        235 modules ✅
  Integration tests:       109 files   ✅
  Property tests:          28 tests    ✅
  Contract tests:           7 tests    ✅
  Chaos tests:             10 tests    ✅
  Mutation coverage:       Target 80% 🎯 (manual)

Frontend Tests:
  Vitest (stores/helpers/snapshots): 83 tests / 11 files ✅
  E2E (Playwright):        77 specs    ✅
  i18n tests:              16 tests    ✅

Performance Tests:
  Load (k6):               3 scenarios ✅
  Soak (Rust):             4 scenarios ✅

Coverage:                  ≥45 % backend lib (coverage.yml), 60 % floor frontend
```

---

## Best Practices

### 1. Mutation Testing
- Focus on critical paths (auth, shares, firewall)
- Review missed mutants monthly
- Don't chase 100% (diminishing returns >90%)

### 2. Contract Testing
- Add contract for every new endpoint
- Never remove fields (only deprecate)
- Version breaking changes

### 3. Property Testing
- Test invariants, not implementations
- Use meaningful generators
- Keep properties simple

### 4. i18n Testing
- Update all 4 locales together
- Test with real diacritics
- Verify number/date formatting

### 5. Chaos Testing
- Run before major releases
- Simulate real failure modes
- Test recovery, not just degradation

### 6. Load Testing
- Baseline before changes
- Test staging, not production
- Gradually increase load

### 7. Snapshot Testing
- Review diffs carefully
- Update intentionally, not automatically
- Keep snapshots small

---

## Troubleshooting

### Mutation tests timing out
```bash
# Increase timeout in mutants.toml
timeout_multiplier = 10
```

### Contract tests failing after refactor
1. Check `docs/API.md` for documented contract
2. Verify field names/types unchanged
3. Update test if contract intentionally changed

### Property tests flaky
```bash
# Set seed for reproducibility
PROPTEST_SEED=12345 cargo test
```

### i18n tests failing
1. Check all 4 TOML files have same keys
2. Verify no hardcoded English strings
3. Test locale switching in browser

### Chaos tests unstable
- Increase success threshold if <70% is harsh
- Check system resources (CPU, memory)
- Review logs for actual errors

### Load tests failing
- Start small (10 users) and scale up
- Check server logs during test
- Monitor system resources

### Snapshots constantly breaking
- Reduce snapshot size (test smaller components)
- Exclude timestamps/dynamic content
- Review if test is too brittle

---

## Further Reading

- [Mutation Testing Philosophy](https://stryker-mutator.io/docs/)
- [Property-Based Testing Guide](https://hypothesis.works/articles/what-is-property-based-testing/)
- [k6 Documentation](https://k6.io/docs/)
- [Chaos Engineering Principles](https://principlesofchaos.org/)
