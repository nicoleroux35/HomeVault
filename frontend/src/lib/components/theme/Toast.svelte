<!-- ========================================================================
     File          : Toast.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       App-root toaster wrapping the vendored svelte-sonner Toaster, pinned
       to the light theme (the appliance is light-only, so it must not follow
       the OS preference). Ephemeral action feedback only — the
       NotificationCenter stays the canonical surface for system events.

     Dependencies:
       - svelte (>= 5), svelte-sonner (via components/ui/sonner)

     Usage:
       Mounted once in App.svelte; trigger with toast() from svelte-sonner.
     ======================================================================== -->
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