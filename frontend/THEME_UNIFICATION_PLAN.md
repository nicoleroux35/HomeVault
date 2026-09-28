# Plan d'Unification du Thème HomeVault
**Date:** 2026-07-05  
**Version:** 1.0  
**Objectif:** Créer un thème unique, unifié et harmonisé

---

## 🎯 Objectif Principal

Unifier les 3 systèmes de design tokens actuels en un seul système cohérent utilisant le préfixe `--hv-*`, éliminer les duplications, et standardiser l'architecture CSS.

---

## 📋 Plan d'Action Détaillé

### ÉTAPE 1: Audit et Préparation (✅ COMPLÉTÉ)

**Durée:** 1-2h

- [x] Analyse complète du système actuel
- [x] Identification des conflits et duplications
- [x] Comptage des occurrences (`--hv-*`: 1683, `--color-*`: 1373)
- [x] Documentation des problèmes (THEME_ANALYSIS_REPORT.md)
- [x] Définition de la stratégie de migration

**Résultat:** Rapport d'analyse complet créé

---

### ÉTAPE 2: Consolidation des Design Tokens

**Durée:** 2-3h  
**Priorité:** 🔴 CRITIQUE

#### 2.1 Enrichir tokens.css avec les Tokens Manquants

**Action:** Fusionner `globals.css` dans `tokens.css`

**Tokens à ajouter dans tokens.css:**

```css
/* Variables manquantes référencées dans app.css */
--hv-font-family: 'Inter', -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
--hv-radius-default: 4px;

/* Couleurs primaires avec échelle complète (depuis globals.css) */
--hv-primary-50: #eff6ff;
--hv-primary-100: #dbeafe;
--hv-primary-200: #bfdbfe;
--hv-primary-300: #93c5fd;
--hv-primary-400: #60a5fa;
--hv-primary-500: #4a90d9;  /* Aligner avec --hv-primary ou choisir */
--hv-primary-600: #3b7ac7;
--hv-primary-700: #2c5fa5;
--hv-primary-800: #1e4485;
--hv-primary-900: #1e3a8a;

/* Neutrals avec échelle complète */
--hv-neutral-50: #f9fafb;
--hv-neutral-100: #f3f4f6;
--hv-neutral-200: #e5e7eb;
--hv-neutral-300: #d1d5db;
--hv-neutral-400: #9ca3af;
--hv-neutral-500: #6b7280;
--hv-neutral-600: #4b5563;
--hv-neutral-700: #374151;
--hv-neutral-800: #1f2937;
--hv-neutral-900: #111827;

/* Sémantiques depuis globals.css */
--hv-success-light: #dcfce7;
--hv-warning-light: #fef3c7;
--hv-error-light: #fee2e2;
--hv-info-light: #dbeafe;

/* Contextuels */
--hv-background: #ffffff;
--hv-background-alt: var(--hv-surface-container-low);
--hv-border: var(--hv-outline-variant);
```

#### 2.2 Décider de la Couleur Primaire Officielle

**Conflit actuel:**
- `globals.css`: `#4a90d9` (bleu clair)
- `tokens.css`: `#2E72C9` (bleu moyen)
- `cove-tailwind.js`: `#2196f3` (Material blue)

**Recommandation:** Garder `#2E72C9` de `tokens.css`
- ✅ Déjà défini dans le système `--hv-*`
- ✅ Documenté dans THEME_CONFORMITY_REPORT
- ✅ Plus proche de l'identité visuelle établie

**Action:**
```css
/* tokens.css - Garder */
--hv-primary: #2E72C9;

/* Ajuster globals.css pour correspondre ou supprimer */
--color-primary-500: #2E72C9;  /* Avant: #4a90d9 */
```

#### 2.3 Créer Script de Migration Automatique

**Fichier:** `scripts/migrate-css-variables.js`

```javascript
#!/usr/bin/env node
/**
 * Script de migration des variables CSS
 * Convertit var(--color-*) et var(--text-*) vers var(--hv-*)
 */

import { readFileSync, writeFileSync } from 'fs';
import { globSync } from 'glob';

const MIGRATION_MAP = {
  // Couleurs
  'var(--color-primary)': 'var(--hv-primary)',
  'var(--color-primary-light)': 'var(--hv-primary-light)',
  'var(--color-primary-50)': 'var(--hv-primary-50)',
  'var(--color-primary-100)': 'var(--hv-primary-100)',
  'var(--color-primary-200)': 'var(--hv-primary-200)',
  'var(--color-primary-300)': 'var(--hv-primary-300)',
  'var(--color-primary-400)': 'var(--hv-primary-400)',
  'var(--color-primary-500)': 'var(--hv-primary)',
  'var(--color-primary-600)': 'var(--hv-primary-600)',
  'var(--color-primary-700)': 'var(--hv-primary-700)',
  
  'var(--color-success)': 'var(--hv-success)',
  'var(--color-success-light)': 'var(--hv-success-light)',
  'var(--color-warning)': 'var(--hv-warning)',
  'var(--color-warning-light)': 'var(--hv-warning-light)',
  'var(--color-error)': 'var(--hv-error)',
  'var(--color-error-light)': 'var(--hv-error-light)',
  'var(--color-info)': 'var(--hv-info)',
  'var(--color-info-light)': 'var(--hv-info-light)',
  
  'var(--color-neutral-50)': 'var(--hv-neutral-50)',
  'var(--color-neutral-100)': 'var(--hv-neutral-100)',
  'var(--color-neutral-200)': 'var(--hv-neutral-200)',
  'var(--color-neutral-300)': 'var(--hv-neutral-300)',
  'var(--color-neutral-400)': 'var(--hv-neutral-400)',
  'var(--color-neutral-500)': 'var(--hv-neutral-500)',
  'var(--color-neutral-600)': 'var(--hv-neutral-600)',
  'var(--color-neutral-700)': 'var(--hv-neutral-700)',
  'var(--color-neutral-800)': 'var(--hv-neutral-800)',
  'var(--color-neutral-900)': 'var(--hv-neutral-900)',
  
  'var(--color-background)': 'var(--hv-background)',
  'var(--color-surface)': 'var(--hv-surface)',
  'var(--color-border)': 'var(--hv-border)',
  
  // Texte
  'var(--color-text)': 'var(--hv-text-primary)',
  'var(--color-text-secondary)': 'var(--hv-text-secondary)',
  'var(--color-text-muted)': 'var(--hv-text-tertiary)',
  'var(--text-primary)': 'var(--hv-text-primary)',
  'var(--text-secondary)': 'var(--hv-text-secondary)',
  
  // Typographie
  'var(--text-xs)': 'var(--hv-font-size-xs)',
  'var(--text-sm)': 'var(--hv-font-size-sm)',
  'var(--text-base)': 'var(--hv-font-size-base)',
  'var(--text-md)': 'var(--hv-font-size-md)',
  'var(--text-lg)': 'var(--hv-font-size-lg)',
  'var(--text-xl)': 'var(--hv-font-size-xl)',
  'var(--text-2xl)': 'var(--hv-font-size-2xl)',
  'var(--text-3xl)': 'var(--hv-font-size-3xl)',
  
  // Espacement
  'var(--spacing-0)': 'var(--hv-space-0)',
  'var(--spacing-1)': 'var(--hv-space-1)',
  'var(--spacing-2)': 'var(--hv-space-2)',
  'var(--spacing-3)': 'var(--hv-space-3)',
  'var(--spacing-4)': 'var(--hv-space-4)',
  'var(--spacing-5)': 'var(--hv-space-5)',
  'var(--spacing-6)': 'var(--hv-space-6)',
  'var(--spacing-8)': 'var(--hv-space-8)',
  'var(--spacing-10)': 'var(--hv-space-10)',
  'var(--spacing-12)': 'var(--hv-space-12)',
  'var(--spacing-16)': 'var(--hv-space-16)',
  
  // Border radius
  'var(--radius-sm)': 'var(--hv-radius-sm)',
  'var(--radius-base)': 'var(--hv-radius-md)',
  'var(--radius-md)': 'var(--hv-radius-md)',
  'var(--radius-lg)': 'var(--hv-radius-lg)',
  'var(--radius-xl)': 'var(--hv-radius-xl)',
  'var(--radius-2xl)': 'var(--hv-radius-2xl)',
  'var(--radius-full)': 'var(--hv-radius-full)',
  
  // Shadows
  'var(--shadow-sm)': 'var(--hv-shadow-sm)',
  'var(--shadow-base)': 'var(--hv-shadow-md)',
  'var(--shadow-md)': 'var(--hv-shadow-md)',
  'var(--shadow-lg)': 'var(--hv-shadow-lg)',
  'var(--shadow-xl)': 'var(--hv-shadow-xl)',
  'var(--shadow-2xl)': 'var(--hv-shadow-2xl)',
  
  // Transitions
  'var(--transition-fast)': 'var(--hv-transition-fast)',
  'var(--transition-base)': 'var(--hv-transition-base)',
  'var(--transition-slow)': 'var(--hv-transition-slow)',
  
  // Fonts
  'var(--font-sans)': 'var(--hv-font-family)',
  'var(--font-mono)': 'var(--hv-font-family-mono)',
};

function migrateFile(filePath) {
  let content = readFileSync(filePath, 'utf-8');
  let modified = false;
  
  for (const [oldVar, newVar] of Object.entries(MIGRATION_MAP)) {
    if (content.includes(oldVar)) {
      content = content.replaceAll(oldVar, newVar);
      modified = true;
    }
  }
  
  if (modified) {
    writeFileSync(filePath, content, 'utf-8');
    console.log(`✅ Migrated: ${filePath}`);
    return 1;
  }
  
  return 0;
}

// Exécution
const files = [
  ...globSync('src/**/*.svelte'),
  ...globSync('src/**/*.css'),
  ...globSync('src/**/*.ts'),
];

console.log(`🔍 Scanning ${files.length} files...`);
let migratedCount = 0;

files.forEach(file => {
  migratedCount += migrateFile(file);
});

console.log(`\n✅ Migration complete: ${migratedCount} files modified`);
```

**Exécution:**
```bash
cd frontend
node scripts/migrate-css-variables.js
```

---

### ÉTAPE 3: Restructuration de l'Architecture

**Durée:** 1-2h  
**Priorité:** 🟡 IMPORTANTE

#### 3.1 Modifier main.ts

**Avant:**
```typescript
import './fonts.css'
import './app.css'
import './app/globals.css'
```

**Après:**
```typescript
import './fonts.css'
import './lib/styles/theme/index.css'  // Charge tokens en premier
import './app.css'
```

#### 3.2 Mettre à Jour tailwind.config.js

**Fichier:** `tailwind.config.js`

Remplacer toutes les références `var(--color-*)` par `var(--hv-*)`

```javascript
export default {
  content: [
    "./index.html",
    "./src/**/*.{svelte,js,ts,jsx,tsx}",
  ],
  darkMode: 'class',
  theme: {
    extend: {
      colors: {
        primary: {
          DEFAULT: 'var(--hv-primary)',
          50: 'var(--hv-primary-50)',
          100: 'var(--hv-primary-100)',
          200: 'var(--hv-primary-200)',
          300: 'var(--hv-primary-300)',
          400: 'var(--hv-primary-400)',
          500: 'var(--hv-primary)',
          600: 'var(--hv-primary-600)',
          700: 'var(--hv-primary-700)',
          light: 'var(--hv-primary-light)',
        },
        success: {
          DEFAULT: 'var(--hv-success)',
          light: 'var(--hv-success-light)',
        },
        warning: {
          DEFAULT: 'var(--hv-warning)',
          light: 'var(--hv-warning-light)',
        },
        error: {
          DEFAULT: 'var(--hv-error)',
          light: 'var(--hv-error-light)',
        },
        info: {
          DEFAULT: 'var(--hv-info)',
          light: 'var(--hv-info-light)',
        },
        neutral: {
          50: 'var(--hv-neutral-50)',
          100: 'var(--hv-neutral-100)',
          200: 'var(--hv-neutral-200)',
          300: 'var(--hv-neutral-300)',
          400: 'var(--hv-neutral-400)',
          500: 'var(--hv-neutral-500)',
          600: 'var(--hv-neutral-600)',
          700: 'var(--hv-neutral-700)',
          800: 'var(--hv-neutral-800)',
          900: 'var(--hv-neutral-900)',
        },
        background: {
          DEFAULT: 'var(--hv-background)',
          alt: 'var(--hv-surface-container-low)',
        },
        text: {
          DEFAULT: 'var(--hv-text-primary)',
          secondary: 'var(--hv-text-secondary)',
          muted: 'var(--hv-text-tertiary)',
        },
        border: {
          DEFAULT: 'var(--hv-border)',
        },
      },
      fontFamily: {
        sans: 'var(--hv-font-family)',
        mono: 'var(--hv-font-family-mono)',
      },
      fontSize: {
        xs: 'var(--hv-font-size-xs)',
        sm: 'var(--hv-font-size-sm)',
        base: 'var(--hv-font-size-base)',
        lg: 'var(--hv-font-size-lg)',
        xl: 'var(--hv-font-size-xl)',
        '2xl': 'var(--hv-font-size-2xl)',
        '3xl': 'var(--hv-font-size-3xl)',
      },
      spacing: {
        '1': 'var(--hv-space-1)',
        '2': 'var(--hv-space-2)',
        '3': 'var(--hv-space-3)',
        '4': 'var(--hv-space-4)',
        '5': 'var(--hv-space-5)',
        '6': 'var(--hv-space-6)',
        '8': 'var(--hv-space-8)',
        '10': 'var(--hv-space-10)',
        '12': 'var(--hv-space-12)',
        '16': 'var(--hv-space-16)',
      },
      borderRadius: {
        sm: 'var(--hv-radius-sm)',
        DEFAULT: 'var(--hv-radius-md)',
        md: 'var(--hv-radius-md)',
        lg: 'var(--hv-radius-lg)',
        xl: 'var(--hv-radius-xl)',
        full: '9999px',
      },
      boxShadow: {
        sm: 'var(--hv-shadow-sm)',
        DEFAULT: 'var(--hv-shadow-md)',
        md: 'var(--hv-shadow-md)',
        lg: 'var(--hv-shadow-lg)',
        xl: 'var(--hv-shadow-xl)',
        '2xl': 'var(--hv-shadow-2xl)',
      },
      transitionDuration: {
        fast: 'var(--hv-transition-fast)',
        DEFAULT: 'var(--hv-transition-base)',
        slow: 'var(--hv-transition-slow)',
      },
    },
  },
  plugins: [],
}
```

#### 3.3 Mettre à Jour app.css

**Fichier:** `src/app.css`

```css
@tailwind base;
@tailwind components;
@tailwind utilities;

/* Global Styles */
:root {
  --sidebar-width: 260px;
  --sidebar-collapsed-width: 72px;
  --header-height: 56px;
}

* {
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
}

body {
  margin: 0;
  padding: 0;
  font-family: var(--hv-font-family);
  background: white;
  color: var(--hv-text-primary);
}

/* Dark mode */
.dark {
  color-scheme: dark;
}

.dark body {
  background: #0f172a;
  color: #e2e8f0;
}

/* Custom scrollbar */
::-webkit-scrollbar {
  width: 8px;
  height: 8px;
}

::-webkit-scrollbar-track {
  background: #f1f3f5;
}

::-webkit-scrollbar-thumb {
  background: #cbd5e1;
  border-radius: var(--hv-radius-md);
}

::-webkit-scrollbar-thumb:hover {
  background: #94a3b8;
}

.dark ::-webkit-scrollbar-track {
  background: #1e293b;
}

.dark ::-webkit-scrollbar-thumb {
  background: #475569;
}

.dark ::-webkit-scrollbar-thumb:hover {
  background: #64748b;
}
```

#### 3.4 Supprimer Fichiers Obsolètes

**À supprimer:**
```bash
# Option 1: Supprimer complètement
rm frontend/src/app/globals.css
rm frontend/src/styles/cove-tailwind.js

# Option 2: Archiver (recommandé pour rollback)
mkdir -p frontend/.archive/2026-07-05
mv frontend/src/app/globals.css frontend/.archive/2026-07-05/
mv frontend/src/styles/cove-tailwind.js frontend/.archive/2026-07-05/
```

**Raisons:**
- `globals.css` → Fusionné dans `tokens.css`
- `cove-tailwind.js` → Non utilisé (0 occurrences)

---

### ÉTAPE 4: Validation et Tests

**Durée:** 2-3h  
**Priorité:** 🔴 CRITIQUE

#### 4.1 Tests de Build

```bash
cd frontend

# Clean install
rm -rf node_modules package-lock.json
npm install

# Test build
npm run build

# Vérifier aucune erreur CSS
# Vérifier aucune variable non définie
# Vérifier bundle size (devrait être réduit)
```

**Critères de succès:**
- ✅ Build sans erreurs
- ✅ Build sans warnings CSS non définis
- ✅ Bundle CSS réduit (~15%)

#### 4.2 Tests Visuels

**Routes à tester:**
1. `/login` - Page de connexion
2. `/desktop` - Bureau principal
3. `/explorer` - Explorateur de fichiers
4. `/settings` - Paramètres
5. `/raid` - Gestion RAID
6. `/apps` - Centre d'applications
7. `/users` - Gestion utilisateurs

**Checklist visuelle par route:**
- [ ] Couleurs correctes
- [ ] Espacements cohérents
- [ ] Typographie correcte
- [ ] Boutons styles
- [ ] Inputs fonctionnels
- [ ] Cartes affichées
- [ ] Modals correctes
- [ ] Toasts visibles

#### 4.3 Tests Dark Mode

```javascript
// Dans DevTools Console
document.documentElement.classList.add('dark');
// Vérifier tous les composants
document.documentElement.classList.remove('dark');
```

**Vérifier:**
- [ ] Contraste suffisant
- [ ] Couleurs inversées correctement
- [ ] Lisibilité maintenue
- [ ] Ombres visibles

#### 4.4 Tests Responsive

**Breakpoints à tester:**
- 375px (Mobile)
- 768px (Tablet)
- 1024px (Desktop)
- 1920px (Large desktop)

**Vérifier:**
- [ ] Layout adaptatif
- [ ] Sidebar collapse
- [ ] Touch targets (44x44px min)
- [ ] Texte lisible

#### 4.5 Tests d'Accessibilité

```bash
# Installer axe-core
npm install -D @axe-core/cli

# Tester l'accessibilité
npx axe http://localhost:5173 --tags wcag2aa
```

**Vérifier:**
- [ ] Contraste > 4.5:1
- [ ] Focus visible
- [ ] Navigation clavier
- [ ] Screen reader labels

#### 4.6 Tests de Performance

```bash
# Bundle analyzer
npm install -D rollup-plugin-visualizer
```

**Métriques à comparer (avant/après):**
- CSS bundle size
- Initial load time
- Time to interactive
- Lighthouse score

---

### ÉTAPE 5: Documentation

**Durée:** 1-2h  
**Priorité:** 🟡 IMPORTANTE

#### 5.1 Mettre à Jour THEME_QUICK_START.md

**Sections à modifier:**
- Ordre d'import des CSS
- Variables CSS disponibles
- Exemples avec `--hv-*` uniquement
- Suppression références Cove

#### 5.2 Créer Guide de Migration

**Fichier:** `THEME_MIGRATION_GUIDE.md`

Contenu:
- Changements apportés
- Mapping `--color-*` → `--hv-*`
- Exemples avant/après
- Troubleshooting

#### 5.3 Mettre à Jour README.md Principal

Ajouter section:
```markdown
## 🎨 Système de Thème

HomeVault utilise un système de design unifié avec des tokens CSS préfixés `--hv-*`.

- **Documentation complète:** `frontend/THEME_QUICK_START.md`
- **Référence des tokens:** `frontend/src/lib/styles/theme/tokens.css`
- **Guide de migration:** `frontend/THEME_MIGRATION_GUIDE.md`

### Démarrage Rapide

\`\`\`typescript
// Les tokens sont chargés automatiquement
import './lib/styles/theme/index.css';

// Utiliser dans vos composants
.my-component {
  color: var(--hv-text-primary);
  background: var(--hv-surface);
  padding: var(--hv-space-4);
  border-radius: var(--hv-radius-md);
}
\`\`\`
```

#### 5.4 Créer Référence Visuelle

**Fichier:** `frontend/src/routes/ThemeReference.svelte`

Page de démo montrant:
- Tous les tokens de couleurs
- Échelles de spacing
- Échelles de typographie
- Composants stylisés
- Dark mode toggle

---

## 📊 Checklist Finale

### Avant de Commencer

- [ ] Lire THEME_ANALYSIS_REPORT.md
- [ ] Comprendre les changements
- [ ] Créer branche git: `git checkout -b theme/unification`
- [ ] Backup des fichiers: `cp -r frontend frontend.backup`

### Pendant la Migration

- [ ] Enrichir tokens.css avec tokens manquants
- [ ] Décider couleur primaire officielle
- [ ] Créer et exécuter script de migration
- [ ] Modifier main.ts
- [ ] Mettre à jour tailwind.config.js
- [ ] Mettre à jour app.css
- [ ] Archiver fichiers obsolètes

### Tests

- [ ] Build sans erreurs
- [ ] Tests visuels (7 routes)
- [ ] Tests dark mode
- [ ] Tests responsive (4 breakpoints)
- [ ] Tests accessibilité (axe-core)
- [ ] Tests performance (bundle size)

### Documentation

- [ ] Mettre à jour THEME_QUICK_START.md
- [ ] Créer THEME_MIGRATION_GUIDE.md
- [ ] Mettre à jour README.md
- [ ] Créer page ThemeReference.svelte

### Finalisation

- [ ] Commit: `git commit -m "feat(theme): unify design system with --hv-* tokens"`
- [ ] Push: `git push origin theme/unification`
- [ ] Créer PR avec détails
- [ ] Review par équipe
- [ ] Merge vers main

---

## 🚨 Plan de Rollback

Si problème critique détecté:

### Option 1: Rollback Git

```bash
git reset --hard HEAD~1
git push --force
```

### Option 2: Restaurer Backup

```bash
rm -rf frontend
mv frontend.backup frontend
```

### Option 3: Restaurer Fichiers Archivés

```bash
cp frontend/.archive/2026-07-05/globals.css frontend/src/app/
# Restaurer import dans main.ts
```

---

## 📈 Métriques de Succès

### Critères de Réussite

- ✅ 1 système de design tokens unique (`--hv-*`)
- ✅ 0 conflit de variables
- ✅ 0 fichier CSS orphelin
- ✅ 0 variable non définie
- ✅ Build sans warnings
- ✅ Bundle CSS réduit de ~15%
- ✅ Tous les tests visuels passent
- ✅ Score accessibilité maintenu (WCAG AA)

### KPIs

| Métrique | Avant | Objectif Après | Réel Après |
|----------|-------|----------------|------------|
| Systèmes de tokens | 3 | 1 | |
| Conflits de variables | ~50 | 0 | |
| Variables non définies | 3 | 0 | |
| CSS bundle size | XXX KB | -15% | |
| Build warnings | X | 0 | |
| Occurrences `--hv-*` | 1683 | 3000+ | |
| Occurrences `--color-*` | 1373 | 0 | |

---

## 🎯 Prochaines Étapes Après Unification

1. **Thématisation dynamique** - Permettre changement de thème runtime
2. **Thèmes personnalisés** - Permettre utilisateurs de créer leurs thèmes
3. **Mode haute contraste** - Support WCAG AAA
4. **Composants supplémentaires** - Enrichir la bibliothèque
5. **Storybook** - Documentation interactive des composants

---

**Document créé par:** Claude Code Agent (Opus 4.8)  
**Date:** 2026-07-05  
**Prochaine action:** Exécuter ÉTAPE 2
