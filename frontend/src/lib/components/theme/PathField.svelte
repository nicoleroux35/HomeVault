<!-- ========================================================================
     File          : PathField.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Path input field pairing a bindable text value with the modal
       FolderPicker for browsing; supports folder and file modes. Frozen in
       favour of PathPickerField for new call sites (no free typing there).

     Dependencies:
       - svelte (>= 5)

     Usage:
       Legacy call sites only — prefer PathPickerField.
     ======================================================================== -->
<script lang="ts">
  import { t } from '$lib/stores/i18n';
  import { IconFolderOpen, IconFile as FileIcon } from '@tabler/icons-svelte';
  import FolderPicker from './FolderPicker.svelte';

  let {
    value = $bindable(''),
    mode = 'folder',
    id,
    placeholder = '',
    required = false,
  }: {
    value?: string;
    mode?: 'folder' | 'file';
    id?: string;
    placeholder?: string;
    required?: boolean;
  } = $props();

  let pickerOpen = $state(false);

  function handleSelect(path: string) {
    value = path;
    pickerOpen = false;
  }
</script>

<div class="hv-pf-row">
  <input
    {id}
    class="hv-pf-input"
    type="text"
    bind:value
    {placeholder}
    {required}
  />
  <button
    type="button"
    class="hv-pf-browse"
    onclick={() => (pickerOpen = true)}
  >
    {#if mode === 'file'}
      <FileIcon size={14} />
      <span>{$t('path_field.select_file')}</span>
    {:else}
      <IconFolderOpen size={14} />
      <span>{$t('path_field.select_folder')}</span>
    {/if}
  </button>
</div>

<FolderPicker
  open={pickerOpen}
  {mode}
  {value}
  onselect={handleSelect}
  onclose={() => (pickerOpen = false)}
/>

<style>
  .hv-pf-row {
    display: flex;
    align-items: center;
    gap: var(--hv-space-2, 8px);
    width: 100%;
  }

  .hv-pf-input {
    flex: 1;
    min-width: 0;
    height: 40px;
    padding: 0 12px;
    border: 1px solid var(--hv-border);
    border-radius: var(--hv-radius-md, 8px);
    font-size: var(--hv-text-md, 14px);
    font-family: inherit;
    color: var(--hv-text-primary);
    background: var(--hv-surface-container-lowest);
  }

  .hv-pf-input:focus {
    outline: none;
    border-color: var(--hv-primary);
    box-shadow: 0 0 0 3px var(--hv-overlay-hover, rgba(15, 108, 189, 0.15));
  }

  .hv-pf-browse {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 40px;
    padding: 0 var(--hv-space-3);
    border: 1px solid var(--hv-border);
    border-radius: var(--hv-radius-md, 8px);
    background: var(--hv-surface-container-lowest);
    color: var(--hv-text-secondary);
    font-size: var(--hv-text-sm);
    font-weight: var(--hv-font-medium);
    white-space: nowrap;
    cursor: pointer;
    transition:
      color var(--hv-transition-base) var(--hv-ease-default),
      background-color var(--hv-transition-base) var(--hv-ease-default);
  }

  .hv-pf-browse:hover {
    background: var(--hv-surface-container);
    color: var(--hv-text-primary);
  }
</style>
