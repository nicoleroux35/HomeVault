# HomeVault Theme System - Quick Start Guide

**Version**: 2.0 | **Date**: 2026-07-03

## 🚀 Démarrage Rapide (5 minutes)

### 1. Importer le Thème

Dans votre fichier principal ou layout :

```typescript
// +layout.svelte ou app.css
import '$lib/styles/theme/index.css';
```

### 2. Initialiser le Store (Optionnel)

```svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import { initTheme } from '$lib/stores/theme';

  onMount(() => {
    initTheme();
  });
</script>
```

### 3. Utiliser les Composants

```svelte
<script>
  import { Button, Input, Card, Badge } from '$lib/components/theme';
  
  let email = '';
</script>

<Card>
  <div slot="header">
    <h2 class="hv-card-title">Connexion</h2>
    <Badge variant="success">Actif</Badge>
  </div>

  <Input
    label="Email"
    type="email"
    bind:value={email}
    placeholder="votre@email.com"
  />

  <div slot="footer">
    <Button variant="primary">Se connecter</Button>
  </div>
</Card>
```

## 📦 Composants Disponibles

```typescript
import {
  Button,      // Boutons avec variantes
  Input,       // Champs de formulaire
  Badge,       // Badges de statut
  Card,        // Cartes avec header/footer
  Modal,       // Modales
  Message,     // Alertes/Messages
  Toggle,      // Interrupteurs
  Spinner,     // Loaders
  Sidebar,     // Barre latérale
  SidebarItem, // Items de sidebar
  Topbar,      // Barre supérieure
} from '$lib/components/theme';
```

## 🎨 Classes Utilitaires

```html
<!-- Layout -->
<div class="hv-flex hv-items-center hv-gap-4">
  <div class="hv-flex-1">Flex item</div>
</div>

<!-- Espacement -->
<div class="hv-p-4 hv-mb-6">Padding et margin</div>

<!-- Typographie -->
<h1 class="hv-text-3xl hv-font-semibold hv-text-primary">Titre</h1>
<p class="hv-text-base hv-text-secondary">Texte</p>

<!-- Couleurs -->
<div class="hv-bg-primary hv-text-white hv-rounded-lg">Contenu</div>
```

## 💻 Variables CSS

```css
.my-component {
  color: var(--hv-text-primary);
  background: var(--hv-surface-container-lowest);
  padding: var(--hv-space-4);
  border-radius: var(--hv-radius-lg);
  box-shadow: var(--hv-shadow-md);
}
```

## 🛠️ Utilitaires TypeScript

```typescript
import { themeColors, spacing, cn, formatBytes } from '$lib/utils/theme';

// Couleurs
const color = themeColors.primary; // '#2E72C9'

// Espacements
const padding = spacing(4); // '16px'

// Classes conditionnelles
const classes = cn('hv-btn', isActive && 'active');

// Formatage
const size = formatBytes(1024000); // '1 MB'
```

## 🎭 Theme Store

```svelte
<script>
  import { isDarkMode, toggleThemeMode } from '$lib/stores/theme';
</script>

<button on:click={toggleThemeMode}>
  {$isDarkMode ? 'Mode Clair' : 'Mode Sombre'}
</button>
```

## 📖 Exemples Complets

### Formulaire de Connexion

```svelte
<script>
  import { Button, Input, Card, Message } from '$lib/components/theme';
  
  let email = '';
  let password = '';
  let error = '';

  function handleSubmit() {
    if (!email || !password) {
      error = 'Tous les champs sont requis';
      return;
    }
    // Logique de connexion
  }
</script>

<div class="hv-min-h-screen hv-flex hv-items-center hv-justify-center hv-bg-surface">
  <Card class="hv-w-full" style="max-width: 440px;">
    <div slot="header">
      <h2 class="hv-text-2xl hv-font-semibold">Connexion</h2>
    </div>

    {#if error}
      <Message variant="error">{error}</Message>
    {/if}

    <form on:submit|preventDefault={handleSubmit}>
      <Input
        label="Email"
        type="email"
        bind:value={email}
        placeholder="votre@email.com"
        required
      />

      <Input
        label="Mot de passe"
        type="password"
        bind:value={password}
        placeholder="••••••••"
        required
      />

      <Button variant="primary" type="submit" fullWidth>
        Se connecter
      </Button>
    </form>
  </Card>
</div>
```

### Dashboard avec Sidebar

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
          Accueil
        </SidebarItem>

        <SidebarItem 
          active={activeSection === 'files'}
          on:click={() => activeSection = 'files'}
          badge="3"
        >
          <svelte:fragment slot="icon">
            <Folder size={20} />
          </svelte:fragment>
          Fichiers
        </SidebarItem>

        <SidebarItem 
          active={activeSection === 'settings'}
          on:click={() => activeSection = 'settings'}
        >
          <svelte:fragment slot="icon">
            <Settings size={20} />
          </svelte:fragment>
          Paramètres
        </SidebarItem>
      </div>
    </Sidebar>

    <main class="hv-flex-1 hv-p-8">
      <h1 class="hv-text-3xl hv-font-semibold hv-mb-6">Dashboard</h1>
      
      <div class="grid grid-cols-3 hv-gap-6">
        <Card hover>
          <div class="hv-flex hv-items-center hv-justify-between">
            <div>
              <div class="hv-text-sm hv-text-secondary">Stockage</div>
              <div class="hv-text-2xl hv-font-semibold">2.4 TB</div>
            </div>
            <Badge variant="success">75%</Badge>
          </div>
        </Card>

        <!-- Plus de cards... -->
      </div>
    </main>
  </div>
</div>
```

## 📚 Documentation Complète

- **Guide Complet**: `THEME_SYSTEM.md`
- **Rapport de Création**: `THEME_CREATION_REPORT.md`
- **README Thème**: `frontend/src/lib/styles/theme/README.md`
- **Page Démo**: `/theme-demo`

## 🎨 Design Tokens Principaux

```css
/* Couleurs */
--hv-primary: #2E72C9;
--hv-success: #10b981;
--hv-error: #ba1a1a;
--hv-warning: #f59e0b;

/* Espacements */
--hv-space-2: 8px;
--hv-space-4: 16px;
--hv-space-6: 24px;

/* Typographie */
--hv-font-size-base: 14px;
--hv-font-size-lg: 18px;
--hv-font-size-2xl: 24px;

/* Border Radius */
--hv-radius-md: 6px;
--hv-radius-lg: 8px;
--hv-radius-xl: 12px;
```

## 🔥 Tips & Tricks

### 1. Préfixe `hv-`
Toutes les classes et variables utilisent le préfixe `hv-` (HomeVault)

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

Les composants s'adaptent à leur conteneur (`@container`) :

```html
<div class="@container grid grid-cols-2 @min-[768px]:grid-cols-3 @min-[1024px]:grid-cols-4">
```

## ⚡ Prêt à Commencer !

Vous êtes maintenant prêt à utiliser le système de thème HomeVault. Commencez par :

1. Voir la démo : `/theme-demo`
2. Lire la doc : `THEME_SYSTEM.md`
3. Créer votre première page avec les composants

**Bon développement ! 🚀**
