<!-- ========================================================================
     File          : LazyLoad.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Lazily loads an app's component module the first time its window
       opens (code-splitting): shows a spinner while the chunk loads and an
       inline error with a retry when the import fails.

     Dependencies:
       - svelte (>= 5), @tabler/icons-svelte

     Usage:
       Wraps the dynamic import in the window manager's component loaders.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { IconCircleX } from '@tabler/icons-svelte';

  /** Function that returns a promise resolving to the component module to lazily load */
  export let componentLoader: () => Promise<any>;
  /** Title displayed while the component loads */
  export let windowTitle: string;

  let Component: any = null;
  let loading = true;
  let error: string | null = null;

  onMount(async () => {
    try {
      const module = await componentLoader();
      Component = module.default;
      loading = false;
    } catch (err) {
      console.error('Failed to load component:', err);
      error = 'Failed to load component';
      loading = false;
    }
  });
</script>

{#if loading}
  <div class="lazy-loading">
    <div class="loading-spinner"></div>
    <p>Loading {windowTitle}...</p>
  </div>
{:else if error}
  <div class="lazy-error">
    <IconCircleX size={48} stroke={2} />
    <p>{error}</p>
    <button onclick={() => window.location.reload()}>Reload</button>
  </div>
{:else if Component}
  <svelte:component this={Component} />
{/if}

<style>
  .lazy-loading,
  .lazy-error {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    height: 100%;
    padding: var(--hv-space-10);
  }

  .loading-spinner {
    width: 40px;
    height: 40px;
    border: 4px solid var(--hv-neutral-100);
    border-top-color: var(--hv-primary);
    border-radius: var(--hv-radius-full);
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to { transform: rotate(360deg); }
  }

  .lazy-loading p {
    margin-top: var(--hv-space-4);
    font-size: var(--hv-font-size-base);
    color: var(--hv-neutral-500);
  }

  .lazy-error {
    color: var(--hv-error);
  }

  .lazy-error :global(svg) {
    margin-bottom: var(--hv-space-4);
  }

  .lazy-error button {
    margin-top: var(--hv-space-4);
    padding: var(--hv-space-2) 16px;
    background: var(--hv-primary);
    color: white;
    border: none;
    border-radius: var(--hv-radius-default);
    cursor: pointer;
  }
</style>
