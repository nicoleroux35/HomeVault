<script lang="ts">
  /**
   * HOMEVAULT DESIGN SYSTEM — PageLayout (page composite, `app/` layer)
   *
   * The one standard anatomy for every in-window app screen (2026-09-12):
   *   WindowHeader (flush, flex-shrink:0)
   *   optional: WindowSidebar rail (left) in a row with the content column
   *   content column: optional WindowToolbar → scrolling body → optional footer
   *
   * The page root is a non-scrolling flex column (height:100%; overflow:hidden);
   * the header sits flush at the top, and the `.*-body` child takes
   * flex:1; min-height:0; overflow-y:auto and carries the page padding
   * (--hv-window-content-padding). See docs/guides/design-system.md §4.1.
   *
   * Props are WindowHeader pass-throughs + the optional rails. The default
   * children snippet is the scrolling body content.
   */
  import type { Snippet } from 'svelte';
  import WindowHeader from '$lib/components/desktop/WindowHeader.svelte';
  import WindowToolbar from '$lib/components/desktop/WindowToolbar.svelte';
  import WindowSidebar from '$lib/components/desktop/WindowSidebar.svelte';
  import WindowFooter from '$lib/components/desktop/WindowFooter.svelte';
  import { cn } from '$lib/utils.js';

  interface Tab {
    id: string;
    label: string;
    icon?: any;
    count?: number;
  }

  interface Props {
    /** WindowHeader pass-throughs. */
    title?: string;
    iconName?: string;
    icon?: Snippet;
    accent?: string;
    center?: Snippet;
    actions?: Snippet;
    extra?: Snippet;
    actionIcon?: any;
    actionLabel?: string;
    actionDisabled?: boolean;
    onAction?: () => void;
    tabs?: Tab[];
    activeTab?: string;
    onTabChange?: (id: string) => void;
    tabsLabel?: string;

    /** Optional left rail (WindowSidebar). */
    sidebar?: Snippet;
    sidebarLabel?: string;
    sidebarWidth?: string;
    /** Extra classes forwarded to the WindowSidebar element (surface colour,
     *  border weight, a `[--winsidebar-pad:…]` override, …). */
    sidebarClass?: string;
    /** Test hook forwarded to the WindowSidebar element. */
    sidebarTestId?: string;
    /** Hide the left rail (e.g. a narrow window with the detail pane open). */
    sidebarHidden?: boolean;
    /** Opt into the rail's icon-rail collapse (see WindowSidebar). */
    sidebarCollapseable?: boolean;
    /** Collapsed state of the rail (bindable). */
    sidebarCollapsed?: boolean;
    /** Fired when the rail's collapse / expand chevron is pressed. */
    sidebarOnToggle?: () => void;
    /** Optional rail footer (e.g. a "+" button, a summary). */
    sidebarFooter?: Snippet;

    /** Optional action row under the header (WindowToolbar). */
    toolbarLeft?: Snippet;
    toolbarRight?: Snippet;

    /** Optional bottom bar (WindowFooter). mode="actions" | "status". */
    footer?: Snippet;
    footerMode?: 'actions' | 'status';

    /** Body wrapper class (e.g. extra layout on the scroll area). */
    bodyClass?: string;

    /** Test hook rendered on the page root. */
    'data-testid'?: string;

    /** The scrolling page content. */
    children: Snippet;
  }

  let {
    title,
    iconName,
    icon,
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
    sidebar,
    sidebarLabel,
    sidebarWidth,
    sidebarClass,
    sidebarTestId,
    sidebarHidden = false,
    sidebarCollapseable = false,
    sidebarCollapsed = $bindable(false),
    sidebarOnToggle,
    sidebarFooter,
    toolbarLeft,
    toolbarRight,
    footer,
    footerMode = 'actions',
    bodyClass = '',
    'data-testid': testId,
    children,
  }: Props = $props();

  const hasToolbar = $derived(!!toolbarLeft || !!toolbarRight);
  const hasSidebar = $derived(!!sidebar && !sidebarHidden);
</script>

<div class="hv-page h-full w-full overflow-hidden flex flex-col bg-background text-foreground" data-testid={testId}>
  <WindowHeader
    {title}
    {iconName}
    {icon}
    {accent}
    {center}
    {actions}
    {extra}
    actionIcon={ActionIcon}
    {actionLabel}
    {actionDisabled}
    {onAction}
    {tabs}
    bind:activeTab
    {onTabChange}
    {tabsLabel}
  />

  {#if hasSidebar}
    <div class="hv-page-mid flex flex-1 min-h-0">
      <WindowSidebar
        label={sidebarLabel ?? title ?? ''}
        width={sidebarWidth}
        class={sidebarClass}
        collapseable={sidebarCollapseable}
        bind:collapsed={sidebarCollapsed}
        onToggle={sidebarOnToggle}
        data-testid={sidebarTestId}
      >
        {#if sidebar}{@render sidebar()}{/if}
        {#snippet footer()}
          {#if sidebarFooter}{@render sidebarFooter()}{/if}
        {/snippet}
      </WindowSidebar>
      <div class="hv-page-col flex flex-col flex-1 min-w-0 min-h-0">
        {#if hasToolbar}
          <WindowToolbar>
            {#snippet left()}{#if toolbarLeft}{@render toolbarLeft()}{/if}{/snippet}
            {#snippet right()}{#if toolbarRight}{@render toolbarRight()}{/if}{/snippet}
          </WindowToolbar>
        {/if}
        <main class="@container {cn('hv-page-body flex-1 min-h-0 overflow-y-auto p-6', bodyClass)}">
          {@render children()}
        </main>
        {#if footer}
          <WindowFooter mode={footerMode}>{@render footer()}</WindowFooter>
        {/if}
      </div>
    </div>
  {:else}
    <div class="hv-page-col flex flex-col flex-1 min-h-0">
      {#if hasToolbar}
        <WindowToolbar>
          {#snippet left()}{#if toolbarLeft}{@render toolbarLeft()}{/if}{/snippet}
          {#snippet right()}{#if toolbarRight}{@render toolbarRight()}{/if}{/snippet}
        </WindowToolbar>
      {/if}
      <main class="@container {cn('hv-page-body flex-1 min-h-0 overflow-y-auto p-6', bodyClass)}">
        {@render children()}
      </main>
      {#if footer}
        <WindowFooter mode={footerMode}>{@render footer()}</WindowFooter>
      {/if}
    </div>
  {/if}
</div>