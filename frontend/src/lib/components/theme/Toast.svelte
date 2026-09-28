<script lang="ts">
  /**
   * HOMEVAULT DESIGN SYSTEM - Toast Component
   * Wraps the shadcn-svelte Sonner toaster (svelte-sonner, see
   * components/ui/sonner). Mount this once at the app root (App.svelte) and
   * trigger toasts from anywhere via `toast()` imported from
   * `svelte-sonner`. Hardcoded light theme — HomeVault is a light-only
   * appliance (no dark mode), so sonner must not follow the OS preference.
   * The NotificationCenter stays the canonical surface for system events;
   * toasts are for ephemeral action feedback.
   */
  import { Toaster } from '$lib/components/ui/sonner/index.js';
  import type { ToasterProps } from 'svelte-sonner';

  let {
    position = 'bottom-right',
    richColors = false,
    closeButton = true,
    ...restProps
  }: {
    position?: ToasterProps['position'];
    richColors?: boolean;
    closeButton?: boolean;
  } & ToasterProps = $props();
</script>

<Toaster {position} {richColors} {closeButton} {...restProps} />