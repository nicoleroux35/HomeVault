#!/usr/bin/env node
/**
 * D1 — token / magic-number gate.
 *
 * Fails on design-system escapes:
 *   - raw hex colours outside the token definitions,
 *   - `transition: all`,
 *   - `text-[length:var(--hv-text-*)]` (the second type vocabulary W1 removes),
 *   - arbitrary radius / spacing values off the 4px scale,
 *   - raw z-index literals above the small utility band,
 *   - dead `var(--hv-*, #hex)` fallbacks.
 *
 * Like the i18n gate this is a RATCHET over existing debt: counts live in
 * `lint-tokens-baseline.json`; only a NEW escape fails. Regenerate after a
 * cleanup:
 *
 *   node scripts/lint-tokens.mjs --update
 *   node scripts/lint-tokens.mjs            # CI
 *
 * Token definitions themselves (theme/tokens.css) are exempt; they are the one
 * place a raw value is the point.
 */
import fs from 'node:fs';
import path from 'node:path';

const ROOT = path.resolve(import.meta.dirname, '..');
const SRC = path.join(ROOT, 'src');
const BASELINE = path.join(ROOT, 'lint-tokens-baseline.json');
const UPDATE = process.argv.includes('--update');
const QUIET = process.argv.includes('--quiet');

const SKIP_DIRS = new Set(['node_modules', 'dist', '.svelte-kit']);
const EXEMPT_FILES = new Set(['src/lib/styles/theme/tokens.css']);

/** The 4px rhythm (plus the two half-steps the design system documents). */
const SPACING_SCALE = new Set([0, 2, 4, 6, 8, 10, 12, 16, 20, 24, 32, 40, 48, 64, 80, 96]);
/** Radius stops documented in tokens.css. */
const RADIUS_SCALE = new Set([0, 2, 4, 6, 8, 10, 12, 16, 18, 20, 24, 32, 999]);

function walk(dir) {
  const out = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    if (SKIP_DIRS.has(entry.name)) continue;
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) out.push(...walk(full));
    else if (/\.(svelte|css)$/.test(entry.name)) out.push(full);
  }
  return out;
}

function lineOf(source, index) {
  return source.slice(0, index).split('\n').length;
}

function ignoredLine(source, index) {
  const start = source.lastIndexOf('\n', index) + 1;
  const end = source.indexOf('\n', index);
  return source.slice(start, end === -1 ? undefined : end).includes('token-ignore');
}

function add(hits, rule, value, line) {
  hits.push({ rule, value, line });
}

function scanFile(file, rel) {
  if (EXEMPT_FILES.has(rel)) return [];
  const source = fs.readFileSync(file, 'utf8');
  const hits = [];

  // Raw hex colours. Skip HTML entities and CSS fragments that are selectors
  // (`href="#main"`, `url(#gradient)`), which are not colours.
  const hexRe = /(?<![\w&])#([0-9a-fA-F]{8}|[0-9a-fA-F]{6}|[0-9a-fA-F]{4}|[0-9a-fA-F]{3})\b/g;
  let m;
  while ((m = hexRe.exec(source))) {
    const before = source.slice(Math.max(0, m.index - 12), m.index);
    if (/(url\(|href=|id=|xlink:href=)\s*["']?$/.test(before)) continue;
    if (ignoredLine(source, m.index)) continue;
    add(hits, 'raw-hex', `#${m[1].toLowerCase()}`, lineOf(source, m.index));
  }

  const transitionRe = /transition(?:-property)?\s*:\s*all\b/gi;
  while ((m = transitionRe.exec(source))) {
    if (!ignoredLine(source, m.index)) add(hits, 'transition-all', 'transition: all', lineOf(source, m.index));
  }

  const textLengthRe = /text-\[length:/g;
  while ((m = textLengthRe.exec(source))) {
    if (!ignoredLine(source, m.index)) add(hits, 'text-length-var', 'text-[length:', lineOf(source, m.index));
  }

  const radiusRe = /rounded-\[(\d+(?:\.\d+)?)px\]/g;
  while ((m = radiusRe.exec(source))) {
    const value = Number(m[1]);
    if (SPACING_SCALE.has(value) || RADIUS_SCALE.has(value)) continue;
    if (ignoredLine(source, m.index)) continue;
    add(hits, 'arbitrary-radius', m[0], lineOf(source, m.index));
  }

  const spacingRe = /\b(?:p|px|py|pt|pb|pl|pr|m|mx|my|mt|mb|ml|mr|gap|space-x|space-y)-\[(\d+(?:\.\d+)?)px\]/g;
  while ((m = spacingRe.exec(source))) {
    if (SPACING_SCALE.has(Number(m[1]))) continue;
    if (ignoredLine(source, m.index)) continue;
    add(hits, 'arbitrary-spacing', m[0], lineOf(source, m.index));
  }

  const zRe = /\bz-index\s*:\s*(\d{2,})\s*;/g;
  while ((m = zRe.exec(source))) {
    if (Number(m[1]) <= 50) continue;
    if (ignoredLine(source, m.index)) continue;
    add(hits, 'z-index-literal', `z-index: ${m[1]}`, lineOf(source, m.index));
  }

  const fallbackRe = /var\(--hv-[a-z0-9-]+\s*,\s*#[0-9a-fA-F]{3,8}\)/g;
  while ((m = fallbackRe.exec(source))) {
    if (ignoredLine(source, m.index)) continue;
    add(hits, 'dead-fallback', m[0], lineOf(source, m.index));
  }

  return hits;
}

function readBaseline() {
  try {
    return JSON.parse(fs.readFileSync(BASELINE, 'utf8'));
  } catch {
    return {};
  }
}

const files = walk(SRC);
const current = {};
const allHits = [];

for (const file of files) {
  const rel = path.relative(ROOT, file).split(path.sep).join('/');
  const hits = scanFile(file, rel);
  if (hits.length === 0) continue;
  current[rel] = {};
  for (const hit of hits) {
    const key = `${hit.rule}::${hit.value}`;
    current[rel][key] = (current[rel][key] ?? 0) + 1;
    allHits.push({ rel, ...hit });
  }
}

if (UPDATE) {
  fs.writeFileSync(BASELINE, `${JSON.stringify(current, null, 2)}\n`);
  const total = Object.values(current).reduce(
    (sum, rules) => sum + Object.values(rules).reduce((a, b) => a + b, 0),
    0,
  );
  console.log(`token lint: baseline written (${total} known escapes in ${Object.keys(current).length} files).`);
  process.exit(0);
}

const baseline = readBaseline();
const added = [];
for (const [file, rules] of Object.entries(current)) {
  for (const [key, count] of Object.entries(rules)) {
    const known = baseline[file]?.[key] ?? 0;
    if (count > known) {
      const occurrences = allHits.filter((h) => h.rel === file && `${h.rule}::${h.value}` === key);
      for (const hit of occurrences.slice(known)) added.push({ file, ...hit });
    }
  }
}

if (added.length > 0) {
  console.error(`token lint: ${added.length} new design-system escape(s).\n`);
  for (const hit of added.slice(0, 40)) {
    console.error(`  ${hit.file}:${hit.line}  [${hit.rule}]  ${hit.value}`);
  }
  if (added.length > 40) console.error(`  … and ${added.length - 40} more.`);
  console.error(
    '\nUse a --hv-* token, a scale step, or a named transition property. ' +
      'Add <!-- token-ignore --> on the line only for a documented exception.',
  );
  process.exit(1);
}

if (!QUIET) {
  const known = Object.values(baseline).reduce(
    (sum, rules) => sum + Object.values(rules).reduce((a, b) => a + b, 0),
    0,
  );
  console.log(`token lint: OK — no new escapes (${known} baselined occurrences remain).`);
}
