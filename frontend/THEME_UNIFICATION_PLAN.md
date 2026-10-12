# HomeVault Theme Unification Plan
**Date:** 2026-07-05  
**Version:** 1.0  
**Objective:** Create a single, unified and harmonized theme

---

## 🎯 Main Objective

Unify the 3 current design token systems into a single coherent system using the `--hv-*` prefix, eliminate duplications, and standardize the CSS architecture.

---

## 📋 Detailed Action Plan

### STEP 1: Audit and Preparation (✅ COMPLETED)

**Duration:** 1-2h

- [x] Complete analysis of the current system
- [x] Identification of the conflicts and duplications
- [x] Counting of the occurrences (`--hv-*`: 1683, `--color-*`: 1373)
- [x] Documentation of the problems (THEME_ANALYSIS_REPORT.md)
- [x] Definition of the migration strategy

**Result:** Complete analysis report created

---

### STEP 2: Consolidation of the Design Tokens

**Duration:** 2-3h  
**Priority:** 🔴 CRITICAL

#### 2.1 Enrich tokens.css with the Missing Tokens

**Action:** Merge `globals.css` into `tokens.css`

**Tokens to add to tokens.css:**

```css
/* Missing variables referenced in app.css */
--hv-font-family: 'Inter', -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
--hv-radius-default: 4px;

/* Primary colors with full scale (from globals.css) */
--hv-primary-50: #eff6ff;
--hv-primary-100: #dbeafe;
--hv-primary-200: #bfdbfe;
--hv-primary-300: #93c5fd;
--hv-primary-400: #60a5fa;
--hv-primary-500: #4a90d9;  /* Align with --hv-primary or choose */
--hv-primary-600: #3b7ac7;
--hv-primary-700: #2c5fa5;
--hv-primary-800: #1e4485;
--hv-primary-900: #1e3a8a;

/* Neutrals with full scale */
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

/* Semantics from globals.css */
--hv-success-light: #dcfce7;
--hv-warning-light: #fef3c7;
--hv-error-light: #fee2e2;
--hv-info-light: #dbeafe;

/* Contextual */
--hv-background: #ffffff;
--hv-background-alt: var(--hv-surface-container-low);
--hv-border: var(--hv-outline-variant);
```

#### 2.2 Decide the Official Primary Color

**Current conflict:**
- `globals.css`: `#4a90d9` (light blue)
- `tokens.css`: `#2E72C9` (medium blue)
- `cove-tailwind.js`: `#2196f3` (Material blue)

**Recommendation:** Keep `#2E72C9` from `tokens.css`
- ✅ Already defined in the `--hv-*` system
- ✅ Documented in THEME_CONFORMITY_REPORT
- ✅ Closer to the established visual identity

**Action:**
```css
/* tokens.css - Keep */
--hv-primary: #2E72C9;

/* Adjust globals.css to match or remove */
--color-primary-500: #2E72C9;  /* Before: #4a90d9 */
```

#### 2.3 Create the Automatic Migration Script

**File:** `scripts/migrate-css-variables.js`

```javascript
#!/usr/bin/env node
/**
 * CSS variables migration script
 * Converts var(--color-*) and var(--text-*) to var(--hv-*)
 */

import { readFileSync, writeFileSync } from 'fs';
import { globSync } from 'glob';

const MIGRATION_MAP = {
  // Colors
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
  
  // Text
  'var(--color-text)': 'var(--hv-text-primary)',
  'var(--color-text-secondary)': 'var(--hv-text-secondary)',
  'var(--color-text-muted)': 'var(--hv-text-tertiary)',
  'var(--text-primary)': 'var(--hv-text-primary)',
  'var(--text-secondary)': 'var(--hv-text-secondary)',
  
  // Typography
  'var(--text-xs)': 'var(--hv-font-size-xs)',
  'var(--text-sm)': 'var(--hv-font-size-sm)',
  'var(--text-base)': 'var(--hv-font-size-base)',
  'var(--text-md)': 'var(--hv-font-size-md)',
  'var(--text-lg)': 'var(--hv-font-size-lg)',
  'var(--text-xl)': 'var(--hv-font-size-xl)',
  'var(--text-2xl)': 'var(--hv-font-size-2xl)',
  'var(--text-3xl)': 'var(--hv-font-size-3xl)',
  
  // Spacing
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

// Execution
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

**Execution:**
```bash
cd frontend
node scripts/migrate-css-variables.js
```

---

### STEP 3: Restructuring of the Architecture

**Duration:** 1-2h  
**Priority:** 🟡 IMPORTANT

#### 3.1 Modify main.ts

**Before:**
```typescript
import './fonts.css'
import './app.css'
import './app/globals.css'
```

**After:**
```typescript
import './fonts.css'
import './lib/styles/theme/index.css'  // Loads tokens first
import './app.css'
```

#### 3.2 Update tailwind.config.js

**File:** `tailwind.config.js`

Replace all the `var(--color-*)` references with `var(--hv-*)`

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

#### 3.3 Update app.css

**File:** `src/app.css`

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

#### 3.4 Remove Obsolete Files

**To remove:**
```bash
# Option 1: Remove completely
rm frontend/src/app/globals.css
rm frontend/src/styles/cove-tailwind.js

# Option 2: Archive (recommended for rollback)
mkdir -p frontend/.archive/2026-07-05
mv frontend/src/app/globals.css frontend/.archive/2026-07-05/
mv frontend/src/styles/cove-tailwind.js frontend/.archive/2026-07-05/
```

**Reasons:**
- `globals.css` → Merged into `tokens.css`
- `cove-tailwind.js` → Unused (0 occurrences)

---

### STEP 4: Validation and Tests

**Duration:** 2-3h  
**Priority:** 🔴 CRITICAL

#### 4.1 Build Tests

```bash
cd frontend

# Clean install
rm -rf node_modules package-lock.json
npm install

# Test build
npm run build

# Verify no CSS errors
# Verify no undefined variables
# Verify bundle size (should be reduced)
```

**Success criteria:**
- ✅ Build without errors
- ✅ Build without undefined CSS warnings
- ✅ Reduced CSS bundle (~15%)

#### 4.2 Visual Tests

**Routes to test:**
1. `/login` - Login page
2. `/desktop` - Main desktop
3. `/explorer` - File explorer
4. `/settings` - Settings
5. `/raid` - RAID management
6. `/apps` - Application center
7. `/users` - User management

**Visual checklist per route:**
- [ ] Correct colors
- [ ] Consistent spacings
- [ ] Correct typography
- [ ] Styled buttons
- [ ] Functional inputs
- [ ] Displayed cards
- [ ] Correct modals
- [ ] Visible toasts

#### 4.3 Dark Mode Tests

```javascript
// In DevTools Console
document.documentElement.classList.add('dark');
// Verify all components
document.documentElement.classList.remove('dark');
```

**Verify:**
- [ ] Sufficient contrast
- [ ] Colors correctly inverted
- [ ] Readability maintained
- [ ] Visible shadows

#### 4.4 Responsive Tests

**Breakpoints to test:**
- 375px (Mobile)
- 768px (Tablet)
- 1024px (Desktop)
- 1920px (Large desktop)

**Verify:**
- [ ] Adaptive layout
- [ ] Sidebar collapse
- [ ] Touch targets (44x44px min)
- [ ] Readable text

#### 4.5 Accessibility Tests

```bash
# Install axe-core
npm install -D @axe-core/cli

# Test accessibility
npx axe http://localhost:5173 --tags wcag2aa
```

**Verify:**
- [ ] Contrast > 4.5:1
- [ ] Focus visible
- [ ] Keyboard navigation
- [ ] Screen reader labels

#### 4.6 Performance Tests

```bash
# Bundle analyzer
npm install -D rollup-plugin-visualizer
```

**Metrics to compare (before/after):**
- CSS bundle size
- Initial load time
- Time to interactive
- Lighthouse score

---

### STEP 5: Documentation

**Duration:** 1-2h  
**Priority:** 🟡 IMPORTANT

#### 5.1 Update THEME_QUICK_START.md

**Sections to modify:**
- CSS import order
- Available CSS variables
- Examples with `--hv-*` only
- Removal of Cove references

#### 5.2 Create the Migration Guide

**File:** `THEME_MIGRATION_GUIDE.md`

Content:
- Changes made
- Mapping `--color-*` → `--hv-*`
- Before/after examples
- Troubleshooting

#### 5.3 Update the Main README.md

Add a section:
```markdown
## 🎨 Theme System

HomeVault uses a unified design system with CSS tokens prefixed `--hv-*`.

- **Complete documentation:** `frontend/THEME_QUICK_START.md`
- **Token reference:** `frontend/src/lib/styles/theme/tokens.css`
- **Migration guide:** `frontend/THEME_MIGRATION_GUIDE.md`

### Quick Start

\`\`\`typescript
// The tokens are loaded automatically
import './lib/styles/theme/index.css';

// Use in your components
.my-component {
  color: var(--hv-text-primary);
  background: var(--hv-surface);
  padding: var(--hv-space-4);
  border-radius: var(--hv-radius-md);
}
\`\`\`
```

#### 5.4 Create the Visual Reference

**File:** `frontend/src/routes/ThemeReference.svelte`

Demo page showing:
- All the color tokens
- Spacing scales
- Typography scales
- Styled components
- Dark mode toggle

---

## 📊 Final Checklist

### Before Starting

- [ ] Read THEME_ANALYSIS_REPORT.md
- [ ] Understand the changes
- [ ] Create a git branch: `git checkout -b theme/unification`
- [ ] Backup of the files: `cp -r frontend frontend.backup`

### During the Migration

- [ ] Enrich tokens.css with the missing tokens
- [ ] Decide the official primary color
- [ ] Create and run the migration script
- [ ] Modify main.ts
- [ ] Update tailwind.config.js
- [ ] Update app.css
- [ ] Archive obsolete files

### Tests

- [ ] Build without errors
- [ ] Visual tests (7 routes)
- [ ] Dark mode tests
- [ ] Responsive tests (4 breakpoints)
- [ ] Accessibility tests (axe-core)
- [ ] Performance tests (bundle size)

### Documentation

- [ ] Update THEME_QUICK_START.md
- [ ] Create THEME_MIGRATION_GUIDE.md
- [ ] Update README.md
- [ ] Create the ThemeReference.svelte page

### Finalization

- [ ] Commit: `git commit -m "feat(theme): unify design system with --hv-* tokens"`
- [ ] Push: `git push origin theme/unification`
- [ ] Create a PR with details
- [ ] Review by the team
- [ ] Merge to main

---

## 🚨 Rollback Plan

If a critical problem is detected:

### Option 1: Git Rollback

```bash
git reset --hard HEAD~1
git push --force
```

### Option 2: Restore Backup

```bash
rm -rf frontend
mv frontend.backup frontend
```

### Option 3: Restore Archived Files

```bash
cp frontend/.archive/2026-07-05/globals.css frontend/src/app/
# Restore the import in main.ts
```

---

## 📈 Success Metrics

### Success Criteria

- ✅ 1 single design token system (`--hv-*`)
- ✅ 0 variable conflicts
- ✅ 0 orphan CSS file
- ✅ 0 undefined variable
- ✅ Build without warnings
- ✅ CSS bundle reduced by ~15%
- ✅ All visual tests pass
- ✅ Accessibility score maintained (WCAG AA)

### KPIs

| Metric | Before | Target After | Actual After |
|----------|-------|----------------|------------|
| Token systems | 3 | 1 | |
| Variable conflicts | ~50 | 0 | |
| Undefined variables | 3 | 0 | |
| CSS bundle size | XXX KB | -15% | |
| Build warnings | X | 0 | |
| `--hv-*` occurrences | 1683 | 3000+ | |
| `--color-*` occurrences | 1373 | 0 | |

---

## 🎯 Next Steps After Unification

1. **Dynamic theming** - Allow runtime theme changes
2. **Custom themes** - Let users create their own themes
3. **High-contrast mode** - WCAG AAA support
4. **Additional components** - Enrich the library
5. **Storybook** - Interactive component documentation

---

**Document created by:** [CC] Agent (Opus 4.8)  
**Date:** 2026-07-05  
**Next action:** Execute STEP 2
