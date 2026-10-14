<!-- ========================================================================
     File          : WindowFooter.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Standard window footer with two modes: a full-height action bar
       (right-aligned buttons for forms/editors) or a thin status strip
       (counts, live state, sync info).

     Dependencies:
       - svelte (>= 5)

     Usage:
       Optional footer slot of PageLayout.
     ======================================================================== -->
<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    /**
     * "actions" — a full-height bar with right-aligned buttons
     * (window-level forms / editors).
     * "status" — a thin state strip (counts, live state, sync info).
     */
    mode?: 'actions' | 'status';
    /** Content of the bar. */
    children: Snippet;
    /** Hide the footer entirely. */
    hidden?: boolean;
  }

  let { mode = 'actions', children, hidden = false }: Props = $props();
</script>

<!--
  WindowFooter — the standard bottom bar of the in-window anatomy.
  mode="actions": 56px bar, buttons right-aligned (children own their
  buttons — style primary/secondary with the shared button classes).
  mode="status": 32px strip of muted state text.
-->
{#if !hidden}
  <div class="hv-winfooter" class:hv-winfooter--status={mode === 'status'}>
    <div class="hv-winfooter-inner">
      {@render children()}
    </div>
  </div>
{/if}

<style>
  .hv-winfooter {
    flex: none;
    min-height: var(--hv-window-footer-h, 56px);
    display: flex;
    align-items: stretch;
    padding: var(--hv-space-2-5) var(--hv-space-4);
    background: var(--hv-shell-glass);
    border-top: 1px solid var(--hv-shell-border);
  }

  .hv-winfooter--status {
    min-height: var(--hv-window-footer-h-status, 32px);
    padding: 4px 16px;
    background: var(--hv-shell-fill);
    border-top: 1px solid var(--hv-shell-border-soft);
  }

  .hv-winfooter-inner {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: var(--hv-space-2-5);
  }

  .hv-winfooter--status .hv-winfooter-inner {
    justify-content: flex-start;
    gap: var(--hv-space-3);
    color: var(--hv-shell-text-2);
    font-size: var(--hv-text-sm, 12px);
  }

  /* Actions-bar button shape (status mode stays text-only) — 44px touch
     target, appliance 8px radius. Hover/disabled follow the exact
     appliance subtle + disabled slots. */
  .hv-winfooter:not(.hv-winfooter--status) :global(.hv-winfoot-btn) {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    min-height: 44px;
    padding: 0 20px;
    border: 1px solid var(--hv-shell-border-strong);
    border-radius: var(--hv-radius-default);
    background: var(--hv-shell-glass);
    color: var(--hv-shell-text-strong);
    font-size: var(--hv-text-md, 14px);
    font-weight: var(--hv-font-weight-semibold);
    cursor: pointer;
    transition: background var(--hv-chrome-dur-fast, 150ms) ease,
                color var(--hv-chrome-dur-fast, 150ms) ease;
  }

  .hv-winfooter:not(.hv-winfooter--status) :global(.hv-winfoot-btn:hover) {
    background: var(--hv-subtle-hover);
  }

  .hv-winfooter:not(.hv-winfooter--status) :global(.hv-winfoot-btn.is-primary) {
    background: var(--hv-primary);
    border-color: var(--hv-primary);
    color: var(--hv-on-primary);
  }

  .hv-winfooter:not(.hv-winfooter--status) :global(.hv-winfoot-btn.is-primary:hover) {
    background: var(--hv-primary-hover);
    border-color: var(--hv-primary-hover);
  }

  .hv-winfooter:not(.hv-winfooter--status) :global(.hv-winfoot-btn.is-danger) {
    background: var(--hv-error);
    border-color: var(--hv-error);
    color: var(--hv-on-error);
  }

  .hv-winfooter:not(.hv-winfooter--status) :global(.hv-winfoot-btn.is-danger:hover) {
    background: var(--hv-error-hover);
    border-color: var(--hv-error-hover);
  }

  .hv-winfooter:not(.hv-winfooter--status) :global(.hv-winfoot-btn:disabled) {
    background: var(--hv-shell-border-soft);
    border-color: transparent;
    color: var(--hv-disabled-fg);
    opacity: 1;
    cursor: not-allowed;
  }

  .hv-winfooter:not(.hv-winfooter--status) :global(.hv-winfoot-btn:focus-visible) {
    outline: none;
    box-shadow: var(--hv-focus-ring);
  }

  /* Touch: flat hovers. */
  @media (hover: none) {
    .hv-winfooter:not(.hv-winfooter--status) :global(.hv-winfoot-btn:hover) {
      background: var(--hv-shell-glass);
      color: var(--hv-shell-text-strong);
    }
    .hv-winfooter:not(.hv-winfooter--status) :global(.hv-winfoot-btn.is-primary:hover) {
      background: var(--hv-primary);
      color: var(--hv-on-primary);
    }
    .hv-winfooter:not(.hv-winfooter--status) :global(.hv-winfoot-btn.is-danger:hover) {
      background: var(--hv-error);
      color: var(--hv-on-error);
    }
  }
</style>