#!/usr/bin/env node

/**
 * Script pour extraire les variables de couleur et de style du thème Orchis
 * et générer des variables CSS/Tailwind pour HomeVault
 */

import { readFileSync, writeFileSync } from 'fs';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const projectRoot = join(__dirname, '..');

// Chemins vers les fichiers Orchis
const ORCHIS_COLORS = join(projectRoot, 'Orchis-theme/src/_sass/_colors.scss');
const ORCHIS_VARIABLES = join(projectRoot, 'Orchis-theme/src/_sass/_variables.scss');

// Palette de couleurs Orchis extraite manuellement (Material Design)
const ORCHIS_COLOR_PALETTE = {
  // Primary colors (Blue - Orchis default)
  blue: {
    50: '#e3f2fd',
    100: '#bbdefb',
    200: '#90caf9',
    300: '#64b5f6',
    400: '#42a5f5',
    500: '#2196f3',
    600: '#1e88e5',
    700: '#1976d2',
    800: '#1565c0',
    900: '#0d47a1',
    light: '#64b5f6',
    dark: '#1976d2',
  },

  // Alternative theme colors
  purple: { light: '#ba68c8', dark: '#8e24aa' },
  pink: { light: '#f06292', dark: '#c2185b' },
  red: { light: '#e57373', dark: '#d32f2f' },
  orange: { light: '#ff9800', dark: '#f57c00' },
  yellow: { light: '#ffeb3b', dark: '#fbc02d' },
  green: { light: '#66bb6a', dark: '#43a047' },
  teal: { light: '#26a69a', dark: '#00897b' },
  sea: { light: '#26c6da', dark: '#0097a7' },

  // Neutral grays (Orchis light/dark)
  grey: {
    '050': '#fafafa',
    '100': '#f5f5f5',
    '200': '#eeeeee',
    '250': '#ebebeb',
    '300': '#e0e0e0',
    '400': '#bdbdbd',
    '500': '#9e9e9e',
    '600': '#757575',
    '650': '#616161',
    '700': '#525252',
    '750': '#3c3c3c',
    '800': '#383838',
    '850': '#2d2d2d',
    '900': '#242424',
    '950': '#1a1a1a',
  },

  white: '#ffffff',
  black: '#000000',
};

// Variables Orchis pour les composants GTK
const ORCHIS_COMPONENT_STYLES = {
  // Spacing (extrait de _variables.scss)
  spacing: {
    compact: {
      space: '4px',
      margin: '2px',
      small: '22px',
      medium: '32px',
      large: '40px',
    },
    standard: {
      space: '6px',
      margin: '4px',
      small: '24px',
      medium: '36px',
      large: '48px',
    },
  },

  // Border radius
  radius: {
    corner: '12px',        // default_corner (peut être modifié)
    window: '18px',        // corner + space
    material: '10px',
    menu: '9px',
    popup: '18px',
    menuitem: '5px',
    circular: '9999px',
  },

  // Shadows (Material Design elevation)
  shadows: {
    z1: '0 2px 2px -2px rgba(0, 0, 0, 0.3), 0 1px 2px -1px rgba(0, 0, 0, 0.24), 0 1px 2px -1px rgba(0, 0, 0, 0.17)',
    z2: '0 3px 2px -3px rgba(0, 0, 0, 0.3), 0 2px 2px -1px rgba(0, 0, 0, 0.24), 0 1px 3px 0 rgba(0, 0, 0, 0.12)',
    z3: '0 3px 3px -2px rgba(0, 0, 0, 0.2), 0 3px 3px 0 rgba(0, 0, 0, 0.14), 0 1px 5px 0 rgba(0, 0, 0, 0.12)',
    z4: '0 2px 2px -1px rgba(0, 0, 0, 0.2), 0 4px 4px 0 rgba(0, 0, 0, 0.14), 0 1px 6px 0 rgba(0, 0, 0, 0.12)',
    z6: '0 3px 3px -1px rgba(0, 0, 0, 0.2), 0 6px 6px 0 rgba(0, 0, 0, 0.14), 0 1px 11px 0 rgba(0, 0, 0, 0.12)',
    z8: '0 5px 5px -3px rgba(0, 0, 0, 0.2), 0 8px 8px 1px rgba(0, 0, 0, 0.14), 0 3px 9px 2px rgba(0, 0, 0, 0.12)',
    z12: '0 7px 7px -4px rgba(0, 0, 0, 0.2), 0 12px 12px 2px rgba(0, 0, 0, 0.14), 0 5px 13px 4px rgba(0, 0, 0, 0.12)',
    z16: '0 8px 6px -5px rgba(0, 0, 0, 0.2), 0 16px 16px 2px rgba(0, 0, 0, 0.14), 0 6px 18px 5px rgba(0, 0, 0, 0.12)',
    z24: '0 11px 11px -7px rgba(0, 0, 0, 0.2), 0 24px 24px 3px rgba(0, 0, 0, 0.14), 0 9px 28px 8px rgba(0, 0, 0, 0.12)',
  },

  // Transitions (Material Design timing)
  transitions: {
    duration: '75ms',
    shorter: '100ms',
    longer: '150ms',
    rippleFadeIn: '225ms',
    rippleFadeOut: '300ms',
    ease: 'cubic-bezier(0.4, 0.0, 0.2, 1)',
    easeOut: 'cubic-bezier(0.0, 0.0, 0.2, 1)',
    easeIn: 'cubic-bezier(0.4, 0.0, 1, 1)',
    easeInOut: 'cubic-bezier(0.4, 0.0, 0.6, 1)',
  },

  // Typography
  typography: {
    fontFamily: '"Inter", "Cantarell", Roboto, -apple-system, sans-serif',
    rootSize: '14px',
    compactSize: '13px',
    subheadingSize: '16px',
  },
};

// États de surface (overlay) pour les interactions
const ORCHIS_OVERLAY_STATES = {
  normal: 'rgba(0, 0, 0, 0.05)',
  hover: 'rgba(0, 0, 0, 0.08)',
  focus: 'rgba(0, 0, 0, 0.08)',
  focusHover: 'rgba(0, 0, 0, 0.16)',
  active: 'rgba(0, 0, 0, 0.12)',
  checked: 'rgba(0, 0, 0, 0.10)',
  selected: 'rgba(0, 0, 0, 0.06)',
};

/**
 * Génère le fichier CSS avec les variables Orchis
 */
function generateOrchisCSS() {
  console.log('🎨 Génération des variables CSS Orchis...\n');

  const cssContent = `/* Orchis Theme Variables for HomeVault
 * Extracted from: https://github.com/vinceliuice/Orchis-theme
 * Material Design inspired GTK theme
 */

:root {
  /* ============================================
     ORCHIS COLOR PALETTE
     ============================================ */

  /* Primary Blue (Orchis default) */
  --orchis-blue-50: ${ORCHIS_COLOR_PALETTE.blue[50]};
  --orchis-blue-100: ${ORCHIS_COLOR_PALETTE.blue[100]};
  --orchis-blue-200: ${ORCHIS_COLOR_PALETTE.blue[200]};
  --orchis-blue-300: ${ORCHIS_COLOR_PALETTE.blue[300]};
  --orchis-blue-400: ${ORCHIS_COLOR_PALETTE.blue[400]};
  --orchis-blue-500: ${ORCHIS_COLOR_PALETTE.blue[500]};
  --orchis-blue-600: ${ORCHIS_COLOR_PALETTE.blue[600]};
  --orchis-blue-700: ${ORCHIS_COLOR_PALETTE.blue[700]};
  --orchis-blue-800: ${ORCHIS_COLOR_PALETTE.blue[800]};
  --orchis-blue-900: ${ORCHIS_COLOR_PALETTE.blue[900]};

  /* Alternative Theme Colors */
  --orchis-purple-light: ${ORCHIS_COLOR_PALETTE.purple.light};
  --orchis-purple-dark: ${ORCHIS_COLOR_PALETTE.purple.dark};
  --orchis-pink-light: ${ORCHIS_COLOR_PALETTE.pink.light};
  --orchis-pink-dark: ${ORCHIS_COLOR_PALETTE.pink.dark};
  --orchis-red-light: ${ORCHIS_COLOR_PALETTE.red.light};
  --orchis-red-dark: ${ORCHIS_COLOR_PALETTE.red.dark};
  --orchis-orange-light: ${ORCHIS_COLOR_PALETTE.orange.light};
  --orchis-orange-dark: ${ORCHIS_COLOR_PALETTE.orange.dark};
  --orchis-yellow-light: ${ORCHIS_COLOR_PALETTE.yellow.light};
  --orchis-yellow-dark: ${ORCHIS_COLOR_PALETTE.yellow.dark};
  --orchis-green-light: ${ORCHIS_COLOR_PALETTE.green.light};
  --orchis-green-dark: ${ORCHIS_COLOR_PALETTE.green.dark};
  --orchis-teal-light: ${ORCHIS_COLOR_PALETTE.teal.light};
  --orchis-teal-dark: ${ORCHIS_COLOR_PALETTE.teal.dark};

  /* Neutral Grays */
  --orchis-grey-050: ${ORCHIS_COLOR_PALETTE.grey['050']};
  --orchis-grey-100: ${ORCHIS_COLOR_PALETTE.grey['100']};
  --orchis-grey-200: ${ORCHIS_COLOR_PALETTE.grey['200']};
  --orchis-grey-250: ${ORCHIS_COLOR_PALETTE.grey['250']};
  --orchis-grey-300: ${ORCHIS_COLOR_PALETTE.grey['300']};
  --orchis-grey-400: ${ORCHIS_COLOR_PALETTE.grey['400']};
  --orchis-grey-500: ${ORCHIS_COLOR_PALETTE.grey['500']};
  --orchis-grey-600: ${ORCHIS_COLOR_PALETTE.grey['600']};
  --orchis-grey-650: ${ORCHIS_COLOR_PALETTE.grey['650']};
  --orchis-grey-700: ${ORCHIS_COLOR_PALETTE.grey['700']};
  --orchis-grey-750: ${ORCHIS_COLOR_PALETTE.grey['750']};
  --orchis-grey-800: ${ORCHIS_COLOR_PALETTE.grey['800']};
  --orchis-grey-850: ${ORCHIS_COLOR_PALETTE.grey['850']};
  --orchis-grey-900: ${ORCHIS_COLOR_PALETTE.grey['900']};
  --orchis-grey-950: ${ORCHIS_COLOR_PALETTE.grey['950']};

  /* ============================================
     ORCHIS SEMANTIC COLORS (LIGHT MODE)
     ============================================ */

  --orchis-primary: var(--orchis-blue-700);
  --orchis-background: ${ORCHIS_COLOR_PALETTE.grey['100']};
  --orchis-surface: ${ORCHIS_COLOR_PALETTE.white};
  --orchis-surface-variant: ${ORCHIS_COLOR_PALETTE.grey['050']};
  --orchis-base: ${ORCHIS_COLOR_PALETTE.white};

  /* Text colors */
  --orchis-text: rgba(0, 0, 0, 0.87);
  --orchis-text-secondary: rgba(0, 0, 0, 0.6);
  --orchis-text-disabled: rgba(0, 0, 0, 0.38);

  /* Borders */
  --orchis-border: rgba(0, 0, 0, 0.12);
  --orchis-border-strong: rgba(0, 0, 0, 0.16);
  --orchis-divider: rgba(0, 0, 0, 0.12);

  /* Overlay states */
  --orchis-overlay-normal: ${ORCHIS_OVERLAY_STATES.normal};
  --orchis-overlay-hover: ${ORCHIS_OVERLAY_STATES.hover};
  --orchis-overlay-focus: ${ORCHIS_OVERLAY_STATES.focus};
  --orchis-overlay-active: ${ORCHIS_OVERLAY_STATES.active};
  --orchis-overlay-selected: ${ORCHIS_OVERLAY_STATES.selected};

  /* Semantic feedback */
  --orchis-success: ${ORCHIS_COLOR_PALETTE.green.dark};
  --orchis-warning: ${ORCHIS_COLOR_PALETTE.yellow.dark};
  --orchis-error: ${ORCHIS_COLOR_PALETTE.red.dark};
  --orchis-info: ${ORCHIS_COLOR_PALETTE.blue.dark};

  /* ============================================
     ORCHIS SPACING
     ============================================ */

  --orchis-space: ${ORCHIS_COMPONENT_STYLES.spacing.standard.space};
  --orchis-margin: ${ORCHIS_COMPONENT_STYLES.spacing.standard.margin};
  --orchis-size-small: ${ORCHIS_COMPONENT_STYLES.spacing.standard.small};
  --orchis-size-medium: ${ORCHIS_COMPONENT_STYLES.spacing.standard.medium};
  --orchis-size-large: ${ORCHIS_COMPONENT_STYLES.spacing.standard.large};

  /* ============================================
     ORCHIS BORDER RADIUS
     ============================================ */

  --orchis-radius-corner: ${ORCHIS_COMPONENT_STYLES.radius.corner};
  --orchis-radius-window: ${ORCHIS_COMPONENT_STYLES.radius.window};
  --orchis-radius-material: ${ORCHIS_COMPONENT_STYLES.radius.material};
  --orchis-radius-menu: ${ORCHIS_COMPONENT_STYLES.radius.menu};
  --orchis-radius-popup: ${ORCHIS_COMPONENT_STYLES.radius.popup};
  --orchis-radius-menuitem: ${ORCHIS_COMPONENT_STYLES.radius.menuitem};
  --orchis-radius-circular: ${ORCHIS_COMPONENT_STYLES.radius.circular};

  /* ============================================
     ORCHIS SHADOWS (Material Design Elevation)
     ============================================ */

  --orchis-shadow-z1: ${ORCHIS_COMPONENT_STYLES.shadows.z1};
  --orchis-shadow-z2: ${ORCHIS_COMPONENT_STYLES.shadows.z2};
  --orchis-shadow-z3: ${ORCHIS_COMPONENT_STYLES.shadows.z3};
  --orchis-shadow-z4: ${ORCHIS_COMPONENT_STYLES.shadows.z4};
  --orchis-shadow-z6: ${ORCHIS_COMPONENT_STYLES.shadows.z6};
  --orchis-shadow-z8: ${ORCHIS_COMPONENT_STYLES.shadows.z8};
  --orchis-shadow-z12: ${ORCHIS_COMPONENT_STYLES.shadows.z12};
  --orchis-shadow-z16: ${ORCHIS_COMPONENT_STYLES.shadows.z16};
  --orchis-shadow-z24: ${ORCHIS_COMPONENT_STYLES.shadows.z24};

  /* ============================================
     ORCHIS TRANSITIONS
     ============================================ */

  --orchis-duration: ${ORCHIS_COMPONENT_STYLES.transitions.duration};
  --orchis-duration-shorter: ${ORCHIS_COMPONENT_STYLES.transitions.shorter};
  --orchis-duration-longer: ${ORCHIS_COMPONENT_STYLES.transitions.longer};
  --orchis-ease: ${ORCHIS_COMPONENT_STYLES.transitions.ease};
  --orchis-ease-out: ${ORCHIS_COMPONENT_STYLES.transitions.easeOut};
  --orchis-ease-in: ${ORCHIS_COMPONENT_STYLES.transitions.easeIn};
  --orchis-transition: all var(--orchis-duration) var(--orchis-ease-out);

  /* ============================================
     ORCHIS TYPOGRAPHY
     ============================================ */

  --orchis-font-family: ${ORCHIS_COMPONENT_STYLES.typography.fontFamily};
  --orchis-font-size: ${ORCHIS_COMPONENT_STYLES.typography.rootSize};
  --orchis-font-size-subheading: ${ORCHIS_COMPONENT_STYLES.typography.subheadingSize};
}

/* ============================================
   DARK MODE OVERRIDES
   ============================================ */

.dark {
  --orchis-primary: var(--orchis-blue-400);
  --orchis-background: var(--orchis-grey-800);
  --orchis-surface: var(--orchis-grey-850);
  --orchis-surface-variant: var(--orchis-grey-900);
  --orchis-base: var(--orchis-grey-700);

  /* Text colors (dark mode) */
  --orchis-text: ${ORCHIS_COLOR_PALETTE.white};
  --orchis-text-secondary: rgba(255, 255, 255, 0.7);
  --orchis-text-disabled: rgba(255, 255, 255, 0.5);

  /* Borders (dark mode) */
  --orchis-border: rgba(255, 255, 255, 0.12);
  --orchis-border-strong: rgba(255, 255, 255, 0.16);
  --orchis-divider: rgba(255, 255, 255, 0.12);

  /* Overlay states (dark mode) */
  --orchis-overlay-normal: rgba(255, 255, 255, 0.05);
  --orchis-overlay-hover: rgba(255, 255, 255, 0.08);
  --orchis-overlay-focus: rgba(255, 255, 255, 0.08);
  --orchis-overlay-active: rgba(255, 255, 255, 0.12);
  --orchis-overlay-selected: rgba(255, 255, 255, 0.06);

  /* Semantic feedback (dark mode - lighter variants) */
  --orchis-success: ${ORCHIS_COLOR_PALETTE.green.light};
  --orchis-warning: ${ORCHIS_COLOR_PALETTE.yellow.light};
  --orchis-error: ${ORCHIS_COLOR_PALETTE.red.light};
  --orchis-info: ${ORCHIS_COLOR_PALETTE.blue.light};
}
`;

  const outputPath = join(projectRoot, 'frontend/src/styles/orchis-variables.css');
  writeFileSync(outputPath, cssContent, 'utf-8');

  console.log(`✓ Variables CSS Orchis générées: ${outputPath}`);
  return outputPath;
}

/**
 * Génère la configuration Tailwind avec les couleurs Orchis
 */
function generateTailwindConfig() {
  console.log('🎨 Génération de la configuration Tailwind avec Orchis...\n');

  const tailwindExtension = {
    colors: ORCHIS_COLOR_PALETTE,
    spacing: {
      'orchis-space': ORCHIS_COMPONENT_STYLES.spacing.standard.space,
      'orchis-margin': ORCHIS_COMPONENT_STYLES.spacing.standard.margin,
      'orchis-small': ORCHIS_COMPONENT_STYLES.spacing.standard.small,
      'orchis-medium': ORCHIS_COMPONENT_STYLES.spacing.standard.medium,
      'orchis-large': ORCHIS_COMPONENT_STYLES.spacing.standard.large,
    },
    borderRadius: {
      'orchis-corner': ORCHIS_COMPONENT_STYLES.radius.corner,
      'orchis-window': ORCHIS_COMPONENT_STYLES.radius.window,
      'orchis-material': ORCHIS_COMPONENT_STYLES.radius.material,
      'orchis-menu': ORCHIS_COMPONENT_STYLES.radius.menu,
      'orchis-popup': ORCHIS_COMPONENT_STYLES.radius.popup,
      'orchis-menuitem': ORCHIS_COMPONENT_STYLES.radius.menuitem,
    },
    boxShadow: ORCHIS_COMPONENT_STYLES.shadows,
  };

  const configContent = `// Orchis Theme Extension for Tailwind CSS
// Auto-generated from Orchis GTK theme

export const orchisTheme = ${JSON.stringify(tailwindExtension, null, 2)};
`;

  const outputPath = join(projectRoot, 'frontend/src/styles/orchis-tailwind.js');
  writeFileSync(outputPath, configContent, 'utf-8');

  console.log(`✓ Configuration Tailwind Orchis générée: ${outputPath}`);
  return outputPath;
}

/**
 * Génère le fichier TypeScript avec les constantes Orchis
 */
function generateOrchisConstants() {
  console.log('🎨 Génération des constantes TypeScript Orchis...\n');

  const tsContent = `/**
 * Orchis Theme Constants for HomeVault
 * Type-safe access to Orchis design tokens
 */

export const OrchisColors = ${JSON.stringify(ORCHIS_COLOR_PALETTE, null, 2)} as const;

export const OrchisSpacing = ${JSON.stringify(ORCHIS_COMPONENT_STYLES.spacing, null, 2)} as const;

export const OrchisRadius = ${JSON.stringify(ORCHIS_COMPONENT_STYLES.radius, null, 2)} as const;

export const OrchisShadows = ${JSON.stringify(ORCHIS_COMPONENT_STYLES.shadows, null, 2)} as const;

export const OrchisTransitions = ${JSON.stringify(ORCHIS_COMPONENT_STYLES.transitions, null, 2)} as const;

export const OrchisOverlay = ${JSON.stringify(ORCHIS_OVERLAY_STATES, null, 2)} as const;

/**
 * Get Orchis color by path
 * @example getOrchisColor('blue.500') // '#2196f3'
 */
export function getOrchisColor(path: string): string {
  const parts = path.split('.');
  let value: any = OrchisColors;

  for (const part of parts) {
    value = value[part];
    if (!value) return '';
  }

  return typeof value === 'string' ? value : '';
}

/**
 * CSS variable names for Orchis theme
 */
export const OrchisVars = {
  primary: '--orchis-primary',
  background: '--orchis-background',
  surface: '--orchis-surface',
  text: '--orchis-text',
  border: '--orchis-border',
  // ... add more as needed
} as const;
`;

  const outputPath = join(projectRoot, 'frontend/src/lib/constants/orchis.ts');
  writeFileSync(outputPath, tsContent, 'utf-8');

  console.log(`✓ Constantes TypeScript Orchis générées: ${outputPath}`);
  return outputPath;
}

// Exécution
try {
  console.log('╔═══════════════════════════════════════════════════════════════╗');
  console.log('║   🎨 EXTRACTION DES VARIABLES ORCHIS THEME                   ║');
  console.log('╚═══════════════════════════════════════════════════════════════╝\n');

  const cssPath = generateOrchisCSS();
  const tailwindPath = generateTailwindConfig();
  const tsPath = generateOrchisConstants();

  console.log('\n✨ Extraction terminée avec succès!\n');
  console.log('📦 Fichiers générés:');
  console.log(`  1. ${cssPath}`);
  console.log(`  2. ${tailwindPath}`);
  console.log(`  3. ${tsPath}`);
  console.log('\n📚 Prochaines étapes:');
  console.log('  1. Importer orchis-variables.css dans app.css');
  console.log('  2. Mettre à jour les composants UI pour utiliser les variables Orchis');
  console.log('  3. Adapter WindowFrame, Button, Input avec les styles GTK\n');

} catch (error) {
  console.error('❌ Erreur:', error.message);
  process.exit(1);
}
