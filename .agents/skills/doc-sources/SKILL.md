---
name: doc-sources
description: Document HomeVault source files with the mandatory standard file header, and maintain the project-wide documentation coverage index — Rust backend modules (src/), Svelte components and TypeScript services/stores (frontend/src), tests. Use when asked to document, explain, annotate, or review the documentation of source files, to scan the project for undocumented files, or to create/update a docs/guides/<area>.md area guide from code.
---

# Documenting HomeVault source files

You are the project's documentation agent. You work in two modes:

- **Single file / area**: document the requested file(s), then refresh the
  coverage index (section 2).
- **Full-project inventory**: scan every source file, maintain the coverage
  index, and work through it file by file (section 2).

In both modes, for every file you document you:

1. add or refresh the **standard file header** (section 3) — mandatory,
2. write or improve the **in-code documentation** (section 4),
3. when the request covers a whole feature, create or update its
   `docs/guides/<area>.md` guide (section 5).

Never guess behavior, never restate signatures, never invent usage examples.

## 1. Read before writing

For the file (or module) given as argument, always read it **and its
neighbors** so the documentation describes real behavior:

| What you were given | Also read |
|---|---|
| `src/handlers/<x>.rs` | router entries in `src/web/mod.rs` (paths + HTTP verbs), the storage/service layer it calls, `docs/API.md` rows for the area |
| any other `src/<x>.rs` | its callers (`rg "<name>" src/`), related handlers |
| `frontend/src/routes/<X>.svelte` | the `lib/services/api.ts` methods it uses, the guide for its area |
| `frontend/src/lib/services/api.ts`, stores, utils | the backend handlers behind the endpoints, consumers (`rg "<fn>" frontend/src`) |
| an area name ("document the firewall") | the backend handler(s) + route component(s) + existing `docs/guides/<area>.md` if any |

## 2. Full-project inventory (the coverage index)

The index of every source file and its documentation state is
`.agents/skills/doc-sources/INDEX.md`. It is **generated** — never hand-edit
it; regenerate it with:

```sh
.agents/skills/doc-sources/scripts/scan.sh        # whole project (~15 s), always full scope
```

The scanner covers `src/**/*.rs`, `frontend/src/**/*.{ts,svelte}` and
`tests/` (Rust integration + Playwright specs), and assigns one status per
file. **Vendored code is excluded from the campaign:**
`frontend/src/lib/components/ui/` holds ~178 shadcn-svelte primitives (thin
wrappers over bits-ui, added wholesale) — they are third-party library files,
not project-authored code, so they get no per-file banner; the project-authored
wrappers live in `components/theme/`.

| Status | Meaning |
|---|---|
| `documented` | standard header complete and not older than the last commit |
| `to update` | header present but older than the last commit — stale, verify content still true |
| `invalid header` | header present but a mandatory field is missing, or `File` does not match the actual file name — fix in place |
| `not documented` | no header at all |

When asked to inventory, scan, or report documentation status (or given no
specific file):

1. Run the scan and read the fresh `INDEX.md`.
2. Report the coverage line (documented / to update / invalid header / not
   documented) and list the worst offenders per area.
3. If asked to actually document, process files in this priority order:
   **to update** first (stale documentation is worse than none), then
   **invalid header**, then **not documented** — always grouped by area so
   neighbouring files are read in one pass.
4. After each batch (say every 10–15 files), re-run the scan so `INDEX.md`
   reflects the work done; the counters are your progress bar.

For a single-file request, finish with one scan re-run so the file's row is
fresh. The index is a generated artifact and is gitignored; the script and
this skill are committed.

## 3. Standard file header — mandatory, on every documented file

Every documented source file starts with the standard banner below. Keep the
exact structure, field order and column alignment. The entire header is
written in English (repo rule: English-only comments). Adapt only the comment
syntax to the file type:

**Rust (`.rs`)** — block comment on the first lines, before the `//!` module
docs:

```rust
/* =============================================================================
 * File          : antivirus.rs
 * Author        : Nicolas Le Roux
 * Created       : 12/03/2025
 * Last modified : 04/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   What this file does for the project, the main modules it relies on and
 *   how it is organised. Two to five lines, plain language.
 *
 * Dependencies:
 *   - tokio (>= 1.40)
 *   - chrono (>= 0.4)
 *
 * Usage:
 *   How the file is used: CLI invocation for a binary, the HTTP endpoints
 *   the module serves, or a short import/call example for a library module.
 * =============================================================================
 */
```

**TypeScript (`.ts`)** — the same banner inside a `/* … */` block, first lines
of the file.

**Svelte (`.svelte`)** — the same banner inside an HTML comment `<!-- … -->`,
first lines of the file, before `<script lang="ts">`.

### Header metadata — how to fill each field

- **File** — the file name with its extension (e.g. `antivirus.rs`).
- **Author** — `Nicolas Le Roux` (see attribution rules below; never a tool or
  model name, never a raw git username).
- **Created** — date of the file's first commit, format `DD/MM/YYYY`:
  `git log --follow --format=%ad --date=format:%d/%m/%Y -- <file> | tail -1`.
  For a brand-new uncommitted file, use today's date.
- **Last modified … by** — date of the latest commit plus the author's
  display name (`git log -1 --format=%ad::%an --date=format:%d/%m/%Y --
  <file>`). This repo's commits are authored as `nicleroux35`, which displays
  as `Nicolas Le Roux`. For a new or docs-only-uncommitted file: today's
  date, `Nicolas Le Roux`.
- **Version** — `1.0.0` when adding a header to a file that has none. If a
  header already exists, bump the patch number for a documentation-only pass.
- **Description** — what the file does for the project, the main modules it
  relies on, its general architecture. Clear and concise; a developer new to
  the project should understand it on first read.
- **Dependencies** — the main *external* dependencies actually used (crates
  for `.rs`, npm packages for `.ts`/`.svelte`), with versions taken from
  `Cargo.toml` / `frontend/package.json`. Internal modules are not listed
  here — name them in the Description instead.
- **Usage** — concrete and language-appropriate: the CLI line for binaries,
  the endpoints served for handler modules, a short import/call snippet for
  library modules and components. Python-style `>>>` only in actual Python
  files.

**Idempotence:** if the file already carries a header, refresh `Created`,
`Last modified` and `Version` in place. Never stack a second banner.

**Rust pitfall:** block comments nest in Rust, so never write the `/*`
sequence inside the banner — endpoint globs like `/api/auth/*` reopen a
comment and break the build. Write "routes under /api/auth" instead.

### Attribution rules (hard constraints)

- **Never state or imply that an AI wrote or documented the code.** No tool
  name, model name, or "generated" wording anywhere in headers, comments or
  docs. Author and by-lines carry the human author name (`Nicolas Le Roux`).
- **Never cite, compare with, or credit unrelated third-party software or
  products** ("like X", "inspired by Y", "alternative to Z") — including
  other NAS / self-hosting platforms (Umbrel, TrueNAS, OpenMediaVault,
  Synology DSM, CasaOS, Unraid, QNAP, Proxmox, …). Only real dependencies
  may be named, in the Dependencies field, and installable catalog apps in
  their functional context.
- The checks are automated: run
  `python3 .agents/skills/doc-sources/scripts/audit.py` (16+ checks on every
  file: banner integrity, git-date consistency, attribution, English-only,
  platform names). Git-date checks ignore `docs:` commits, so a
  documentation-only commit never makes the tree look stale.

## 4. In-code documentation — understandable at every level

- **English only.** Short sentences, one idea per sentence.
- **Readable by any developer level.** Expand acronyms at first use (e.g.
  "LUKS2 (Linux Unified Key Setup)"), prefer plain words over insider
  shorthand, explain a non-obvious mechanism instead of naming it. Stay
  factual — no filler, no condescension.
- **Explain the why, not the what.** Design constraints, ordering rules,
  concurrency guards, permission checks — that is what doc comments are for.
  The signature already says the what.
- Rust: `//!` module docs (purpose + design constraints, after the header
  banner) and `///` on public items. TS/Svelte: `/** … */` TSDoc on exported
  functions and non-obvious props/stores — document units, side effects, and
  the endpoint (method + path + auth level) a service method wraps.

## 5. Area-level docs (feature guides)

When asked to document a feature/area, or a file that has no guide yet,
create or update `docs/guides/<area>.md` (lowercase filename = area name),
matching existing guides like `docs/guides/avatars.md`:

```markdown
# <Area>

**Purpose:** one or two sentences — what it does for the user.

## Backend

- `VERB /api/path` (`handle_x`) — auth|admin|public; what it does.
...

## Frontend

- `frontend/src/lib/services/api.ts`: method names + one-line role.
- Route component(s) and other consumers.

## Invariants

- Non-obvious constraints that are easy to break again.

## Related

- Links to sibling guides.
```

When creating a **new** guide, add its row to the "Application / area guides"
table in `docs/INDEX.md`. Keep `docs/API.md` and `docs/BACKEND.md` /
`docs/FRONTEND.md` consistent — fix whichever is stale (`CLAUDE.md` wins on
rules).

## 6. House rules (from CLAUDE.md — non-negotiable)

- **English only**, in headers, comments and docs.
- **Auth model is part of the doc.** Every endpoint line states `auth`,
  `admin`, or `public`, matching the actual `require_auth` / `require_admin` /
  unguarded routing in `src/web/mod.rs`. Never paper over a mismatch — flag it.
- **Invariants are the point.** Document what is easy to break again
  (concurrency guards, ordering, permissions, hardware limits).
- **No secrets, no absolute paths, no terminal dumps, no scratch files.**
  Documentation lives in the source or in `docs/`, nowhere else.
- **Attribution:** see section 3 — no AI mentions, no third-software citations.
- If the code is ambiguous or looks wrong, say so in your report to the user —
  do not document invented behavior.

## 7. New-feature checklist

If the area is a brand-new feature (not just docs for existing code), remind
the user (do not silently do it) that it also needs, per `AGENTS.md`:

- a `FeatureDoc` entry in `src/rag_indexers.rs` + a "Document Sources" row in
  `docs/guides/rag.md`,
- tests (`#[cfg(test)]`, `tests/*_integration.rs`, `tests/e2e/<screen>/`) and a
  `TESTING.md` count update.

## 8. Finish

Docs-only changes need no test runs. If you touched `.svelte`/`.ts` files
(even comments), run `cd frontend && npm run check`. Re-run the coverage scan
(section 2) so `INDEX.md` is current, then re-check every touched file:
exactly one valid header, correct dates/version, English throughout. Report:
files edited, headers added/refreshed, coverage counters before → after,
guide created/updated, anything flagged as ambiguous or inconsistent.
