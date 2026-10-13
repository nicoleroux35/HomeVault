<script lang="ts">
  /**
   * Edit Widgets — drag & drop layout editor.
   *
   * Two zones:
   *   - **Featured** (max 3): the widgets pinned to the top of the desktop
   *     widget column. Dragging a widget in or out, or reordering within the
   *     zone, is the whole point of this page.
   *   - **All widgets**: the full catalog in the account's saved order, each
   *     with its visibility toggle (visibility itself lives in `monitorStore`).
   *
   * Drag & drop uses the native HTML5 API (`draggable` + dataTransfer) with a
   * keyboard alternative: each tile carries move up/down buttons, so the
   * reorder is reachable with mouse, touch, pen and keyboard (WCAG 2.2 —
   * a drag-only interaction is not acceptable). The "featured" toggle is a
   * real button too, so the cap can be hit without dragging.
   */
  import { t, translate } from '$lib/stores/i18n';
  import { monitorStore } from '$lib/stores/monitorStore';
  import { widgetLayoutStore, MAX_FEATURED } from '$lib/stores/widgetLayout';
  import { WIDGET_DEFS } from '$lib/stores/widgetCatalog';
  import { PageLayout } from '$lib/components/app';
  import { Button } from '$lib/components/theme';
  import { cn } from '$lib/utils.js';
  import {
    IconLayoutGrid,
    IconStar,
    IconStarFilled,
    IconGripVertical,
    IconArrowUp,
    IconArrowDown,
    IconEye,
    IconEyeOff,
    IconRefresh,
  } from '@tabler/icons-svelte';

  let dragging: string | null = null;
  /** The zone the pointer is currently over, for the drop highlight. */
  let dragOverZone: 'featured' | 'all' | null = null;
  let capNotice = '';

  $: layout = $widgetLayoutStore;
  $: defById = new Map(WIDGET_DEFS.map((d) => [d.id, d]));
  $: orderedDefs = layout.order
    .map((id) => defById.get(id))
    .filter((d): d is (typeof WIDGET_DEFS)[number] => !!d);
  $: featuredDefs = layout.featured
    .map((id) => defById.get(id))
    .filter((d): d is (typeof WIDGET_DEFS)[number] => !!d);

  function toggleFeatured(id: string) {
    const ok = widgetLayoutStore.toggleFeatured(id);
    capNotice = ok ? '' : translate('edit_widgets.cap_reached', { max: String(MAX_FEATURED) });
  }

  function isFeatured(id: string): boolean {
    return layout.featured.includes(id);
  }

  function onDragStart(e: DragEvent, id: string) {
    dragging = id;
    e.dataTransfer?.setData('text/plain', id);
    if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move';
  }

  function onDragOver(e: DragEvent, zone: 'featured' | 'all') {
    e.preventDefault();
    dragOverZone = zone;
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'move';
  }

  function onDrop(e: DragEvent, zone: 'featured' | 'all', beforeId: string | null) {
    e.preventDefault();
    dragOverZone = null;
    const id = dragging ?? e.dataTransfer?.getData('text/plain') ?? null;
    dragging = null;
    if (!id) return;
    if (zone === 'featured' && !isFeatured(id)) {
      // Dragging into Featured is a request to feature it — honour the cap.
      if (!widgetLayoutStore.toggleFeatured(id)) {
        capNotice = translate('edit_widgets.cap_reached', { max: String(MAX_FEATURED) });
        return;
      }
    }
    if (zone === 'all' && isFeatured(id)) {
      // Dragging out of Featured un-features it.
      widgetLayoutStore.toggleFeatured(id);
    }
    widgetLayoutStore.moveBefore(id, beforeId);
    capNotice = '';
  }

  function onDragEnd() {
    dragging = null;
    dragOverZone = null;
  }

  /** Keyboard reorder: move `id` one slot up/down in the saved order. */
  function move(id: string, dir: -1 | 1) {
    const order = [...layout.order];
    const i = order.indexOf(id);
    const j = i + dir;
    if (i < 0 || j < 0 || j >= order.length) return;
    [order[i], order[j]] = [order[j], order[i]];
    widgetLayoutStore.setOrder(order);
  }

  function resetLayout() {
    for (const d of WIDGET_DEFS) d.setVisible(true);
    widgetLayoutStore.set({ featured: [], order: WIDGET_DEFS.map((d) => d.id) });
    capNotice = '';
  }
</script>

<PageLayout title={$t('edit_widgets.title')} data-testid="edit-widgets-page">
  {#snippet icon()}
    <IconLayoutGrid stroke={1.8} />
  {/snippet}

  <!-- Featured zone -->
  <section class="mb-6" aria-labelledby="ew-featured-h">
    <div class="flex items-baseline gap-2 mb-2">
      <h2 id="ew-featured-h" class="m-0 text-c-md font-semibold text-[var(--hv-shell-text-strong)]">
        {$t('edit_widgets.featured_title')}
      </h2>
      <span data-testid="edit-widgets-count" class="text-c-base text-[var(--hv-shell-text-2)]">
        {featuredDefs.length} / {MAX_FEATURED}
      </span>
    </div>
    <p class="m-0 mb-3 text-c-base text-[var(--hv-shell-text-2)]">{$t('edit_widgets.featured_hint')}</p>

    {#if capNotice}
      <p data-testid="edit-widgets-cap" role="alert" class="mb-3 text-c-base text-[var(--hv-warning)]">{capNotice}</p>
    {/if}

    <div
      data-testid="edit-widgets-featured"
      class={cn(
        'flex flex-wrap gap-3 min-h-[96px] p-3 rounded-xl border-2 border-dashed',
        dragOverZone === 'featured'
          ? 'border-[var(--hv-primary)] bg-[var(--hv-primary-selected-bg)]'
          : 'border-[var(--hv-shell-border)]',
      )}
      role="list"
      ondragover={(e) => onDragOver(e, 'featured')}
      ondragleave={() => (dragOverZone = null)}
      ondrop={(e) => onDrop(e, 'featured', null)}
    >
      {#if featuredDefs.length === 0}
        <p data-testid="edit-widgets-featured-empty" class="m-auto text-c-base text-[var(--hv-shell-text-3)]">
          {$t('edit_widgets.featured_empty')}
        </p>
      {/if}
      {#each featuredDefs as d (d.id)}
        <div
          role="listitem"
          draggable="true"
          data-testid="edit-widgets-featured-tile"
          class="flex items-center gap-2 px-3 py-2 rounded-lg bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)] cursor-grab"
          ondragstart={(e) => onDragStart(e, d.id)}
          ondragover={(e) => onDragOver(e, 'featured')}
          ondrop={(e) => { e.stopPropagation(); onDrop(e, 'featured', d.id); }}
          ondragend={onDragEnd}
        >
          <IconGripVertical size={16} stroke={1.8} class="text-[var(--hv-shell-text-3)]" aria-hidden="true" />
          <span class="grid place-items-center w-8 h-8 rounded-lg flex-none" style="background:{d.color}">
            <svelte:component this={d.icon} size={18} color="white" stroke={2} />
          </span>
          <span class="text-c-base font-semibold text-[var(--hv-shell-text-strong)]">{$t(d.nameKey)}</span>
          <button
            type="button"
            class="hv-ic text-[var(--hv-warning)]"
            data-testid="edit-widgets-unfeature"
            aria-label={$t('edit_widgets.unfeature', { name: translate(d.nameKey) })}
            onclick={() => toggleFeatured(d.id)}
          >
            <IconStarFilled size={17} />
          </button>
        </div>
      {/each}
    </div>
  </section>

  <!-- All widgets -->
  <section aria-labelledby="ew-all-h">
    <h2 id="ew-all-h" class="m-0 mb-3 text-c-md font-semibold text-[var(--hv-shell-text-strong)]">
      {$t('edit_widgets.all_title')}
    </h2>
    <div
      data-testid="edit-widgets-all"
      class={cn('flex flex-col gap-2 p-3 rounded-xl border-2 border-dashed', dragOverZone === 'all' ? 'border-[var(--hv-primary)]' : 'border-transparent')}
      role="list"
      ondragover={(e) => onDragOver(e, 'all')}
      ondragleave={() => (dragOverZone = null)}
      ondrop={(e) => onDrop(e, 'all', null)}
    >
      {#each orderedDefs as d, i (d.id)}
        <div
          role="listitem"
          draggable="true"
          data-testid="edit-widgets-tile"
          class={cn(
            'flex items-center gap-3 min-h-[60px] px-3 rounded-lg bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] cursor-grab',
            dragging === d.id && 'opacity-50',
          )}
          ondragstart={(e) => onDragStart(e, d.id)}
          ondragover={(e) => onDragOver(e, 'all')}
          ondrop={(e) => { e.stopPropagation(); onDrop(e, 'all', d.id); }}
          ondragend={onDragEnd}
        >
          <IconGripVertical size={16} stroke={1.8} class="text-[var(--hv-shell-text-3)] flex-none" aria-hidden="true" />
          <span class="grid place-items-center w-10 h-10 rounded-lg flex-none" style="background:{d.color}">
            <svelte:component this={d.icon} size={22} color="white" stroke={2} />
          </span>
          <div class="flex-1 min-w-0">
            <div class="text-c-base font-semibold text-[var(--hv-shell-text-strong)] truncate">{$t(d.nameKey)}</div>
            <div class="text-xs text-[var(--hv-shell-text-2)] truncate">{$t(d.descKey)}</div>
          </div>

          <div class="flex items-center gap-1 flex-none">
            <button
              type="button"
              class="hv-ic"
              data-testid="edit-widgets-up"
              aria-label={$t('edit_widgets.move_up', { name: translate(d.nameKey) })}
              disabled={i === 0}
              onclick={() => move(d.id, -1)}
            >
              <IconArrowUp size={17} stroke={1.8} />
            </button>
            <button
              type="button"
              class="hv-ic"
              data-testid="edit-widgets-down"
              aria-label={$t('edit_widgets.move_down', { name: translate(d.nameKey) })}
              disabled={i === orderedDefs.length - 1}
              onclick={() => move(d.id, 1)}
            >
              <IconArrowDown size={17} stroke={1.8} />
            </button>
            <button
              type="button"
              class="hv-ic"
              data-testid="edit-widgets-feature"
              aria-pressed={isFeatured(d.id)}
              aria-label={isFeatured(d.id) ? $t('edit_widgets.unfeature', { name: translate(d.nameKey) }) : $t('edit_widgets.feature', { name: translate(d.nameKey) })}
              onclick={() => toggleFeatured(d.id)}
            >
              {#if isFeatured(d.id)}<IconStarFilled size={18} class="text-[var(--hv-warning)]" />{:else}<IconStar size={18} stroke={1.8} />{/if}
            </button>
            <button
              type="button"
              class="hv-ic"
              data-testid="edit-widgets-visibility"
              aria-pressed={$monitorStore[d.visibleKey]}
              aria-label={$monitorStore[d.visibleKey] ? $t('edit_widgets.hide', { name: translate(d.nameKey) }) : $t('edit_widgets.show', { name: translate(d.nameKey) })}
              onclick={() => d.setVisible(!$monitorStore[d.visibleKey])}
            >
              {#if $monitorStore[d.visibleKey]}<IconEye size={18} stroke={1.8} />{:else}<IconEyeOff size={18} stroke={1.8} />{/if}
            </button>
          </div>
        </div>
      {/each}
    </div>
  </section>

  {#snippet footer()}
    <div class="flex w-full items-center justify-between gap-4">
      <span class="text-c-base text-[var(--hv-shell-text-2)]">{$t('edit_widgets.footer_hint')}</span>
      <Button variant="ghost" onclick={resetLayout} data-testid="edit-widgets-reset">
        <IconRefresh size={18} stroke={2} />
        {$t('edit_widgets.reset')}
      </Button>
    </div>
  {/snippet}
</PageLayout>
