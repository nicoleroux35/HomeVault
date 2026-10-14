<!-- ========================================================================
     File          : Input.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Design-system text input wrapping the vendored shadcn-svelte Input:
       bindable value, label/error/helper layout kept on the hv-form-*
       classes.

     Dependencies:
       - svelte (>= 5), components/ui/input (vendored)

     Usage:
       `import { Input } from '$lib/components/theme'`.
     ======================================================================== -->
<script lang="ts">
  /**
   * HOMEVAULT DESIGN SYSTEM - Input Component
   * Version: 5.0 - wraps the real shadcn-svelte Input (see
   * components/ui/input). External prop API unchanged (replaces the 4.0 Web
   * Awesome wrapper 2026-09-11).
   *
   * `value` binds to the native input's `value` directly; user edits are
   * reflected back through `bind:value`. The label/error/helper layout stays
   * on the `hv-form-*` classes.
   */

  import { Input as ShadcnInput } from '$lib/components/ui/input/index.js';

  let {
    label = '',
    value = $bindable(''),
    type = 'text',
    placeholder = '',
    disabled = false,
    error = '',
    helper = '',
    required = false,
    id = '',
    maxlength = undefined,
    oninput,
    onchange,
    onfocus,
    onblur,
    onkeydown,
    onkeyup,
    children,
    ...rest
  }: {
    label?: string;
    value?: string;
    type?: 'text' | 'password' | 'email' | 'number' | 'tel';
    placeholder?: string;
    disabled?: boolean;
    error?: string;
    helper?: string;
    required?: boolean;
    id?: string;
    maxlength?: number;
    oninput?: (e: Event) => void;
    onchange?: (e: Event) => void;
    onfocus?: (e: FocusEvent) => void;
    onblur?: (e: FocusEvent) => void;
    onkeydown?: (e: KeyboardEvent) => void;
    onkeyup?: (e: KeyboardEvent) => void;
    children?: import('svelte').Snippet;
  } = $props();

  let inputClasses = $derived([
    'hv-form-input',
    error ? 'hv-error' : '',
  ].filter(Boolean).join(' '));
</script>

<div class="hv-form-group">
  {#if label}
    <label for={id} class="hv-form-label">
      {label}
      {#if required}
        <span class="hv-text-error">*</span>
      {/if}
    </label>
  {/if}

  <ShadcnInput
    {id}
    {type}
    bind:value
    {placeholder}
    {disabled}
    {required}
    {maxlength}
    class={inputClasses}
    {oninput}
    {onchange}
    {onfocus}
    {onblur}
    {onkeydown}
    {onkeyup}
    {...rest}
  />

  {#if error}
    <span class="hv-form-error">{error}</span>
  {:else if helper}
    <span class="hv-form-helper">{helper}</span>
  {/if}
</div>