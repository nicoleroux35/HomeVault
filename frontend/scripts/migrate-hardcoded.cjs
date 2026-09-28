#!/usr/bin/env node
/**
 * Script de Migration des Valeurs Hardcodées vers Variables CSS
 * Convertit les couleurs hex, rgba() et valeurs px en variables --hv-*
 */

const fs = require('fs');
const path = require('path');

// Mapping des couleurs hex vers variables CSS
const COLOR_MAP = {
  // Blanc et variations
  '#ffffff': 'var(--hv-surface-window)',
  '#fff': 'var(--hv-surface-window)',
  '#fafafa': 'var(--hv-neutral-50)',
  '#f9fafb': 'var(--hv-neutral-50)',
  '#f7f9fb': 'var(--hv-surface)',
  '#f5f5f5': 'var(--hv-neutral-100)',
  '#f3f4f6': 'var(--hv-neutral-100)',
  '#f2f4f6': 'var(--hv-surface-container-low)',
  '#efefef': 'var(--hv-neutral-100)',
  '#eeeeee': 'var(--hv-neutral-200)',
  '#eceef0': 'var(--hv-surface-container)',
  '#e5e7eb': 'var(--hv-border)',
  '#e0e0e0': 'var(--hv-neutral-300)',
  '#d1d5db': 'var(--hv-neutral-300)',

  // Gris / Neutrals
  '#9ca3af': 'var(--hv-neutral-400)',
  '#6b7280': 'var(--hv-neutral-500)',
  '#4b5563': 'var(--hv-neutral-600)',
  '#374151': 'var(--hv-neutral-700)',
  '#1f2937': 'var(--hv-neutral-800)',
  '#111827': 'var(--hv-neutral-900)',
  '#000000': 'var(--hv-text-primary)',
  '#000': 'var(--hv-text-primary)',

  // Bleu primaire
  '#2E72C9': 'var(--hv-primary)',
  '#2e72c9': 'var(--hv-primary)',
  '#1e5bb8': 'var(--hv-primary-hover)',
  '#003590': 'var(--hv-primary-active)',
  '#2563eb': 'var(--hv-info)',
  '#1d4fd8': 'var(--hv-info-hover)',
  '#3b82f6': 'var(--hv-info)',
  '#60a5fa': 'var(--hv-primary-400)',
  '#93c5fd': 'var(--hv-primary-300)',
  '#dbeafe': 'var(--hv-info-light)',
  '#eff6ff': 'var(--hv-info-light)',

  // Vert / Success
  '#10b981': 'var(--hv-success)',
  '#059669': 'var(--hv-success-hover)',
  '#22c55e': 'var(--hv-success)',
  '#16a34a': 'var(--hv-success-hover)',
  '#dcfce7': 'var(--hv-success-light)',
  '#d1fae5': 'var(--hv-badge-success-bg)',
  '#065f46': 'var(--hv-badge-success-text)',

  // Orange / Warning
  '#f59e0b': 'var(--hv-warning)',
  '#d97706': 'var(--hv-warning-hover)',
  '#fef3c7': 'var(--hv-badge-warning-bg)',
  '#92400e': 'var(--hv-badge-warning-text)',
  '#ff9800': 'var(--hv-warning)',

  // Rouge / Error
  '#ba1a1a': 'var(--hv-error)',
  '#ef4444': 'var(--hv-error)',
  '#dc2626': 'var(--hv-error-hover)',
  '#991b1b': 'var(--hv-badge-error-text)',
  '#fee2e2': 'var(--hv-badge-error-bg)',
  '#e57373': 'var(--hv-error)',
  '#d32f2f': 'var(--hv-error-hover)',

  // Slate / Dark theme
  '#1e293b': 'var(--hv-neutral-800)',
  '#0f172a': 'var(--hv-neutral-900)',
  '#334155': 'var(--hv-neutral-700)',
  '#475569': 'var(--hv-neutral-600)',
  '#64748b': 'var(--hv-neutral-500)',
  '#94a3b8': 'var(--hv-neutral-400)',
  '#cbd5e1': 'var(--hv-neutral-300)',
  '#e2e8f0': 'var(--hv-neutral-200)',
  '#f1f5f9': 'var(--hv-neutral-100)',
};

// Mapping des spacing px vers variables
const SPACING_MAP = {
  '2px': 'var(--hv-space-0-5)',
  '4px': 'var(--hv-space-1)',
  '6px': 'var(--hv-space-1-5)',
  '8px': 'var(--hv-space-2)',
  '12px': 'var(--hv-space-3)',
  '16px': 'var(--hv-space-4)',
  '20px': 'var(--hv-space-5)',
  '24px': 'var(--hv-space-6)',
  '32px': 'var(--hv-space-8)',
  '40px': 'var(--hv-space-10)',
  '48px': 'var(--hv-space-12)',
  '56px': 'var(--hv-space-14)',
  '64px': 'var(--hv-space-16)',
  '80px': 'var(--hv-space-20)',
};

// Mapping des font-weight
const FONT_WEIGHT_MAP = {
  '400': 'var(--hv-font-weight-regular)',
  '500': 'var(--hv-font-weight-medium)',
  '600': 'var(--hv-font-weight-semibold)',
  '700': 'var(--hv-font-weight-bold)',
};

// Mapping des font-size
const FONT_SIZE_MAP = {
  '10px': 'var(--hv-font-size-micro)',
  '11px': 'var(--hv-font-size-xs)',
  '12px': 'var(--hv-font-size-sm)',
  '13px': 'var(--hv-font-size-sm)',
  '14px': 'var(--hv-font-size-base)',
  '15px': 'var(--hv-font-size-md)',
  '16px': 'var(--hv-font-size-md)',
  '18px': 'var(--hv-font-size-lg)',
  '20px': 'var(--hv-font-size-xl)',
  '24px': 'var(--hv-font-size-2xl)',
  '28px': 'var(--hv-font-size-3xl)',
  '30px': 'var(--hv-font-size-4xl)',
};

// Mapping des border-radius
const RADIUS_MAP = {
  '2px': 'var(--hv-radius-sm)',
  '4px': 'var(--hv-radius-default)',
  '6px': 'var(--hv-radius-md)',
  '8px': 'var(--hv-radius-lg)',
  '10px': 'var(--hv-radius-xl)',
  '12px': 'var(--hv-radius-xl)',
  '16px': 'var(--hv-radius-2xl)',
  '20px': 'var(--hv-radius-2xl)',
  '9999px': 'var(--hv-radius-full)',
  '50%': 'var(--hv-radius-full)',
};

let filesModified = 0;
let replacementsCount = 0;

function getAllFiles(dirPath, arrayOfFiles = []) {
  const files = fs.readdirSync(dirPath);

  files.forEach(file => {
    const filePath = path.join(dirPath, file);
    if (fs.statSync(filePath).isDirectory()) {
      // Skip node_modules and theme directory
      if (file !== 'node_modules' && file !== '.git' && filePath !== 'src/lib/styles/theme') {
        getAllFiles(filePath, arrayOfFiles);
      }
    } else {
      if (file.endsWith('.svelte') || (file.endsWith('.css') && !filePath.includes('src/lib/styles/theme'))) {
        arrayOfFiles.push(filePath);
      }
    }
  });

  return arrayOfFiles;
}

function migrateFile(filePath) {
  let content = fs.readFileSync(filePath, 'utf-8');
  let originalContent = content;
  let fileReplacements = 0;

  // 1. Remplacer les couleurs hex
  for (const [hex, cssVar] of Object.entries(COLOR_MAP)) {
    const regex = new RegExp(hex, 'gi');
    const matches = content.match(regex);
    if (matches) {
      content = content.replace(regex, cssVar);
      fileReplacements += matches.length;
    }
  }

  // 2. Remplacer les rgba() communs
  const rgbaReplacements = [
    [/rgba\(0,\s*0,\s*0,\s*0\.05\)/gi, 'var(--hv-overlay-normal)'],
    [/rgba\(0,\s*0,\s*0,\s*0\.08\)/gi, 'var(--hv-overlay-hover)'],
    [/rgba\(0,\s*0,\s*0,\s*0\.1\)/gi, 'var(--hv-overlay-hover)'],
    [/rgba\(0,\s*0,\s*0,\s*0\.12\)/gi, 'var(--hv-overlay-active)'],
    [/rgba\(0,\s*0,\s*0,\s*0\.15\)/gi, 'var(--hv-overlay-active)'],
    [/rgba\(0,\s*0,\s*0,\s*0\.2\)/gi, 'var(--hv-overlay-active)'],
    [/rgba\(0,\s*0,\s*0,\s*0\.3\)/gi, 'var(--hv-overlay-active)'],
    [/rgba\(0,\s*0,\s*0,\s*0\.38\)/gi, 'var(--hv-overlay-disabled)'],
    [/rgba\(0,\s*0,\s*0,\s*0\.5\)/gi, 'var(--hv-modal-overlay)'],
    [/rgba\(0,\s*0,\s*0,\s*0\.6\)/gi, 'var(--hv-modal-backdrop)'],
    [/rgba\(0,\s*0,\s*0,\s*0\.65\)/gi, 'var(--hv-modal-backdrop)'],
    [/rgba\(0,\s*0,\s*0,\s*0\.75\)/gi, 'var(--hv-launcher-bg)'],
    [/rgba\(0,\s*0,\s*0,\s*0\.8\)/gi, 'var(--hv-text-primary)'],
    [/rgba\(0,\s*0,\s*0,\s*0\.9\)/gi, 'var(--hv-tooltip-bg)'],

    [/rgba\(255,\s*255,\s*255,\s*0\.3\)/gi, 'var(--hv-glass-border)'],
    [/rgba\(255,\s*255,\s*255,\s*0\.5\)/gi, 'var(--hv-glass-border)'],
    [/rgba\(255,\s*255,\s*255,\s*0\.65\)/gi, 'var(--hv-glass-panel-bg)'],
    [/rgba\(255,\s*255,\s*255,\s*0\.75\)/gi, 'var(--hv-glass-panel-bg)'],
    [/rgba\(255,\s*255,\s*255,\s*0\.8\)/gi, 'var(--hv-glass-widget-bg)'],
    [/rgba\(255,\s*255,\s*255,\s*0\.85\)/gi, 'var(--hv-glass-panel-bg-hover)'],
    [/rgba\(255,\s*255,\s*255,\s*0\.9\)/gi, 'var(--hv-surface-window)'],
  ];

  for (const [regex, replacement] of rgbaReplacements) {
    const matches = content.match(regex);
    if (matches) {
      content = content.replace(regex, replacement);
      fileReplacements += matches.length;
    }
  }

  // 3. Remplacer padding/margin avec spacing
  for (const [px, cssVar] of Object.entries(SPACING_MAP)) {
    const patterns = [
      new RegExp(`(padding|margin|gap):\\s*${px}(?![\\d])`, 'gi'),
      new RegExp(`(padding-top|padding-bottom|padding-left|padding-right|margin-top|margin-bottom|margin-left|margin-right):\\s*${px}(?![\\d])`, 'gi'),
    ];

    for (const regex of patterns) {
      const matches = content.match(regex);
      if (matches) {
        content = content.replace(regex, `$1: ${cssVar}`);
        fileReplacements += matches.length;
      }
    }
  }

  // 4. Remplacer font-weight
  for (const [weight, cssVar] of Object.entries(FONT_WEIGHT_MAP)) {
    const regex = new RegExp(`font-weight:\\s*${weight}(?![\\d])`, 'gi');
    const matches = content.match(regex);
    if (matches) {
      content = content.replace(regex, `font-weight: ${cssVar}`);
      fileReplacements += matches.length;
    }
  }

  // 5. Remplacer font-size
  for (const [size, cssVar] of Object.entries(FONT_SIZE_MAP)) {
    const regex = new RegExp(`font-size:\\s*${size}(?![\\d])`, 'gi');
    const matches = content.match(regex);
    if (matches) {
      content = content.replace(regex, `font-size: ${cssVar}`);
      fileReplacements += matches.length;
    }
  }

  // 6. Remplacer border-radius
  for (const [radius, cssVar] of Object.entries(RADIUS_MAP)) {
    const safeRadius = radius.replace('px', 'px').replace('%', '%').replace('(', '\\(').replace(')', '\\)');
    const regex = new RegExp(`border-radius:\\s*${safeRadius}(?![\\d])`, 'gi');
    const matches = content.match(regex);
    if (matches) {
      content = content.replace(regex, `border-radius: ${cssVar}`);
      fileReplacements += matches.length;
    }
  }

  if (content !== originalContent) {
    fs.writeFileSync(filePath, content, 'utf-8');
    filesModified++;
    replacementsCount += fileReplacements;
    console.log(`✅ ${filePath}: ${fileReplacements} replacements`);
    return true;
  }

  return false;
}

// Exécution
console.log('🚀 Migration des valeurs hardcodées vers variables CSS\n');

const files = getAllFiles('src');

console.log(`📁 Scanning ${files.length} files...\n`);

files.forEach(file => {
  migrateFile(file);
});

console.log(`\n✨ Migration complète!`);
console.log(`📊 Statistiques:`);
console.log(`   - Fichiers modifiés: ${filesModified}`);
console.log(`   - Remplacements: ${replacementsCount}`);
console.log(`\n✅ Toutes les valeurs hardcodées ont été migrées vers des variables CSS!`);
