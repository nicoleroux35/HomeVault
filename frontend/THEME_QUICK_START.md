# HomeVault Theme System - Quick Start Guide

**Version**: 2.0 | **Date**: 2026-07-03

## 🚀 Quick Start (5 minutes)

### 1. Import the Theme

In your main file or layout:

```typescript
// +layout.svelte or app.css
import '$lib/styles/theme/index.css';
```

### 2. Initialize the Store (Optional)

```svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import { initTheme } from '$lib/stores/theme';

  onMount(() => {
    initTheme();
  });
</script>
```

### 3. Use the Components

```svelte
<script>
  import { Button, Input, Card, Badge } from '$lib/components/theme';
  
  let email = '';
</script>

<Card>
  <div slot="header">
    <h2 class="hv-card-title">Login</h2>
    <Badge variant="success">Active</Badge>
  </div>

  <Input
    label="Email"
    type="email"
    bind:value={email}
    placeholder="your@email.com"
  />

  <div slot="footer">
    <Button variant="primary">Log in</Button>
  </div>
</Card>
```

## 📦 Available Components

```typescript
import {
  Button,      // Buttons with variants
  Input,       // Form fields
  Badge,       // Status badges
  Card,        // Cards with header/footer
  Modal,       // Modals
  Message,     // Alerts/Messages
  Toggle,      // Switches
  Spinner,     // Loaders
  Sidebar,     // Sidebar
  SidebarItem, // Sidebar items
  Topbar,      // Top bar
} from '$lib/components/theme';
```

## 🎨 Utility Classes

```html
<!-- Layout -->
<div class="hv-flex hv-items-center hv-gap-4">
  <div class="hv-flex-1">Flex item</div>
</div>

<!-- Spacing -->
<div class="hv-p-4 hv-mb-6">Padding and margin</div>

<!-- Typography -->
<h1 class="hv-text-3xl hv-font-semibold hv-text-primary">Title</h1>
<p class="hv-text-base hv-text-secondary">Text</p>

<!-- Colors -->
<div class="hv-bg-primary hv-text-white hv-rounded-lg">Content</div>
```

## 💻 CSS Variables

```css
.my-component {
  color: var(--hv-text-primary);
  background: var(--hv-surface-container-lowest);
  padding: var(--hv-space-4);
  border-radius: var(--hv-radius-lg);
  box-shadow: var(--hv-shadow-md);
}
```

## 🛠️ TypeScript Utilities

```typescript
import { themeColors, spacing, cn, formatBytes } from '$lib/utils/theme';

// Colors
const color = themeColors.primary; // '#2E72C9'

// Spacings
const padding = spacing(4); // '16px'

// Conditional classes
const classes = cn('hv-btn', isActive && 'active');

// Formatting
const size = formatBytes(1024000); // '1 MB'
```

## 🎭 Theme Store

```svelte
<script>
  import { isDarkMode, toggleThemeMode } from '$lib/stores/theme';
</script>

<button on:click={toggleThemeMode}>
  {$isDarkMode ? 'Light Mode' : 'Dark Mode'}
</button>
```

## 📖 Complete Examples

### Login Form

```svelte
<script>
  import { Button, Input, Card, Message } from '$lib/components/theme';
  
  let email = '';
  let password = '';
  let error = '';

  function handleSubmit() {
    if (!email || !password) {
      error = 'All fields are required';
      return;
    }
    // Login logic
  }
</script>

<div class="hv-min-h-screen hv-flex hv-items-center hv-justify-center hv-bg-surface">
  <Card class="hv-w-full" style="max-width: 440px;">
    <div slot="header">
      <h2 class="hv-text-2xl hv-font-semibold">Login</h2>
    </div>

    {#if error}
      <Message variant="error">{error}</Message>
    {/if}

    <form on:submit|preventDefault={handleSubmit}>
      <Input
        label="Email"
        type="email"
        bind:value={email}
        placeholder="your@email.com"
        required
      />

      <Input
        label="Password"
        type="password"
        bind:value={password}
        placeholder="••••••••"
        required
      />

      <Button variant="primary" type="submit" fullWidth>
        Log in
      </Button>
    </form>
  </Card>
</div>
```

### Dashboard with Sidebar

```svelte
<script>
  import { Topbar, Sidebar, SidebarItem, Card, Badge } from '$lib/components/theme';
  import { Home, Folder, Settings } from 'lucide-svelte';
  
  let activeSection = 'home';
</script>

<div class="hv-min-h-screen hv-bg-surface">
  <Topbar title="HomeVault">
    <svelte:fragment slot="right">
      <div class="hv-topbar-avatar">JD</div>
    </svelte:fragment>
  </Topbar>

  <div class="hv-flex">
    <Sidebar>
      <div class="hv-sidebar-section">
        <div class="hv-sidebar-section-title">NAVIGATION</div>

        <SidebarItem 
          active={activeSection === 'home'}
          on:click={() => activeSection = 'home'}
        >
          <svelte:fragment slot="icon">
            <Home size={20} />
          </svelte:fragment>
          Home
        </SidebarItem>

        <SidebarItem 
          active={activeSection === 'files'}
          on:click={() => activeSection = 'files'}
          badge="3"
        >
          <svelte:fragment slot="icon">
            <Folder size={20} />
          </svelte:fragment>
          Files
        </SidebarItem>

        <SidebarItem 
          active={activeSection === 'settings'}
          on:click={() => activeSection = 'settings'}
        >
          <svelte:fragment slot="icon">
            <Settings size={20} />
          </svelte:fragment>
          Settings
        </SidebarItem>
      </div>
    </Sidebar>

    <main class="hv-flex-1 hv-p-8">
      <h1 class="hv-text-3xl hv-font-semibold hv-mb-6">Dashboard</h1>
      
      <div class="grid grid-cols-3 hv-gap-6">
        <Card hover>
          <div class="hv-flex hv-items-center hv-justify-between">
            <div>
              <div class="hv-text-sm hv-text-secondary">Storage</div>
              <div class="hv-text-2xl hv-font-semibold">2.4 TB</div>
            </div>
            <Badge variant="success">75%</Badge>
          </div>
        </Card>

        <!-- More cards... -->
      </div>
    </main>
  </div>
</div>
```

## 📚 Complete Documentation

- **Full Guide**: `THEME_SYSTEM.md`
- **Creation Report**: `THEME_CREATION_REPORT.md`
- **Theme README**: `frontend/src/lib/styles/theme/README.md`
- **Demo Page**: `/theme-demo`

## 🎨 Main Design Tokens

```css
/* Colors */
--hv-primary: #2E72C9;
--hv-success: #10b981;
--hv-error: #ba1a1a;
--hv-warning: #f59e0b;

/* Spacings */
--hv-space-2: 8px;
--hv-space-4: 16px;
--hv-space-6: 24px;

/* Typography */
--hv-font-size-base: 14px;
--hv-font-size-lg: 18px;
--hv-font-size-2xl: 24px;

/* Border Radius */
--hv-radius-md: 6px;
--hv-radius-lg: 8px;
--hv-radius-xl: 12px;
```

## 🔥 Tips & Tricks

### 1. `hv-` Prefix
All classes and variables use the `hv-` prefix (HomeVault)

### 2. Combine Classes
```html
<div class="hv-flex hv-items-center hv-gap-4 hv-p-6 hv-rounded-lg hv-shadow-md">
```

### 3. Use TypeScript Utils
```typescript
import { cn } from '$lib/utils/theme';

const buttonClass = cn(
  'hv-btn',
  isPrimary && 'hv-btn-primary',
  isDisabled && 'disabled'
);
```

### 4. Responsive

Components adapt to their container (`@container`):

```html
<div class="@container grid grid-cols-2 @min-[768px]:grid-cols-3 @min-[1024px]:grid-cols-4">
```

## ⚡ Ready to Start!

You are now ready to use the HomeVault theme system. Start with:

1. See the demo: `/theme-demo`
2. Read the docs: `THEME_SYSTEM.md`
3. Create your first page with the components

**Happy developing! 🚀**
