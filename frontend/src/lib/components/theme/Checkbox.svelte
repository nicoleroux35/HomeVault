<!-- ========================================================================
     File          : Checkbox.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Design-system checkbox wrapping the vendored bits-ui checkbox:
       bindable checked + indeterminate, with the label/description/error
       layout kept on the hv-checkbox-* classes.

     Dependencies:
       - svelte (>= 5), components/ui/checkbox (vendored)

     Usage:
       `import { Checkbox } from '$lib/components/theme'`.
     ======================================================================== -->
<script lang="ts">
  /**
   * HOMEVAULT DESIGN SYSTEM - Checkbox Component
   * Version: 5.0 - wraps the real shadcn-svelte Checkbox (bits-ui/Melt
   * primitive, see components/ui/checkbox). External prop API unchanged
   * (replaces the 4.0 Web Awesome wrapper 2026-09-11).
   *
   * `checked` / `indeterminate` bind to the bits-ui Checkbox's properties;
   * the label/description/error layout stays on the `hv-checkbox-*` classes.
   */

  import { Checkbox as ShadcnCheckbox } from '$lib/components/ui/checkbox/index.js';

  let {
    checked = $bindable(false),
    indeterminate = false,
    label = '',
    description = '',
    disabled = false,
    required = false,
    error = '',
    value = '',
    name = '',
    size = 'md',
    onchange,
  }: {
    checked?: boolean;
    indeterminate?: boolean;
    label?: string;
    description?: string;
    disabled?: boolean;
    required?: boolean;
    error?: string;
    value?: string;
    name?: string;
    size?: 'sm' | 'md' | 'lg';
    onchange?: (checked: boolean) => void;
  } = $props();

  function handleCheckedChange(next: boolean | 'indeterminate') {
    checked = next === true;
    indeterminate = next === 'indeterminate';
    onchange?.(checked);
  }
</script>

<label class="hv-checkbox" class:hv-checkbox-disabled={disabled}>
  <ShadcnCheckbox
    bind:checked
    bind:indeterminate
    {name}
    {value}
    disabled={disabled || undefined}
    required={required || undefined}
    onCheckedChange={handleCheckedChange}
  ></ShadcnCheckbox>

  {#if label || description}
    <div class="hv-checkbox-content">
      {#if label}
        <span class="hv-checkbox-label" class:hv-checkbox-required={required}>
          {label}
        </span>
      {/if}
      {#if description}
        <span class="hv-checkbox-description">
          {description}
        </span>
      {/if}
    </div>
  {/if}
</label>

{#if error}
  <span class="hv-checkbox-error-text">{error}</span>
{/if}