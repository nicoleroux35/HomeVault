<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    /** Left cluster: search, filters, breadcrumbs. */
    left?: Snippet;
    /** Right cluster: primary buttons, kebabs. */
    right?: Snippet;
    /** Hide the toolbar entirely. */
    hidden?: boolean;
  }

  let { left, right, hidden = false }: Props = $props();
</script>

<!--
  WindowToolbar — the standard action row under the WindowHeader.
  Fill background + bottom hairline; interactive children must keep
  the ≥44px touch target (style them with .hv-wintoolbar-btn or your
  own 44px-min control).
-->
{#if !hidden}
  <div class="hv-wintoolbar">
    <div class="hv-wintoolbar-left">
      {#if left}{@render left()}{/if}
    </div>
    <div class="hv-wintoolbar-right">
      {#if right}{@render right()}{/if}
    </div>
  </div>
{/if}

<style>
  .hv-wintoolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex: none;
    min-height: var(--hv-window-toolbar-h, 44px);
    padding: 6px 16px;
    background: var(--hv-shell-fill);
    border-bottom: 1px solid var(--hv-shell-border-soft);
  }

  .hv-wintoolbar-left {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    flex: 1;
  }

  .hv-wintoolbar-right {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: none;
  }

  /* Shared tool-button shape — 44px touch target, appliance 8px
     radius, card surface. Hover/pressed/disabled follow the exact appliance
     subtle + disabled slots (see tokens.css). */
  .hv-wintoolbar :global(.hv-wintool-btn) {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    min-height: 44px;
    min-width: 44px;
    padding: 0 12px;
    border: 1px solid var(--hv-shell-border);
    border-radius: var(--hv-radius-default);
    background: var(--hv-shell-glass);
    color: var(--hv-shell-text-2);
    font-size: var(--hv-text-md, 14px);
    font-weight: var(--hv-font-weight-medium);
    cursor: pointer;
    transition: background var(--hv-chrome-dur-fast, 150ms) ease,
                color var(--hv-chrome-dur-fast, 150ms) ease,
                border-color var(--hv-chrome-dur-fast, 150ms) ease;
  }

  .hv-wintoolbar :global(.hv-wintool-btn:hover) {
    background: var(--hv-subtle-hover);
    color: var(--hv-shell-text-strong);
    border-color: var(--hv-stroke-hover);
  }

  .hv-wintoolbar :global(.hv-wintool-btn.is-danger) {
    color: var(--hv-error);
  }

  .hv-wintoolbar :global(.hv-wintool-btn.is-danger:hover) {
    background: var(--hv-error-bg);
    border-color: var(--hv-error-border);
    color: var(--hv-error-text);
  }

  .hv-wintoolbar :global(.hv-wintool-btn:disabled) {
    background: var(--hv-shell-border-soft);
    border-color: transparent;
    color: var(--hv-disabled-fg);
    opacity: 1;
    cursor: not-allowed;
  }

  .hv-wintoolbar :global(.hv-wintool-btn:focus-visible) {
    outline: none;
    box-shadow: var(--hv-focus-ring);
  }

  /* Touch: no hover affordance. */
  @media (hover: none) {
    .hv-wintoolbar :global(.hv-wintool-btn:hover) {
      background: var(--hv-shell-glass);
      border-color: var(--hv-shell-border);
      color: var(--hv-shell-text-2);
    }
  }
</style>