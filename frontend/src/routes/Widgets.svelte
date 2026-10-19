<!-- ========================================================================
     File          : Widgets.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
     The Widgets app: catalog grid of the desktop monitor widgets with
     per-widget visibility toggles and the shared refresh-cadence choice.

     Dependencies:
       - svelte (>= 5), stores/widgetCatalog + monitorStore

     Usage:
     Opened as a desktop window app.
     ======================================================================== -->
<script lang="ts">
  import { IconCircleCheck, IconLayoutGrid, IconCirclePlus, IconRefresh } from '@tabler/icons-svelte';
  import { monitorStore } from '$lib/stores/monitorStore';
  import { WIDGET_DEFS } from '$lib/stores/widgetCatalog';
  import { t, translate } from '$lib/stores/i18n';
  import { Button } from '$lib/components/theme';
  import { PageLayout } from '$lib/components/app';
  import { cn } from '$lib/utils.js';

  // Every widget this app offers is a large, single-instance draggable
  // desktop panel whose visibility lives in monitorStore. The catalog itself
  // is shared with Settings > Widgets (`stores/widgetCatalog.ts`); this app
  // only projects each entry onto the live store value plus a toggle.
  interface WidgetTile {
    id: string;
    icon: any;
    color: string;
    nameKey: string;
    descKey: string;
    visible: boolean;
    toggle: () => void;
  }

  $: widgetDefs = WIDGET_DEFS.map((d) => ({
    id: d.id,
    icon: d.icon,
    color: d.color,
    nameKey: d.nameKey,
    descKey: d.descKey,
    visible: $monitorStore[d.visibleKey],
    toggle: () => d.setVisible(!$monitorStore[d.visibleKey]),
  })) satisfies WidgetTile[];

  $: totalActive = widgetDefs.filter((w) => w.visible).length;

  // "Reset to Default" restores the shipped defaults — and the shipped
  // default for every monitor is **off** (see `defaultPositions()` in
  // `monitorStore.ts`): a fresh install opens on a clean desktop. Positions
  // are kept; only visibility goes back to the default.
  function resetAll() {
    for (const d of WIDGET_DEFS) d.setVisible(false);
  }
</script>

<PageLayout
  title={$t('widgets.title')}
  data-testid="widgets-page"
>
  {#snippet icon()}
    <IconLayoutGrid stroke={1.8} />
  {/snippet}

  <div
    data-testid="widgets-grid"
    class="grid gap-4 [grid-template-columns:repeat(auto-fill,minmax(280px,1fr))]"
  >
    {#each widgetDefs as widget (widget.id)}
      <button
        data-testid="widgets-tile"
        class:active={widget.visible}
        class={cn(
          'group relative flex items-center gap-4 p-5 rounded-lg bg-white border-2 text-left cursor-pointer transition-all hover:-translate-y-0.5',
          widget.visible
            ? 'border-[var(--hv-primary)] bg-[var(--hv-info-light)] shadow-[0_4px_12px_var(--hv-overlay-hover)]'
            : 'border-[var(--hv-border)] hover:border-[var(--hv-neutral-300)] hover:shadow-[0_4px_12px_var(--hv-overlay-hover)]',
        )}
        onclick={widget.toggle}
      >
        <span
          data-testid="widgets-tile-icon"
          class="grid place-items-center w-14 h-14 rounded-lg flex-none shadow-[0_2px_8px_var(--hv-overlay-hover)]"
          style="background: {widget.color};"
        >
          <widget.icon size={32} color="white" stroke={2} />
        </span>
        <span class="min-w-0 flex-1">
          <span data-testid="widgets-tile-name" class="block text-md font-semibold text-foreground truncate">
            {$t(widget.nameKey)}
          </span>
          <span data-testid="widgets-tile-desc" class="block text-sm text-muted-foreground truncate">
            {$t(widget.descKey)}
          </span>
        </span>
        {#if widget.visible}
          <span
            data-testid="widgets-tile-badge"
            class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-[var(--hv-radius-default)] bg-[var(--hv-success)] text-white text-sm font-semibold flex-none"
          >
            <IconCircleCheck size={16} stroke={2} />
            {$t('widgets.active')}
          </span>
        {:else}
          <span
            data-testid="widgets-tile-add"
            class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-[var(--hv-radius-default)] bg-[var(--hv-neutral-100)] text-[var(--hv-neutral-600)] text-sm font-semibold flex-none group-hover:bg-[var(--hv-primary)] group-hover:text-white"
          >
            <IconCirclePlus size={16} stroke={2} />
            {$t('widgets.add')}
          </span>
        {/if}
      </button>
    {/each}
  </div>

  {#snippet footer()}
    <div class="flex w-full items-center justify-between gap-4">
      <div data-testid="widgets-stats" class="flex items-center gap-2 text-base text-muted-foreground">
        <IconLayoutGrid size={20} stroke={2} class="text-[var(--hv-neutral-400)]" />
        <span>{translate('widgets.count_on_desktop', { count: String(totalActive) })}</span>
      </div>
      <Button variant="ghost" onclick={resetAll} data-testid="widgets-reset">
        <IconRefresh size={18} stroke={2} />
        {$t('widgets.reset_button')}
      </Button>
    </div>
  {/snippet}
</PageLayout>