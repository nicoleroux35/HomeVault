<script lang="ts">
  import { onMount } from 'svelte';
  import { authStore } from '$lib/stores/auth';
  import { translationsReady } from '$lib/stores/i18n';
  import { wallpaperStore } from '$lib/stores/wallpaper';
  import { api } from '$lib/services/api';
  import Icon from '$lib/components/theme/Icon.svelte';
  import { Spinner, Toast } from '$lib/components/theme';
  import Login from './routes/Login.svelte';
  import Desktop from './routes/Desktop.svelte';

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
  // Separate from `ready`: whether authStore.checkAuth() has resolved.
  // Rendering Login/Desktop before this is what caused the F5-refresh
  // flash below — Desktop (topbar, widgets, dock) used to mount instantly
  // using whatever route the URL already had, fully visible, then get torn
  // down a moment later once the auth check finished and (for an
  // expired/invalid session) redirected to /login. Gating on this instead
  // means nothing renders until the real, confirmed route is known, so
  // there's exactly one render of the correct screen — never a render of
  // the wrong one first.
  let authChecked = false;
  // Same reasoning as authChecked above, for the i18n store: on a hard
  // refresh the module-level translationsStore starts out only seeded with
  // the (incomplete) embedded English fallback, so rendering before the real
  // backend translation map has been fetched shows raw keys like
  // "desktop.raid_arrays" instead of translated text. Gating on this too
  // means Desktop/Login never render before real translations are known.

  function handleNavigation() {
    currentPath = window.location.pathname;
  }

  function navigate(path: string) {
    window.history.pushState({}, '', path);
    handleNavigation();
  }

  onMount(() => {
    handleNavigation();
    window.addEventListener('popstate', handleNavigation);

    ready = true;
    authStore.checkAuth().then(() => {
      if ($authStore.isAuthenticated) {
        hydrateAppearanceFromServer();
        if (currentPath === '/' || currentPath === '/login') {
          navigate('/desktop');
        }
      } else if (!$authStore.isAuthenticated && currentPath !== '/' && currentPath !== '/login') {
        navigate('/login');
      }
      authChecked = true;
    });

    return () => {
      window.removeEventListener('popstate', handleNavigation);
    };
  });

  $: isDesktopRoute = currentPath === '/desktop';
  $: isLoginRoute = currentPath === '/' || currentPath === '/login';
</script>

{#if ready && authChecked && $translationsReady}
  {#if isLoginRoute}
    <Login />
  {:else if isDesktopRoute}
    <Desktop />
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
