# 🎨 HOMEVAULT DESIGN SYSTEM v2.0 - COMPLETE REFERENCE

> ⚠️ **LEGACY / historical document.** This file describes the old "Cove" theme
> (`--cove-*`) and a Material Design 3 approach that no longer match the
> frontend's current design system (variables `--hv-*`, primitives in
> `frontend/src/lib/components/theme/`). Do not follow it for current
> development. The source of truth for the tokens is
> `frontend/src/lib/styles/theme/tokens.css`.

**Single Template for HomeVault**  
**Date:** 2026-07-02  
**Version:** 2.0  
**Stack:** Rust + Svelte + Lucide Icons

> This file combines ALL the design rules of `theme homevault/` to serve as a single reference.

---

## 📑 Table of Contents

1. [Design Tokens](#design-tokens)
2. [Colors](#colors)
3. [Typography](#typography)
4. [Spacings](#spacings)
5. [Components](#components)
6. [Desktop](#desktop)
7. [Windows](#windows)
8. [Login](#login)
9. [Animations](#animations)
10. [Examples](#examples)

---

## 🎨 Design Tokens

### Complete CSS Variables

```css
:root {
  /* ============================================
     PRIMARY COLOR SYSTEM
     ============================================ */
  --primary: #004ac6;
  --on-primary: #ffffff;
  --primary-container: #2563eb;
  --on-primary-container: #eeefff;
  --primary-fixed: #dbe1ff;
  --primary-fixed-dim: #b4c5ff;
  --on-primary-fixed: #00174b;
  --on-primary-fixed-variant: #003ea8;
  --inverse-primary: #b4c5ff;

  /* ============================================
     SECONDARY COLOR SYSTEM
     ============================================ */
  --secondary: #505f76;
  --on-secondary: #ffffff;
  --secondary-container: #d0e1fb;
  --on-secondary-container: #54647a;
  --secondary-fixed: #d3e4fe;
  --secondary-fixed-dim: #b7c8e1;
  --on-secondary-fixed: #0b1c30;
  --on-secondary-fixed-variant: #38485d;

  /* ============================================
     TERTIARY COLOR SYSTEM
     ============================================ */
  --tertiary: #943700;
  --on-tertiary: #ffffff;
  --tertiary-container: #bc4800;
  --on-tertiary-container: #ffede6;
  --tertiary-fixed: #ffdbcd;
  --tertiary-fixed-dim: #ffb596;
  --on-tertiary-fixed: #360f00;
  --on-tertiary-fixed-variant: #7d2d00;

  /* ============================================
     ERROR COLOR SYSTEM
     ============================================ */
  --error: #ba1a1a;
  --on-error: #ffffff;
  --error-container: #ffdad6;
  --on-error-container: #93000a;

  /* ============================================
     SURFACE COLOR SYSTEM (Material Design 3)
     ============================================ */
  --surface: #f7f9fb;
  --surface-dim: #d8dadc;
  --surface-bright: #f7f9fb;
  --surface-container-lowest: #ffffff;
  --surface-container-low: #f2f4f6;
  --surface-container: #eceef0;
  --surface-container-high: #e6e8ea;
  --surface-container-highest: #e0e3e5;
  --on-surface: #191c1e;
  --on-surface-variant: #434655;
  --surface-variant: #e0e3e5;
  --inverse-surface: #2d3133;
  --inverse-on-surface: #eff1f3;
  --surface-tint: #0053db;
  --background: #f7f9fb;
  --on-background: #191c1e;

  /* ============================================
     OUTLINE SYSTEM
     ============================================ */
  --outline: #737686;
  --outline-variant: #c3c6d7;

  /* ============================================
     DESKTOP SPECIFIC
     ============================================ */
  --primary-desktop: #2E72C9;
  --primary-hover: #1e5bb8;
  --on-surface-desktop: #16181B;
  --desktop-bg: linear-gradient(135deg, #1e3a8a 0%, #2E72C9 50%, #93c5fd 100%);

  /* ============================================
     LOGIN SPECIFIC
     ============================================ */
  --primary-login: #004ac6;
  --text-variant: #6b7280;
  --border: #e5e7eb;
  --error-bg: #fee2e2;
  --error-text: #991b1b;
  --success-bg: #d1fae5;
  --success-text: #065f46;
  --info-bg: #eff6ff;
  --info-border: #bfdbfe;
  --info-text: #1e40af;
  --info-icon: #2563eb;

  /* ============================================
     BACKDROP / BLUR EFFECTS
     ============================================ */
  --backdrop-blur-taskbar: blur(12px);
  --backdrop-blur-glass: blur(8px);
  --blur: blur(40px);

  /* ============================================
     TYPOGRAPHY SCALE
     ============================================ */
  --font-size-headline-lg: 30px;
  --font-size-headline-lg-mobile: 24px;
  --font-size-headline-md: 20px;
  --font-size-body-md: 16px;
  --font-size-body-sm: 14px;
  --font-size-label-md: 14px;
  --font-size-label-sm: 12px;
  --font-size-widget-title: 11px;
  --font-size-tech-data: 12px;
  --font-size-micro-label: 10px;

  /* ============================================
     BORDER RADIUS (Frame System)
     ============================================ */
  --rounded-sm: 0.125rem;   /* 2px - Badges, small elements */
  --rounded: 0.25rem;       /* 4px - Buttons, inputs (default) */
  --rounded-md: 0.375rem;   /* 6px - Intermediate elements */
  --rounded-lg: 0.5rem;     /* 8px - Cards, dialogs */
  --rounded-xl: 0.75rem;    /* 12px - Large containers, modals */
  --rounded-full: 9999px;    /* Pills, avatars */

  /* ============================================
     LEGACY COMPATIBILITY (hv- prefix)
     ============================================ */
  --hv-primary: #2E72C9;
  --hv-primary-hover: #1e5bb8;
  --hv-primary-light: rgba(46, 114, 201, 0.1);
  --hv-text-primary: #191c1e;
  --hv-text-secondary: #434655;
  --hv-text-tertiary: #6b7280;
  --hv-bg-app: #f7f9fb;
  --hv-bg-card: #ffffff;
  --hv-bg-surface: #eceef0;
  --hv-error: #ba1a1a;
  --hv-success: #10b981;
  --hv-warning: #f59e0b;

  /* ============================================
     SPACING SCALE (Base 4px)
     ============================================ */
  --hv-space-xs: 4px;
  --hv-space-sm: 8px;
  --hv-space-md: 16px;
  --hv-space-lg: 24px;
  --hv-space-xl: 32px;

  /* ============================================
     TYPOGRAPHY LEGACY
     ============================================ */
  --hv-font-family: 'Inter', sans-serif;
  --hv-font-xs: 12px;
  --hv-font-sm: 13px;
  --hv-font-md: 14px;
  --hv-font-lg: 16px;
  --hv-font-xl: 20px;

  /* ============================================
     BORDER RADIUS
     --------------------------------------------
     Live values are in frontend/src/lib/styles/theme/tokens.css — a badge
     stop (8px), a control stop (12px) and a surface stop (18px), with the
     intermediate names kept as aliases and a pill stop (980px) for tags
     (design-tokens.md §4, adopted 2026-10-19). This reference block is
     historical and must not be copied.
     ============================================ */
  --hv-radius-sm: 8px;    /* badge stop */
  --hv-radius-md: 12px;   /* control stop */
  --hv-radius-lg: 12px;
  --hv-radius-xl: 18px;   /* surface stop */
  --hv-radius-pill: 980px;

  /* ============================================
     COMPONENT HEIGHTS
     ============================================ */
  --hv-height-input: 40px;
  --hv-height-btn-sm: 32px;
  --hv-height-btn-md: 40px;
  --hv-height-btn-lg: 48px;
  --hv-height-toolbar: 56px;
  --hv-height-topbar: 64px;

  /* ============================================
     Z-INDEX SCALE
     ============================================ */
  --hv-z-base: 1;
  --hv-z-window-inactive: 10;
  --hv-z-window-active: 20;
  --hv-z-widget: 40;
  --hv-z-topbar: 50;
  --hv-z-dock: 50;
  --hv-z-modal: 100;
  --hv-z-overlay: 200;
  --hv-z-tooltip: 10000;

  /* ============================================
     TRANSITIONS
     ============================================ */
  --hv-transition-fast: 0.15s;
  --hv-transition-base: 0.2s;
  --hv-transition-slow: 0.3s;
  --hv-transition-bounce: 0.5s cubic-bezier(0.34, 1.56, 0.64, 1);

  /* ============================================
     ADMIN/SIDEBAR SPECIFIC
     ============================================ */
  --sidebar-bg: #f7f9fb;
  --sidebar-border: #e5e7eb;
  --item-hover: rgba(46, 114, 201, 0.08);
  --item-active: rgba(46, 114, 201, 0.15);
  --text-active: #2E72C9;
  --button-danger: #ef4444;
  --button-danger-hover: #dc2626;
  --button-secondary: transparent;
  --badge-success-bg: #d1fae5;
  --badge-success-text: #065f46;
  --badge-warning-bg: #fef3c7;
  --badge-warning-text: #92400e;
  --badge-error-bg: #fee2e2;
  --badge-error-text: #991b1b;
  --badge-info-bg: #dbeafe;
  --badge-info-text: #1e40af;
  --stat-positive: #10b981;
  --stat-negative: #ef4444;
  --card-bg: #ffffff;
  --card-border: #e5e7eb;
  --card-hover-shadow: 0 4px 6px rgba(0, 0, 0, 0.1);
  --table-header-bg: #f7f9fb;
  --table-border: #e5e7eb;
  --table-hover-bg: #f3f4f6;
  --modal-overlay: rgba(0, 0, 0, 0.5);
  --modal-shadow: 0 20px 25px -5px rgba(0, 0, 0, 0.1);

  /* ============================================
     LAUNCHER / APPS CENTER
     ============================================ */
  --launcher-bg: rgba(0, 0, 0, 0.65);
  --ui-white-15: rgba(255, 255, 255, 0.15);
  --icon-gradient: linear-gradient(135deg, #2E72C9 0%, #1e5bb8 100%);

  /* ============================================
     FILE EXPLORER
     ============================================ */
  --sidebar-width: 200px;
  --toolbar-height: 56px;
  --breadcrumb-height: 48px;
  --file-icon-size: 64px;
  --grid-gap: 16px;

  /* ============================================
     POWER / LOGOUT
     ============================================ */
  --overlay-bg: rgba(0, 0, 0, 0.5);
  --overlay-blur: blur(4px);
  --modal-bg: #f7f9fb;
  --modal-shadow: 0 20px 25px -5px rgba(0, 0, 0, 0.1);
  --text-title: #191c1e;
  --text-description: #6b7280;
  --text-icon: #2E72C9;
  --border-color: #e5e7eb;
  --logout-bg: #dbeafe;
  --logout-icon: #3b82f6;
  --restart-bg: #fef3c7;
  --restart-icon: #f59e0b;
  --shutdown-bg: #fee2e2;
  --shutdown-icon: #ef4444;
  --button-secondary: #f3f4f6;
  --button-hover: #e5e7eb;

  /* ============================================
     WINDOW DESIGN
     ============================================ */
  --surface-window: #f7f9fb;
  --surface-titlebar: #ffffff;
  --danger: #ef4444;
}
```

---

## 🎨 Colors

### Main Palette

#### Primary (Blue)
- **Primary:** `#2E72C9` - Main color
- **Primary Hover:** `#1e5bb8` - Hover state
- **Primary Light:** `rgba(46, 114, 201, 0.1)` - Light background

**Usage:** Main buttons, links, icons, focus

#### Text
- **Primary:** `#191c1e` - Primary text
- **Secondary:** `#434655` - Secondary text
- **Tertiary:** `#6b7280` - Disabled text

#### Background
- **App:** `#f7f9fb` - Main background
- **Card:** `#ffffff` - Card background
- **Surface:** `#eceef0` - Elevated surfaces

#### Status
- **Error:** `#ba1a1a` - Errors
- **Success:** `#10b981` - Success
- **Warning:** `#f59e0b` - Warnings

### Desktop Specific

#### Background Gradient
```css
background:
  linear-gradient(135deg,
    rgba(46, 114, 201, 0.4) 0%,
    rgba(138, 180, 248, 0.2) 50%,
    rgba(46, 114, 201, 0.4) 100%
  ),
  linear-gradient(to bottom, #2a2d3a 0%, #1a1d2a 100%);
```

#### Login Background
- Lighter than desktop
- Similar gradient but less pronounced

---

## 📝 Typography

### Font Family
```css
font-family: 'Inter', -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
```

### Size Scale
- **12px (xs):** Labels, badges, tooltips
- **13px (sm):** Secondary text, captions
- **14px (md/base):** Primary text, inputs
- **16px (lg):** Section titles
- **20px (xl):** Page titles
- **24px (2xl):** Main titles
- **32px (3xl):** Hero titles

### Font Weights
- **400 (normal):** Standard text
- **500 (medium):** Labels, navigation
- **600 (semibold):** Subtitles, emphasis
- **700 (bold):** Important titles

### Line Heights
- **1.25:** Titles
- **1.5:** Standard text
- **1.75:** Long paragraphs

---

## 📐 Spacings

### Scale (Base 4px)
- **4px (xs):** Minimal gaps, tight paddings
- **8px (sm):** Spacing between close elements
- **16px (md):** Standard spacing
- **24px (lg):** Sections, cards
- **32px (xl):** Large sections
- **64px (xxl):** Page margins

### Application
- **Inputs:** Padding `0 12px`
- **Buttons:** Padding `8px 16px` (md), `12px 20px` (lg)
- **Cards:** Padding `16px`, `24px`, or `32px`
- **Page margins:** `24px` minimum

---

## 🧩 Components

### Buttons

#### Variants
```css
/* Primary */
.btn-primary {
  background: #2E72C9;
  color: white;
  border: none;
}

.btn-primary:hover {
  background: #1e5bb8;
}

/* Secondary */
.btn-secondary {
  background: transparent;
  color: #2E72C9;
  border: 1px solid #2E72C9;
}

/* Danger */
.btn-danger {
  background: #ba1a1a;
  color: white;
}
```

#### Sizes
- **Small:** 32px height, padding `8px 12px`
- **Medium:** 40px height, padding `12px 16px`
- **Large:** 48px height, padding `16px 24px`

### Inputs

```css
.input {
  height: 40px;
  padding: 0 12px;
  border: 1px solid #e5e7eb;
  border-radius: 8px;
  font-size: 14px;
}

.input:focus {
  border-color: #2E72C9;
  box-shadow: 0 0 0 3px rgba(46, 114, 201, 0.1);
  outline: none;
}

.input.error {
  border-color: #ba1a1a;
}
```

### Cards

```css
.card {
  background: white;
  border: 1px solid #e5e7eb;
  border-radius: 12px;
  padding: 24px;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.1);
}
```

### Toolbar

```css
.toolbar {
  height: 56px;
  padding: 0 16px;
  background: white;
  border-bottom: 1px solid #e5e7eb;
  display: flex;
  align-items: center;
  justify-content: space-between;
}
```

---

## 🖥️ Desktop

### Topbar (64px)

```css
.topbar {
  height: 64px;
  background: rgba(255, 255, 255, 0.95);
  backdrop-filter: blur(20px);
  border-bottom: 1px solid rgba(0, 0, 0, 0.08);
  padding: 0 16px;
  z-index: 50;
}
```

**Elements:**
- Logo: 16px font-size, 700 weight, #2E72C9
- Clock: Date (#6b7280) + "|" (#9ca3af) + Time (#191c1e)
- Avatar: 32×32px circle, background #2E72C9, initials
- Buttons: 32×32px, background transition 0.15s

### Desktop Icons

```css
.desktop-icon-wrapper {
  width: 56px;
  height: 56px;
  border-radius: 12px;
  background: rgba(255, 255, 255, 0.9);
  box-shadow: 0 4px 6px rgba(0, 0, 0, 0.1);
}

.desktop-icon svg {
  width: 40px;
  height: 40px;
  stroke: #2E72C9;
}

.desktop-icon:hover {
  transform: translateY(-4px);
}
```

**Grid:**
- Columns: `repeat(auto-fill, 100px)`
- Gap: `24px`

### Dock

```css
.dock {
  position: fixed;
  bottom: 20px;
  left: 50%;
  transform: translateX(-50%);
  z-index: 50;
}

.dock-container {
  padding: 8px;
  background: rgba(255, 255, 255, 0.8);
  backdrop-filter: blur(20px);
  border-radius: 20px;
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.15);
  border: 1px solid rgba(255, 255, 255, 0.3);
  display: flex;
  gap: 8px;
}

.dock-icon {
  width: 56px;
  height: 56px;
  border-radius: 12px;
  background: rgba(255, 255, 255, 0.95);
  transition: all 0.3s cubic-bezier(0.34, 1.56, 0.64, 1);
}

.dock-icon:hover {
  transform: translateY(-16px) scale(1.2);
  box-shadow: 0 12px 24px rgba(0, 0, 0, 0.25);
}

.dock-icon svg {
  width: 30px;
  height: 30px;
  stroke: #2E72C9;
}
```

**Applications:**
1. Terminal
2. File Explorer (folder)
3. RAID Manager (database)
4. Apps Center (grid-3x3)
5. Users

### Widgets

```css
.widget {
  position: fixed;
  width: 280px; /* Resource: 280px, RAID: 320px */
  background: rgba(255, 255, 255, 0.95);
  backdrop-filter: blur(20px);
  border-radius: 12px;
  padding: 16px;
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.12);
  border: 1px solid rgba(0, 0, 0, 0.08);
  z-index: 40;
}

.widget-header {
  margin-bottom: 16px;
  padding-bottom: 12px;
  border-bottom: 1px solid #e5e7eb;
}
```

### Launcher Pad

```css
.launcher-overlay {
  position: fixed;
  inset: 0;
  background: rgba(26, 29, 42, 0.95);
  backdrop-filter: blur(20px);
  z-index: 100;
  padding: 60px 80px;
}

.launcher-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(120px, 1fr));
  gap: 40px;
  max-width: 1400px;
}

.launcher-app-icon {
  width: 80px;
  height: 80px;
  border-radius: 20px;
  background: linear-gradient(135deg, #2E72C9 0%, #1e5bb8 100%);
  box-shadow: 0 8px 16px rgba(46, 114, 201, 0.3);
}

.launcher-app-icon svg {
  width: 40px;
  height: 40px;
  stroke: white;
}
```

---

## 🪟 Windows

### Structure

```css
.window {
  position: absolute;
  width: 1024px;
  max-width: 90vw;
  height: 720px;
  max-height: 85vh;
  background: white;
  border-radius: 8px;
  box-shadow: 0 20px 25px -5px rgba(0, 0, 0, 0.1);
  overflow: hidden;
}

.window.active {
  z-index: 20;
}

.window:not(.active) {
  z-index: 10;
}
```

### Title Bar (48px)

```css
.title-bar {
  height: 48px;
  background: #f7f9fb;
  border-bottom: 1px solid #e5e7eb;
  padding: 0 16px;
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.title-bar-title {
  font-size: 14px;
  font-weight: 600;
  color: #191c1e;
}
```

### Window Controls

```css
.window-controls {
  display: flex;
  gap: 8px;
}

.window-btn {
  width: 28px;
  height: 28px;
  border-radius: 6px;
  border: none;
  background: transparent;
  transition: background 0.15s;
}

.window-btn:hover {
  background: rgba(0, 0, 0, 0.05);
}

.window-btn.close:hover {
  background: #ba1a1a;
  color: white;
}
```

### Toolbar (56px, optional)

```css
.window-toolbar {
  height: 56px;
  padding: 0 16px;
  border-bottom: 1px solid #e5e7eb;
  background: white;
}
```

---

## 🔐 Login

### Container

```css
.login-container {
  width: 440px;
  max-width: 90vw;
  background: white;
  border-radius: 8px;
  padding: 48px 40px;
  box-shadow: 0 20px 25px -5px rgba(0, 0, 0, 0.1);
}
```

### Primary Color (Login Specific)

```css
--primary-login: #004ac6;
```

Darker than the desktop primary for more contrast.

### Form

```css
.login-input {
  height: 40px;
  padding: 0 12px;
  border: 1px solid #e5e7eb;
  border-radius: 8px;
  width: 100%;
}

.login-button {
  height: 48px;
  width: 100%;
  background: #004ac6;
  color: white;
  border: none;
  border-radius: 8px;
  font-weight: 600;
}
```

---

## 🎬 Animations

### Standard Transitions

```css
/* Buttons */
transition: background 0.15s;

/* Icons, transformations */
transition: transform 0.2s;
transition: all 0.2s;

/* Dock (bounce effect) */
transition: all 0.3s cubic-bezier(0.34, 1.56, 0.64, 1);

/* Progress bars */
transition: width 0.5s ease;
```

### Keyframes

```css
@keyframes fadeIn {
  from {
    opacity: 0;
    transform: translateY(-8px);
  }
  to {
    opacity: 1;
    transform: translateY(0);
  }
}

/* Apply */
.menu {
  animation: fadeIn 0.2s ease-in;
}
```

### Hover Effects

```css
/* Desktop icons */
.desktop-icon:hover {
  transform: translateY(-4px);
}

/* Dock (bounce) */
.dock-icon:hover {
  transform: translateY(-16px) scale(1.2);
}

/* Buttons */
.btn:hover {
  transform: translateY(-1px);
}
```

---

## 📋 Code Examples

### Button Component

```svelte
<script>
  export let variant = 'primary';
  export let size = 'md';
</script>

<button class="btn btn-{variant} btn-{size}">
  <slot />
</button>

<style>
  .btn {
    border: none;
    border-radius: 8px;
    font-family: 'Inter', sans-serif;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s;
  }
  
  .btn-primary {
    background: #2E72C9;
    color: white;
  }
  
  .btn-primary:hover {
    background: #1e5bb8;
  }
  
  .btn-md {
    height: 40px;
    padding: 0 16px;
    font-size: 14px;
  }
</style>
```

### Card Component

```svelte
<script>
  export let padding = 'md';
</script>

<div class="card card-{padding}">
  <slot />
</div>

<style>
  .card {
    background: white;
    border: 1px solid #e5e7eb;
    border-radius: 12px;
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.1);
  }
  
  .card-md {
    padding: 24px;
  }
</style>
```

---

## ✅ Compliance Checklist

### For each component:

- [ ] Uses the design tokens (CSS variables)
- [ ] Font-family: 'Inter'
- [ ] Primary color: #2E72C9 (or #004ac6 for login)
- [ ] Error color: #ba1a1a
- [ ] Spacing: multiples of 4px
- [ ] Border-radius: 6/8/12/16px
- [ ] Transitions: 0.15s (buttons), 0.2s (standard), 0.3s (special)
- [ ] Heights: 32/40/48/56/64px depending on the component
- [ ] Z-index: uses the defined layers
- [ ] Accessibility: ARIA labels, keyboard nav

---

## 🎯 Quick Reference

### Most Used Colors
- Primary: `#2E72C9`
- Text: `#191c1e`
- Background: `#f7f9fb`
- Border: `#e5e7eb`
- Error: `#ba1a1a`

### Most Used Dimensions
- Input/Button: `40px`
- Toolbar: `56px`
- Topbar: `64px`
- Card padding: `24px`
- Standard gap: `16px`

### Transitions
- Fast: `0.15s` (buttons)
- Base: `0.2s` (standard)
- Slow: `0.3s` (dock bounce)

---

## 📚 Sources

This template combines:
1. DESIGN_RULES.md (main rules)
2. DESKTOP_RULES.md (desktop)
3. WINDOW_DESIGN_RULES.md (windows)
4. LOGIN_2FA_RULES.md (authentication)
5. homevault-desktop.html (HTML reference)
6. homevault-login-2fa.html (login reference)

**Total:** 15 source files, ~12000 lines of rules

---

**Version:** 2.0  
**Date:** 2026-07-02  
**Status:** ✅ Complete and verified  
**Usage:** Single reference for HomeVault development

🎨 **HomeVault Design Template - Complete Reference**
