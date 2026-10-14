<!-- ========================================================================
     File          : Drawer.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Mobile bottom-sheet drawer rebuilt on the vendored bits-ui Dialog
       (Svelte 5), replacing the old drag/snap library whose dependency
       stack pinned the svelte-4 era. Public API unchanged (open,
       onOpenChange, title, direction, header/footer, dismissible); themed
       with the --hv-* tokens. On desktop layouts prefer Modal.

     Dependencies:
       - svelte (>= 5), bits-ui, @tabler/icons-svelte

     Usage:
       `import { Drawer } from '$lib/components/theme'`.
     ======================================================================== -->
<script lang="ts">
  /**
   * HOMEVAULT DESIGN SYSTEM - Drawer Component
   * Version: 6.0 - rebuilt on the shadcn-svelte / bits-ui `Dialog` primitive
   * (Svelte 5 native), replacing the Vaul Svelte implementation (2026-09-14).
   * Vaul was the only dependency still pinning the svelte-4-era
   * `bits-ui@0.21` / `@melt-ui/svelte@0.76` stack (npm ERESOLVE peer
   * warnings on every `npm install`), and this app never used its swipe /
   * snap-point drag layer, so the Drawer is re-expressed as a positioned
   * Dialog. Mobile bottom-sheet for < 768px layouts; on desktop prefer
   * `<Modal>` (Dialog). The drag handle, overlay and sheet surface are themed
   * with `--hv-*` tokens (paper surface, 16px window radius, modal scrim).
   *
   * The public API is unchanged (`open`, `onOpenChange`, `title`,
   * `description`, `direction`, `dismissible`, `header`, `footer`,
   * `children`). Vaul's `snapPoints` was dropped — the bits-ui Dialog has no
   * drag snapping, so the sheet renders at its full `max-h`/`max-w` size.
   */
  import { Dialog } from 'bits-ui';
  import { IconX } from '@tabler/icons-svelte';
  import { t } from '$lib/stores/i18n';
  import type { Snippet } from 'svelte';

  let {
    open = false,
    onOpenChange,
    title = '',
    description = '',
    direction = 'bottom',
    dismissible = true,
    class: className = '',
    header,
    footer,
    children,
  }: {
    open?: boolean;
    onOpenChange?: (next: boolean) => void;
    title?: string;
    description?: string;
    direction?: 'top' | 'bottom' | 'left' | 'right';
    dismissible?: boolean;
    class?: string;
    header?: Snippet;
    footer?: Snippet;
    children?: Snippet;
  } = $props();

  function handleOpenChange(next: boolean) {
    open = next;
    onOpenChange?.(next);
  }

  // Direction → sheet surface classes (fixed anchor + slide animation).
  const surface: Record<string, string> = {
    bottom:
      'bottom-0 inset-x-0 max-h-[85dvh] rounded-t-[var(--hv-radius-2xl)] data-open:slide-in-from-bottom-full data-closed:slide-out-to-bottom-full',
    top: 'top-0 inset-x-0 max-h-[85dvh] rounded-b-[var(--hv-radius-2xl)] data-open:slide-in-from-top-full data-closed:slide-out-to-top-full',
    left: 'left-0 inset-y-0 max-w-[85vw] rounded-r-[var(--hv-radius-2xl)] data-open:slide-in-from-left-full data-closed:slide-out-to-left-full',
    right: 'right-0 inset-y-0 max-w-[85vw] rounded-l-[var(--hv-radius-2xl)] data-open:slide-in-from-right-full data-closed:slide-out-to-right-full',
  };
</script>

<Dialog.Root {open} onOpenChange={handleOpenChange}>
  <Dialog.Portal>
    <Dialog.Overlay
      class="data-open:animate-in data-closed:animate-out data-closed:fade-out-0 data-open:fade-in-0 bg-[var(--hv-modal-overlay)] fixed inset-0 z-40 duration-100"
    />
    <Dialog.Content
      interactOutsideBehavior={dismissible ? 'close' : 'ignore'}
      class="bg-[var(--hv-shell-card)] border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)] data-open:animate-in data-closed:animate-out data-closed:fade-out-0 data-open:fade-in-0 fixed z-50 flex flex-col gap-0 outline-none duration-200 {surface[direction] ?? surface.bottom} {className}"
    >
      <Dialog.Close
        class="absolute top-3 right-3 z-10 grid h-9 w-9 cursor-pointer place-items-center rounded-full border-none bg-transparent text-[var(--hv-shell-text-2)] hover:text-[var(--hv-shell-text-strong)] hv-ic-abs"
        aria-label={$t('common.close')}
      >
        <IconX size={20} stroke={1.7} />
      </Dialog.Close>

      {#if header}
        <div class="border-b border-[var(--hv-shell-border-soft)] px-5 pt-4 pb-3">
          {@render header()}
        </div>
      {:else if title}
        <div class="border-b border-[var(--hv-shell-border-soft)] px-5 pt-4 pb-3">
          <Dialog.Title class="text-c-lg font-semibold text-[var(--hv-shell-text-strong)]">
            {title}
          </Dialog.Title>
          {#if description}
            <Dialog.Description class="mt-1 text-sm text-[var(--hv-shell-text-2)]">
              {description}
            </Dialog.Description>
          {/if}
        </div>
      {/if}

      <div class="min-h-0 flex-1 overflow-y-auto px-5 py-4">
        {#if children}
          {@render children()}
        {/if}
      </div>

      {#if footer}
        <div class="border-t border-[var(--hv-shell-border-soft)] px-5 py-3">
          {@render footer()}
        </div>
      {/if}
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>