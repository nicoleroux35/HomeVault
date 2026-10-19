# HomeVault Design System

Version: **2.0**  
Date: **2026-07-03**

## 📋 Overview

The HomeVault design system is a complete library of components, tokens and utilities for building the HomeVault NAS interface. It is based on the specifications defined in `theme homevault/`.

## 🎨 Characteristics

- **CSS Design Tokens** - CSS variables for all colors, spacings, typographies
- **Svelte Components** - Reusable and accessible components
- **CSS Utilities** - Utility classes for rapid development
- **TypeScript Support** - TypeScript types and utilities
- **Theme Store** - Svelte state management for the theme
- **Dark Mode Ready** - Ready for dark mode (future)
- **Accessible** - WCAG AA compliant

## 📦 Structure

```
frontend/src/lib/
├── styles/
│   └── theme/
│       ├── index.css          # Main entry point
│       ├── tokens.css         # Design tokens (CSS variables)
│       ├── base.css           # Base styles and reset
│       ├── components.css     # Component styles
│       └── utilities.css      # Utility classes
├── components/
│   └── theme/
│       ├── Button.svelte
│       ├── Input.svelte
│       ├── Badge.svelte
│       ├── Card.svelte
│       ├── Modal.svelte
│       ├── Message.svelte
│       ├── Toggle.svelte
│       ├── Spinner.svelte
│       ├── Sidebar.svelte
│       ├── SidebarItem.svelte
│       ├── Topbar.svelte
│       └── index.ts
├── utils/
│   └── theme.ts              # TypeScript utilities
└── stores/
    └── theme.ts              # Svelte store for the theme
```

## 🚀 Installation

### 1. Import the theme into your app

```typescript
// src/app.css or main entry
import '$lib/styles/theme/index.css';
```

### 2. Initialize the theme store

```svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import { initTheme } from '$lib/stores/theme';

  onMount(() => {
    initTheme();
  });
</script>
```

## 📖 Usage

### Design Tokens (CSS Variables)

Use the CSS variables in your styles:

```css
.my-component {
  color: var(--hv-text-primary);
  background-color: var(--hv-surface-container-lowest);
  padding: var(--hv-space-4);
  border-radius: var(--hv-radius-lg);
  box-shadow: var(--hv-shadow-md);
}
```

### Svelte Components

```svelte
<script>
  import { Button, Input, Card, Badge, Modal } from '$lib/components/theme';
  
  let showModal = false;
</script>

<Card>
  <div slot="header">
    <h2>My Title</h2>
    <Badge variant="success">Active</Badge>
  </div>
  
  <Input
    label="Email"
    type="email"
    placeholder="your@email.com"
    bind:value={email}
  />
  
  <div slot="footer">
    <Button variant="secondary">Cancel</Button>
    <Button variant="primary" on:click={() => showModal = true}>
      Confirm
    </Button>
  </div>
</Card>

<Modal bind:open={showModal} title="Confirmation">
  <p>Are you sure?</p>
  
  <div slot="footer">
    <Button variant="secondary" on:click={() => showModal = false}>
      No
    </Button>
    <Button variant="primary">Yes</Button>
  </div>
</Modal>
```

### Utility Classes

```html
<div class="hv-flex hv-items-center hv-gap-4 hv-p-6 hv-rounded-lg hv-shadow-md">
  <div class="hv-flex-1">
    <h3 class="hv-text-lg hv-font-semibold hv-text-primary">Title</h3>
    <p class="hv-text-sm hv-text-secondary">Description</p>
  </div>
  <button class="hv-btn hv-btn-primary">Action</button>
</div>
```

### TypeScript Utilities

```typescript
import {
  themeColors,
  spacing,
  fontSize,
  cn,
  formatBytes,
  debounce
} from '$lib/utils/theme';

// Use the colors
const primaryColor = themeColors.primary;

// Format spacings
const padding = spacing(4); // '16px'

// Concatenate classes conditionally
const classes = cn(
  'hv-btn',
  isActive && 'active',
  isDisabled && 'disabled'
);

// Format bytes
const size = formatBytes(1024000); // '1 MB'
```

### Theme Store

```svelte
<script>
  import { themeMode, isDarkMode, setThemeMode, toggleThemeMode } from '$lib/stores/theme';
</script>

<div>
  Current mode: {$themeMode}
  Dark mode: {$isDarkMode ? 'Yes' : 'No'}
  
  <button on:click={toggleThemeMode}>
    Toggle the mode
  </button>
  
  <button on:click={() => setThemeMode('dark')}>
    Dark mode
  </button>
  
  <button on:click={() => setThemeMode('light')}>
    Light mode
  </button>
</div>
```

## 🎨 Design Tokens

### Colors

- **Primary**: `--hv-primary` (#1E62AE — "HomeVault Blue", the product's one accent)
- **Secondary**: `--hv-secondary` (#454F5B)
- **Success**: `--hv-success` (#1E7E34)
- **Error**: `--hv-error` (#C22B21)
- **Warning**: `--hv-warning` (#B45309)

### Spacings

- `--hv-space-1` (4px) to `--hv-space-20` (80px)
- Names: xs, sm, md, lg, xl, xxl, xxxl

### Typography

- **Font Family**: Inter
- **Sizes**: micro/xs (11px — the readability floor, nothing readable renders smaller) to 5xl (62px)
- **Weights**: regular (400), medium (500), semibold (600), bold (700)

### Border Radius

- `--hv-radius-sm` (4px) to `--hv-radius-4xl` (24px)
- `--hv-radius-full` (9999px) for circles

### Shadows

- `--hv-shadow-sm` to `--hv-shadow-2xl`

## 📚 Available Components

### Button
Button with primary, secondary, danger, ghost variants

### Input
Form field with label, error, helper

### Badge
Badge with success, warning, error, info variants

### Card
Card with header, body, footer

### Modal
Modal with overlay, header, content, footer

### Message
Alert message with error, success, info, warning variants

### Toggle
On/off switch

### Spinner
Animated loader

### Sidebar
Sidebar with items

### Topbar
Top bar with logo and actions

## 🎯 Best Practices

1. **Use the design tokens** instead of hardcoded values
2. **Prefer components** over custom styles
3. **Use the utility classes** for simple layouts
4. **Respect the typographic hierarchy**
5. **Stay consistent** with the design system
6. **Test accessibility** with screen readers

## 🔧 Customization

To customize the theme, edit the CSS variables in `tokens.css` or use the store:

```typescript
import { setPrimaryColor } from '$lib/stores/theme';

setPrimaryColor('#FF5733');
```

## 📱 Responsive

Single convention for the application: Tailwind v4 container queries
(`@container` is set on the page body, dialogs and the wizard).

```html
<div class="@container grid grid-cols-2 @min-[640px]:grid-cols-3 @min-[900px]:grid-cols-4">
  <!-- Content -->
</div>
```

## ♿ Accessibility

All components are designed with accessibility in mind:

- Appropriate ARIA roles
- Full keyboard support
- Labels for screen readers
- Minimum WCAG AA contrast

## 🐛 Known Issues

None at the moment.

## 📝 Changelog

### Version 2.0 (2026-07-03)
- Initial creation of the design system
- 10+ Svelte components
- Complete design tokens
- TypeScript utilities
- Svelte theme store

## 📄 License

MIT — see the repository [`LICENSE`](../../../../../LICENSE).
Copyright (c) 2026 Nicolas Le Roux.

## 👥 Contributors

- Nicolas Le Roux
