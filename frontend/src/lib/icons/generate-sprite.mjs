#!/usr/bin/env node

/**
 * Script to generate unified icon sprite from multiple icon libraries:
 * - Phosphor Icons (@phosphor-icons/web) - from selection.json
 * - Tabler Icons (@tabler/icons-svelte) - from Svelte components
 */

import { readFileSync, writeFileSync, readdirSync, mkdirSync, existsSync } from 'fs';
import { join, basename, dirname, extname } from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

// Correct paths - script is in src/lib/icons/, need to go up 3 levels
const FRONTEND_DIR = join(__dirname, '../../../');
const PUBLIC_DIR = join(FRONTEND_DIR, 'public');
const OUTPUT_SPRITE = join(PUBLIC_DIR, 'sprite-unified.svg');
const OUTPUT_MAPPING = join(__dirname, 'mapping-unified.ts');

// ---------------------------------------------------------------------------
// Icon subsetting
// ---------------------------------------------------------------------------
// Phosphor + Tabler together ship 7 600+ glyphs (~3.6 MB of SVG), of which the
// UI references a few hundred. `main.ts` fetches this sprite and injects it
// into the DOM on every page load, so the unused glyphs are pure transfer,
// parse and memory cost on the low-power boards HomeVault targets.
//
// Every pixel of sprite usage goes through `theme/Icon.svelte`'s `name` prop,
// and every value that can reach that prop is a string literal in one of a
// *small, statically identifiable* set of files (see `scanFiles`). Restricting
// the scan to that set is what makes the subset meaningful: icon names are
// ordinary short words (`home`, `user`, `lock`, `folder`), so a whole-tree
// literal scan matches nearly the entire library and removes almost nothing —
// measured, it keeps 7 659 of 7 750 glyphs (a ~40 KB saving), while this
// closure keeps ~70 (a ~3.5 MB saving).
//
// The Docker app catalog never reaches the sprite either: its manifest `icon`
// is resolved to a bundled Tabler *component* in
// `routes/Docker.svelte::catalogIconFor`, so it is not scanned.
//
// If a runtime-constructed name ever turns up, regenerate everything with
// `HV_SPRITE_FULL=1 npm run icons:generate`.
const SRC_DIR = join(FRONTEND_DIR, 'src');
const ROOT_DIR = join(FRONTEND_DIR, '..');
const CATALOG_DIR = join(ROOT_DIR, 'catalog');

/** Extensions scanned for icon-name literals. */
const SCANNED_EXTENSIONS = new Set(['.svelte', '.ts']);

/** A component rendering the shared sprite (`theme/Icon.svelte`). */
const SPRITE_USAGE = /<Icon[ >]/;

/**
 * A literal handed to a prop whose name mentions "icon" — `icon="gauge"`,
 * `iconName="terminal"` — i.e. a caller feeding an icon name downwards.
 */
const ICON_PROP_LITERAL = /\b\w*[Ii]con\w*\s*=\s*["']/;

/**
 * Icons that must survive regardless of what the scan finds — names reachable
 * only through a component's public API rather than a literal at the call
 * site. Keep this list short and justified: anything genuinely used is already
 * a literal in the files above.
 */
const ALWAYS_INCLUDE = [];

/** Recursively list every file under `dir` (absolute paths). */
function walkFiles(dir, out = []) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) {
      walkFiles(full, out);
    } else if (entry.isFile()) {
      out.push(full);
    }
  }
  return out;
}

/**
 * Every `.svelte` / `.ts` file whose string literals can become a sprite icon
 * name at runtime:
 *
 *   1. it renders the sprite (`<Icon name="folder">`, or a literal inside a
 *      `name={cond ? 'a' : 'b'}` expression),
 *   2. it hands an icon name down as a prop (`<EmptyState icon="gauge">`),
 *   3. it is a store/service module that supplies such a component with its
 *      data (`$lib/stores/**`, `$lib/services/**`).
 */
function scanFiles() {
  const all = walkFiles(SRC_DIR);
  const sources = all.filter((f) => SCANNED_EXTENSIONS.has(extname(f)));

  const rendersSprite = sources.filter((f) => SPRITE_USAGE.test(readFileSync(f, 'utf-8')));
  const passesIconProp = sources.filter((f) => ICON_PROP_LITERAL.test(readFileSync(f, 'utf-8')));
  const sharedModules = all.filter(
    (f) => f.endsWith('.ts') && /[\\/](stores|services)[\\/]/.test(f)
  );

  return new Set([...rendersSprite, ...passesIconProp, ...sharedModules]);
}

/**
 * Every string literal in `source`, e.g. `'folder'`, `"caret-up"`. Deliberately
 * over-inclusive: a false positive only keeps one extra glyph in the sprite,
 * while a false negative ships a blank icon at runtime.
 */
function stringLiterals(source) {
  const found = new Set();
  const pattern = /'([^'\\\n]{1,64})'|"([^"\\\n]{1,64})"/g;
  for (const match of source.matchAll(pattern)) {
    found.add(match[1] ?? match[2]);
  }
  return found;
}

/**
 * Base names of every icon the frontend can reference: the string literals of
 * the files `scanFiles` selects, plus the first-party app catalog manifests,
 * which name their icon as a plain kebab-case string (`icon = "book-open"`).
 */
function collectUsedNames() {
  const used = new Set(ALWAYS_INCLUDE);
  const files = scanFiles();

  for (const file of files) {
    for (const literal of stringLiterals(readFileSync(file, 'utf-8'))) {
      // `name="tabler-foo"` and `name="foo"` both mean base name `foo`.
      used.add(literal);
      used.add(literal.replace(/^(tabler|phosphor)-/, ''));
    }
  }

  const indexHtml = join(FRONTEND_DIR, 'index.html');
  if (existsSync(indexHtml)) {
    for (const literal of stringLiterals(readFileSync(indexHtml, 'utf-8'))) used.add(literal);
  }

  if (existsSync(CATALOG_DIR)) {
    const manifests = walkFiles(CATALOG_DIR).filter((f) => f.endsWith('.toml'));
    for (const file of manifests) {
      for (const match of readFileSync(file, 'utf-8').matchAll(/^\s*icon\s*=\s*"([^"]+)"/gm)) {
        used.add(match[1]);
      }
    }
  }

  console.log(`\nScanned ${files.size} source file(s) for icon references.`);
  return used;
}

/** Keep only the icons whose base name is actually referenced. */
function subsetIcons(allIcons) {
  if (process.env.HV_SPRITE_FULL === '1') {
    console.log('\nHV_SPRITE_FULL=1 — keeping the complete sprite.');
    return allIcons;
  }

  const used = collectUsedNames();
  const kept = {};
  for (const [name, icon] of Object.entries(allIcons)) {
    if (used.has(name.replace(/^(phosphor|tabler)-/, ''))) kept[name] = icon;
  }

  const total = Object.keys(allIcons).length;
  const count = Object.keys(kept).length;
  console.log(
    `\nSubset: ${count}/${total} icons referenced from ${used.size} scanned names ` +
      `(${total - count} unused glyphs dropped)`
  );
  return kept;
}

console.log('Frontend dir:', FRONTEND_DIR);

/**
 * Parse SVG string to extract paths and viewBox
 */
function parseSvgContent(svgString) {
  const viewBoxMatch = svgString.match(/viewBox="([^"]*)"/);
  const viewBox = viewBoxMatch ? viewBoxMatch[1] : '0 0 24 24';

  const contentMatch = svgString.match(/<svg[^>]*>([\s\S]*)<\/svg>/);
  if (!contentMatch) return null;

  let content = contentMatch[1];
  content = content.replace(/\s(width|height)="[^"]*"/g, '');

  return { viewBox, content: content.trim() };
}

/**
 * Generate SVG symbol
 */
function createSymbol(id, viewBox, content) {
  return `  <symbol id="${id}" viewBox="${viewBox}">
${content.split('\n').map(line => '    ' + line.trim()).filter(Boolean).join('\n')}
  </symbol>`;
}

/**
 * Read Phosphor icons from selection.json
 */
function readPhosphorIcons() {
  const icons = {};
  const selectionPath = join(FRONTEND_DIR, 'node_modules/@phosphor-icons/web/src/regular/selection.json');
  console.log('  Phosphor path:', selectionPath);

  if (!existsSync(selectionPath)) {
    console.warn('Phosphor selection.json not found');
    return icons;
  }

  const selection = JSON.parse(readFileSync(selectionPath, 'utf-8'));

  for (const item of selection.icons) {
    if (!item.properties || !item.icon) continue;

    const name = item.properties.name;
    if (!name) continue;

    const paths = item.icon.paths || [];
    if (paths.length === 0) continue;

    const pathElements = paths.map(p => `<path d="${p}" />`).join('\n');

    icons[`phosphor-${name}`] = {
      viewBox: '0 0 256 256',
      content: pathElements
    };
  }

  return icons;
}

/**
 * Read Tabler icons from Svelte components
 * Format: const iconNode = [["path", { "d": "..." }], ...];
 */
function readTablerIcons() {
  const icons = {};
  const iconsDir = join(FRONTEND_DIR, 'node_modules/@tabler/icons-svelte/dist/icons');
  console.log('  Tabler path:', iconsDir);

  if (!existsSync(iconsDir)) {
    console.warn('Tabler icons directory not found');
    return icons;
  }

  const files = readdirSync(iconsDir).filter(f => f.endsWith('.svelte'));

  for (const file of files) {
    const name = basename(file, '.svelte');
    const content = readFileSync(join(iconsDir, file), 'utf-8');

    // Parse iconNode from script tag
    const nodeMatch = content.match(/const iconNode\s*=\s*(\[[\s\S]*?\]);/);
    if (!nodeMatch) continue;

    try {
      const iconNodes = JSON.parse(nodeMatch[1]);

      // Convert nodes to SVG elements
      const elements = iconNodes.map(([tag, attrs]) => {
        const { d, ...rest } = attrs;
        const attrStr = Object.entries(rest)
          .map(([k, v]) => `${k}="${v}"`)
          .join(' ');
        return `<${tag} d="${d}" ${attrStr} />`;
      }).join('\n');

      icons[`tabler-${name}`] = {
        viewBox: '0 0 24 24',
        content: elements
      };
    } catch (e) {
      // Skip malformed icons
    }
  }

  return icons;
}

/**
 * Main generation function
 */
async function generateSprite() {
  console.log('Generating unified icon sprite...\n');

  const allIcons = {};

  console.log('Reading Phosphor icons...');
  const phosphorIcons = readPhosphorIcons();
  Object.assign(allIcons, phosphorIcons);
  console.log(`  Found ${Object.keys(phosphorIcons).length} Phosphor icons`);

  console.log('Reading Tabler icons...');
  const tablerIcons = readTablerIcons();
  Object.assign(allIcons, tablerIcons);
  console.log(`  Found ${Object.keys(tablerIcons).length} Tabler icons`);

  const total = Object.keys(allIcons).length;
  console.log(`\nTotal available: ${total} icons`);

  // Drop everything the frontend never references (see `subsetIcons`).
  const used = subsetIcons(allIcons);
  const kept = Object.keys(used).length;

  // A scan that silently stopped matching (moved directory, renamed prop)
  // would generate an empty sprite and blank every icon in the UI, with no
  // error anywhere — fail the build instead.
  if (kept === 0) {
    throw new Error(
      'Icon subset is empty — the reference scan matched nothing. ' +
        'Run with HV_SPRITE_FULL=1 to fall back to the full sprite.'
    );
  }

  // Generate sprite
  const symbols = Object.keys(used)
    .sort()
    .map(name => {
      const icon = used[name];
      return createSymbol(name, icon.viewBox, icon.content);
    });

  const sprite = `<?xml version="1.0" encoding="UTF-8"?>
<!--
  Unified Icon Sprite
  Generated from: Phosphor Icons, Tabler Icons
  Subset to the ${kept} of ${total} glyphs the frontend actually references —
  regenerate with the same script (\`npm run icons:generate\`), or with
  \`HV_SPRITE_FULL=1\` for the complete set.

  Usage:
  <svg><use href="#icon-name" /></svg>

  Prefixes:
  - phosphor- : Phosphor Icons (regular style)
  - tabler-   : Tabler Icons
-->
<svg xmlns="http://www.w3.org/2000/svg" style="display:none;">
${symbols.join('\n')}
</svg>
`;

  if (!existsSync(PUBLIC_DIR)) {
    mkdirSync(PUBLIC_DIR, { recursive: true });
  }

  writeFileSync(OUTPUT_SPRITE, sprite, 'utf-8');
  console.log(`\nSprite written to: ${OUTPUT_SPRITE}`);

  // Generate mapping file
  generateMapping(used);

  return kept;
}

/**
 * Generate TypeScript mapping file
 */
function generateMapping(allIcons) {
  const byName = {};

  for (const fullName of Object.keys(allIcons)) {
    const match = fullName.match(/^(phosphor|tabler)-(.*)$/);
    if (!match) continue;

    const [, prefix, baseName] = match;

    if (!byName[baseName]) {
      byName[baseName] = {};
    }
    byName[baseName][prefix] = fullName;
  }

  const phosphorCount = Object.keys(allIcons).filter(n => n.startsWith('phosphor-')).length;
  const tablerCount = Object.keys(allIcons).filter(n => n.startsWith('tabler-')).length;

  const mapping = `/**
 * Unified Icon Mapping
 * Generated from: Phosphor Icons, Tabler Icons
 *
 * Usage:
 * import { getIcon, iconNames } from '$lib/icons/mapping-unified';
 */

export const iconLibraries = ['phosphor', 'tabler'] as const;
export type IconLibrary = typeof iconLibraries[number];

export interface IconEntry {
  phosphor?: string;
  tabler?: string;
}

export const iconCount = {
  phosphor: ${phosphorCount},
  tabler: ${tablerCount},
  total: ${Object.keys(allIcons).length}
};

export const iconLookup: Record<string, IconEntry> = ${JSON.stringify(byName, null, 2)};

export function getIcon(baseName: string, library?: IconLibrary): string | null {
  const entry = iconLookup[baseName];
  if (!entry) return null;

  if (library) {
    return entry[library] || null;
  }

  return entry.tabler || entry.phosphor || null;
}

export function hasIcon(baseName: string): boolean {
  return baseName in iconLookup;
}

export function getAllIcons(baseName: string): string[] {
  const entry = iconLookup[baseName];
  if (!entry) return [];
  return Object.values(entry).filter(Boolean);
}

export const iconNames = Object.keys(iconLookup);
`;

  writeFileSync(OUTPUT_MAPPING, mapping, 'utf-8');
  console.log(`Mapping written to: ${OUTPUT_MAPPING}`);
}

// Run
generateSprite()
  .then(total => {
    console.log(`\n✓ Successfully generated unified sprite with ${total} icons`);
    process.exit(0);
  })
  .catch(e => {
    console.error('Error:', e);
    process.exit(1);
  });
