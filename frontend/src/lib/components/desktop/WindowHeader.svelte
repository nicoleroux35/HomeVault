<!-- ========================================================================
     File          : WindowHeader.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Window title bar: app icon tinted with the window's accent (from the
       hvAppAccent context, matching dock/Launchpad), title, and the window
       control cluster (minimize/maximize/close).

     Dependencies:
       - svelte (>= 5), theme/Icon

     Usage:
       Flush top edge inside PageLayout/WindowFrame.
     ======================================================================== -->
<script lang="ts">
  import { getContext, type Snippet } from 'svelte';
  import Icon from '$lib/components/theme/Icon.svelte';

  /** The app's identity colour/gradient, published by WindowFrame as the
   *  `hvAppAccent` context (see the app-icon-color-consistency rule). Used
   *  to tint the icon tile so it matches the dock / Launchpad. An explicit
   *  `accent` prop wins over it; the shared accent gradient
   *  (`--hv-app-accent-grad`) is the last resort. Every app resolves to its
   *  own `--hv-app-<id>-grad` gradient — the header icon tile is deliberately
   *  never flat. */
  const ctxAccent = getContext<string | null>('hvAppAccent');

  interface Tab {
    id: string;
    label: string;
    /** Optional icon component shown before the label. */
    icon?: any;
    /** Optional count badge shown after the label. */
    count?: number;
  }

  interface Props {
    /** Custom glyph for the icon tile. Falls back to <Icon name={iconName}>. */
    icon?: Snippet;
    /** Sprite icon name (usually `win.icon`) for the default glyph. */
    iconName?: string;
    /** App / window title — the ONLY text this bar carries. */
    title?: string;
    /** Icon-tile background — gradient or flat colour. Defaults to the shared
     *  app accent gradient via the `hvAppAccent` context. */
    accent?: string;
    /** Flexible centre region (e.g. the Terminal's tab strip). */
    center?: Snippet;
    /** Right-aligned content, placed before the space this bar reserves for
     *  WindowFrame's min / max / close trio (see `--hv-winhdr-btn-clear`). */
    actions?: Snippet;
    /** Extra content left of the primary action button (status pills,
     *  secondary buttons, loaders). Rendered in the same cluster as `actions`. */
    extra?: Snippet;
    /** Optional icon component shown before the primary action label. */
    actionIcon?: any;
    /** Label of the built-in primary action button. Omit to hide it. */
    actionLabel?: string;
    /** Disables the primary action button (e.g. while a request is in flight). */
    actionDisabled?: boolean;
    /** Called when the primary action button is clicked. */
    onAction?: () => void;
    /** Sub-section tabs shown under the header bar. Omit to hide the tab row. */
    tabs?: Tab[];
    /** Currently active tab id (bindable). */
    activeTab?: string;
    /** Called when a tab is selected, in addition to updating `activeTab`. */
    onTabChange?: (id: string) => void;
    /** Accessible name for the tab bar (falls back to the title). */
    tabsLabel?: string;
  }

  let {
    icon,
    iconName,
    title,
    accent,
    center,
    actions,
    extra,
    actionIcon: ActionIcon,
    actionLabel,
    actionDisabled = false,
    onAction,
    tabs = [],
    activeTab = $bindable(),
    onTabChange,
    tabsLabel,
  }: Props = $props();

  const tileBg = $derived(accent ?? ctxAccent ?? 'var(--hv-app-accent-grad)');

  function selectTab(id: string) {
    activeTab = id;
    onTabChange?.(id);
  }

  // Arrow-key navigation across the tab bar (WAI-ARIA tabs pattern):
  // Left/Right move + select the neighbour, Home/End jump to the ends.
  function onTabKeydown(e: KeyboardEvent, index: number) {
    const last = tabs.length - 1;
    let next = -1;
    // KeyboardEvent.key for the arrow keys is "ArrowRight"/"ArrowLeft"/
    // "ArrowUp"/"ArrowDown" — never an icon *component* name. A sitewide
    // icon-import rename pass once clobbered these string literals, silently
    // breaking arrow-key tab navigation on every consumer.
    if (e.key === 'ArrowRight' || e.key === 'ArrowDown') next = index === last ? 0 : index + 1;
    else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') next = index === 0 ? last : index - 1;
    else if (e.key === 'Home') next = 0;
    else if (e.key === 'End') next = last;
    else return;
    e.preventDefault();
    selectTab(tabs[next].id);
    const strip = (e.currentTarget as HTMLElement).parentElement;
    (strip?.children[next] as HTMLElement | undefined)?.focus();
  }
</script>

<!-- The standardized solid window header. The bar is marked [data-hv-drag] so
     WindowFrame moves the window when it (but not a button / input inside it)
     is pressed. Self-contained light solid background so it reads correctly
     above the wallpaper and app content. When `tabs` are supplied a tab row is
     rendered under the bar inside the same solid wrap.

     Standardized anatomy — ONE control row, everything on a shared vertical
     axis: icon tile · title · [center] · [actions]. No subtitle, no decorative
     status badge/version pill: a description belongs in the page body, and a
     badge there would push the title off the row's centre line. The icon tile
     is --hv-winhdr-icon-size, i.e. exactly --hv-winctrl-size, so it matches the
     min/max/close trio WindowFrame pins to this same bar. -->
<div class="hv-winhdr-wrap">
  <div class="hv-winhdr" data-hv-drag>
    <div class="hv-winhdr-icon" style="background: {tileBg};">
      {#if icon}
        {@render icon()}
      {:else if iconName}
        <!-- No `size` on purpose: the tile's glyph size is owned by the
             `.hv-winhdr-icon :global(svg)` clamp below, which a stylesheet
             wins over the SVG width/height attribute `<Icon>` sets from its
             `size` prop. Passing one here (and in every route's icon snippet)
             only duplicated the clamp. `stroke` is NOT clamped by any rule, so
             it stays explicit or the glyph would visibly thicken. -->
        <Icon name={iconName} stroke={1.8} />
      {/if}
    </div>

    {#if title}
      <div class="hv-winhdr-text">
        <!-- The window title IS the screen's one <h1> (UI audit W8); in-route
             headings demote to h2/h3. Class stays so E2E title assertions
             (`.hv-winhdr-title`) keep working. -->
        <h1 class="hv-winhdr-title">{title}</h1>
      </div>
    {/if}

    {#if center}
      <div class="hv-winhdr-center">{@render center()}</div>
    {/if}

    {#if extra || actions || actionLabel}
      <div class="hv-winhdr-actions">
        {#if extra}{@render extra()}{/if}
        {#if actions}{@render actions()}{/if}
        {#if actionLabel}
          <button class="hv-winhdr-action-btn" onclick={onAction} disabled={actionDisabled}>
            {#if ActionIcon}
              <ActionIcon size={15} stroke={2} />
            {/if}
            <span>{actionLabel}</span>
          </button>
        {/if}
      </div>
    {/if}
  </div>

  {#if tabs.length}
    <div class="hv-winhdr-tabs" role="tablist" aria-label={tabsLabel ?? title}>
      {#each tabs as tab, i (tab.id)}
        <button
          class="hv-winhdr-tab"
          class:hv-winhdr-tab-active={activeTab === tab.id}
          role="tab"
          aria-selected={activeTab === tab.id}
          tabindex={activeTab === tab.id || (activeTab === undefined && i === 0) ? 0 : -1}
          onclick={() => selectTab(tab.id)}
          onkeydown={(e) => onTabKeydown(e, i)}
        >
          {#if tab.icon}
            <tab.icon size={15} stroke={2} />
          {/if}
          {tab.label}
          {#if tab.count !== undefined}
            <span class="hv-winhdr-tab-count">{tab.count}</span>
          {/if}
        </button>
      {/each}
    </div>
  {/if}
</div>

<style>
  .hv-winhdr-wrap {
    flex-shrink: 0;
    background: var(--hv-winhdr-bg);
    border-bottom: 1px solid var(--hv-winhdr-border);
  }

  .hv-winhdr {
    display: flex;
    align-items: center;
    gap: var(--hv-space-3);
    /* One control row: --hv-winhdr-pad-y + the 28px row + --hv-winhdr-pad-y.
       Everything inside is 28px tall or centred by align-items, so the icon
       tile, the title and the pinned min/max/close trio share one vertical
       centre line. `min-height` (not `height`) so the <=480px action cluster
       can still wrap to a second row instead of overflowing. */
    min-height: var(--hv-winhdr-h);
    box-sizing: border-box;
    /* Right padding reserves the min / max / close trio WindowFrame pins to
       this bar's top-right (3 × 28px + 2 × 6px + 12px inset + 12px gap). */
    padding: var(--hv-winhdr-pad-y) var(--hv-winhdr-btn-clear, 120px) var(--hv-winhdr-pad-y) var(--hv-space-3);
    user-select: none;
    -webkit-user-select: none;
    cursor: grab;
    /* Let the drag gesture drive the pointer handlers instead of the browser
       trying to scroll/pan the page — required for touch dragging. */
    touch-action: none;
  }

  .hv-winhdr:active {
    cursor: grabbing;
  }

  .hv-winhdr-icon {
    display: grid;
    place-items: center;
    width: var(--hv-winhdr-icon-size);
    height: var(--hv-winhdr-icon-size);
    border-radius: var(--hv-winhdr-icon-radius);
    color: var(--hv-on-primary);
    flex: none;
  }

  /* Glyph ratio of the mock's 24px tile (14px) carried onto the 28px one. */
  .hv-winhdr-icon :global(svg) {
    width: 16px;
    height: 16px;
  }

  .hv-winhdr-text {
    min-width: 0;
  }

  .hv-winhdr-title {
    /* 14px semibold on the mock's 20px line — the bar is only 44px, so the
       title stays on the compact chrome step (--hv-text-md). It is the bar's
       only text, so it sits dead centre of the 28px control row. */
    font-size: var(--hv-text-md);
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-winhdr-title);
    line-height: 20px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .hv-winhdr-center {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    /* Re-enable horizontal touch scrolling for a centre region that holds a
       scrollable strip (the Terminal's tab bar); the header itself blocks all
       gestures so the window drag can win. */
    touch-action: pan-x;
  }

  .hv-winhdr-actions {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: var(--hv-space-2-5);
    flex-shrink: 0;
  }

  /* When a centre region is present it already pushes actions right. */
  .hv-winhdr-center ~ .hv-winhdr-actions {
    margin-left: 0;
  }

  .hv-winhdr-action-btn {
    display: inline-flex;
    align-items: center;
    gap: var(--hv-space-1-5, 6px);
    /* The header's own control row height — the same 28px the window trio
       uses, so the primary action sits flush with it instead of filling the
       44px bar edge to edge. */
    min-height: var(--hv-winctrl-size);
    padding: 0 12px;
    border-radius: var(--hv-winctrl-radius);
    font-size: var(--hv-text-base);
    font-weight: var(--hv-font-weight-semibold);
    font-family: inherit;
    cursor: pointer;
    border: 1px solid var(--hv-primary);
    background: var(--hv-primary);
    color: var(--hv-on-primary);
    white-space: nowrap;
    transition: background var(--hv-transition-fast, 150ms ease), border-color var(--hv-transition-fast, 150ms ease);
  }

  .hv-winhdr-action-btn:hover:not(:disabled) {
    background: var(--hv-primary-hover);
    border-color: var(--hv-primary-hover);
  }

  .hv-winhdr-action-btn:disabled {
    /* Exact disabled pair (--hv-shell-border-soft bg + --hv-disabled-fg
       text), never an opacity wash. */
    background: var(--hv-shell-border-soft);
    border-color: transparent;
    color: var(--hv-disabled-fg);
    opacity: 1;
    cursor: not-allowed;
  }

  /* --- Tab row --- */

  .hv-winhdr-tabs {
    display: flex;
    align-items: center;
    gap: 0;
    padding: 0 16px;
    border-top: 1px solid var(--hv-winhdr-border);
    overflow-x: auto;
    scrollbar-width: none;
    -webkit-overflow-scrolling: touch;
  }

  .hv-winhdr-tabs::-webkit-scrollbar { display: none; }

  /* On touch devices a swipe is the only way to reach off-screen tabs, so
     show a slim scrollbar as the "there's more →" hint. `any-pointer` so a
     hybrid touch-laptop (fine primary pointer) still gets it. */
  @media (any-pointer: coarse) {
    .hv-winhdr-tabs {
      scrollbar-width: thin;
      scrollbar-color: var(--hv-shell-border) transparent;
    }
    .hv-winhdr-tabs::-webkit-scrollbar { display: block; height: 3px; }
    .hv-winhdr-tabs::-webkit-scrollbar-thumb {
      background: var(--hv-shell-border);
      border-radius: 3px;
    }
  }

  .hv-winhdr-tab {
    display: inline-flex;
    align-items: center;
    gap: var(--hv-space-1-5, 6px);
    /* 12px vertical padding ≈ 44px tab rows (touch target). */
    padding: 12px 4px;
    margin-right: var(--hv-space-4);
    font-size: var(--hv-text-base);
    font-weight: var(--hv-font-weight-semibold);
    font-family: inherit;
    color: var(--hv-shell-text);
    border: none;
    background: transparent;
    cursor: pointer;
    border-bottom: 2px solid transparent;
    white-space: nowrap;
    transition: color var(--hv-transition-fast, 150ms ease), border-color var(--hv-transition-fast, 150ms ease);
  }

  .hv-winhdr-tab:focus-visible {
    outline: 2px solid var(--hv-primary);
    outline-offset: -2px;
    border-radius: var(--hv-radius-sm, 4px);
  }

  .hv-winhdr-tab:hover {
    color: var(--hv-shell-text-strong);
  }

  .hv-winhdr-tab-active {
    color: var(--hv-primary);
    border-bottom-color: var(--hv-primary);
  }

  .hv-winhdr-tab-count {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 17px;
    height: 17px;
    padding: 0 5px;
    border-radius: 999px;
    background: var(--hv-shell-fill);
    color: var(--hv-shell-text);
    font-size: var(--hv-font-size-micro);
    font-weight: var(--hv-font-weight-bold);
  }

  .hv-winhdr-tab-active .hv-winhdr-tab-count {
    background: color-mix(in srgb, var(--hv-primary) 15%, transparent);
    color: var(--hv-primary);
  }

  @media (max-width: 480px) {
    .hv-winhdr {
      gap: var(--hv-space-2-5);
      padding-right: var(--hv-winhdr-btn-clear-sm, 108px);
      flex-wrap: wrap;
    }

    /* Let a wide actions cluster (badges + primary button) drop to its own
       row under the title instead of overflowing. */
    .hv-winhdr-actions {
      width: 100%;
      margin-left: 0;
      flex-wrap: wrap;
    }

    .hv-winhdr-tabs { padding: 0 12px; }
  }

  @media (prefers-reduced-motion: reduce) {
    .hv-winhdr,
    .hv-winhdr * {
      transition: none !important;
    }
  }
</style>
