<!-- ========================================================================
     File          : FormRow.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       The one standard label + control pairing for app forms: accessible
       <label> over the control, optional help line, and an error line that
       replaces help when present. Every input in an app screen sits in one.

     Dependencies:
       - svelte (>= 5)

     Usage:
       `import { FormRow } from '$lib/components/app'` wrapping an
       Input/Select/Toggle/Checkbox/Radio.
     ======================================================================== -->
<script lang="ts">
  /**
   * HOMEVAULT DESIGN SYSTEM — FormRow (page composite, `app/` layer)
   *
   * The one standard label + control pairing for app forms: an accessible
   * `<label>` over the control, an optional help line and an optional error
   * line (error line swaps in for help when present). Use `<Input>` /
   * `<Select>` / `<Toggle>` / `<Checkbox>` / `<Radio>` as the control.
   */
  import type { Snippet } from 'svelte';

  interface Props {
    /** Label text. */
    label: string;
    /** id of the control the label points to. */
    for?: string;
    /** Marks the field as required. */
    required?: boolean;
    /** Help text under the control. */
    help?: string;
    /** Error text — replaces the help line and tints it danger. */
    error?: string;
    /** The control. */
    children: Snippet;
  }

  let { label, for: htmlFor, required = false, help, error, children }: Props = $props();
</script>

<div class="hv-form-row">
  <label class="block text-c-md font-medium text-foreground" for={htmlFor}>
    {label}
    {#if required}<span class="text-[var(--hv-error)]" aria-hidden="true"> *</span>{/if}
  </label>
  <div class="mt-1.5">
    {@render children()}
    {#if error}
      <p class="mt-1 text-c-sm text-[var(--hv-error-text)]" role="alert">{error}</p>
    {:else if help}
      <p class="mt-1 text-c-sm text-muted-foreground">{help}</p>
    {/if}
  </div>
</div>