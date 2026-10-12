<script lang="ts">
  import { onMount } from 'svelte';
  import { translationsReady } from '$lib/stores/i18n';
  import { wallpaperStore } from '$lib/stores/wallpaper';
  import { api } from '$lib/services/api';
  import { appGate } from '$lib/stores/appGate';
  import { Spinner, Toast } from '$lib/components/theme';
  import Login from './routes/Login.svelte';
  import Desktop from './routes/Desktop.svelte';
  import SetupWizard from './routes/SetupWizard.svelte';

  // Wallpaper/accent color are saved server-side (per-account, via the Settings
  // page) but the local wallpaperStore only ever hydrates from this browser's
  // own localStorage. Since localStorage is scoped per origin (host:port), the
  // same account accessed from two different addresses (e.g. a LAN IP and a
  // public IP forwarded to the same server) each keep their own independent
  // copy and can show a different wallpaper/accent color. Fetching the
  // server's real saved values once per boot keeps every origin in sync.
  async function hydrateAppearanceFromServer() {
    try {
      const response = await api.getAllSettings();
      if (response.success && response.data) {
        const appearance = response.data.appearance;
        if (appearance?.accent_color) {
          document.documentElement.style.setProperty('--hv-primary', appearance.accent_color);
        }
        if (appearance?.wallpaper) {
          try {
            // normalizeWallpaper (inside applyRaw) keeps only a bundled
            // /wallpapers/ image and falls back to the default gradient for
            // anything else (legacy colours, gradients, custom uploads).
            wallpaperStore.applyRaw(JSON.parse(appearance.wallpaper));
          } catch {
            wallpaperStore.reset();
          }
        }
      }
    } catch {
      // Non-fatal: fall back to whatever this origin's localStorage already has.
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
