#!/usr/bin/env node

/**
 * Script to generate minimal Lucide icon sprite for HomeVault
 * Only includes icons that are actually used in the codebase
 */

import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'fs';
import { join, basename, dirname } from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

const FRONTEND_DIR = join(__dirname, '../');
const PUBLIC_DIR = join(FRONTEND_DIR, 'public');
const OUTPUT_SPRITE = join(PUBLIC_DIR, 'sprite-lucide.svg');

// Icons to include (from lucide-mapping.ts + used icons)
const ICONS_TO_INCLUDE = [
  'loader', 'user-round', 'users-round', 'layout-grid', 'moon', 'bell', 'cpu', 'hard-drive',
  'wifi', 'folder-plus', 'list', 'grid-3x3', 'copy', 'file-input', 'cloud-upload', 'cloud-check',
  'shield-check', 'cast', 'play', 'alert-circle', 'clapperboard', 'globe', 'power', 'x',
  'user', 'settings', 'lock', 'shield', 'refresh-cw', 'wifi-off', 'bookmark', 'home',
  'arrow-up', 'upload', 'trash-2', 'download', 'eye', 'archive-restore', 'pencil', 'folder-open',
  'chevron-left', 'plus', 'check-circle', 'pause', 'square', 'usb', 'hard-drive',
  'square-terminal', 'app-window', 'layout-dashboard',
  'folder', 'container', 'network', 'save', 'users', 'share-2',
  'cloud', 'heart-pulse', 'archive'
];

function parseSvgContent(svgString) {
  const viewBoxMatch = svgString.match(/viewBox="([^"]*)"/);
  const viewBox = viewBoxMatch ? viewBoxMatch[1] : '0 0 24 24';

  const contentMatch = svgString.match(/<svg[^>]*>([\s\S]*)<\/svg>/);
  if (!contentMatch) return null;

  let content = contentMatch[1];
  content = content.replace(/\s(width|height)="[^"]*"/g, '');

  return { viewBox, content: content.trim() };
}

function createSymbol(id, viewBox, content) {
  return `  <symbol id="${id}" viewBox="${viewBox}">
${content.split('\n').map(line => '    ' + line.trim()).filter(Boolean).join('\n')}
  </symbol>`;
}

function readLucideIcons() {
  const icons = {};
  const iconsDir = join(FRONTEND_DIR, 'node_modules/lucide-static/icons');

  if (!existsSync(iconsDir)) {
    console.warn('Lucide icons directory not found');
    return icons;
  }

  for (const iconName of ICONS_TO_INCLUDE) {
    const filePath = join(iconsDir, `${iconName}.svg`);
    if (!existsSync(filePath)) {
      console.warn(`Icon not found: ${iconName}`);
      continue;
    }

    const content = readFileSync(filePath, 'utf-8');
    const parsed = parseSvgContent(content);

    if (parsed) {
      icons[`lucide-${iconName}`] = {
        viewBox: parsed.viewBox,
        content: parsed.content
      };
    }
  }

  return icons;
}

async function generateSprite() {
  console.log('Generating minimal Lucide sprite...\n');

  console.log('Reading Lucide icons...');
  const lucideIcons = readLucideIcons();
  const total = Object.keys(lucideIcons).length;
  console.log(`  Found ${total} icons`);

  // Generate sprite
  const symbols = Object.keys(lucideIcons)
    .sort()
    .map(name => {
      const icon = lucideIcons[name];
      return createSymbol(name, icon.viewBox, icon.content);
    });

  const sprite = `<?xml version="1.0" encoding="UTF-8"?>
<!--
  Lucide Icon Sprite - HomeVault
  Generated from: Lucide Icons (minimal set)
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
  console.log(`Size: ${(sprite.length / 1024).toFixed(1)} KB`);
  console.log(`\n✓ Successfully generated minimal sprite with ${total} icons`);
}

generateSprite().catch(e => {
  console.error('Error:', e);
  process.exit(1);
});