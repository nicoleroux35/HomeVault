<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { t } from '$lib/stores/i18n';
  import { IconClock, IconStarFilled } from '@tabler/icons-svelte';

  /** Compact quick-access card: global favorites + recent locations.
   *  Self-sufficient (loads its own data); `onopen` navigates the host. */
  let { onopen }: { onopen?: (path: string) => void } = $props();

  let favorites = $state<{ path: string; label: string }[]>([]);
  let recents = $state<{ path: string; label: string }[]>([]);
  let loaded = $state(false);

  onMount(async () => {
    try {
      const [favs, recs] = await Promise.all([
        api.getExplorerFavorites(),
        api.getExplorerRecents(),
      ]);
      if (favs.success && Array.isArray(favs.data)) {
        favorites = favs.data
          .slice(0, 6)
          .map((f: any) => ({ path: f.path, label: f.label || f.path.split('/').filter(Boolean).pop() || f.path }));
      }
      if (recs.success && Array.isArray(recs.data)) {
        recents = recs.data
          .slice(0, 6)
          .map((e: any) => ({ path: e.path, label: e.label || e.path.split('/').filter(Boolean).pop() || e.path }));
      }
    } catch {
      /* offline — the card simply stays empty */
    } finally {
      loaded = true;
    }
  });

  function open(path: string) {
    if (onopen) onopen(path);
  }
</script>

{#if loaded && (favorites.length > 0 || recents.length > 0)}
  <section data-testid="explorer-widget" aria-label={$t('explorer.recents')} class="mx-4 mt-3 grid gap-3 rounded-xl border border-[var(--hv-shell-border)] bg-[var(--hv-shell-glass)] p-3 @min-[720px]:grid-cols-2">
    {#if favorites.length > 0}
      <div class="min-w-0">
        <h3 class="m-0 mb-1.5 flex items-center gap-1.5 text-c-xs font-semibold uppercase tracking-wide text-[var(--hv-shell-text-2)]">
          <IconStarFilled size={13} class="text-[var(--hv-warning)]" /> {$t('explorer.favorites')}
        </h3>
        <ul class="m-0 flex flex-wrap gap-1.5 p-0 list-none">
          {#each favorites as f (f.path)}
            <li>
              <button class="max-w-44 truncate rounded-lg border border-[var(--hv-shell-border-soft)] bg-transparent px-2.5 py-1.5 text-c-sm text-[var(--hv-shell-text-strong)] cursor-pointer hover:bg-[var(--hv-subtle-hover)] hv-ic" onclick={() => open(f.path)} title={f.path}>{f.label}</button>
            </li>
          {/each}
        </ul>
      </div>
    {/if}
    {#if recents.length > 0}
      <div class="min-w-0">
        <h3 class="m-0 mb-1.5 flex items-center gap-1.5 text-c-xs font-semibold uppercase tracking-wide text-[var(--hv-shell-text-2)]">
          <IconClock size={13} /> {$t('explorer.recents')}
        </h3>
        <ul class="m-0 flex flex-wrap gap-1.5 p-0 list-none">
          {#each recents as r (r.path)}
            <li>
              <button class="max-w-44 truncate rounded-lg border border-[var(--hv-shell-border-soft)] bg-transparent px-2.5 py-1.5 text-c-sm text-[var(--hv-shell-text-strong)] cursor-pointer hover:bg-[var(--hv-subtle-hover)] hv-ic" onclick={() => open(r.path)} title={r.path}>{r.label}</button>
            </li>
          {/each}
        </ul>
      </div>
    {/if}
  </section>
{/if}
