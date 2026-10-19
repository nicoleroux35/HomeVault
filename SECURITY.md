# Security Policy

## Supported versions

Security fixes land on the default branch (`main`). Only the latest release
is supported — there are no backports to older versions.

| Version | Supported          |
| ------- | ------------------ |
| latest (`main`) | :white_check_mark: |
| older releases  | :x:                |

## Reporting a vulnerability

**Do not open a public issue for a suspected vulnerability.**

Report privately via
[GitHub Security Advisories](https://github.com/nicoleroux35/HomeVault/security/advisories/new)
(private vulnerability reporting). Include:

1. Affected component / endpoint and version or commit SHA.
2. Steps to reproduce or a minimal proof of concept.
3. Impact assessment (what an attacker can read, write, or execute).
4. Any relevant logs (redact secrets, tokens, and password hashes).

We will acknowledge receipt within 72 hours, share a remediation timeline,
and credit the reporter on request once a fix is released.

## Scope notes

HomeVault runs as root by design (see `docs/ARCHITECTURE.md` → Deployment
model) and exposes SMB/NFS/FTP/WebDAV plus a web terminal. Reports about
this documented deployment model should demonstrate a concrete privilege
or data boundary violation, not the model itself.

Out of scope: automated scanner output without a working PoC, social
engineering, physical access, and third-party services documented in
`PRIVACY.md` (Open-Meteo, registries, admin-configured SMTP/webhooks).

## Hardening baseline

CI enforces `cargo audit`, `npm audit`, `clippy` and `gitleaks` on every
push and pull request. See `.github/workflows/ci.yml`.
