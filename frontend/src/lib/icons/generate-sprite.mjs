#!/usr/bin/env node

/**
 * Script to generate unified icon sprite from multiple icon libraries:
 * - Phosphor Icons (@phosphor-icons/web) - from selection.json
 * - Tabler Icons (@tabler/icons-svelte) - from Svelte components
 */

import { readFileSync, writeFileSync, readdirSync, mkdirSync, existsSync } from 'fs';
import { join, basename, dirname } from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

// Correct paths - script is in src/lib/icons/, need to go up 3 levels
const FRONTEND_DIR = join(__dirname, '../../../');
const PUBLIC_DIR = join(FRONTEND_DIR, 'public');
const OUTPUT_SPRITE = join(PUBLIC_DIR, 'sprite-unified.svg');
const OUTPUT_MAPPING = join(__dirname, 'mapping-unified.ts');

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
  console.log(`\nTotal: ${total} icons`);

  // Generate sprite
  const symbols = Object.keys(allIcons)
    .sort()
    .map(name => {
      const icon = allIcons[name];
      return createSymbol(name, icon.viewBox, icon.content);
    });

  const sprite = `<?xml version="1.0" encoding="UTF-8"?>
<!--
  Unified Icon Sprite
  Generated from: Phosphor Icons, Tabler Icons

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
  generateMapping(allIcons);

  return total;
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
