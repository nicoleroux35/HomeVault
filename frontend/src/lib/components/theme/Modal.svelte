<script lang="ts">
  /**
   * HOMEVAULT DESIGN SYSTEM - Modal Component
   * Version: 5.0 - wraps the real shadcn-svelte Dialog (bits-ui/Melt
   * primitive, see components/ui/dialog). External prop API unchanged
   * (replaces the 4.0 Web Awesome wrapper 2026-09-11).
   *
   * bits-ui's Dialog is declarative: the `open` *property* shows and hides
   * it, and every user-initiated close (X / Escape / backdrop) emits
   * `onOpenChange` → `onclose`. `closeOnBackdrop=false` maps to omitting
   * `interactOutside`/light-dismiss handling (Escape still closes either
   * way, matching the previous behavior). The close button is built in —
   * no custom slot needed. Sizes via a max-width class.
   */
  import * as Dialog from '$lib/components/ui/dialog/index.js';
  import type { Snippet } from 'svelte';

  let {
    open = false,
    title = '',
    size = 'md',
    closeOnBackdrop = true,
    onclose,
    header,
    icon,
    footer,
    children,
  }: {
    open?: boolean;
    title?: string;
    size?: 'sm' | 'md' | 'lg';
    closeOnBackdrop?: boolean;
    onclose?: () => void;
    header?: Snippet;
    icon?: Snippet;
    footer?: Snippet;
    children?: Snippet;
  } = $props();

  const widthClass: Record<'sm' | 'md' | 'lg', string> = {
    sm: 'sm:max-w-sm',
    md: 'sm:max-w-md',
    lg: 'sm:max-w-2xl',
  };

  function handleOpenChange(next: boolean) {
    if (open && !next) {
      open = false;
      onclose?.();
    }
  }
</script>

<Dialog.Root open={open} onOpenChange={handleOpenChange}>
  <Dialog.Content class={widthClass[size]} onInteractOutside={(e) => { if (!closeOnBackdrop) e.preventDefault(); }}>
    {#if header}
      <Dialog.Header>
        {@render header()}
      </Dialog.Header>
    {:else if title}
      <Dialog.Header>
        {#if icon}
          <span class="hv-modal-icon">{@render icon()}</span>
        {/if}
        <Dialog.Title class="hv-modal-title">{title}</Dialog.Title>
      </Dialog.Header>
    {/if}

    {#if children}
      {@render children()}
    {/if}

    {#if footer}
      <Dialog.Footer>
        {@render footer()}
      </Dialog.Footer>
    {/if}
  </Dialog.Content>
</Dialog.Root>

<style>
  .hv-modal-icon {
    display: inline-flex;
    align-items: center;
    gap: 0.5rem;
  }
</style>