<script lang="ts">
  import { getContext, type Snippet } from 'svelte';

  const railCtx = getContext<{ collapsed: boolean } | undefined>('hvSidebarCollapsed');
  const collapsed = $derived(railCtx?.collapsed ?? false);

  interface Props {
    /** Section heading (uppercase micro-label). Omit or pass '' for an
     *  unlabelled group (e.g. a single item such as Trash). */
    label?: string;
    /** The nav items / content of the section. */
    children: Snippet;
    /** Extra class on the wrapping `<section>`. */
    class?: string;
  }

  let { label, children, class: klass = '' }: Props = $props();
</script>

<!--
  WindowSidebarSection — the standard grouped heading of the in-window rail.
  Renders the shared `.hv-winsidebar-title` micro-label above its children
  and hides it when the parent WindowSidebar is collapsed to its icon rail.
-->
<section class={klass ? `hv-winsidebar-section ${klass}` : 'hv-winsidebar-section'} aria-label={label ?? ''}>
  {#if !collapsed && label}
    <h2 class="hv-winsidebar-title">{label}</h2>
  {/if}
  {@render children()}
</section>

<style>
  .hv-winsidebar-section {
    display: block;
    margin-bottom: var(--hv-space-3);
  }

  .hv-winsidebar-section:last-child {
    margin-bottom: 0;
  }

  /* Icon-rail mode: drop the section spacing, keep items touching. */
  :global(.hv-winsidebar--collapsed) .hv-winsidebar-section {
    margin-bottom: 6px;
  }
</style>