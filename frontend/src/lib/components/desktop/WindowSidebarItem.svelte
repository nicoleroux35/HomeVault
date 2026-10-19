<!-- ========================================================================
     File          : WindowSidebarItem.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       One sidebar-rail entry: icon + label that collapses to an icon-only
       44px target with a tooltip when the rail is collapsed (reads the
       hvSidebarCollapsed context).

     Dependencies:
       - svelte (>= 5), theme/Icon, utils/cn

     Usage:
       Inside WindowSidebar.
     ======================================================================== -->
<script lang="ts">
  import { getContext, type Snippet } from 'svelte';
  import Icon from '$lib/components/theme/Icon.svelte';
  import { cn } from '$lib/utils.js';

  const railCtx = getContext<{ collapsed: boolean } | undefined>('hvSidebarCollapsed');
  const collapsed = $derived(railCtx?.collapsed ?? false);

  interface Props {
    /** Custom glyph (a tabler icon, an inline SVG, …). Preferred over
     *  `iconName`; rendered in both the expanded and collapsed states. */
    icon?: Snippet;
    /** Fallback sprite glyph (`<Icon name=…>`) when no custom snippet is given. */
    iconName?: string;
    /** Row label. Shown expanded; becomes the tooltip (unless `title`) when
     *  the rail is collapsed. */
    label: string;
    /** Opt into the two-line "rich" entry: a coloured icon tile plus an
     *  optional `subtitle`, as used by Settings' category rail. Default
     *  `'default'` keeps the historic single-line row byte-identical. */
    variant?: 'default' | 'rich';
    /** Second line, rendered under `label`. Only meaningful with
     *  `variant="rich"`; when absent the rich row is a single line. */
    subtitle?: string;
    /** Category colour driving the rich tile (tinted at rest, solid when
     *  active) and the trailing active bar. Any CSS colour — pass the
     *  `--hv-settings-cat-*` token so the rail stays on the palette. */
    accent?: string;
    /** Render the trailing accent bar on the active row. Default `true`;
     *  only the rich variant draws it. */
    accentBar?: boolean;
    /** Selected state — the shared `is-active` highlight. */
    active?: boolean;
    /** Optional count badge (e.g. Docker's per-tab container counts). */
    count?: number;
    /** Tooltip for the count badge. */
    countTitle?: string;
    /** Muted trailing text (e.g. a disk size) shown expanded only. */
    detail?: string;
    /** Tooltip override. Defaults to `label` on a collapsed rail. */
    title?: string;
    /** Test hook rendered on the button. */
    testId?: string;
    /** Extra class on the label span (e.g. legacy `.disk-name` test hooks). */
    labelClass?: string;
    /** Highlights the row as a drop target (e.g. Explorer's sidebar
     *  destinations accepting an OS file drag to upload there). */
    dropActive?: boolean;
    /** Optional trailing action rendered as a sibling of the row button
     *  (unpin ×, eject …) — hidden when collapsed. Kept outside the button
     *  so a click on it never triggers the row's own handler. */
    action?: Snippet;
    /** Merged with the shared `.hv-winsidebar-item` base classes. */
    class?: string;
    onclick?: (e: MouseEvent) => void;
    onkeydown?: (e: KeyboardEvent) => void;
    ondrop?: (e: DragEvent) => void;
    ondragover?: (e: DragEvent) => void;
    ondragleave?: (e: DragEvent) => void;
    /** Native button attrs (e.g. Docker's tab role / roving tabindex). */
    role?: string;
    ariaSelected?: boolean;
    tabindex?: number;
    disabled?: boolean;
    'data-testid'?: string;
  }

  let {
    icon,
    iconName,
    label,
    variant = 'default',
    subtitle,
    accent,
    accentBar = true,
    active = false,
    count,
    countTitle,
    detail,
    title,
    testId,
    labelClass,
    dropActive = false,
    action,
    class: klass = '',
    onclick,
    onkeydown,
    ondrop,
    ondragover,
    ondragleave,
    role,
    ariaSelected,
    tabindex,
    disabled = false,
    'data-testid': dataTestId,
  }: Props = $props();

  const tooltip = $derived(collapsed ? (title ?? label) : title);

  const isRich = $derived(variant === 'rich');
  /* The accent reaches the CSS as a custom property so the tile / bar rules
     can stay in the stylesheet instead of inline styles on every element. */
  const rowStyle = $derived(accent ? `--hv-row-accent: ${accent}` : '');
</script>

<!--
  WindowSidebarItem — the one standard nav row of the in-window rail.
  Full label + optional count / detail / trailing `action` when expanded;
  icon-only + tooltip when the parent WindowSidebar is collapsed to its
  icon rail (read via the `hvSidebarCollapsed` context). The base row shape
  comes from WindowSidebar's `.hv-winsidebar-item` styles; the optional
  action renders as a *sibling* of the row button (never a nested button).
-->
<div class="hv-winsidebar-item-row" class:is-collapsed={collapsed}>
  <button
    class={cn('hv-winsidebar-item', klass)}
    class:is-active={active}
    class:is-drop-active={dropActive}
    class:is-rich={isRich}
    class:has-accent-bar={accentBar}
    style={rowStyle}
    data-testid={testId ?? dataTestId}
    title={tooltip}
    {role}
    aria-selected={ariaSelected}
    aria-current={!role && active ? 'page' : undefined}
    {tabindex}
    {disabled}
    {onclick}
    {onkeydown}
    {ondrop}
    {ondragover}
    {ondragleave}
  >
    {#if icon}
      <span class="hv-winsidebar-item-icon">{@render icon()}</span>
    {:else if iconName}
      <span class="hv-winsidebar-item-icon"><Icon name={iconName} size={18} /></span>
    {/if}

    {#if !collapsed}
      {#if subtitle !== undefined && subtitle !== ''}
        <span class="hv-winsidebar-item-text">
          <span class={cn('hv-winsidebar-item-label', labelClass)}>{label}</span>
          <span class="hv-winsidebar-item-subtitle">{subtitle}</span>
        </span>
      {:else}
        <span class={cn('hv-winsidebar-item-label', labelClass)}>{label}</span>
      {/if}
      {#if detail !== undefined && detail !== ''}
        <span class="hv-winsidebar-item-detail">{detail}</span>
      {/if}
      {#if count !== undefined}
        <span class="hv-winsidebar-item-count" title={countTitle}>{count > 99 ? '99+' : count}</span>
      {/if}
    {/if}
  </button>

  {#if !collapsed && action}
    <span class="hv-winsidebar-item-action">
      {@render action()}
    </span>
  {/if}
</div>

<style>
  .hv-winsidebar-item-row {
    display: flex;
    align-items: center;
    gap: 2px;
    width: 100%;
  }

  /* The row button grows to fill the rail; width:100% would overflow the
     trailing action. */
  .hv-winsidebar-item-row .hv-winsidebar-item {
    flex: 1;
    min-width: 0;
    width: auto;
  }

  .hv-winsidebar-item-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    flex: none;
    color: inherit;
  }

  .hv-winsidebar-item-label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* ---- Rich variant (opt-in via `variant="rich"`) --------------------- */

  /* The icon slot becomes a tinted circle at `--hv-settings-icon-tile` (28px).
     `padding: 6px` around a 16px glyph is what the design specifies;
     `box-sizing: border-box` (base.css) makes the token the rendered size.
     The shape is the same circle as the dock tiles / topbar buttons / window
     header icon: this screen used to draw 12px and 18px corners on its tiles,
     so three different components read as one family (2026-10-19). The rich
     variant has exactly one consumer — the Settings category rail — which is
     why the token is the `--hv-settings-*` one and not a generic size. */
  .hv-winsidebar-item.is-rich .hv-winsidebar-item-icon {
    width: var(--hv-settings-icon-tile, 28px);
    height: var(--hv-settings-icon-tile, 28px);
    padding: 6px;
    border-radius: var(--hv-radius-full);
    background: color-mix(
      in srgb,
      var(--hv-row-accent, transparent) var(--hv-settings-cat-tint, 9%),
      transparent
    );
    color: var(--hv-row-accent, currentColor);
  }

  /* Active: the tile goes solid and the glyph flips to white. */
  .hv-winsidebar-item.is-rich.is-active .hv-winsidebar-item-icon {
    background: var(--hv-row-accent, currentColor);
    color: var(--hv-on-primary);
  }

  /* Two-line label block: title over subtitle, both elided. */
  .hv-winsidebar-item-text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    flex: 1;
    min-width: 0;
  }

  /* Inside the text block the label no longer grows on its own — the block
     owns the `flex: 1`. */
  .hv-winsidebar-item-text .hv-winsidebar-item-label {
    flex: none;
  }

  .hv-winsidebar-item-subtitle {
    font-size: var(--hv-text-xs, 11px);
    font-weight: var(--hv-font-weight-regular);
    line-height: var(--hv-line-height-tight, 1.2);
    color: var(--hv-settings-label);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .hv-winsidebar-item.is-rich.is-active .hv-winsidebar-item-subtitle {
    color: var(--hv-settings-row);
  }

  .hv-winsidebar-item-detail {
    flex: none;
    font-size: var(--hv-text-xs, 11px);
    color: var(--hv-shell-text-3);
  }

  .hv-winsidebar-item-count {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 18px;
    height: 18px;
    padding: 0 5px;
    border-radius: var(--hv-radius-full);
    background: var(--hv-shell-border-soft);
    color: var(--hv-shell-text-2);
    font-size: var(--hv-font-size-micro, 11px);
    font-weight: var(--hv-font-weight-bold);
  }

  .hv-winsidebar-item.is-active .hv-winsidebar-item-count {
    background: color-mix(in srgb, var(--hv-primary) 15%, transparent);
    color: var(--hv-primary);
  }

  /* Active drag-over target — an inset primary ring over the row. */
  .hv-winsidebar-item.is-drop-active {
    background: var(--hv-primary-light);
    box-shadow: inset 0 0 0 2px var(--hv-primary);
    color: var(--hv-shell-text-strong);
  }

  /* The trailing action is a small icon button (e.g. unpin, eject) — its
     own click never bubbles into the row button since it's a sibling. A
     transparent 44px hit area keeps the small visible 28px glyph while
     meeting --hv-touch-target-min (WCAG 2.5.8 / ISO 9241-410). */
  .hv-winsidebar-item-action :global(button) {
    position: relative;
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    flex: none;
    border: none;
    border-radius: var(--hv-radius-default);
    background: transparent;
    color: var(--hv-shell-text-2);
    cursor: pointer;
    transition: background var(--hv-chrome-dur-fast, 150ms) ease,
                color var(--hv-chrome-dur-fast, 150ms) ease;
  }

  .hv-winsidebar-item-action :global(button::after) {
    content: '';
    position: absolute;
    top: 50%;
    left: 50%;
    width: var(--hv-touch-target-min);
    height: var(--hv-touch-target-min);
    transform: translate(-50%, -50%);
    border-radius: inherit;
  }

  .hv-winsidebar-item-action :global(button:hover) {
    background: var(--hv-subtle-hover);
    color: var(--hv-shell-text-strong);
  }

  .hv-winsidebar-item-action :global(button.is-danger:hover) {
    background: var(--hv-error-bg);
    color: var(--hv-error);
  }
</style>