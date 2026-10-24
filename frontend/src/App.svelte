<!-- ========================================================================
     File          : App.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 12/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
     Application root: waits for the translation bundle, applies the chosen
     wallpaper, boots the live SSE connection after login, and renders the
     route the app gate selects (setup / recovery / login / desktop).

     Dependencies:
       - svelte (>= 5), stores (i18n, wallpaper, appGate), services/api

     Usage:
       Mounted once by main.ts.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { toast } from 'svelte-sonner';
  import { translationsReady, translate } from '$lib/stores/i18n';
  import { wallpaperStore } from '$lib/stores/wallpaper';
  import { api } from '$lib/services/api';
  import { appGate } from '$lib/stores/appGate';
  import { Spinner, Toast } from '$lib/components/theme';
  import Login from './routes/Login.svelte';
  import Desktop from './routes/Desktop.svelte';
  import SetupWizard from './routes/SetupWizard.svelte';

  // Wallpaper/accent color are saved server-side, once for the whole
  // appliance (an admin saves them from Settings → Appearance), but the local
  // wallpaperStore only ever hydrates from this browser's own localStorage.
  // Since localStorage is scoped per origin (host:port), the same account
  // accessed from two different addresses (e.g. a LAN IP and a public IP
  // forwarded to the same server) each keep their own independent copy and
  // could show a different wallpaper/accent color. Fetching the server's
  // real values once per boot keeps every origin in sync.
  //
  // ROADMAP 26.1: this used to call `getAllSettings()`, which is admin-only,
  // so a non-admin's hydration failed and the error was swallowed — the
  // desktop silently kept that browser's stale copy. It now calls the
  // session-readable `GET /settings/appearance` (wallpaper + accent only),
  // and a failure is reported once instead of disappearing.
  async function hydrateAppearanceFromServer() {
    try {
      const response = await api.getAppearanceSettings();
      if (!response.success || !response.data) {
        throw new Error(response.message || 'appearance unavailable');
      }
      if (response.data.accent_color) {
        document.documentElement.style.setProperty('--hv-primary', response.data.accent_color);
      }
      if (response.data.wallpaper) {
        try {
          // normalizeWallpaper (inside applyRaw) keeps only a bundled
          // /wallpapers/ image and falls back to the default photo for
          // anything else. The server already validates on save (ROADMAP
          // 26.1), so this catch is a safety net, not the only check.
          wallpaperStore.applyRaw(JSON.parse(response.data.wallpaper));
        } catch {
          wallpaperStore.reset();
        }
      }
    } catch (error) {
      // Non-fatal — the desktop keeps this origin's localStorage value — but
      // no longer silent: the user is told the appliance look could not be
      // loaded, so a stale wallpaper has an explanation.
      console.warn('Could not load the appliance appearance:', error);
      toast.error(translate('settings.appearance_load_failed'));
    }
  }

  let currentPath = '/';
  let ready = false;
  // Guards the one-shot server appearance hydration. Reset when the gate
  // leaves the desktop so the next account to sign in loads its own
  // wallpaper/accent instead of reusing the previous session's.
  let appearanceHydrated = false;

  function handleNavigation() {
    currentPath = window.location.pathname;
  }

  function navigate(path: string) {
    if (window.location.pathname === path) return;
    window.history.pushState({}, '', path);
    handleNavigation();
  }

  onMount(() => {
    handleNavigation();
    window.addEventListener('popstate', handleNavigation);

    ready = true;
    // The gate decides setup / recovery / login / desktop in one place (see
    // `$lib/stores/appGate`) — the shell no longer renders Login/Desktop
    // before that decision is known, so there is exactly one render of the
    // correct screen and never a flash of the wrong one.
    appGate.refresh();

    return () => {
      window.removeEventListener('popstate', handleNavigation);
    };
  });

  $: gateRoute = $appGate.route;

  // The gate is the authority; the address bar follows it.
  $: if (ready && gateRoute === 'app' && currentPath !== '/desktop') {
    navigate('/desktop');
  }
  $: if (
    ready &&
    (gateRoute === 'login' ||
      gateRoute === 'setup' ||
      gateRoute === 'recovery' ||
      gateRoute === 'password-change' ||
      gateRoute === 'error') &&
    currentPath !== '/login' &&
    currentPath !== '/'
  ) {
    navigate('/login');
  }

  $: if (ready && gateRoute === 'app' && !appearanceHydrated) {
    appearanceHydrated = true;
    hydrateAppearanceFromServer();
  }
  $: if (gateRoute !== 'app' && appearanceHydrated) {
    appearanceHydrated = false;
  }
</script>

{#if ready && $translationsReady && $appGate.route !== 'loading'}
  {#if $appGate.route === 'setup'}
    <SetupWizard />
  {:else if $appGate.route === 'recovery'}
    <Login recoveryMode />
  {:else if $appGate.route === 'password-change'}
    <Login passwordChangeMode />
  {:else if $appGate.route === 'error'}
    <Login connectionError />
  {:else if $appGate.route === 'login'}
    <Login />
  {:else}
    <Desktop />
  {/if}
  <Toast />
{:else if ready}
  <div class="hv-app-boot-loading">
    <Spinner size="lg" />
  </div>
{/if}

<style>
  .hv-app-boot-loading {
    position: fixed;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--hv-background);
  }
</style>
