<script lang="ts">
  /**
   * HOMEVAULT DESIGN SYSTEM - Radio Component
   * Version: 5.0 - wraps the real shadcn-svelte RadioGroup item (bits-ui/Melt
   * primitive, see components/ui/radio-group). External prop API unchanged
   * (replaces the 4.0 Web Awesome wrapper 2026-09-11).
   *
   * A single Radio renders one RadioGroup item; the label/description/error
   * layout stays on the `hv-radio-*` classes. `onchange` fires when this
   * item becomes the selected value.
   */

  import { RadioGroup as ShadcnRadioGroup, RadioGroupItem } from '$lib/components/ui/radio-group/index.js';

  let {
    value = $bindable(''),
    name = '',
    label = '',
    description = '',
    disabled = false,
    required = false,
    error = '',
    size = 'md',
    onchange,
  }: {
    value?: string;
    name?: string;
    label?: string;
    description?: string;
    disabled?: boolean;
    required?: boolean;
    error?: string;
    size?: 'sm' | 'md' | 'lg';
    onchange?: (value: string) => void;
  } = $props();

  function handleValueChange(next: string) {
    value = next;
    onchange?.(next);
  }
</script>

<label class="hv-radio" class:hv-radio-disabled={disabled}>
  <ShadcnRadioGroup value={value} onValueChange={handleValueChange} class="hv-radio-group">
    <RadioGroupItem
      value={value}
      {disabled}
      aria-label={label || undefined}
    ></RadioGroupItem>
  </ShadcnRadioGroup>

  {#if label || description}
    <div class="hv-radio-content">
      {#if label}
        <span class="hv-radio-label" class:hv-radio-required={required}>
          {label}
        </span>
      {/if}
      {#if description}
        <span class="hv-radio-description">
          {description}
        </span>
      {/if}
    </div>
  {/if}
</label>

{#if error}
  <span class="hv-radio-error-text">{error}</span>
{/if}