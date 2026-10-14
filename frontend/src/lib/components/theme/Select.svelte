<!-- ========================================================================
     File          : Select.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Design-system single-select dropdown wrapping the vendored bits-ui
       Select: portalled to <body> (never clipped by a window), full
       keyboard/ARIA behaviour inherited. Single-select only — the old
       multiple/searchable options are gone with the hand-rolled listbox.

     Dependencies:
       - svelte (>= 5), components/ui/select (vendored)

     Usage:
       `import { Select } from '$lib/components/theme'`.
     ======================================================================== -->
<script lang="ts">
  /**
   * HOMEVAULT DESIGN SYSTEM - Select Component
   * Version: 5.0 - wraps the real shadcn-svelte Select (bits-ui/Melt
   * primitive, see components/ui/select). External prop API mostly unchanged
   * (replaces the hand-rolled 2.0 listbox 2026-09-12). The dropdown portals
   * to <body> (never clipped by a window's overflow), comes with full
   * keyboard/ARIA from Melt, and uses the shadcn @theme bridge in app.css
   * (appliance tokens). `multiple`/`searchable` are gone - bits-ui Select
   * is single-select only and no call site used those.
   */

  import SelectRoot from '$lib/components/ui/select/select.svelte';
  import SelectTrigger from '$lib/components/ui/select/select-trigger.svelte';
  import SelectValue from '$lib/components/ui/select/select-value.svelte';
  import SelectContent from '$lib/components/ui/select/select-content.svelte';
  import SelectItem from '$lib/components/ui/select/select-item.svelte';
  import SelectGroup from '$lib/components/ui/select/select-group.svelte';
  import SelectGroupHeading from '$lib/components/ui/select/select-group-heading.svelte';
  import { IconX } from '@tabler/icons-svelte';
  import { t } from '$lib/stores/i18n';

  type SelectOption = { value: string; label: string; disabled?: boolean; group?: string };

  let {
    value = $bindable(''),
    options = [],
    placeholder = '',
    label = '',
    error = '',
    helper = '',
    disabled = false,
    required = false,
    clearable = false,
    size = 'md',
    id = '',
    onchange,
    onopen,
    onclose,
    class: extraClass = '',
    ...rest
  }: {
    value?: string;
    options?: SelectOption[];
    placeholder?: string;
    label?: string;
    error?: string;
    helper?: string;
    disabled?: boolean;
    required?: boolean;
    clearable?: boolean;
    size?: 'sm' | 'md';
    id?: string;
    onchange?: (value: string) => void;
    onopen?: () => void;
    onclose?: () => void;
    class?: string;
    children?: import('svelte').Snippet;
  } = $props();

  const groupedOptions = $derived(
    (options as SelectOption[]).reduce(
      (acc, opt) => {
        const group = opt.group || '_default';
        if (!acc[group]) acc[group] = [];
        acc[group].push(opt);
        return acc;
      },
      {} as Record<string, SelectOption[]>
    )
  );

  function handleOpenChange(next: boolean) {
    if (next) onopen?.();
    else onclose?.();
  }

  function handleValueChange(next: string) {
    value = next;
    onchange?.(next);
  }

  function clearSelection() {
    value = '';
    onchange?.('');
  }
</script>

<div class="hv-form-group {extraClass}">
  {#if label}
    <label for={id} class="hv-form-label">
      {label}
      {#if required}
        <span class="hv-text-error">*</span>
      {/if}
    </label>
  {/if}

  <SelectRoot
    bind:value
    type="single"
    {disabled}
    {required}
    onOpenChange={handleOpenChange}
    onValueChange={handleValueChange}
    {...rest}
  >
    <SelectTrigger
      id={id}
      size={size === 'sm' ? 'sm' : 'default'}
      class={size === 'sm' ? 'hv-select-trigger hv-select-trigger--sm w-full' : 'hv-select-trigger w-full'}
      aria-invalid={error ? 'true' : undefined}
    >
      <SelectValue placeholder={placeholder} />
      {#if clearable && value}
        <span
          role="button"
          tabindex="0"
          class="hv-select-clear hv-ic"
          aria-label={$t('common.clear')}
          onclick={(e) => {
            e.stopPropagation();
            clearSelection();
          }}
          onkeydown={(e) => {
            if (e.key === 'Enter' || e.key === ' ') {
              e.preventDefault();
              e.stopPropagation();
              clearSelection();
            }
          }}
        >
          <IconX size={16} stroke={2} />
        </span>
      {/if}
    </SelectTrigger>
    <SelectContent>
      {#each Object.entries(groupedOptions) as [groupName, groupOptions]}
        {#if groupName !== '_default'}
          <SelectGroup>
            <SelectGroupHeading>{groupName}</SelectGroupHeading>
            {#each groupOptions as opt}
              <SelectItem value={opt.value} label={opt.label} disabled={opt.disabled} />
            {/each}
          </SelectGroup>
        {:else}
          {#each groupOptions as opt}
            <SelectItem value={opt.value} label={opt.label} disabled={opt.disabled} />
          {/each}
        {/if}
      {/each}
    </SelectContent>
  </SelectRoot>

  {#if error}
    <span class="hv-form-error">{error}</span>
  {:else if helper}
    <span class="hv-form-helper">{helper}</span>
  {/if}
</div>

<style>
  /* Unlayered rules win over Tailwind v4 utilities (which live in
     @layer utilities), so these match the .hv-form-input look without
     !important. */
  :global(.hv-select-trigger) {
    height: var(--hv-input-height);
    padding: var(--hv-input-padding);
    padding-right: 0.5rem;
    border-radius: var(--hv-radius-lg);
    /* --hv-shell-border-strong (#87929E, 3.16:1 on white) so the control
       boundary is perceivable (WCAG 1.4.11). */
    border-color: var(--hv-shell-border-strong);
    background-color: var(--hv-surface-container-lowest);
    font-size: var(--hv-font-size-base);
    color: var(--hv-text-primary);
  }

  :global(.hv-select-trigger.hv-select-trigger--sm) {
    height: 36px;
    padding: 0 12px;
  }

  :global(.hv-select-trigger:hover:not(:disabled)) {
    border-color: var(--hv-primary);
  }

  :global(.hv-select-trigger[aria-invalid='true']) {
    border-color: var(--hv-error);
  }

  :global(.hv-select-clear) {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    border-radius: var(--hv-radius-sm);
    color: var(--hv-text-tertiary);
    cursor: pointer;
    flex-shrink: 0;
    transition: color var(--hv-transition-base);
  }

  :global(.hv-select-clear:hover) {
    color: var(--hv-text-primary);
  }
</style>