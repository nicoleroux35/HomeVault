<script lang="ts">
  /**
   * HOMEVAULT DESIGN SYSTEM - Badge Component
   * Version: 5.0 - wraps the real shadcn-svelte Badge (see
   * components/ui/badge). External prop API unchanged (replaces the 4.0 Web
   * Awesome wrapper 2026-09-11).
   *
   * `variant` maps 1:1 to shadcn badge variants; the semantic tints use the
   * HomeVault `--hv-badge-*` tokens (soft bg + dark ink). `dot` stays a
   * hand-built indicator (a status dot isn't part of shadcn's badge design);
   * `pulse` animates that dot. Sizes map to an inline font-size.
   */

  import { Badge as ShadcnBadge } from '$lib/components/ui/badge/index.js';

  export let variant: 'success' | 'warning' | 'error' | 'info' | 'secondary' = 'info';
  /** When true, renders a small dot indicator before the badge content */
  export let dot = false;
  /** When true, plays a pulse animation for "live" statuses */
  export let pulse = false;
  export let size: 'sm' | 'md' | 'lg' = 'md';

  const shadcnVariant: Record<typeof variant, 'success' | 'warning' | 'error' | 'info' | 'secondary'> = {
    success: 'success',
    warning: 'warning',
    error: 'error',
    info: 'info',
    secondary: 'secondary',
  };
  const fontSize: Record<'sm' | 'md' | 'lg', string> = {
    sm: 'var(--hv-font-size-xs)',
    md: 'var(--hv-font-size-sm)',
    lg: 'var(--hv-font-size-base)',
  };
</script>

<ShadcnBadge
  variant={shadcnVariant[variant]}
  style="font-size: {fontSize[size]};"
  {...$$restProps}
>
  {#if dot}
    <span class="hv-badge-dot {pulse ? 'hv-badge-dot-live' : ''}" style="background-color: currentColor;"></span>
  {/if}
  <slot />
</ShadcnBadge>

<style>
  :global(.hv-badge-dot) {
    display: inline-block;
    width: 0.6em;
    height: 0.6em;
    border-radius: 50%;
    margin-right: 0.4em;
    flex-shrink: 0;
  }
  :global(.hv-badge-dot-live) {
    animation: hv-badge-pulse 1.6s ease-in-out infinite;
  }
  @keyframes hv-badge-pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.35; }
  }
</style>