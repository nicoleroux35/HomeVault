<!-- ========================================================================
     File          : WindowFrame.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       The draggable/resizable/maximizable window shell of the desktop:
       owns position, size and z-order through the window manager, drag and
       resize with edge snapping, content-fit auto-grow until the user
       resizes manually, a Svelte error boundary per window (with the
       test-only RenderErrorProbe), and app-accent context for the chrome.

     Dependencies:
       - svelte (>= 5), stores/windowManager, utils/snapUtils

     Usage:
       Rendered once per open window by the desktop shell.
     ======================================================================== -->
<script lang="ts">
  import { createEventDispatcher, onMount, onDestroy, setContext } from 'svelte';
  import { windowManager, type Window } from '$lib/stores/windowManager';
  import { t } from '$lib/stores/i18n';
  import { applySnap, DEFAULT_SNAP_CONFIG } from '$lib/utils/snapUtils';
  import WindowHeader from '$lib/components/desktop/WindowHeader.svelte';
  import RenderErrorProbe from '$lib/components/desktop/RenderErrorProbe.svelte';

  /** The window descriptor from the window manager — owns position, size, z-index, and state. */
  export let win: Window;

  /** Publish the app's identity colour to everything rendered inside this
   *  window (WindowHeader, and any app chrome that opts in). A window's colour
   *  never changes after it opens, so a plain context value is enough. */
  setContext('hvAppAccent', win.color ?? null);
  export let onClose: () => void;
  export let onMinimize: () => void;
  export let onMaximize: () => void;
  export let onFocus: () => void;
  export let onUpdatePosition: (x: number, y: number) => void;
  export let onUpdateSize: (width: number, height: number) => void;

  let isDragging = false;
  let isResizing = false;
  let resizeDirection = '';
  let isMinimizing = false;

  /* Live viewport width — decides whether a maximized window is truly
     fullscreen (>900px, the topbar + dock auto-hide) or has to be inset
     below the topbar and above the dock (<=900px, where that chrome stays
     put — an edge-reveal is unusable with touch). Threshold mirrors
     Desktop.svelte's `chromeAutoHideEligible` (viewportWidth > 900). */
  let viewportWidth = typeof window !== 'undefined' ? window.innerWidth : 1280;

  /* Drag origin snapshots — captured on mousedown so we can compute deltas. */
  let dragStartX = 0;
  let dragStartY = 0;
  let windowStartX = 0;
  let windowStartY = 0;

  /* Resize origin snapshots. */
  let resizeStartX = 0;
  let resizeStartY = 0;
  let windowStartWidth = 0;
  let windowStartHeight = 0;
  let windowStartLeft = 0;
  let windowStartTop = 0;

  const MIN_WIDTH = 400;
  const MIN_HEIGHT = 300;

  /* Capture the initial pointer state and attach move / end listeners to the
     document so dragging works even when the pointer leaves the window frame.
     Pointer Events (not mouse events) so it works with touch and pen, exactly
     like the resize handles. */
  function handleDragStart(e: PointerEvent) {
    const target = e.target as HTMLElement;
    if (win.isMaximized) return;
    // Every window drags from a [data-hv-drag] region — the shared
    // WindowHeader marks itself, and every app in OWN_HEADER_APPS marks its
    // own header. Never start a drag when the press landed on an actual control
    // (a tab, a button, an input, ...).
    if (!target.closest('[data-hv-drag]')) return;
    if (target.closest('button, a, input, textarea, select, label, [role="button"]')) return;

    e.preventDefault();
    e.stopPropagation();
    isDragging = true;
    onFocus();

    dragStartX = e.clientX;
    dragStartY = e.clientY;
    windowStartX = win.x ?? 0;
    windowStartY = win.y ?? 0;

    document.addEventListener('pointermove', handleDragMove, { passive: false });
    document.addEventListener('pointerup', handleDragEnd);
    document.addEventListener('pointercancel', handleDragEnd);
    document.body.style.cursor = 'grabbing';
  }

  /* Computes the new position from the drag delta, clamped and edge-snapped
     using the same rules as the desktop's resource monitor widgets
     (see snapUtils.ts) so window and widget dragging feel identical. */
  function handleDragMove(e: PointerEvent) {
    if (!isDragging) return;
    e.preventDefault();

    const deltaX = e.clientX - dragStartX;
    const deltaY = e.clientY - dragStartY;

    const newX = windowStartX + deltaX;
    const newY = windowStartY + deltaY;

    const result = applySnap(newX, newY, win.width, win.height);
    onUpdatePosition(result.x, result.y);
  }

  /* Cleans up drag listeners and resets cursor. */
  function handleDragEnd() {
    if (!isDragging) return;

    isDragging = false;
    document.body.style.cursor = '';
    document.removeEventListener('pointermove', handleDragMove);
    document.removeEventListener('pointerup', handleDragEnd);
    document.removeEventListener('pointercancel', handleDragEnd);
  }

  /* --- Resize --- */

  /* Captures the initial window geometry and attaches resize listeners.
     Direction determines which edges the user is pulling (e.g. "nw", "se").
     Uses Pointer Events (not mouse events) so the resize handles — especially
     the enlarged SE grip — work with touch and pen input, not just a mouse. */
  function handleResizeStart(e: PointerEvent, direction: string) {
    if (win.isMaximized) return;

    const target = e.currentTarget as HTMLElement;
    if (!target.classList.contains('hv-resize-handle')) return;

    e.preventDefault();
    e.stopPropagation();

    isResizing = true;
    resizeDirection = direction;
    onFocus();
    // A deliberate manual resize permanently opts this window out of the
    // content-fit auto-grow below — it must never fight a size the user
    // chose themselves.
    windowManager.markUserResized(win.id);

    resizeStartX = e.clientX;
    resizeStartY = e.clientY;
    windowStartWidth = win.width;
    windowStartHeight = win.height;
    windowStartLeft = win.x ?? 0;
    windowStartTop = win.y ?? 0;

    document.addEventListener('pointermove', handleResizeMove, { passive: false });
    document.addEventListener('pointerup', handleResizeEnd);
    document.addEventListener('pointercancel', handleResizeEnd);
    document.body.style.cursor = getCursorForDirection(direction);
  }

  /* Maps a compass direction to the appropriate CSS resize cursor. */
  function getCursorForDirection(dir: string): string {
    const cursors: Record<string, string> = {
      'n': 'ns-resize', 's': 'ns-resize',
      'e': 'ew-resize', 'w': 'ew-resize',
      'ne': 'nesw-resize', 'sw': 'nesw-resize',
      'nw': 'nwse-resize', 'se': 'nwse-resize'
    };
    return cursors[dir] || 'default';
  }

  /* Computes new width / height from the mouse delta, adjusting the window's
     origin when the user pulls a left or top edge. */
  function handleResizeMove(e: PointerEvent) {
    if (!isResizing) return;
    e.preventDefault();

    const deltaX = e.clientX - resizeStartX;
    const deltaY = e.clientY - resizeStartY;

    let newWidth = windowStartWidth;
    let newHeight = windowStartHeight;
    let newX = windowStartLeft;
    let newY = windowStartTop;

    const isNorth = resizeDirection.includes('n');
    const isSouth = resizeDirection.includes('s');
    const isEast = resizeDirection.includes('e');
    const isWest = resizeDirection.includes('w');

    if (isEast) {
      newWidth = Math.max(MIN_WIDTH, windowStartWidth + deltaX);
    }
    if (isWest) {
      newWidth = Math.max(MIN_WIDTH, windowStartWidth - deltaX);
      newX = windowStartLeft + (windowStartWidth - newWidth);
    }
    if (isSouth) {
      newHeight = Math.max(MIN_HEIGHT, windowStartHeight + deltaY);
    }
    if (isNorth) {
      newHeight = Math.max(MIN_HEIGHT, windowStartHeight - deltaY);
      newY = windowStartTop + (windowStartHeight - newHeight);
    }

    onUpdateSize(newWidth, newHeight);
    onUpdatePosition(newX, newY);
  }

  /* Cleans up resize listeners. */
  function handleResizeEnd() {
    if (!isResizing) return;

    isResizing = false;
    resizeDirection = '';
    document.body.style.cursor = '';
    document.removeEventListener('pointermove', handleResizeMove);
    document.removeEventListener('pointerup', handleResizeEnd);
    document.removeEventListener('pointercancel', handleResizeEnd);
  }

  /* Double-clicking the title bar toggles maximized / restored. */
  function handleDoubleClick() {
    onMaximize();
  }

  /* --- Content-fit auto-grow ---
     Opens a window at the usual default geometry, then — while the user
     hasn't manually resized it and it isn't maximized — grows it (never
     shrinks) to eliminate any scrollbar its own content is forced into,
     clamped to whatever viewport room is actually available from its
     current x/y. Polls rather than relying on ResizeObserver because the
     thing that needs measuring is *scroll* overflow (content bigger than
     the box), which doesn't fire a ResizeObserver callback on the
     never-changing clipped container — and polling works uniformly across
     every app page's own internal layout (fill-height flex, natural
     content height, async-loaded rows, ...) without assuming anything
     about how any specific one is built. */
  let contentEl: HTMLDivElement;
  let autoFitTimer: ReturnType<typeof setInterval> | null = null;
  let autoFitElapsedMs = 0;
  const AUTO_FIT_DURATION_MS = 5000;
  const AUTO_FIT_TICK_MS = 250;

  function checkContentOverflow() {
    if (!contentEl || win.isMaximized || win.userResized) return;

    // Almost every app page's own root sets `height: 100%; overflow-y:
    // auto` on itself (a deliberate, established pattern in this
    // codebase — see e.g. Settings/Dashboard/Restic/Storage/Network's
    // `.*-page` rules), which makes it establish its own scroll
    // boundary sized to exactly fill `.hv-window-content`. That means
    // `.hv-window-content` itself never overflows no matter how tall
    // the real content is — the actual overflow always shows up one
    // level down, on that inner page-root element's own scrollWidth/
    // scrollHeight vs. its clientWidth/clientHeight. Measure that
    // slotted child directly; fall back to the wrapper itself for the
    // rare page with no single root element.
    const measureEl = (contentEl.firstElementChild as HTMLElement) ?? contentEl;

    const overflowX = measureEl.scrollWidth - measureEl.clientWidth;
    const overflowY = measureEl.scrollHeight - measureEl.clientHeight;
    if (overflowX <= 0 && overflowY <= 0) return;

    const vw = typeof window !== 'undefined' ? window.innerWidth : win.width;
    const vh = typeof window !== 'undefined' ? window.innerHeight : win.height;
    const margin = DEFAULT_SNAP_CONFIG.edgeMargin;
    const availableWidth = vw - (win.x ?? 0) - margin;
    const availableHeight = vh - (win.y ?? 0) - DEFAULT_SNAP_CONFIG.dockBottom - DEFAULT_SNAP_CONFIG.dockHeight - margin;

    const newWidth = Math.max(win.width, Math.min(availableWidth, win.width + Math.max(0, overflowX)));
    const newHeight = Math.max(win.height, Math.min(availableHeight, win.height + Math.max(0, overflowY)));

    if (newWidth > win.width || newHeight > win.height) {
      onUpdateSize(newWidth, newHeight);
    }
  }

  onMount(() => {
    autoFitTimer = setInterval(() => {
      autoFitElapsedMs += AUTO_FIT_TICK_MS;
      if (win.userResized || win.isMaximized || autoFitElapsedMs >= AUTO_FIT_DURATION_MS) {
        if (autoFitTimer) clearInterval(autoFitTimer);
        autoFitTimer = null;
        return;
      }
      checkContentOverflow();
    }, AUTO_FIT_TICK_MS);
  });

  onDestroy(() => {
    if (autoFitTimer) clearInterval(autoFitTimer);
  });

  /* Any mousedown on the window surface brings it to the front. */
  function handleWindowClick() {
    onFocus();
  }

  /* --- Render-error boundary (UI audit W10) ---
     The route content is wrapped in a <svelte:boundary> below, so one
     throwing widget degrades to a readable fallback inside the window
     instead of blanking the whole desktop. E2E hook: opening the app with
     `?hv-render-error` arms a probe component that throws on its first
     render, letting tests/e2e/desktop assert the fallback + the retry path
     without shipping a deliberately broken screen; the Retry action disarms
     it, so the real content comes back. */
  let renderErrorProbe =
    typeof window !== 'undefined' && new URLSearchParams(window.location.search).has('hv-render-error');

  /* Plays a brief shrink animation before telling the window manager to hide. */
  function handleMinimize() {
    isMinimizing = true;
    setTimeout(() => {
      onMinimize();
      isMinimizing = false;
    }, 220);
  }

  /* Computes inline styles for the absolute-positioned window frame.
     When maximized on a roomy viewport (>900px) the window takes the ENTIRE
     screen — the topbar pill and dock auto-hide (Desktop.svelte's
     chromeAutoHideEligible), so nothing floats over it; each peeks back on
     an edge hover. On <=900px that chrome stays put (edge-reveal is fiddly
     with touch), so a maximized window is inset below the topbar and above
     the dock instead, or its header/footer would render behind them. */
  /* Every window uses the same paper chrome: solid surface, the min / max /
     close trio inline in the header bar, no separate title/footer bars. Every
     app ships its own in-content header — the single standardized
     <WindowHeader> component (icon tile + title, optional tab row /
     action button / extra cluster, [data-hv-drag]). WindowFrame must NOT also
     stack its own <WindowHeader> on top for these apps, or the app shows a
     blank duplicate header (the extra one's `win.icon` sprite name has no
     glyph). The set is kept explicit so a future app that genuinely wants the
     frame-provided header can opt back in by being left out of it.
     `quotas` was added on 2026-09-14 and missed here, so it shipped a stacked
     duplicate header until the E2E `14.9` guard (one header per window, across
     the registry) caught it — add every new app id to this set. */
  const OWN_HEADER_APPS = new Set([
    'terminal', 'chat', 'docker', 'dashboard', 'users', 'groups', 'shares', 'account', 'settings',
    'explorer', 'backup', 'health', 'network', 'vpn', 'rescue', 'downloader', 'rclone',
    'storage', 'raid', 'journal', 'syncthing', 'widgets', 'virtual-machines', 'quotas',
  ]);
  $: ownHeader = OWN_HEADER_APPS.has(win.appId);

  $: windowStyle = (() => {
    if (win.isMaximized) {
      // `.hv-window` is position: fixed, so these are viewport-relative.
      if (viewportWidth <= 600) {
        // Phone: truly fullscreen. Desktop.svelte hides the topbar + dock
        // while a window is open (phoneChromeHidden), so the app owns the
        // whole viewport — only the OS safe areas (notch / home indicator)
        // are kept clear.
        return 'left: 0; width: 100vw;'
          + ' top: env(safe-area-inset-top, 0px);'
          + ' height: calc(100dvh - env(safe-area-inset-top, 0px) - env(safe-area-inset-bottom, 0px));';
      }
      if (viewportWidth <= 900) {
        // Tablet: topbar + dock stay visible — keep clear of both.
        return 'left: 0; width: 100vw;'
          + ' top: var(--hv-chrome-topbar-keepout);'
          + ' height: calc(100vh - var(--hv-chrome-topbar-keepout) - var(--hv-chrome-dock-keepout));';
      }
      // Roomy viewport: truly fullscreen, the chrome hides itself.
      return 'left: 0; top: 0; width: 100vw; height: 100vh;';
    }
    return `left: ${win.x ?? 0}px; top: ${win.y ?? 0}px; width: ${win.width}px; height: ${win.height}px;`;
  })();
</script>

<svelte:window bind:innerWidth={viewportWidth} on:pointerup={() => { handleDragEnd(); handleResizeEnd(); }} on:pointercancel={() => { handleDragEnd(); handleResizeEnd(); }} />

{#if !win.isMinimized}
  <div
    class="hv-window"
    class:maximized={win.isMaximized}
    class:dragging={isDragging}
    class:resizing={isResizing}
    class:minimizing={isMinimizing}
    style="{windowStyle} z-index: {win.zIndex};"
    onpointerdown={(e) => { handleWindowClick(); handleDragStart(e); }}
    ondblclick={(e) => {
      if ((e.target as HTMLElement).closest('[data-hv-drag]')
        && !(e.target as HTMLElement).closest('button, a, input, [role="button"]')) handleDoubleClick();
    }}
    role="dialog"
    tabindex="-1"
    aria-label={win.title}
  >
    <!-- The min / max / close trio sits IN the header bar, top-right, on the
         mock's 8px / 12px insets — not floating over it. Each app's own
         <WindowHeader> reserves the matching --hv-winhdr-btn-clear on its
         right edge. Amber = minimize, blue = maximize/restore, red = close;
         the three 14px glyphs are the mock's own paths (Figma "AppWindow"). -->
    <div class="hv-window-controls">
      <button class="hv-cl-btn hv-cl-btn-min" data-testid="win-minimize" onclick={handleMinimize} title={$t('desktop.window_minimize')} aria-label={$t('desktop.window_minimize')}>
        <svg width="14" height="14" viewBox="0 0 14 14" fill="none" aria-hidden="true"><path d="M2.9165 6.99994H11.0831" stroke="currentColor" stroke-width="1.45832" stroke-linecap="round"/></svg>
      </button>
      <button class="hv-cl-btn hv-cl-btn-max" data-testid="win-maximize" onclick={onMaximize} title={win.isMaximized ? $t('desktop.window_restore') : $t('desktop.window_maximize')} aria-label={win.isMaximized ? $t('desktop.window_restore') : $t('desktop.window_maximize')}>
        <svg width="14" height="14" viewBox="0 0 14 14" fill="none" aria-hidden="true"><path d="M4.66665 1.75H2.91666C2.60724 1.75 2.3105 1.87292 2.09171 2.09171C1.87292 2.3105 1.75 2.60724 1.75 2.91666V4.66665M12.2499 4.66665V2.91666C12.2499 2.60724 12.127 2.3105 11.9082 2.09171C11.6894 1.87292 11.3927 1.75 11.0833 1.75H9.33328M1.75 9.33328V11.0833C1.75 11.3927 1.87292 11.6894 2.09171 11.9082C2.3105 12.127 2.60724 12.2499 2.91666 12.2499H4.66665M9.33328 12.2499H11.0833C11.3927 12.2499 11.6894 12.127 11.9082 11.9082C12.127 11.6894 12.2499 11.3927 12.2499 11.0833V9.33328" stroke="currentColor" stroke-width="1.16666" stroke-linecap="round" stroke-linejoin="round"/></svg>
      </button>
      <button class="hv-cl-btn hv-cl-btn-close" data-testid="win-close" onclick={onClose} title={$t('common.close')} aria-label={$t('common.close')}>
        <svg width="14" height="14" viewBox="0 0 14 14" fill="none" aria-hidden="true"><path d="M10.4999 3.5L3.5 10.4999M3.5 3.5L10.4999 10.4999" stroke="currentColor" stroke-width="1.45832" stroke-linecap="round"/></svg>
      </button>
    </div>

    {#if !ownHeader}
      <WindowHeader iconName={win.icon} title={win.title} />
    {/if}

    <div class="hv-window-content" bind:this={contentEl}>
      <svelte:boundary>
        {#if renderErrorProbe}
          <RenderErrorProbe />
        {/if}
        <slot />
        {#snippet failed(error, reset)}
          <!-- W10: the window chrome (header, min/max/close, other windows)
               stays alive; only the route body degrades. `role="alert"`
               announces the failure to assistive tech. -->
          <div class="hv-render-error" data-testid="hv-render-error" role="alert">
            <p class="hv-render-error-msg">{$t('common.render_error')}</p>
            <button
              class="hv-btn hv-btn-primary hv-btn-sm"
              data-testid="hv-render-retry"
              onclick={() => { renderErrorProbe = false; reset(); }}
            >{$t('common.render_retry')}</button>
          </div>
        {/snippet}
      </svelte:boundary>
    </div>

    {#if !win.isMaximized}
      <!-- Invisible SE corner handle (no visible footer grip in the paper
           design); the 7 edge/corner strips below keep the window resizable. -->
      <div class="hv-resize-handle hv-resize-se-corner" role="presentation" onpointerdown={(e) => handleResizeStart(e, 'se')}></div>

      <div class="hv-resize-handle hv-resize-n" role="presentation" onpointerdown={(e) => handleResizeStart(e, 'n')}></div>
      <div class="hv-resize-handle hv-resize-s" role="presentation" onpointerdown={(e) => handleResizeStart(e, 's')}></div>
      <div class="hv-resize-handle hv-resize-e" role="presentation" onpointerdown={(e) => handleResizeStart(e, 'e')}></div>
      <div class="hv-resize-handle hv-resize-w" role="presentation" onpointerdown={(e) => handleResizeStart(e, 'w')}></div>
      <div class="hv-resize-handle hv-resize-ne" role="presentation" onpointerdown={(e) => handleResizeStart(e, 'ne')}></div>
      <div class="hv-resize-handle hv-resize-nw" role="presentation" onpointerdown={(e) => handleResizeStart(e, 'nw')}></div>
      <div class="hv-resize-handle hv-resize-sw" role="presentation" onpointerdown={(e) => handleResizeStart(e, 'sw')}></div>
    {/if}
  </div>
{/if}

<style>
  .hv-window {
    /* Fixed (not absolute) so left/top are viewport-relative, matching the
       coordinate system applySnap() and the desktop's resource-monitor
       widgets use — .hv-window otherwise sits inside <main class="hv-desktop">,
       a flex child offset below the topbar, which would double-count that
       offset against the snap boundaries computed in WindowFrame's drag logic. */
    position: fixed;
    display: flex;
    flex-direction: column;
    /* Opaque paper surface, a slate-300 hairline and one soft two-layer
       shadow — the AppWindow mock's exact frame. The frame owns the surface
       itself now (it used to stay transparent and let the header + the app's
       own content paint it), so a window is legible even where an app page
       leaves a gap. Radius is the --hv-window-radius window stop (16px). */
    background: var(--hv-window-bg);
    border: 1px solid var(--hv-window-border);
    box-sizing: border-box;
    border-radius: var(--hv-window-radius);
    box-shadow: var(--hv-window-shadow);
    overflow: hidden;
    will-change: left, top;
    transform: translate3d(0, 0, 0);
    /* Plays once when a window's DOM node is first created (a fresh open, or
       a restore from minimized — the {#if !win.isMinimized} block re-mounts
       the node either way) — transform/opacity only, and it doesn't replay
       on drag/resize/focus since those only touch the inline style/other
       classes, never this element's animation-name. */
    animation: hv-window-open 180ms var(--hv-ease-out);
  }

  @keyframes hv-window-open {
    from {
      opacity: 0;
      transform: scale(0.96) translateY(8px);
    }
    to {
      opacity: 1;
      transform: scale(1) translateY(0);
    }
  }

  .hv-window.dragging {
    cursor: grabbing;
  }

  .hv-window.minimizing {
    animation: hv-minimize 220ms cubic-bezier(0.4, 0, 0.6, 1) forwards;
    transform-origin: 50% calc(100% - 68px);
  }

  @keyframes hv-minimize {
    0% {
      opacity: 1;
      transform: scale(1) translateY(0);
    }
    100% {
      opacity: 0;
      transform: scale(0.2) translateY(40vh);
    }
  }

  .hv-window.resizing {
    transition: none;
    user-select: none;
  }

  .hv-window.maximized {
    border-radius: 0;
  }

  /* Tablet (601–900px): a maximized window is inset between the topbar and
     the dock (see windowStyle), so it reads better with rounded corners.
     A phone window is edge-to-edge fullscreen, so it keeps square corners. */
  @media (min-width: 601px) and (max-width: 900px) {
    .hv-window.maximized {
      border-radius: var(--hv-window-radius);
    }
  }

  .hv-window-controls {
    position: absolute;
    /* Absolute inside .hv-window (position: fixed), so `top` is measured from
       the window's own top edge = the header bar's top edge. Derived from the
       two tokens rather than a literal 8px so the trio stays dead centre of the
       header bar if either the bar's height or the control size changes — and
       so it centres on the SAME axis as the app's icon tile and title (both
       centred on the bar's one 28px control row). */
    top: calc((var(--hv-winhdr-h) - var(--hv-winctrl-size)) / 2);
    right: var(--hv-space-3);
    z-index: 30;
    display: flex;
    align-items: center;
    gap: var(--hv-winctrl-gap);
  }

  .hv-cl-btn {
    /* The button IS the mock's 28px rounded square — no inner ::before and
       no shadow, exactly as drawn. */
    position: relative;
    display: grid;
    place-items: center;
    width: var(--hv-winctrl-size);
    height: var(--hv-winctrl-size);
    border: none;
    border-radius: var(--hv-winctrl-radius);
    background: var(--hv-winctrl-max);
    color: var(--hv-on-primary);
    cursor: pointer;
    transition: background 150ms ease, transform 150ms ease;
  }

  /* Transparent 34px hit pad — 3px a side is the most the 6px gap allows
     before two neighbours' targets would overlap. Keeps a usable touch
     target without changing the 28px square the mock specifies. */
  .hv-cl-btn::after {
    content: '';
    position: absolute;
    inset: -3px;
    border-radius: calc(var(--hv-winctrl-radius) + 3px);
  }

  .hv-cl-btn:active {
    transform: scale(0.94);
  }

  /* Control colours — amber (minimize) / blue (maximize) / red (close). */
  .hv-cl-btn-min {
    background: var(--hv-winctrl-min);
  }

  .hv-cl-btn-min:hover {
    background: var(--hv-winctrl-min-hover);
  }

  .hv-cl-btn-max:hover {
    background: var(--hv-winctrl-max-hover);
  }

  .hv-cl-btn-close {
    background: var(--hv-winctrl-close);
  }

  .hv-cl-btn-close:hover {
    background: var(--hv-winctrl-close-hover);
  }

  .hv-resize-se-corner {
    position: absolute;
    right: 0;
    bottom: 0;
    width: 18px;
    height: 18px;
    cursor: nwse-resize;
  }

  /* Phone: the trio keeps the same 28px square — three of them plus gaps and
     the 12px inset are 108px, which fits a 360px viewport with room for the
     title. Only the reserve shrinks (--hv-winhdr-btn-clear-sm). */

  .hv-window-content {
    flex: 1;
    overflow: auto;
    background: transparent;
    /* Keep a scroll gesture (and its overscroll bounce / pull-to-refresh)
       inside the window instead of chaining to the desktop behind it. */
    overscroll-behavior: contain;
    -webkit-overflow-scrolling: touch;
  }

  /* W10 boundary fallback — centred in the window body so the chrome keeps
     reading as a normal window while the route body is degraded. */
  .hv-render-error {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--hv-space-3);
    min-height: 100%;
    padding: var(--hv-space-5);
    text-align: center;
  }

  .hv-render-error-msg {
    margin: 0;
    color: var(--hv-text-secondary);
    font-size: var(--hv-font-size-base);
  }

  .hv-resize-handle {
    position: absolute;
    z-index: 10;
    /* Without this, touch browsers try to scroll/zoom the page instead of
       letting pointerdown/pointermove drive the resize. */
    touch-action: none;
  }

  .hv-resize-n {
    top: 0;
    left: 8px;
    right: 8px;
    height: 4px;
    cursor: ns-resize;
  }

  .hv-resize-s {
    bottom: 0;
    left: 8px;
    right: 8px;
    height: 4px;
    cursor: ns-resize;
  }

  .hv-resize-e {
    right: 0;
    top: 8px;
    bottom: 8px;
    width: 4px;
    cursor: ew-resize;
  }

  .hv-resize-w {
    left: 0;
    top: 8px;
    bottom: 8px;
    width: 4px;
    cursor: ew-resize;
  }

  .hv-resize-ne {
    top: 0;
    right: 0;
    width: 8px;
    height: 8px;
    cursor: nesw-resize;
  }

  .hv-resize-nw {
    top: 0;
    left: 0;
    width: 8px;
    height: 8px;
    cursor: nwse-resize;
  }

  .hv-resize-sw {
    bottom: 0;
    left: 0;
    width: 8px;
    height: 8px;
    cursor: nesw-resize;
  }
</style>
