<!-- ========================================================================
     File          : HvLaunchpad.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       The full-screen app launcher: searchable/filterable app grid with
       keyboard navigation, focus-trapped while open, drag-out support that
       seeds the desktop drag state, and the "add a web shortcut" tile as the
       grid's LAST cell.

     Dependencies:
       - svelte (>= 5), theme/Icon, stores/dragState, stores/i18n,
         utils/focusTrap, @tabler/icons-svelte

     Usage:
       Opened from the desktop dock / menubar.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import Icon from '$lib/components/theme/Icon.svelte';
  import { startDrag } from '$lib/stores/dragState';
  import { languageStore, t } from '$lib/stores/i18n';
  import { focusTrap } from '$lib/utils/focusTrap';
  import { IconPlus } from '@tabler/icons-svelte';

  let {
    apps = [],
    onAppClick = (_appId: string) => {},
    onOpenExternal = (_url: string, _mode: 'tab' | 'window') => {},
    onAddShortcut = () => {},
    canAddShortcut = true,
    onClose = () => {},
  }: {
    apps: Array<{
      id: string;
      name: string;
      icon: string;
      color?: string;
      /** Per-app identity background (solid colour or a `var(--hv-app-*-grad)`)
       *  — same value the dock tile / app header use. */
      iconBg?: string;
      component?: any;
      customIcon?: string;
      category?: string;
      desc?: string;
      /** Docker catalog app: clicking the card doesn't open an in-app
       *  window, it opens `url` in a new tab / window (the user picks). */
      isDockerApp?: boolean;
      /** Docker app: false when installed but stopped → card greys out. */
      running?: boolean;
      /** The app's web UI address; undefined when it publishes no port. */
      url?: string;
    }>;
    onAppClick: (appId: string) => void;
    /** Open a Docker app's web UI externally. */
    onOpenExternal: (url: string, mode: 'tab' | 'window') => void;
    /** Open the shell's "add a web shortcut" dialog. The add tile is the
     *  LAST cell of the grid, after every app — the desktop no longer carries
     *  that action, so this is its only entry point. */
    onAddShortcut?: () => void;
    /** False once the shortcut cap (`MAX_SHORTCUTS`) is reached: the add tile
     *  is then not rendered at all, rather than rendered disabled. */
    canAddShortcut?: boolean;
    onClose: () => void;
  } = $props();

  // Which Docker-app card currently has its "new tab / new window" menu open,
  // and where to anchor that menu (fixed-positioned so the launcher panel's
  // own scroll container can't clip it).
  let externalMenuFor = $state<string | null>(null);
  let menuPos = $state({ x: 0, y: 0, flip: false });

  const externalMenuApp = $derived(
    apps.find(a => a.id === externalMenuFor && a.isDockerApp && a.running && a.url),
  );

  /** A Docker-app card can be clicked only when its app is running and has
   *  a reachable web UI. */
  function dockerCardReady(app: typeof apps[0]): boolean {
    return !!(app.running && app.url);
  }

  /** The state line under a Docker-app tile, by state. */
  function dockerCardDesc(app: typeof apps[0]): string {
    if (!app.running) return $t('desktop.launcher_not_running');
    if (!app.url) return $t('desktop.launcher_no_web_ui');
    return app.desc || $t('desktop.launcher_open_external');
  }

  function toggleExternalMenu(e: MouseEvent, app: typeof apps[0]) {
    if (!dockerCardReady(app)) return;
    if (externalMenuFor === app.id) {
      externalMenuFor = null;
      return;
    }
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const flip = r.bottom + 120 > window.innerHeight;
    menuPos = { x: r.left + r.width / 2, y: flip ? r.top - 6 : r.bottom + 6, flip };
    externalMenuFor = app.id;
  }

  let searchQuery = $state('');
  let selectedCategory = $state<string | null>(null);
  let draggingAppId = $state<string | null>(null);
  let searchInput = $state<HTMLInputElement | undefined>();

  /** Advisory caution for an app whose actions are destructive or
   *  connection-breaking, read from `desktop.app_<id>_info` (absent for most
   *  apps → empty string). The mock's tile has room for an icon and a label
   *  only, so this rides along as the tile's accessible description instead of
   *  a visible line: dropping it outright would lose the one place a user is
   *  warned before opening, say, Rescue or Users. */
  function infoFor(app: typeof apps[0]): string {
    const key = `desktop.app_${app.id}_info`;
    const v = $t(key);
    return v === key ? '' : v;
  }

  /** Category display label — localised via `desktop.launcher_cat_<slug>`,
   *  falling back to the raw English key the app defs carry. */
  function catLabel(category: string): string {
    const key = `desktop.launcher_cat_${category.toLowerCase().replace(/\s+/g, '_')}`;
    const v = $t(key);
    return v === key ? category : v;
  }

  /** The one collation rule for everything the launcher sorts. Locale-aware,
   *  keyed on the UI language (so French files "Écrans" under E) rather than
   *  the runtime's default; `sensitivity: 'base'` folds case and accents so
   *  "camera" / "Camera" do not split into two runs; `numeric` compares digit
   *  runs as numbers ("App 2" before "App 10"). */
  const collator = $derived(
    new Intl.Collator($languageStore, { sensitivity: 'base', numeric: true })
  );

  /** Categories present in the current app list, alphabetical by their
   *  LOCALISED label — the chips must read in the same A→Z order as the tiles
   *  below them, so the order follows the label the user actually sees, not
   *  the English key the app definitions carry. These are the launcher's
   *  filter chips ("All apps" is rendered first, outside this list). */
  let activeCategories = $derived(
    (() => {
      const set = new Set<string>();
      for (const a of apps) { if (a.category) set.add(a.category); }
      return [...set].sort((a, b) => collator.compare(catLabel(a), catLabel(b)));
    })()
  );

  let filteredApps = $derived(
    apps.filter(app =>
      !searchQuery.trim()
      || app.name.toLowerCase().includes(searchQuery.toLowerCase())
      || (app.category && catLabel(app.category).toLowerCase().includes(searchQuery.toLowerCase()))
      || (app.category && app.category.toLowerCase().includes(searchQuery.toLowerCase()))
      || (app.desc && app.desc.toLowerCase().includes(searchQuery.toLowerCase()))
    )
  );

  // ONE alphabetical order, no category grouping: the grid reads A→Z whatever
  // the query / chip filter is (the chip row is the only grouping affordance,
  // and grouping the tiles by category on top of it left the grid's order
  // unrelated to what the user was scanning for). `filteredApps` is already a
  // fresh array, so the in-place sort is safe.
  let visibleApps = $derived(
    filteredApps
      .filter(app => !selectedCategory || app.category === selectedCategory)
      .sort((a, b) => collator.compare(a.name, b.name))
  );

  function handleDragStart(event: DragEvent, app: typeof apps[0]) {
    if (!event.dataTransfer) return;
    draggingAppId = app.id;
    startDrag(app.id, app.name);
    const dragData = {
      appId: app.id,
      appName: app.name,
      appIcon: app.icon,
      appColor: app.color,
      customIcon: app.customIcon,
    };
    event.dataTransfer.setData('application/json', JSON.stringify(dragData));
    event.dataTransfer.effectAllowed = 'copy';
    const dragImage = document.createElement('div');
    dragImage.style.cssText = `
      position: absolute; top: -1000px; width: 64px; height: 64px;
      border-radius: 50%; background: var(--hv-launcher-surface);
      display: flex; align-items: center; justify-content: center;
      box-shadow: 0 4px 8px rgba(0,0,0,0.14), 0 0 2px rgba(0,0,0,0.12);
    `;
    document.body.appendChild(dragImage);
    event.dataTransfer.setDragImage(dragImage, 32, 32);
    setTimeout(() => document.body.removeChild(dragImage), 0);
  }

  function handleDragEnd() {
    draggingAppId = null;
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') onClose();
    // "/" is the launcher's search shortcut (the shell's Ctrl/⌘-K opens the
    // separate global search overlay).
    if (e.key === '/' && document.activeElement !== searchInput) {
      e.preventDefault();
      searchInput?.focus();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="lp-overlay">
  <!-- The scrim is its own button rather than a click handler on the overlay
       wrapper: the wrapper also contains the dialog, and a `role="button"`
       ancestor holding real <button> children both double-announces and
       swallows Space (same rule as the desktop's other overlays). -->
  <button
    type="button"
    class="lp-scrim"
    aria-label={$t('desktop.launcher_close')}
    onclick={onClose}
  ></button>

  <!-- `onclick` stopPropagation below is required: `Desktop.svelte` closes
       every panel from a `<svelte:window onclick>` (`showAppsCenter =
       false`). Without it, clicking a chip, the clear button or Reset — none
       of which are meant to dismiss the launcher — bubbles up and closes it.
       Clicks are the ONLY thing stopped: a keydown stopPropagation here was a
       real bug once (it swallowed Escape), so do not add one. The scrim is a
       sibling, so its clicks still bubble and still close.
       `svelte-ignore` is deliberate — a11y wants a keyboard handler beside a
       click handler, but this one has nothing keyboard-shaped to do; the
       dialog's keyboard contract is Escape (focusTrap), Tab cycling and the
       real <button> children. -->
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div
    class="lp-panel"
    role="dialog"
    aria-modal="true"
    aria-label={$t('desktop.launcher_title')}
    tabindex="-1"
    use:focusTrap={{ escape: onClose }}
    onclick={(e) => e.stopPropagation()}
  >
    <div class="lp-search">
      <svg class="lp-search-icon" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <circle cx="11" cy="11" r="8"/>
        <line x1="21" x2="16.65" y1="21" y2="16.65"/>
      </svg>
      <input
        bind:this={searchInput}
        type="text"
        inputmode="search"
        placeholder={$t('desktop.launcher_search_placeholder')}
        bind:value={searchQuery}
        class="lp-search-field"
        aria-label={$t('desktop.launcher_search_placeholder')}
      />
      {#if searchQuery}
        <button class="lp-search-clear hv-ic-abs" onclick={() => (searchQuery = '')} aria-label={$t('desktop.launcher_search_clear')}>
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round">
            <path d="M6 6l12 12M18 6L6 18"/>
          </svg>
        </button>
      {/if}
    </div>

    <!-- Category filter chips. `aria-pressed` (not a tablist): they narrow the
         one grid in place instead of swapping panels. -->
    <div class="lp-cats" role="group" aria-label={$t('desktop.launcher_all_apps')}>
      <button
        type="button"
        class="lp-cat"
        class:lp-cat-active={selectedCategory === null}
        aria-pressed={selectedCategory === null}
        onclick={() => { selectedCategory = null; }}
      >
        {$t('desktop.launcher_all_apps')}
      </button>
      {#each activeCategories as cat (cat)}
        <button
          type="button"
          class="lp-cat"
          class:lp-cat-active={selectedCategory === cat}
          aria-pressed={selectedCategory === cat}
          onclick={() => { selectedCategory = selectedCategory === cat ? null : cat; }}
        >
          {catLabel(cat)}
        </button>
      {/each}
    </div>

    <div class="lp-content">
      {#if searchQuery.trim() && visibleApps.length > 0}
        <p class="lp-count">{$t('desktop.launcher_results')} · {visibleApps.length}</p>
      {/if}
      {#if visibleApps.length === 0}
        <div class="lp-empty">
          <span class="lp-empty-icon" aria-hidden="true">
            <svg width="34" height="34" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
              <circle cx="11" cy="11" r="8"/>
              <line x1="21" x2="16.65" y1="21" y2="16.65"/>
            </svg>
          </span>
          <h2>{$t('desktop.launcher_no_apps_title')}</h2>
          <p>{$t('desktop.launcher_no_apps_desc')}</p>
          <button class="lp-reset" onclick={() => { searchQuery = ''; selectedCategory = null; }}>{$t('desktop.launcher_reset')}</button>
        </div>
      {/if}
      <!-- The grid always renders: its last cell is the "add a web shortcut"
           tile, which must sit after every app (the desktop no longer carries
           that action). It is not part of the filtered result set, so it is
           withheld while a query or a category chip narrows the grid — a
           filtered grid is an answer to "where is X?", not a menu. -->
      <div class="lp-grid">
        {#each visibleApps as app (app.id)}
          {@const info = app.isDockerApp ? '' : infoFor(app)}
          {#if app.isDockerApp}
            <!-- Docker catalog app: opens its web UI in a new tab / window
                 (the user picks), never an in-app window. -->
            <button
              type="button"
              class="lp-card"
              class:lp-card-open={externalMenuFor === app.id}
              disabled={!dockerCardReady(app)}
              aria-haspopup={dockerCardReady(app) ? 'menu' : undefined}
              aria-expanded={dockerCardReady(app) ? externalMenuFor === app.id : undefined}
              aria-describedby={`lp-state-${app.id}`}
              onclick={(e) => toggleExternalMenu(e, app)}
              title={app.name}
            >
              <span class="lp-card-icon" style="--lp-ic: {app.iconBg ?? app.color ?? 'var(--hv-primary)'};">
                {#if app.customIcon}
                  {@html app.customIcon}
                {:else}
                  <Icon name={app.icon} size={28} stroke={1.8} />
                {/if}
                <!-- Running marker. Presence, not colour, carries the
                     meaning — exactly how the dock's running indicator
                     works. A grey "stopped" dot cannot be drawn here: the
                     tile ground resolves to mid-grey (~rgb(100,105,117))
                     over the scrim, so a neutral dot lands at ~1:1 against
                     it and a green one at ~1.1:1. The tile's accessible
                     description already states the stopped case. -->
                {#if app.running}
                  <span class="lp-card-state" aria-hidden="true"></span>
                {/if}
              </span>
              <span class="lp-card-name">{app.name}</span>
              <span class="lp-card-sub" id={`lp-state-${app.id}`}>{dockerCardDesc(app)}</span>
            </button>
          {:else}
            <button
              type="button"
              class="lp-card"
              class:dragging={draggingAppId === app.id}
              draggable="true"
              aria-describedby={info ? `lp-info-${app.id}` : undefined}
              onclick={() => { onAppClick(app.id); onClose(); }}
              ondragstart={(e) => handleDragStart(e, app)}
              ondragend={handleDragEnd}
              title={app.name}
            >
              <span class="lp-card-icon" style="--lp-ic: {app.iconBg ?? app.color ?? 'var(--hv-primary)'};">
                {#if app.customIcon}
                  {@html app.customIcon}
                {:else}
                  <Icon name={app.icon} size={28} stroke={1.8} />
                {/if}
              </span>
              <span class="lp-card-name">{app.name}</span>
              {#if info}
                <span class="lp-sr" id={`lp-info-${app.id}`}>{info}</span>
              {/if}
            </button>
          {/if}
        {/each}

        {#if canAddShortcut && !searchQuery.trim() && !selectedCategory}
          <!-- The last cell: "add a web shortcut". Same tile geometry as an
               app tile so the grid stays one rhythm; only the icon ground
               differs (a dashed ring = "empty slot", not an app's colour).
               `aria-label` is the dialog's full title; the visible label is
               the short one, exactly as the desktop's old tile did. -->
          <button
            type="button"
            class="lp-card lp-card-add"
            data-testid="launcher-shortcut-add"
            aria-label={$t('shortcuts.add')}
            onclick={() => { onAddShortcut(); onClose(); }}
          >
            <span class="lp-card-icon lp-card-icon-add" aria-hidden="true">
              <IconPlus size={28} stroke={1.8} />
            </span>
            <span class="lp-card-name">{$t('shortcuts.add_short')}</span>
          </button>
        {/if}
      </div>
    </div>
  </div>

  {#if externalMenuApp}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <button
      type="button"
      class="lp-ext-backdrop"
      aria-label={$t('desktop.launcher_close')}
      onclick={() => externalMenuFor = null}
    ></button>
    <div
      class="lp-ext-menu"
      class:flip={menuPos.flip}
      role="menu"
      style="left: {menuPos.x}px; top: {menuPos.y}px;"
    >
      <button type="button" role="menuitem" onclick={() => { onOpenExternal(externalMenuApp.url!, 'tab'); externalMenuFor = null; onClose(); }}>
        {$t('docker.open_in_new_tab')}
      </button>
      <button type="button" role="menuitem" onclick={() => { onOpenExternal(externalMenuApp.url!, 'window'); externalMenuFor = null; onClose(); }}>
        {$t('docker.open_in_new_window')}
      </button>
    </div>
  {/if}
</div>

<style>
  /* ── Full-screen dimmed scrim (Figma 23:6646) ──
     The mock's launcher is not a card: it is the whole screen, darkened and
     blurred, with a top-aligned content column. Everything below is that
     column's stack: search → category chips → tile grid. */
  .lp-overlay {
    position: fixed;
    inset: 0;
    z-index: var(--hv-z-launcher);
    display: flex;
    flex-direction: column;
    align-items: center;
    /* 64px top matches the mock's own content inset. */
    padding: 64px 16px 24px;
    background: var(--hv-launcher-scrim);
    /* The mock's own `blur(2px)` on this frame, raised to 6px in
       `--hv-launcher-blur` (see tokens.css) once the desktop behind it turned
       out to stay legible. Write the STANDARD property only — never the
       `-webkit-` pair: the CSS minifier collapses the two into a lone
       `-webkit-backdrop-filter`, and Firefox (which reports
       `CSS.supports('-webkit-backdrop-filter', …) === false`) then drops the
       blur entirely. E2E 5.7 asserts the resolved value, so the pair cannot
       come back unnoticed.
       This is the one place in the shell that blurs: the rule against
       `backdrop-filter` (tokens.css) protects INK surfaces from a photo
       wallpaper, and the scrim does the opposite — it darkens that wallpaper
       so the tiles' white labels read on it. */
    backdrop-filter: blur(var(--hv-launcher-blur));
    animation: lpFade 200ms ease-out;
  }

  @keyframes lpFade {
    from { opacity: 0; }
    to { opacity: 1; }
  }

  .lp-scrim {
    position: absolute;
    inset: 0;
    padding: 0;
    border: none;
    background: transparent;
    cursor: default;
  }

  /* THE SCROLLER IS THE PANEL, NOT THE GRID WRAPPER — and that is a width
     decision, not a scrolling one. This engine's scrollbar overlays the
     content (a scrollbar here occupies 0px), so the drift is invisible in the
     E2E run; where the scrollbar is classic and does take layout width — Chrome
     on Windows/Linux, a GTK desktop with overlay scrolling off — it comes off
     its own container's content box. With `.lp-content` scrolling, that width
     came off the grid alone (the search bar and the chip row are outside it),
     so the grid sat narrower and further left than the other two whenever the
     tiles overflowed. Scrolling the panel instead puts all three children in
     one content box, so the gutter is taken from all three equally whatever
     the engine does. `scrollbar-gutter: stable` keeps it reserved when a filter
     shortens the list, so the column does not resize as the chips are used.
     E2E 5.12 measures the three widths with the tiles forced to overflow —
     the only regime where this can drift. */
  .lp-panel {
    position: relative;
    z-index: 1;
    width: min(100%, var(--hv-launcher-content-w));
    max-height: 100%;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow-y: auto;
    overflow-x: hidden;
    scrollbar-gutter: stable;
    -webkit-overflow-scrolling: touch;
    color: var(--hv-launcher-tile-fg);
    font-family: var(--hv-font-family);
    animation: lpPop 240ms cubic-bezier(0.16, 1, 0.3, 1);
  }

  .lp-panel::-webkit-scrollbar { width: 8px; }
  .lp-panel::-webkit-scrollbar-thumb {
    background: rgba(255, 255, 255, 0.28);
    border-radius: var(--hv-radius-sm);
  }

  @keyframes lpPop {
    from { opacity: 0; transform: translateY(-8px); }
    to { opacity: 1; transform: translateY(0); }
  }

  /* ── Search ──
     The mock's row is 56px tall (12px padding around a 32px input). The input
     itself carries the 44px touch floor here and the row padding drops to 6px,
     so the bar renders at the mock's 56px with a compliant target inside it.
     Shape: a CAPSULE (`--hv-radius-full`, clamped to height/2 = 28px), the same
     silhouette as the dock and topbar pills — the launcher is part of the same
     chrome family, and a 16px rounded rectangle here was the last surface that
     did not speak that language (it was 20px before the 2026-09-16 restyle).
     Sticky because the panel is the scroller (see `.lp-panel`), so the query
     field would otherwise scroll away with the tiles. `sticky` is a positioned
     value, so the clear button's `.hv-ic-abs` still finds this bar as its
     containing block. */
  .lp-search {
    position: sticky;
    top: 0;
    z-index: 2;
    display: flex;
    align-items: center;
    gap: 12px;
    flex-shrink: 0;
    /* Left padding clears the capsule's arc (the magnifier is the first thing
       in the bar); the clear button owns the right end. */
    padding: 6px 16px;
    margin-bottom: 16px;
    border-radius: var(--hv-radius-full);
    background: var(--hv-launcher-surface);
    box-shadow: var(--hv-launcher-search-shadow);
  }

  .lp-search-icon {
    flex: none;
    color: var(--hv-launcher-placeholder);
  }

  .lp-search-field {
    flex: 1;
    min-width: 0;
    height: var(--hv-touch-target-min);
    padding: 0;
    border: none;
    background: transparent;
    color: var(--hv-shell-text-strong);
    font-family: inherit;
    /* 16px: the mock's own search size, and the iOS no-auto-zoom threshold. */
    font-size: var(--hv-font-size-md);
    outline: none;
  }

  .lp-search-field::placeholder { color: var(--hv-launcher-placeholder); }

  /* Focus indicator — the launcher's OWN neutral ring, never the brand blue.
     Two reasons the brand ring had to go: (1) the dialog's focus trap lands on
     this field when the launcher opens, so a brand ring here was a permanent
     blue halo around the capsule, not a "you just tabbed here" affordance;
     (2) it was painted TWICE — once by this rule and once by the global
     `input:focus` rule in base.css, on the field inside the capsule.
     The replacement is a 2px slate rim drawn INSIDE the capsule (`inset`, so
     the bar keeps its exact silhouette): the rim composites to
     ≈`rgb(82,88,101)`, i.e. **7.1:1** against the white surface it marks (the
     3:1 floor of a focus indicator, 1.4.11 / 2.4.7, with a wide margin), and it
     introduces no second hue on a screen whose only accent is the app tiles'
     own gradients. An OUTSIDE ring was tried first and is invisible here: on a
     white capsule it merges with the bar, and on the dark scrim a dark halo is
     only ~10/255 darker than the veil. The chips / tiles keep their white +
     slate halo because they sit ON the scrim; this bar is a white surface,
     where an inset rim is the one reading that survives. */
  .lp-search:focus-within {
    box-shadow:
      inset 0 0 0 2px rgba(15, 23, 42, 0.72),
      var(--hv-launcher-search-shadow);
  }

  /* The field never paints a ring of its own: base.css's `input:focus`
     (specificity 0,1,1) outranks the plain class above and would stamp the
     brand ring back INSIDE the capsule. The wrapper owns the indicator — one
     surface, one ring. Kept as a two-class selector on purpose, so it still
     wins if that global rule ever gains weight. */
  .lp-search .lp-search-field:focus,
  .lp-search .lp-search-field:focus-visible {
    outline: none;
    border-color: transparent;
    box-shadow: none;
  }

  .lp-search-clear {
    right: 8px;
    display: grid;
    place-items: center;
    width: 38px;
    height: 38px;
    border: none;
    border-radius: var(--hv-radius-full);
    background: transparent;
    color: var(--hv-shell-text-2);
    cursor: pointer;
    transition: background 150ms ease;
  }

  .lp-search-clear:hover { background: var(--hv-shell-border-soft); }

  /* ── Category chips ──
     The mock's chip is 32px tall (6px + a 20px line + 6px). The visible pill
     keeps that geometry; a transparent 44px `::after` carries the touch
     target, which is the documented compromise (the chip row has 20px of
     padding below it and the search bar 16px above, so the pad cannot reach a
     neighbouring control).

     The row is a sibling of the grid in the same column, so it is always the
     6-column width. What must reach the grid's right edge is therefore the
     ROW, not each chip: the chips keep their own label width (`flex: 0 0
     auto`) and `justify-content: space-between` pushes the last one flush with
     the edge. The earlier `flex: 1 1 auto` did make every line flush too, but
     by stretching each chip: with six categories on an 818px row a chip grew
     to 172 × 32, i.e. a 5:1 bar rather than the 32px pill it is declared as —
     the *shape* was wrong even though the geometry passed. Equal-delta growth
     still made the longest label the widest chip, which is the part worth
     keeping, and `space-between` keeps it.

     Chips are capsules (`--hv-radius-full`): the same pill silhouette as the
     search bar above them, the dock and the topbar. A chip is never narrower
     than its label and never a stretched bar.

     The `<=640px` block pins `flex: none` (identical to `0 0 auto`) and turns
     the row into a one-line `overflow-x: auto` scroller. */
  .lp-cats {
    display: flex;
    flex-wrap: wrap;
    /* The row spans the grid's width; `space-between` spends the leftover on
       the gaps instead of on the chips, so every chip stays a pill. */
    justify-content: space-between;
    gap: 8px;
    flex-shrink: 0;
    padding-bottom: 20px;
  }

  .lp-cat {
    position: relative;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    /* Natural width: a chip is a 32px pill sized by its label, never stretched
       into a bar (see the `.lp-cats` comment). */
    flex: 0 0 auto;
    height: 32px;
    padding: 0 16px;
    border: none;
    border-radius: var(--hv-radius-full);
    background: var(--hv-launcher-chip);
    color: var(--hv-launcher-chip-fg);
    font-family: inherit;
    font-size: var(--hv-text-md);
    font-weight: var(--hv-font-weight-semibold);
    line-height: var(--hv-line-height-tight);
    white-space: nowrap;
    cursor: pointer;
    transition: background 150ms ease, color 150ms ease;
  }

  .lp-cat::after {
    content: '';
    position: absolute;
    left: 0;
    right: 0;
    top: 50%;
    height: var(--hv-touch-target-min);
    transform: translateY(-50%);
  }

  .lp-cat:hover { background: var(--hv-launcher-chip-hover); }

  .lp-cat-active,
  .lp-cat-active:hover {
    background: var(--hv-launcher-chip-active);
    color: var(--hv-launcher-chip-active-fg);
  }

  /* Dark scrim → the brand ring would disappear into it; the white ring is
     the only indicator that reads on this surface (2.4.7). The two controls
     that live on a *white* surface (the search bar's clear button, the
     external-open menu) keep the brand ring instead. */
  .lp-cat:focus-visible,
  .lp-card:focus-visible,
  .lp-reset:focus-visible {
    outline: none;
    box-shadow: 0 0 0 2px rgba(255, 255, 255, 0.95), 0 0 0 4px rgba(15, 23, 42, 0.55);
  }

  .lp-search-clear:focus-visible,
  .lp-ext-menu button:focus-visible {
    outline: none;
    box-shadow: var(--hv-focus-ring);
    border-radius: var(--hv-radius-control);
  }

  /* ── Content ──
     Not a scroll container: it only groups the empty state / results line with
     the grid, so it just takes its natural height and lets `.lp-panel` scroll. */
  .lp-content {
    flex: none;
  }

  .lp-count {
    margin: 0 2px 12px;
    font-size: var(--hv-text-sm);
    font-weight: var(--hv-font-weight-semibold);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--hv-launcher-tile-muted-fg);
  }

  /* 6 columns of 128px with a 12px gap, filling the 828px content column
     (`--hv-launcher-content-w`; the mock drew 5 — 6 is the requested count).
     `auto-fill` rather than a pinned `repeat(6, …)`: below that width the
     browser drops to 5, 4, … columns on its own and the phone block pins 3,
     so one rule covers every screen without a second breakpoint. The search
     bar and the chip row are siblings in the same column, so all three stay
     exactly the same width — E2E 5.12 fails if they ever drift apart. */
  .lp-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(124px, 1fr));
    gap: 12px;
    padding-bottom: 16px;
  }

  /* ── Application tile ── */
  .lp-card {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--hv-space-2-5);
    padding: 16px 8px;
    min-width: 0;
    border: none;
    /* A launcher tile is a button: the one control corner, like every other
       button in the product (its circular icon inside carries the shape). */
    border-radius: var(--hv-radius-control);
    background: var(--hv-launcher-tile);
    color: var(--hv-launcher-tile-fg);
    font-family: inherit;
    text-align: center;
    cursor: pointer;
    transition: background 160ms ease, transform 160ms ease;
  }

  .lp-card:hover { background: var(--hv-launcher-tile-hover); }
  .lp-card:active { transform: scale(0.98); }

  .lp-card.dragging { opacity: 0.4; }

  .lp-card:disabled { cursor: default; }
  .lp-card:disabled:hover { background: var(--hv-launcher-tile); }

  .lp-card-open { background: var(--hv-launcher-tile-hover); }

  /* The app's own gradient — the one-colour-per-app rule (CLAUDE.md) is what
     the launchpad tile shares with the dock tile and the in-window header.
     Shape: a PERFECT CIRCLE (`50%`, never a px stop), like `.hv-dock-icon`,
     `.hv-topbar-btn` and the `WindowHeader` tile — the launcher was the last
     place where the same app glyph appeared on a rounded square instead, so
     one app read as two different marks depending on where you looked at it.
     The glyph inside is half the tile (28px of 56px), the ratio every other
     chrome tile uses. */
  .lp-card-icon {
    position: relative;
    display: grid;
    place-items: center;
    width: var(--hv-launcher-icon-size);
    height: var(--hv-launcher-icon-size);
    flex: none;
    border-radius: 50%;
    color: var(--hv-launcher-tile-fg);
    /* `--lp-ic` is usually a `linear-gradient(...)` — the `background`
       shorthand takes either; `background-color` would silently drop the
       gradient and leave the white glyph on nothing. */
    background:
      linear-gradient(150deg, rgba(255, 255, 255, 0.24), rgba(255, 255, 255, 0) 62%),
      var(--lp-ic, var(--hv-primary));
    box-shadow: var(--hv-launcher-icon-shadow);
  }

  .lp-card-icon :global(svg),
  .lp-card-icon :global(img) {
    width: var(--hv-launcher-icon-glyph);
    height: var(--hv-launcher-icon-glyph);
  }

  /* The "add a web shortcut" tile (last cell of the grid). It keeps the app
     tile's box and label so the grid stays one rhythm; only the icon ground
     differs — a DASHED ring, the universal "empty slot" reading, instead of
     an app's identity gradient. Declared AFTER `.lp-card-icon` on purpose:
     same specificity, so source order is what drops that gradient. */
  .lp-card-icon-add {
    background: transparent;
    border: 1.5px dashed var(--hv-launcher-tile-muted-fg);
    box-shadow: none;
  }

  /* Docker-app state marker in the tile's top-right corner (the mock's own
     badge position). Rendered only while the container is up — see the
     markup comment. The white ring is what makes the marker perceivable at
     all: the green is 5.1:1 against the ring, and the ring 5.5:1 against the
     tile ground (WCAG 1.4.11), which the bare green would not be. */
  .lp-card-state {
    position: absolute;
    top: -3px;
    right: -3px;
    width: 12px;
    height: 12px;
    border-radius: var(--hv-radius-full);
    background: var(--hv-success);
    box-shadow: 0 0 0 2px var(--hv-launcher-surface);
  }

  .lp-card-name {
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
    font-size: var(--hv-text-sm);
    font-weight: var(--hv-font-weight-semibold);
    line-height: var(--hv-line-height-tight);
    color: var(--hv-launcher-tile-fg);
  }

  .lp-card-sub {
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
    font-size: var(--hv-text-xs);
    line-height: var(--hv-line-height-tight);
    color: var(--hv-launcher-tile-muted-fg);
  }

  /* Visible to assistive tech only — carries the app's caution (see
     `infoFor`) without adding a line the mock does not have. */
  .lp-sr {
    position: absolute;
    width: 1px;
    height: 1px;
    margin: -1px;
    padding: 0;
    border: 0;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }

  /* ── External-open menu (Docker apps) ── */
  .lp-ext-backdrop {
    position: fixed;
    inset: 0;
    z-index: var(--hv-z-launcher);
    border: none;
    background: transparent;
    cursor: default;
  }

  .lp-ext-menu {
    position: fixed;
    z-index: calc(var(--hv-z-launcher) + 1);
    transform: translateX(-50%);
    display: flex;
    flex-direction: column;
    min-width: 210px;
    padding: 6px;
    border-radius: var(--hv-radius-xl);
    background: var(--hv-shell-glass);
    border: 1px solid var(--hv-chrome-surface-border);
    box-shadow: var(--hv-chrome-surface-shadow);
  }

  .lp-ext-menu.flip { transform: translateX(-50%) translateY(-100%); }

  .lp-ext-menu button {
    display: flex;
    align-items: center;
    height: var(--hv-touch-target-min);
    padding: 0 12px;
    border: none;
    border-radius: var(--hv-radius-control);
    background: transparent;
    color: var(--hv-shell-text-strong);
    font-family: inherit;
    font-size: var(--hv-text-base);
    font-weight: var(--hv-font-weight-medium);
    text-align: left;
    cursor: pointer;
  }

  .lp-ext-menu button:hover { background: var(--hv-subtle-hover); }

  /* ── Empty state ── */
  .lp-empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 48px 20px;
    text-align: center;
  }

  /* Also a circle: it is the same "icon tile" role as the app tiles above, so
     it follows the same shape rule (see `.lp-card-icon`). */
  .lp-empty-icon {
    display: grid;
    place-items: center;
    width: 64px;
    height: 64px;
    margin-bottom: 16px;
    border-radius: 50%;
    background: var(--hv-launcher-tile);
    color: var(--hv-launcher-tile-muted-fg);
  }

  .lp-empty h2 {
    margin: 0 0 6px;
    font-size: var(--hv-text-lg);
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-launcher-tile-fg);
  }

  .lp-empty p {
    margin: 0 0 20px;
    font-size: var(--hv-text-base);
    color: var(--hv-launcher-tile-muted-fg);
  }

  .lp-reset {
    height: var(--hv-touch-target-min);
    padding: 0 24px;
    border: none;
    /* An action button, not a chip: control corner, not a capsule. */
    border-radius: var(--hv-radius-control);
    background: var(--hv-launcher-chip-active);
    color: var(--hv-launcher-chip-active-fg);
    font-family: inherit;
    font-size: var(--hv-text-md);
    font-weight: var(--hv-font-weight-semibold);
    cursor: pointer;
  }

  .lp-reset:active { transform: scale(0.96); }

  /* ── Phone: tighter inset, chips scroll sideways, 3-col grid ── */
  @media (max-width: 640px) {
    .lp-overlay {
      padding: calc(env(safe-area-inset-top, 0px) + 16px) 12px 12px;
    }

    .lp-cats {
      flex-wrap: nowrap;
      overflow-x: auto;
      overflow-y: hidden;
      padding-bottom: var(--hv-space-3);
      scrollbar-width: none;
    }

    .lp-cats::-webkit-scrollbar { display: none; }

    /* Back to the natural pill width: on a phone the row is a sideways
       scroller, not a line that has to reach the grid's right edge. */
    .lp-cat { flex: none; }

    .lp-grid {
      grid-template-columns: repeat(auto-fill, minmax(104px, 1fr));
      gap: 8px;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .lp-overlay,
    .lp-panel { animation: none; }
    .lp-card,
    .lp-cat,
    .lp-search-clear { transition: none; }
    .lp-card:active { transform: none; }
  }
</style>
