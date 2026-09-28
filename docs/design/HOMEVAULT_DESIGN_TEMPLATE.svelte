<!--
  HOMEVAULT DESIGN SYSTEM v2.0 - TEMPLATE DE RÉFÉRENCE COMPLET

  Ce fichier combine TOUTES les règles de design de theme homevault/
  Utiliser comme référence unique pour le développement HomeVault

  Sources:
  - theme homevault/DESIGN_RULES.md
  - theme homevault/DESKTOP_RULES.md
  - theme homevault/WINDOW_DESIGN_RULES.md
  - theme homevault/LOGIN_2FA_RULES.md
  - theme homevault/ADMIN_RULES.md
  - theme homevault/APPS_CENTER_RULES.md
  - theme homevault/POWER_LOGOUT_RULES.md
  - theme homevault/FILE_EXPLORER_RULES.md
  - theme homevault/SETTINGS_RULES.md

  Date: 2026-07-03
  Version: 2.1
  Stack: Rust + Svelte + Lucide Icons
-->

<script lang="ts">
  import { onMount } from 'svelte';
  import { Home, Settings, Folder, Database, Grid3x3, Users, Terminal, Network } from '@lucide/svelte';

  let activeSection = 'colors';

  const sections = [
    { id: 'overview', label: 'Vue d\'ensemble' },
    { id: 'colors', label: 'Couleurs' },
    { id: 'typography', label: 'Typographie' },
    { id: 'spacing', label: 'Espacements' },
    { id: 'components', label: 'Composants' },
    { id: 'desktop', label: 'Desktop' },
    { id: 'windows', label: 'Fenêtres' },
    { id: 'login', label: 'Login' },
    { id: 'animations', label: 'Animations' }
  ];

  // ============================================
  // COMPLETE DESIGN TOKENS - ALL FROM theme homevault/
  // ============================================

  // PRIMARY COLOR SYSTEM
  export const primaryColors = {
    primary: '#2E72C9',
    primaryHover: '#1e5bb8',
    primaryLight: 'rgba(46, 114, 201, 0.1)',
    primaryContainer: '#2563eb',
    primaryFixed: '#dbe1ff',
    primaryFixedDim: '#b4c5ff',
    inversePrimary: '#b4c5ff'
  };

  // SECONDARY COLOR SYSTEM
  export const secondaryColors = {
    secondary: '#505f76',
    secondaryHover: '#445063',
    secondaryContainer: '#d0e1fb',
    secondaryFixed: '#d3e4fe',
    secondaryFixedDim: '#b7c8e1'
  };

  // TERTIARY COLOR SYSTEM
  export const tertiaryColors = {
    tertiary: '#943700',
    tertiaryHover: '#7a2e00',
    tertiaryContainer: '#bc4800',
    tertiaryFixed: '#ffdbcd',
    tertiaryFixedDim: '#ffb596'
  };

  // ERROR COLOR SYSTEM
  export const errorColors = {
    error: '#ba1a1a',
    errorHover: '#9e1515',
    errorContainer: '#ffdad6',
    success: '#10b981',
    warning: '#f59e0b'
  };

  // SURFACE COLOR SYSTEM (Material Design 3)
  export const surfaceColors = {
    surface: '#f7f9fb',
    surfaceDim: '#d8dadc',
    surfaceBright: '#f7f9fb',
    surfaceContainerLowest: '#ffffff',
    surfaceContainerLow: '#f2f4f6',
    surfaceContainer: '#eceef0',
    surfaceContainerHigh: '#e6e8ea',
    surfaceContainerHighest: '#e0e3e5',
    surfaceVariant: '#e0e3e5',
    inverseSurface: '#2d3133',
    inverseOnSurface: '#eff1f3',
    surfaceTint: '#0053db',
    background: '#f7f9fb',
    surfaceWindow: '#f7f9fb',
    surfaceTitlebar: '#ffffff'
  };

  // TEXT COLOR SYSTEM
  export const textColors = {
    textPrimary: '#191c1e',
    textSecondary: '#434655',
    textTertiary: '#6b7280',
    onSurface: '#191c1e',
    onSurfaceVariant: '#434655',
    onSurfaceDesktop: '#16181B',
    onBackground: '#191c1e',
    textVariant: '#6b7280'
  };

  // OUTLINE SYSTEM
  export const outlineColors = {
    outline: '#737686',
    outlineVariant: '#c3c6d7',
    border: '#e5e7eb'
  };

  // DESKTOP SPECIFIC
  export const desktopColors = {
    primaryDesktop: '#2E72C9',
    desktopBg: 'linear-gradient(135deg, #1e3a8a 0%, #2E72C9 50%, #93c5fd 100%)'
  };

  // LOGIN SPECIFIC
  export const loginColors = {
    primaryLogin: '#004ac6',
    errorBg: '#fee2e2',
    errorText: '#991b1b',
    successBg: '#d1fae5',
    successText: '#065f46',
    infoBg: '#eff6ff',
    infoBorder: '#bfdbfe',
    infoText: '#1e40af',
    infoIcon: '#2563eb'
  };

  // ADMIN/SIDEBAR SPECIFIC
  export const adminColors = {
    sidebarBg: '#f7f9fb',
    sidebarBorder: '#e5e7eb',
    itemHover: 'rgba(46, 114, 201, 0.08)',
    itemActive: 'rgba(46, 114, 201, 0.15)',
    textActive: '#2E72C9',
    buttonDanger: '#ef4444',
    buttonDangerHover: '#dc2626',
    buttonSecondary: 'transparent',
    badgeSuccessBg: '#d1fae5',
    badgeSuccessText: '#065f46',
    badgeWarningBg: '#fef3c7',
    badgeWarningText: '#92400e',
    badgeErrorBg: '#fee2e2',
    badgeErrorText: '#991b1b',
    badgeInfoBg: '#dbeafe',
    badgeInfoText: '#1e40af',
    statPositive: '#10b981',
    statNegative: '#ef4444',
    cardBg: '#ffffff',
    cardBorder: '#e5e7eb',
    cardHoverShadow: '0 4px 6px rgba(0, 0, 0, 0.1)',
    tableHeaderBg: '#f7f9fb',
    tableBorder: '#e5e7eb',
    tableHoverBg: '#f3f4f6',
    modalOverlay: 'rgba(0, 0, 0, 0.5)',
    modalShadow: '0 20px 25px -5px rgba(0, 0, 0, 0.1)'
  };

  // LAUNCHER / APPS CENTER
  export const launcherColors = {
    launcherBg: 'rgba(0, 0, 0, 0.65)',
    uiWhite15: 'rgba(255, 255, 255, 0.15)',
    iconGradient: 'linear-gradient(135deg, #2E72C9 0%, #1e5bb8 100%)'
  };

  // POWER / LOGOUT
  export const powerColors = {
    overlayBg: 'rgba(0, 0, 0, 0.5)',
    overlayBlur: 'blur(4px)',
    modalBg: '#f7f9fb',
    textTitle: '#191c1e',
    textDescription: '#6b7280',
    textIcon: '#2E72C9',
    borderColor: '#e5e7eb',
    logoutBg: '#dbeafe',
    logoutIcon: '#3b82f6',
    restartBg: '#fef3c7',
    restartIcon: '#f59e0b',
    shutdownBg: '#fee2e2',
    shutdownIcon: '#ef4444',
    buttonSecondary: '#f3f4f6',
    buttonHover: '#e5e7eb'
  };

  // TYPOGRAPHY SCALE
  export const fontSizes = {
    microLabel: '10px',
    widgetTitle: '11px',
    labelSm: '12px',
    techData: '12px',
    fontXs: '12px',
    fontSm: '13px',
    labelMd: '14px',
    bodySm: '14px',
    fontMd: '14px',
    bodyMd: '16px',
    fontLg: '16px',
    headlineMd: '20px',
    fontXl: '20px',
    headlineLgMobile: '24px',
    headlineLg: '30px'
  };

  // FONT WEIGHTS
  export const fontWeights = {
    normal: 400,
    medium: 500,
    semibold: 600,
    bold: 700
  };

  // LINE HEIGHTS
  export const lineHeights = {
    tight: 1.25,
    normal: 1.5,
    relaxed: 1.75
  };

  // SPACING SCALE (Base 4px)
  export const spacing = {
    xs: '4px',
    sm: '8px',
    md: '16px',
    lg: '24px',
    xl: '32px'
  };

  // BORDER RADIUS (Frame System)
  export const borderRadius = {
    sm: '0.125rem',
    base: '0.25rem',
    md: '0.375rem',
    lg: '0.5rem',
    xl: '0.75rem',
    full: '9999px',
    legacySm: '6px',
    legacyMd: '8px',
    legacyLg: '12px',
    legacyXl: '16px'
  };

  // BACKDROP / BLUR EFFECTS
  export const backdrop = {
    blurTaskbar: 'blur(12px)',
    blurGlass: 'blur(8px)',
    blurStrong: 'blur(40px)',
    blurOverlay: 'blur(4px)'
  };

  // Z-INDEX SCALE
  export const zIndex = {
    base: 1,
    windowInactive: 10,
    windowActive: 20,
    widget: 40,
    topbar: 50,
    dock: 50,
    modal: 100,
    overlay: 200,
    tooltip: 10000
  };

  // TRANSITIONS
  export const transitions = {
    fast: '0.15s',
    base: '0.2s',
    slow: '0.3s',
    bounce: '0.5s cubic-bezier(0.34, 1.56, 0.64, 1)',
    long: '0.8s'
  };

  // COMPONENT HEIGHTS
  export const heights = {
    input: '40px',
    btnSm: '32px',
    btnMd: '40px',
    btnLg: '48px',
    toolbar: '56px',
    topbar: '64px',
    fileIcon: '64px'
  };

  // LAYOUT DIMENSIONS
  export const dimensions = {
    sidebarWidth: '200px',
    toolbarHeight: '56px',
    breadcrumbHeight: '48px',
    gridGap: '16px',
    windowMaxWidth: '1024px',
    windowMaxHeight: '720px',
    loginWidth: '440px'
  };

  // COMBINE ALL COLORS FOR CONVENIENCE
  export const colors = {
    ...primaryColors,
    ...secondaryColors,
    ...tertiaryColors,
    ...errorColors,
    ...surfaceColors,
    ...textColors,
    ...outlineColors,
    ...desktopColors,
    ...loginColors,
    ...adminColors,
    ...launcherColors,
    ...powerColors
  };

  // ============================================
  // CSS VARIABLES STRING (for :root)
  // ============================================
  export const cssVariables = `
    /* PRIMARY */
    --primary: #004ac6;
    --on-primary: #ffffff;
    --primary-container: #2563eb;
    --on-primary-container: #eeefff;
    --primary-fixed: #dbe1ff;
    --primary-fixed-dim: #b4c5ff;
    --on-primary-fixed: #00174b;
    --on-primary-fixed-variant: #003ea8;
    --inverse-primary: #b4c5ff;

    /* SECONDARY */
    --secondary: #505f76;
    --on-secondary: #ffffff;
    --secondary-container: #d0e1fb;
    --on-secondary-container: #54647a;
    --secondary-fixed: #d3e4fe;
    --secondary-fixed-dim: #b7c8e1;
    --on-secondary-fixed: #0b1c30;
    --on-secondary-fixed-variant: #38485d;

    /* TERTIARY */
    --tertiary: #943700;
    --on-tertiary: #ffffff;
    --tertiary-container: #bc4800;
    --on-tertiary-container: #ffede6;
    --tertiary-fixed: #ffdbcd;
    --tertiary-fixed-dim: #ffb596;
    --on-tertiary-fixed: #360f00;
    --on-tertiary-fixed-variant: #7d2d00;

    /* ERROR */
    --error: #ba1a1a;
    --on-error: #ffffff;
    --error-container: #ffdad6;
    --on-error-container: #93000a;

    /* SURFACE */
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

    /* OUTLINE */
    --outline: #737686;
    --outline-variant: #c3c6d7;

    /* DESKTOP */
    --primary-desktop: #2E72C9;
    --primary-hover: #1e5bb8;
    --on-surface-desktop: #16181B;
    --desktop-bg: linear-gradient(135deg, #1e3a8a 0%, #2E72C9 50%, #93c5fd 100%);

    /* LOGIN */
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

    /* BACKDROP */
    --backdrop-blur-taskbar: blur(12px);
    --backdrop-blur-glass: blur(8px);
    --blur: blur(40px);

    /* TYPOGRAPHY */
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

    /* BORDER RADIUS */
    --rounded-sm: 0.125rem;
    --rounded: 0.25rem;
    --rounded-md: 0.375rem;
    --rounded-lg: 0.5rem;
    --rounded-xl: 0.75rem;
    --rounded-full: 9999px;

    /* Z-INDEX */
    --z-base: 1;
    --z-window-inactive: 10;
    --z-window-active: 20;
    --z-widget: 40;
    --z-topbar: 50;
    --z-dock: 50;
    --z-modal: 100;
    --z-overlay: 200;
    --z-tooltip: 10000;

    /* TRANSITIONS */
    --transition-fast: 0.15s;
    --transition-base: 0.2s;
    --transition-slow: 0.3s;
    --transition-bounce: 0.5s cubic-bezier(0.34, 1.56, 0.64, 1);
  `;

  onMount(() => {
    // Apply CSS variables to document root
    const root = document.documentElement;
    root.style.cssText += cssVariables;
  });
</script>

<main class="design-template">
  <header class="header">
    <h1>HomeVault Design System v2.1</h1>
    <p>Référence complète des tokens de design</p>
  </header>

  <nav class="sections-nav">
    {#each sections as section}
      <button
        class:active={activeSection === section.id}
        on:click={() => activeSection = section.id}
      >
        {section.label}
      </button>
    {/each}
  </nav>

  <section class="content">
    {#if activeSection === 'overview'}
      <div class="overview">
        <h2>Vue d'ensemble</h2>
        <p>Ce fichier contient TOUS les tokens de design extraits des fichiers theme homevault/</p>
        <div class="stats">
          <div class="stat">
            <span class="stat-value">{Object.keys(primaryColors).length + Object.keys(secondaryColors).length + Object.keys(tertiaryColors).length + Object.keys(errorColors).length + Object.keys(surfaceColors).length}</span>
            <span class="stat-label">Couleurs</span>
          </div>
          <div class="stat">
            <span class="stat-value">{Object.keys(fontSizes).length}</span>
            <span class="stat-label">Tailles police</span>
          </div>
          <div class="stat">
            <span class="stat-value">{Object.keys(borderRadius).length}</span>
            <span class="stat-label">Border radius</span>
          </div>
          <div class="stat">
            <span class="stat-value">{Object.keys(zIndex).length}</span>
            <span class="stat-label">Z-index</span>
          </div>
        </div>
      </div>
    {/if}

    {#if activeSection === 'colors'}
      <div class="colors-grid">
        <div class="color-section">
          <h3>Primary</h3>
          <div class="color-swatches">
            {#each Object.entries(primaryColors) as [name, value]}
              <div class="swatch" style="background: {value}">
                <span class="swatch-name">{name}</span>
                <span class="swatch-value">{value}</span>
              </div>
            {/each}
          </div>
        </div>
        <div class="color-section">
          <h3>Secondary</h3>
          <div class="color-swatches">
            {#each Object.entries(secondaryColors) as [name, value]}
              <div class="swatch" style="background: {value}">
                <span class="swatch-name">{name}</span>
                <span class="swatch-value">{value}</span>
              </div>
            {/each}
          </div>
        </div>
      </div>
    {/if}
  </section>
</main>

<style>
  :global(:root) {
    /* CSS variables will be injected by the script */
  }

  .design-template {
    font-family: 'Inter', -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
    max-width: 1200px;
    margin: 0 auto;
    padding: 24px;
  }

  .header {
    text-align: center;
    margin-bottom: 32px;
  }

  .header h1 {
    font-size: 30px;
    font-weight: 600;
    color: var(--on-surface, #191c1e);
  }

  .sections-nav {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
    margin-bottom: 24px;
    padding-bottom: 16px;
    border-bottom: 1px solid var(--outline-variant, #c3c6d7);
  }

  .sections-nav button {
    padding: 8px 16px;
    border: none;
    border-radius: 0.25rem;
    background: transparent;
    color: var(--on-surface-variant, #434655);
    cursor: pointer;
    transition: all 0.15s;
  }

  .sections-nav button:hover {
    background: var(--surface-container, #eceef0);
  }

  .sections-nav button.active {
    background: var(--primary, #004ac6);
    color: white;
  }

  .content {
    min-height: 400px;
  }

  .overview .stats {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: 16px;
    margin-top: 24px;
  }

  .stat {
    background: var(--surface-container-lowest, #ffffff);
    border: 1px solid var(--outline-variant, #c3c6d7);
    border-radius: 0.5rem;
    padding: 16px;
    text-align: center;
  }

  .stat-value {
    display: block;
    font-size: 30px;
    font-weight: 600;
    color: var(--primary, #004ac6);
  }

  .stat-label {
    font-size: 12px;
    color: var(--on-surface-variant, #434655);
  }

  .colors-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
    gap: 24px;
  }

  .color-section h3 {
    margin-bottom: 12px;
    color: var(--on-surface, #191c1e);
  }

  .color-swatches {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(120px, 1fr));
    gap: 8px;
  }

  .swatch {
    height: 80px;
    border-radius: 0.25rem;
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    padding: 8px;
    font-size: 10px;
    box-shadow: inset 0 0 0 1px rgba(0,0,0,0.1);
  }

  .swatch-name {
    color: white;
    font-weight: 500;
    text-shadow: 0 1px 2px rgba(0,0,0,0.3);
  }

  .swatch-value {
    color: rgba(255,255,255,0.8);
    font-family: monospace;
    font-size: 9px;
  }
</style>
