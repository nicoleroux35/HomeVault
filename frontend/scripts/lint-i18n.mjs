#!/usr/bin/env node
/**
 * D4 — i18n lint gate.
 *
 * Flags user-visible strings that bypass the translation layer:
 *   - markup text nodes with real words (not numbers, units or glyphs),
 *   - literal `aria-label` / `aria-description` / `aria-placeholder` /
 *     `alt` / `placeholder` / `title` attributes.
 *
 * The repository already contains debt, so the check is a RATCHET: current
 * violations live in `lint-i18n-baseline.json` (counts per file + text) and
 * only a NEW string fails. When a screen is translated, regenerate the
 * baseline to shrink it:
 *
 *   node scripts/lint-i18n.mjs --update
 *   node scripts/lint-i18n.mjs            # CI: fail on new strings
 *
 * Escape hatch for a genuinely non-translatable string (a code sample, a
 * brand name): `<!-- i18n-ignore -->` on the same line, or
 * `<!-- i18n-ignore-file -->` anywhere in the file.
 */
import fs from 'node:fs';
import path from 'node:path';

const ROOT = path.resolve(import.meta.dirname, '..');
const SRC = path.join(ROOT, 'src');
const BASELINE = path.join(ROOT, 'lint-i18n-baseline.json');
const UPDATE = process.argv.includes('--update');
const QUIET = process.argv.includes('--quiet');

/** Units, protocols and product/technical tokens that are not prose. */
const ALLOWED_WORDS = new Set(
  `B KB MB GB TB PB KiB MiB GiB TiB Hz MHz GHz ms s min h d w % ° C F A V W
   CPU RAM GPU IP DNS MAC DHCP HTTPS HTTP TCP UDP SSH TLS SSL NTP UPS SMB NFS CIFS
   SMB3 SMB2 AFP SFTP FTP SFTP WebDAV SSD HDD NVMe PCIe NUMA SATA SAS USB RAID
   ZFS BTRFS EXT4 XFS ARC SLOG L2ARC SMART ECC ACL ACLs API APIs ID IDs UUID
   URL URLs URI JSON YAML XML CSV TOML HTML CSS JS TS SQL REST SSE MCP VPN LAN
   WAN NAT MTU VLAN CIDR DNS DHCP DDNS mDNS UPnP iSCSI LUN LUNs VM VMs ISO PXE
   OK OKs UTC GMT ISO SHA MD5 AES RSA PEM CRL OCSP ACME SSO SAML LDAP AD OIDC
   TOTP HOTP MFA 2FA PIN PUK GBps MBps KBps Gbps Mbps fps p pct
   HomeVault Samba SMB Syncthing Restic WireGuard OpenVPN Docker Incus ZFS
   Linux macOS Windows Safari Firefox Chrome Chromium WebKit
   SMART S.M.A.R.T.`.split(/\s+/).filter(Boolean),
);

const ATTRS = ['aria-label', 'aria-description', 'aria-placeholder', 'alt', 'placeholder', 'title'];
const SKIP_DIRS = new Set(['node_modules', 'dist', '.svelte-kit']);

function walk(dir) {
  const out = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    if (SKIP_DIRS.has(entry.name)) continue;
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) out.push(...walk(full));
    else if (entry.name.endsWith('.svelte')) out.push(full);
  }
  return out;
}

function stripTagBlocks(source, tag) {
  return source.replace(new RegExp(`<${tag}\\b[\\s\\S]*?</${tag}>`, 'gi'), '');
}

function hasLetters(text) {
  return /\p{L}/u.test(text);
}

function normalise(text) {
  return text.replace(/\s+/g, ' ').trim();
}

/** True when every word is a unit, protocol or technical token. */
function onlyAllowedWords(text) {
  const words = text.match(/[\p{L}\p{N}.]+/gu) ?? [];
  if (words.length === 0) return true;
  return words.every((word) => ALLOWED_WORDS.has(word));
}

/** Remove `{expression}` (including nested braces) from a text node. */
function stripExpressions(text) {
  let out = text;
  for (let i = 0; i < 5 && out.includes('{'); i++) out = out.replace(/\{[^{}]*\}/g, '');
  return out;
}

function lineOf(source, index) {
  return source.slice(0, index).split('\n').length;
}

/** Collect { rule, value, line } occurrences in one file. */
function scanFile(file) {
  const source = fs.readFileSync(file, 'utf8');
  if (source.includes('i18n-ignore-file')) return [];
  const hits = [];

  const markup = stripTagBlocks(stripTagBlocks(source, 'script'), 'style').replace(
    /<!--[\s\S]*?-->/g,
    (m) => ' '.repeat(m.length),
  );

  // Attributes — only literal values; `{...}` and `$t(...)` are dynamic.
  for (const attr of ATTRS) {
    const re = new RegExp(`\\b${attr}\\s*=\\s*(["'])([\\s\\S]*?)\\1`, 'g');
    let match;
    while ((match = re.exec(markup))) {
      const value = normalise(match[2]);
      if (!value || !hasLetters(value)) continue;
      if (/[{}]/.test(value)) continue;
      if (onlyAllowedWords(value)) continue;
      if (/^(https?:|\/|#|\.|\{\{)/.test(value)) continue;
      const line = lineOf(markup, match.index);
      if (isIgnored(markup, match.index)) continue;
      hits.push({ rule: `attr:${attr}`, value, line });
    }
  }

  // Text nodes between tags.
  const re = />([^<>]+)</g;
  let match;
  while ((match = re.exec(markup))) {
    let text = stripExpressions(match[1]);
    text = text.replace(/&[a-z#0-9]+;/gi, ' ');
    text = normalise(text);
    if (!text || !hasLetters(text)) continue;
    if (onlyAllowedWords(text)) continue;
    const line = lineOf(markup, match.index);
    if (isIgnored(markup, match.index)) continue;
    hits.push({ rule: 'text', value: text, line });
  }

  return hits;
}

function isIgnored(markup, index) {
  const lineStart = markup.lastIndexOf('\n', index) + 1;
  const lineEnd = markup.indexOf('\n', index);
  const line = markup.slice(lineStart, lineEnd === -1 ? undefined : lineEnd);
  return line.includes('i18n-ignore');
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
  const hits = scanFile(file);
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
  console.log(`i18n lint: baseline written (${allHits.length} known strings in ${Object.keys(current).length} files).`);
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
  console.error(`i18n lint: ${added.length} new hard-coded user-visible string(s).\n`);
  for (const hit of added.slice(0, 40)) {
    console.error(`  ${hit.file}:${hit.line}  [${hit.rule}]  ${JSON.stringify(hit.value)}`);
  }
  if (added.length > 40) console.error(`  … and ${added.length - 40} more.`);
  console.error(
    '\nRoute the string through $t(\u2026) / add the key to every locale, or mark the line ' +
      'with <!-- i18n-ignore --> when it is genuinely not prose.',
  );
  process.exit(1);
}

if (!QUIET) {
  const known = Object.values(baseline).reduce(
    (sum, rules) => sum + Object.values(rules).reduce((a, b) => a + b, 0),
    0,
  );
  console.log(`i18n lint: OK — no new strings (${known} baselined occurrences remain).`);
}
