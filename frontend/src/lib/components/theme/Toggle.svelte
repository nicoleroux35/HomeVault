<script lang="ts">
  /**
   * HOMEVAULT DESIGN SYSTEM - Toggle Component
   * Version: 5.0 - wraps the real shadcn-svelte Switch (bits-ui/Melt
   * primitive, see components/ui/switch). External prop API unchanged (both
   * real call sites use `bind:checked`, a genuine two-way binding; replaces
   * the 4.0 Web Awesome wrapper 2026-09-11).
   *
   * `checked` binds to the bits-ui Switch's `checked` property directly, so
   * `bind:checked` keeps working after the first user interaction.
   */

  import { Switch as ShadcnSwitch } from '$lib/components/ui/switch/index.js';

  let {
    checked = $bindable(false),
    disabled = false,
    label = '',
    description = '',
    ariaLabelledby = '',
  }: {
    checked?: boolean;
    disabled?: boolean;
    label?: string;
    description?: string;
    /** Id of an external element that names this switch. Use it when the
     *  visible label lives in a surrounding row instead of in `label` — the
     *  switch then carries no inline text and `label` stays empty. */
    ariaLabelledby?: string;
  } = $props();
</script>

<div class="hv-flex hv-items-center hv-gap-3">
  <ShadcnSwitch bind:checked {disabled} aria-label={label || undefined} aria-labelledby={ariaLabelledby || undefined} />

  {#if label || description}
    <div class="hv-flex hv-flex-col">
      {#if label}
        <span class="hv-text-base hv-font-medium hv-text-primary">{label}</span>
      {/if}
      {#if description}
        <span class="hv-text-sm hv-text-secondary">{description}</span>
      {/if}
    </div>
  {/if}
</div>