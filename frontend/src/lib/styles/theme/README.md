# HomeVault Design System

Version: **2.0**  
Date: **2026-07-03**

## 📋 Vue d'ensemble

Le système de design HomeVault est une bibliothèque complète de composants, tokens et utilitaires pour construire l'interface HomeVault NAS. Il est basé sur les spécifications définies dans `theme homevault/`.

## 🎨 Caractéristiques

- **Design Tokens CSS** - Variables CSS pour toutes les couleurs, espacements, typographies
- **Composants Svelte** - Composants réutilisables et accessibles
- **Utilities CSS** - Classes utilitaires pour un développement rapide
- **TypeScript Support** - Types et utilitaires TypeScript
- **Theme Store** - Gestion d'état Svelte pour le thème
- **Dark Mode Ready** - Prêt pour le mode sombre (futur)
- **Accessible** - Conforme WCAG AA

## 📦 Structure

```
frontend/src/lib/
├── styles/
│   └── theme/
│       ├── index.css          # Point d'entrée principal
│       ├── tokens.css         # Design tokens (variables CSS)
│       ├── base.css           # Styles de base et reset
│       ├── components.css     # Styles des composants
│       └── utilities.css      # Classes utilitaires
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
│   └── theme.ts              # Utilitaires TypeScript
└── stores/
    └── theme.ts              # Store Svelte pour le thème
```

## 🚀 Installation

### 1. Importer le thème dans votre app

```typescript
// src/app.css ou main entry
import '$lib/styles/theme/index.css';
```

### 2. Initialiser le store de thème

```svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import { initTheme } from '$lib/stores/theme';

  onMount(() => {
    initTheme();
  });
</script>
```

## 📖 Utilisation

### Design Tokens (Variables CSS)

Utilisez les variables CSS dans vos styles :

```css
.my-component {
  color: var(--hv-text-primary);
  background-color: var(--hv-surface-container-lowest);
  padding: var(--hv-space-4);
  border-radius: var(--hv-radius-lg);
  box-shadow: var(--hv-shadow-md);
}
```

### Composants Svelte

```svelte
<script>
  import { Button, Input, Card, Badge, Modal } from '$lib/components/theme';
  
  let showModal = false;
</script>

<Card>
  <div slot="header">
    <h2>Mon Titre</h2>
    <Badge variant="success">Actif</Badge>
  </div>
  
  <Input
    label="Email"
    type="email"
    placeholder="votre@email.com"
    bind:value={email}
  />
  
  <div slot="footer">
    <Button variant="secondary">Annuler</Button>
    <Button variant="primary" on:click={() => showModal = true}>
      Valider
    </Button>
  </div>
</Card>

<Modal bind:open={showModal} title="Confirmation">
  <p>Êtes-vous sûr ?</p>
  
  <div slot="footer">
    <Button variant="secondary" on:click={() => showModal = false}>
      Non
    </Button>
    <Button variant="primary">Oui</Button>
  </div>
</Modal>
```

### Classes Utilitaires

```html
<div class="hv-flex hv-items-center hv-gap-4 hv-p-6 hv-rounded-lg hv-shadow-md">
  <div class="hv-flex-1">
    <h3 class="hv-text-lg hv-font-semibold hv-text-primary">Titre</h3>
    <p class="hv-text-sm hv-text-secondary">Description</p>
  </div>
  <button class="hv-btn hv-btn-primary">Action</button>
</div>
```

### Utilitaires TypeScript

```typescript
import {
  themeColors,
  spacing,
  fontSize,
  cn,
  formatBytes,
  debounce
} from '$lib/utils/theme';

// Utiliser les couleurs
const primaryColor = themeColors.primary;

// Formater les espacements
const padding = spacing(4); // '16px'

// Concaténer les classes conditionnellement
const classes = cn(
  'hv-btn',
  isActive && 'active',
  isDisabled && 'disabled'
);

// Formater les octets
const size = formatBytes(1024000); // '1 MB'
```

### Theme Store

```svelte
<script>
  import { themeMode, isDarkMode, setThemeMode, toggleThemeMode } from '$lib/stores/theme';
</script>

<div>
  Mode actuel: {$themeMode}
  Mode sombre: {$isDarkMode ? 'Oui' : 'Non'}
  
  <button on:click={toggleThemeMode}>
    Basculer le mode
  </button>
  
  <button on:click={() => setThemeMode('dark')}>
    Mode sombre
  </button>
  
  <button on:click={() => setThemeMode('light')}>
    Mode clair
  </button>
</div>
```

## 🎨 Design Tokens

### Couleurs

- **Primary**: `--hv-primary` (#2E72C9)
- **Secondary**: `--hv-secondary` (#505f76)
- **Success**: `--hv-success` (#10b981)
- **Error**: `--hv-error` (#ba1a1a)
- **Warning**: `--hv-warning` (#f59e0b)

### Espacements

- `--hv-space-1` (4px) à `--hv-space-20` (80px)
- Noms: xs, sm, md, lg, xl, xxl, xxxl

### Typographie

- **Font Family**: Inter
- **Sizes**: micro (10px) à 4xl (30px)
- **Weights**: regular (400), medium (500), semibold (600), bold (700)

### Border Radius

- `--hv-radius-sm` (2px) à `--hv-radius-2xl` (16px)
- `--hv-radius-full` (9999px) pour les cercles

### Shadows

- `--hv-shadow-sm` à `--hv-shadow-2xl`

## 📚 Composants Disponibles

### Button
Bouton avec variantes primary, secondary, danger, ghost

### Input
Champ de formulaire avec label, error, helper

### Badge
Badge avec variantes success, warning, error, info

### Card
Carte avec header, body, footer

### Modal
Modal avec overlay, header, content, footer

### Message
Message d'alerte avec variantes error, success, info, warning

### Toggle
Interrupteur on/off

### Spinner
Loader animé

### Sidebar
Barre latérale avec items

### Topbar
Barre supérieure avec logo et actions

## 🎯 Bonnes Pratiques

1. **Utilisez les design tokens** au lieu de valeurs hardcodées
2. **Préférez les composants** aux styles custom
3. **Utilisez les classes utilitaires** pour les layouts simples
4. **Respectez la hiérarchie typographique**
5. **Maintenez la cohérence** avec le design system
6. **Testez l'accessibilité** avec les lecteurs d'écran

## 🔧 Personnalisation

Pour personnaliser le thème, modifiez les variables CSS dans `tokens.css` ou utilisez le store :

```typescript
import { setPrimaryColor } from '$lib/stores/theme';

setPrimaryColor('#FF5733');
```

## 📱 Responsive

Convention unique de l'application : les requêtes de conteneur Tailwind v4
(`@container` est posé sur le corps de page, les dialogues et le wizard).

```html
<div class="@container grid grid-cols-2 @min-[640px]:grid-cols-3 @min-[900px]:grid-cols-4">
  <!-- Contenu -->
</div>
```

## ♿ Accessibilité

Tous les composants sont conçus avec l'accessibilité en tête :

- Rôles ARIA appropriés
- Support clavier complet
- Labels pour lecteurs d'écran
- Contrastes WCAG AA minimum

## 🐛 Problèmes Connus

Aucun pour le moment.

## 📝 Changelog

### Version 2.0 (2026-07-03)
- Création initiale du design system
- 10+ composants Svelte
- Design tokens complets
- Utilitaires TypeScript
- Theme store Svelte

## 📄 Licence

Propriétaire - HomeVault Project

## 👥 Contributeurs

- HomeVault Team
