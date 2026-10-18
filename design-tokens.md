[200~# Design Token System

This module defines all visual variables for generating Apple-style UI code. Use these CSS custom properties as the single source of truth for colors, spacing, typography weights, border radii, shadows, gradients, and responsive breakpoints.

> **Usage:** Copy the `:root` blocks below into your generated `<style>` tag. Reference tokens via `var(--token-name)` throughout your CSS. Never hard-code raw values — always use the token.

---

## 1. Colors

Apple's palette is deliberately restrained. Backgrounds alternate between pure white, soft gray, and deep dark to create visual rhythm. Text and accent colors are kept to a tight set.

```css
:root {
  /* Background colors */
  --apple-bg-white: #FFFFFF;
  --apple-bg-dark: #1D1D1F;
  --apple-bg-light-gray: #F5F5F7;
  --apple-bg-elevated: #FBFBFD;

  /* Text colors */
  --apple-text-primary: #1D1D1F;
  --apple-text-secondary: #6E6E73;
  --apple-text-tertiary: #86868B;
  --apple-text-on-dark: #F5F5F7;
  --apple-text-white: #FFFFFF;

  /* Accent / interactive colors */
  --apple-link-blue: #0066CC;
  --apple-link-blue-hover: #0077ED;
  --apple-accent-green: #2D8C3C;
  --apple-accent-orange: #E85D04;
  --apple-accent-red: #E30000;
}
```

**When to use:**
- `--apple-bg-white` for primary content sections.
- `--apple-bg-dark` for hero sections or dramatic contrast blocks.
- `--apple-bg-light-gray` for alternating sections to break visual monotony.
- `--apple-link-blue` for links and call-to-action text only — use sparingly.

---

## 2. Spacing

Generous whitespace is a hallmark of Apple's design. Section gaps are large (80–120 px) to let content breathe. Component-level gaps step down progressively.

```css
:root {
  /* Section-level spacing (between major page blocks) */
  --apple-section-gap: 100px;           /* default; range 80-120px */
  --apple-section-gap-sm: 80px;
  --apple-section-gap-lg: 120px;

  /* Component-level spacing (within sections) */
  --apple-component-gap-sm: 16px;
  --apple-component-gap-md: 32px;
  --apple-component-gap-lg: 48px;

  /* Card / container internal padding */
  --apple-card-padding: 32px;            /* default; range 24-40px */
  --apple-card-padding-sm: 24px;
  --apple-card-padding-lg: 40px;

  /* Content area max-width */
  --apple-content-max-width: 980px;
  --apple-content-max-width-lg: 1200px;
}
```

**When to use:**
- `--apple-section-gap` between top-level `<section>` elements.
- `--apple-component-gap-*` for gaps between headings, paragraphs, and buttons within a section.
- `--apple-card-padding` for internal padding of card and tile components.

---

## 3. Font Weights

Apple uses semibold-to-bold headlines paired with regular-to-medium body text. This contrast creates clear typographic hierarchy without relying on size alone.

```css
:root {
  /* Title / heading weights */
  --apple-weight-title: 600;             /* Semibold — default for headings */
  --apple-weight-title-bold: 700;        /* Bold — for hero headlines */
  --apple-weight-title-heavy: 800;       /* Heavy — for CJK section headings */
  --apple-weight-title-black: 900;       /* Black — for CJK hero headlines */

  /* Body / paragraph weights */
  --apple-weight-body: 400;              /* Regular — default for body text */
  --apple-weight-body-medium: 500;       /* Medium — for emphasized body text */
}
```

**When to use:**
- `--apple-weight-title` (600) for English section headings.
- `--apple-weight-title-bold` (700) for English hero headlines.
- `--apple-weight-title-heavy` (800) for CJK section headings (zh-CN, zh-TW, ja, ko).
- `--apple-weight-title-black` (900) for CJK hero headlines — this creates the bold, professional, high-energy feel seen on Apple's Chinese site.
- `--apple-weight-body` (400) for paragraph text.
- `--apple-weight-body-medium` (500) for captions, labels, or lightly emphasized text.

---

## 4. Border Radius

Apple favors smooth, generous rounding on cards and fully rounded pill shapes on buttons and tags.

```css
:root {
  /* Card / container radii */
  --apple-radius-card: 18px;             /* default; range 12-20px */
  --apple-radius-card-sm: 12px;
  --apple-radius-card-lg: 20px;

  /* Button / pill radii */
  --apple-radius-button: 980px;          /* fully rounded pill shape */

  /* Small element radii */
  --apple-radius-badge: 8px;
}
```

**When to use:**
- `--apple-radius-card` for product tiles, feature cards, and modal containers.
- `--apple-radius-button` for all buttons and tag elements to achieve the pill shape.

---

## 5. Shadows

Apple uses subtle, multi-layer shadows to create depth without heaviness. The hover state elevates the shadow to signal interactivity.

```css
:root {
  /* Card resting shadow — subtle depth */
  --apple-shadow-card: 0 2px 8px rgba(0, 0, 0, 0.04),
                       0 8px 24px rgba(0, 0, 0, 0.08);

  /* Card hover shadow — elevated depth */
  --apple-shadow-hover: 0 4px 12px rgba(0, 0, 0, 0.06),
                        0 12px 32px rgba(0, 0, 0, 0.12);

  /* Small element shadow */
  --apple-shadow-sm: 0 1px 4px rgba(0, 0, 0, 0.04),
                     0 4px 12px rgba(0, 0, 0, 0.06);

  /* Modal / overlay shadow */
  --apple-shadow-modal: 0 8px 20px rgba(0, 0, 0, 0.08),
                        0 20px 60px rgba(0, 0, 0, 0.16);
}
```

**When to use:**
- `--apple-shadow-card` as the default shadow for cards and tiles.
- `--apple-shadow-hover` on `:hover` or `:focus` states to provide lift feedback.
- `--apple-shadow-modal` for overlays, modals, and dropdown menus.

---

## 6. Gradients

Apple frequently uses text gradients for dramatic headlines and background gradients for hero sections. These create visual energy while staying on-brand.

```css
:root {
  /* Text gradient — purple to pink (common on Apple product pages) */
  --apple-gradient-text-purple: linear-gradient(90deg, #7B2FBE, #E040A0);

  /* Text gradient — blue to cyan */
  --apple-gradient-text-blue: linear-gradient(90deg, #2997FF, #5AC8FA);

  /* Text gradient — warm orange to red */
  --apple-gradient-text-warm: linear-gradient(90deg, #E8590C, #D63384);

  /* Text gradient — green to teal */
  --apple-gradient-text-green: linear-gradient(90deg, #30D158, #00C7BE);

  /* Background gradient — subtle light */
  --apple-gradient-bg-light: linear-gradient(180deg, #FBFBFD 0%, #F5F5F7 100%);

  /* Background gradient — dark hero */
  --apple-gradient-bg-dark: linear-gradient(180deg, #1D1D1F 0%, #000000 100%);
}
```

**Applying a text gradient:**

```css
.gradient-headline {
  background: var(--apple-gradient-text-purple);
  -webkit-background-clip: text;
  -webkit-text-fill-color: transparent;
  background-clip: text;
}
```

**When to use:**
- Text gradients for hero headlines or key marketing phrases — one per section at most.
- Background gradients for subtle section transitions or dark hero blocks.

---

## 7. Letter Spacing

Apple fine-tunes letter spacing by language. Chinese text is tracked slightly outward for readability; English display text is tracked tightly for a polished, editorial feel.

```css
:root {
  /* Chinese title letter-spacing */
  --apple-tracking-zh-title: 0.04em;

  /* English title letter-spacing range */
  --apple-tracking-en-title: -0.015em;   /* typical value within -0.02em to 0.01em */
  --apple-tracking-en-title-tight: -0.02em;
  --apple-tracking-en-title-loose: 0.01em;

  /* Body text — generally normal tracking */
  --apple-tracking-body: 0em;
}
```

**When to use:**
- Apply `--apple-tracking-zh-title` to Chinese headings for improved legibility.
- Apply `--apple-tracking-en-title` to English display headings for a tighter, more refined look.
- Body text typically uses default tracking (`0em`).

---

## 8. Line Height

Titles are set with tight line heights to keep multi-line headlines compact. Body text uses more generous line heights for comfortable reading.

```css
:root {
  /* Title / heading line-height */
  --apple-leading-title: 1.1;            /* range 1.05-1.15 */
  --apple-leading-title-tight: 1.05;
  --apple-leading-title-loose: 1.15;

  /* Body / paragraph line-height */
  --apple-leading-body: 1.53;            /* range 1.5-1.58 */
  --apple-leading-body-tight: 1.5;
  --apple-leading-body-loose: 1.58;
}
```

**When to use:**
- `--apple-leading-title` for all headings (h1–h3).
- `--apple-leading-body` for paragraph text and descriptions.

---

## 9. Responsive Breakpoints

Apple's responsive grid uses three breakpoints that match apple.com's layout behavior. Design mobile-first, then layer on tablet and desktop overrides.

```css
:root {
  --apple-breakpoint-sm: 734px;
  --apple-breakpoint-md: 1068px;
  --apple-breakpoint-lg: 1440px;
}
```

**Media query usage:**

```css
/* Mobile-first base styles (< 734px) */
.section { padding: 60px 20px; }

/* Tablet and up */
@media (min-width: 734px) {
  .section { padding: 80px 40px; }
}

/* Desktop and up */
@media (min-width: 1068px) {
  .section {
    padding: var(--apple-section-gap) 0;
    max-width: var(--apple-content-max-width);
    margin: 0 auto;
  }
}

/* Large desktop */
@media (min-width: 1440px) {
  .section {
    max-width: var(--apple-content-max-width-lg);
  }
}
```

**When to use:**
- `734px` — transition from single-column mobile to multi-column tablet layout.
- `1068px` — transition to full desktop layout with centered content area.
- `1440px` — widen content area for large displays.

---

## Quick Reference

| Category | Token prefix | Key values |
|----------|-------------|------------|
| Colors | `--apple-bg-*`, `--apple-text-*`, `--apple-link-*` | #FFFFFF, #1D1D1F, #F5F5F7, #0066CC |
| Spacing | `--apple-section-gap-*`, `--apple-component-gap-*`, `--apple-card-padding-*` | 80–120px, 16–48px, 24–40px |
| Font weight | `--apple-weight-*` | 600–700 (title), 400–500 (body) |
| Border radius | `--apple-radius-*` | 12–20px (card), 980px (button) |
| Shadows | `--apple-shadow-*` | Multi-layer rgba |
| Gradients | `--apple-gradient-*` | Purple-pink, blue-cyan, warm, green-teal |
| Letter spacing | `--apple-tracking-*` | 0.04em (zh), -0.02–0.01em (en) |
| Line height | `--apple-leading-*` | 1.05–1.15 (title), 1.5–1.58 (body) |
| Breakpoints | `--apple-breakpoint-*` | 734px, 1068px, 1440px |
