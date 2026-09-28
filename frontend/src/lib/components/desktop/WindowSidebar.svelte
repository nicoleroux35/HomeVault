<script lang="ts">
  import { setContext, type Snippet } from 'svelte';
  import { t } from '$lib/stores/i18n';
  import { cn } from '$lib/utils.js';

  interface Props {
    /** Rail width when expanded. Defaults to the standard token. */
    width?: string;
    /** Rail width when collapsed to an icon rail. Defaults to the icon token. */
    collapsedWidth?: string;
    /** Accessible name of the rail (e.g. the app title). */
    label: string;
    /** The nav groups / section content. */
    children: Snippet;
    /** Optional rail footer (e.g. a "+" button, a summary). */
    footer?: Snippet;
    /** Hide the rail entirely (e.g. on a collapsed container). */
    hidden?: boolean;
    /** Opt into the icon-rail collapse: when `collapsed` the rail narrows to
     *  `collapsedWidth` and `WindowSidebarItem`/`WindowSidebarSection`
     *  children render icon-only. Omit for rails whose content can't shrink
     *  to icons (e.g. Docker's metrics cards). */
    collapseable?: boolean;
    /** Whether the rail is collapsed (bindable). Only meaningful when
     *  `collapseable`. */
    collapsed?: boolean;
    /** Called when the collapse / expand chevron is pressed. */
    onToggle?: () => void;
    /** Extra classes merged onto the aside — surface colour, border weight
     *  and the body's `--winsidebar-pad` override all travel this way, so a
     *  page can re-dress the rail without a prop per property. */
    class?: string;
    /** Test hook rendered on the aside. */
    'data-testid'?: string;
  }

  let {
    width,
    collapsedWidth,
    label,
    children,
    footer,
    hidden = false,
    collapseable = false,
    collapsed = $bindable(false),
    onToggle,
    class: klass = '',
    'data-testid': testId,
  }: Props = $props();

  /* Published to every nav item / section rendered in the rail so they can
     switch to their icon-only collapsed rendering (Aurora-style rail). The
     context value is a $state object (not the bare prop) so a later toggle
     re-renders the children — a primitive captured by setContext would not. */
  const railCtx = $state<{ collapsed: boolean }>({ collapsed });
  setContext('hvSidebarCollapsed', railCtx);
  $effect(() => {
    railCtx.collapsed = collapsed;
  });

  /* The chevron flips the rail itself (the bindable `collapsed` prop keeps
     the parent in sync) and fires `onToggle` as a notification hook for
     side effects (persistence, extra work). A parent that wants full
     control can ignore the notification and drive `collapsed` directly. */
  function handleToggle() {
    collapsed = !collapsed;
    onToggle?.();
  }
</script>

<!--
  WindowSidebar — the standard left rail of the in-window app anatomy.
  Fill background + right hairline, 44px-min nav items belong to the app
  (rendered inside the default snippet — use the shared `WindowSidebarItem`
  / `WindowSidebarSection` for a standardized rail). When `collapseable` a
  chevron toggle narrows the rail to an icon-only strip instead of hiding
  it entirely; the collapse state is published as the `hvSidebarCollapsed`
  context so items and section titles adapt. Collapses away entirely on
  narrow containers via the window's own container query (see usage docs).
-->
{#if !hidden}
  <aside
    class={cn('hv-winsidebar', klass)}
    class:hv-winsidebar--collapsed={collapseable && collapsed}
    style="--winsidebar-w: {(collapseable && collapsed)
      ? (collapsedWidth ?? 'var(--hv-window-sidebar-w-icon, 64px)')
      : (width ?? 'var(--hv-window-sidebar-w, 256px)')}"
    aria-label={label}
    data-testid={testId}
  >
    {#if collapseable}
      <div class="hv-winsidebar-toggle">
        <button
          class="hv-winsidebar-toggle-btn"
          onclick={handleToggle}
          title={collapsed ? $t('desktop.sidebar_expand') : $t('desktop.sidebar_collapse')}
          aria-label={collapsed ? $t('desktop.sidebar_expand') : $t('desktop.sidebar_collapse')}
        >
          {#if collapsed}
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M9 6l6 6-6 6"/></svg>
          {:else}
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M15 6l-6 6 6 6"/></svg>
          {/if}
        </button>
      </div>
    {/if}

    <div class="hv-winsidebar-body">
      {@render children()}
    </div>
    {#if footer}
      <div class="hv-winsidebar-footer">
        {@render footer()}
      </div>
    {/if}
  </aside>
{/if}

<style>
  .hv-winsidebar {
    width: var(--winsidebar-w);
    flex: none;
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--hv-shell-fill);
    border-right: 1px solid var(--hv-shell-border);
    overflow: hidden;
    transition: width var(--hv-chrome-dur, 180ms) ease;
  }

  .hv-winsidebar-toggle {
    flex: none;
    display: flex;
    align-items: center;
    justify-content: flex-end;
    padding: 4px 6px;
    border-bottom: 1px solid var(--hv-shell-border-soft);
  }

  .hv-winsidebar--collapsed .hv-winsidebar-toggle {
    justify-content: center;
    padding: 4px 0;
  }

  .hv-winsidebar-toggle-btn {
    display: grid;
    place-items: center;
    width: 36px;
    height: 36px;
    border: 1px solid var(--hv-shell-border);
    border-radius: var(--hv-radius-default);
    background: var(--hv-shell-glass);
    color: var(--hv-shell-text-2);
    cursor: pointer;
    transition: background var(--hv-chrome-dur-fast, 150ms) ease,
                color var(--hv-chrome-dur-fast, 150ms) ease;
  }

  .hv-winsidebar-toggle-btn:hover {
    background: var(--hv-subtle-hover);
    color: var(--hv-shell-text-strong);
  }

  .hv-winsidebar-toggle-btn:focus-visible {
    outline: none;
    box-shadow: var(--hv-focus-ring);
  }

  .hv-winsidebar-body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
    padding: var(--winsidebar-pad, 16px 12px);
  }

  .hv-winsidebar--collapsed .hv-winsidebar-body {
    padding: var(--winsidebar-pad, 12px 6px);
  }

  .hv-winsidebar-footer {
    flex: none;
    padding: calc(var(--spacing) * 3);
    border-top: 1px solid var(--hv-shell-border-soft);
  }

  /* Shared nav-item shape — 44px touch rows, appliance 8px radius.
     Hover is the exact appliance subtle-hover slot. */
  .hv-winsidebar-body :global(.hv-winsidebar-item) {
    display: flex;
    align-items: center;
    gap: calc(var(--spacing) * 2.5);
    width: 100%;
    min-height: 44px;
    padding: 0 calc(var(--spacing) * 3);
    border: none;
    border-radius: var(--hv-radius-default);
    background: transparent;
    color: var(--hv-shell-text-2);
    font-size: var(--hv-text-md, 14px);
    font-weight: var(--hv-font-weight-medium);
    text-align: left;
    cursor: pointer;
    transition: background var(--hv-chrome-dur-fast, 150ms) ease,
                color var(--hv-chrome-dur-fast, 150ms) ease;
  }

  .hv-winsidebar-body :global(.hv-winsidebar-item:hover) {
    background: var(--hv-subtle-hover);
    color: var(--hv-shell-text-strong);
  }

  .hv-winsidebar-body :global(.hv-winsidebar-item.is-active) {
    background: var(--hv-primary-selected-bg);
    color: var(--hv-shell-text-strong);
  }

  .hv-winsidebar-body :global(.hv-winsidebar-item:focus-visible) {
    outline: none;
    box-shadow: var(--hv-focus-ring);
  }

  /* Icon-rail mode: center the icon, drop the row padding. */
  .hv-winsidebar--collapsed .hv-winsidebar-body :global(.hv-winsidebar-item) {
    justify-content: center;
    padding: 0;
  }

  .hv-winsidebar-body :global(.hv-winsidebar-title) {
    margin: 16px 12px 6px;
    font-size: var(--hv-text-xs, 11px);
    font-weight: var(--hv-font-weight-semibold);
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--hv-shell-text-3);
  }

  /* Touch: no hover state on the rail. */
  @media (hover: none) {
    .hv-winsidebar-body :global(.hv-winsidebar-item:hover) {
      background: transparent;
      color: var(--hv-shell-text-2);
    }
    .hv-winsidebar-body :global(.hv-winsidebar-item.is-active:hover) {
      background: var(--hv-primary-selected-bg);
      color: var(--hv-shell-text-strong);
    }
  }

  /* ---- Rich nav item (opt-in via `variant="rich"`) --------------------
     A two-line entry with a coloured icon tile, used by Settings' category
     rail. Deliberately declared LAST so it wins the specificity tie with
     the `@media (hover: none)` block above (same selector weight, later
     source order). Consumers that don't pass `variant="rich"` are
     unaffected — every rule below is scoped to `.is-rich`. */
  .hv-winsidebar-body :global(.hv-winsidebar-item.is-rich) {
    min-height: 0;
    padding: calc(var(--spacing) * 2.5) calc(var(--spacing) * 3);
    gap: calc(var(--spacing) * 3);
    border-radius: var(--hv-radius-xl, 12px);
    font-size: var(--hv-text-sm, 12px);
    font-weight: var(--hv-font-weight-semibold);
    line-height: 16px;
    color: var(--hv-settings-row);
  }

  .hv-winsidebar-body :global(.hv-winsidebar-item.is-rich:hover) {
    background: var(--hv-settings-hairline);
    color: var(--hv-settings-title-strong);
  }

  .hv-winsidebar-body :global(.hv-winsidebar-item.is-rich.is-active) {
    background: var(--hv-settings-hairline);
    color: var(--hv-settings-title-strong);
  }

  /* The trailing 4x20 accent bar — a pseudo-element so the markup stays a
     plain label + controls and the bar is pure decoration (never a hit
     target, never read out). */
  .hv-winsidebar-body :global(.hv-winsidebar-item.is-rich.is-active.has-accent-bar::after) {
    content: '';
    flex: none;
    width: 4px;
    height: 20px;
    margin-left: auto;
    border-radius: var(--hv-radius-full);
    background: var(--hv-row-accent, transparent);
  }
</style>