<script lang="ts">
  /**
   * HOMEVAULT DESIGN SYSTEM - Spinner/Loader Component
   * Version: 5.0 - wraps the real shadcn-svelte Spinner (lucide loader-2,
   * see components/ui/spinner). External prop API unchanged (replaces the
   * 4.0 Web Awesome wrapper 2026-09-11).
   *
   * Sizes map to font-size on the HomeVault content scale; the indicator
   * stays the brand `--hv-primary`.
   */

  import { Spinner as ShadcnSpinner } from '$lib/components/ui/spinner/index.js';
  import { t } from '$lib/stores/i18n';

  export let size: 'sm' | 'md' | 'lg' = 'md';
  /** Accessible label for screen readers (i18n; override per call site). */
  export let label = $t('common.loading_short');

  const fontSize: Record<'sm' | 'md' | 'lg', string> = {
    sm: '1rem',
    md: '1.5rem',
    lg: '2rem',
  };
</script>

<div class="hv-flex hv-flex-col hv-items-center hv-gap-3">
  <ShadcnSpinner
    style="width: {fontSize[size]}; height: {fontSize[size]}; color: var(--hv-primary);"
    aria-label={label}
  />
  {#if $$slots.default}
    <div class="hv-text-sm hv-text-secondary">
      <slot />
    </div>
  {/if}
</div>