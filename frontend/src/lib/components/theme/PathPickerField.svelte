<script lang="ts">
  import { t } from '$lib/stores/i18n';
  import { IconFolderOpen, IconFile as FileIcon } from '@tabler/icons-svelte';
  import FloatingPathPicker from './FloatingPathPicker.svelte';

  /**
   * Standard "pick a path" field: a read-only display of the chosen path + a
   * Browse button that opens the non-modal `FloatingPathPicker`. No manual
   * typing — use this wherever a user shouldn't have to know the filesystem
   * layout. Drop-in shape-compatible with the (frozen) `PathField.svelte`,
   * minus the free-text input.
   */
  let {
    value = $bindable(''),
    mode = 'folder',
    id,
    ariaLabel,
  }: {
    value?: string;
    mode?: 'folder' | 'file';
    id?: string;
    /** Accessible name for the read-only field (it has no visible <label> of its own). */
    ariaLabel?: string;
  } = $props();

  let pickerOpen = $state(false);
  let browseBtn = $state<HTMLElement | null>(null);

  function openPicker() {
    pickerOpen = true;
  }

  function handleSelect(path: string) {
    value = path;
    pickerOpen = false;
  }

  const browseLabel = $derived(
    mode === 'file' ? $t('path_field.select_file') : $t('path_field.select_folder'),
  );
</script>

<div class="hv-ppf">
  <input
    {id}
    class="hv-ppf-display"
    class:hv-ppf-empty={!value}
    type="text"
    readonly
    tabindex="-1"
    aria-label={ariaLabel ?? browseLabel}
    value={value || $t('folder_picker.none_selected')}
    onclick={openPicker}
  />
  <button type="button" class="hv-ppf-browse" bind:this={browseBtn} onclick={openPicker}>
    {#if mode === 'file'}
      <FileIcon size={14} />
    {:else}
      <IconFolderOpen size={14} />
    {/if}
    <span>{browseLabel}</span>
  </button>
</div>

<FloatingPathPicker
  open={pickerOpen}
  {value}
  {mode}
  anchor={browseBtn}
  onselect={handleSelect}
  onclose={() => (pickerOpen = false)}
/>

<style>
  .hv-ppf {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
  }

  .hv-ppf-display {
    flex: 1;
    min-width: 0;
    height: var(--hv-touch-target-min, 44px);
    padding: 0 12px;
    border: 1px solid var(--hv-border);
    border-radius: var(--hv-radius-md, 8px);
    background: var(--hv-surface-container-low);
    color: var(--hv-text-primary);
    font-size: var(--hv-text-md, 14px);
    font-family: inherit;
    box-sizing: border-box;
    cursor: pointer;
    text-overflow: ellipsis;
  }
  .hv-ppf-display.hv-ppf-empty { color: var(--hv-text-tertiary); }
  .hv-ppf-display:focus { outline: none; border-color: var(--hv-primary); }

  .hv-ppf-browse {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: var(--hv-touch-target-min, 44px);
    padding: 0 var(--hv-space-3);
    border: 1px solid var(--hv-border);
    border-radius: var(--hv-radius-md, 8px);
    background: var(--hv-surface-container-lowest);
    color: var(--hv-text-secondary);
    font-size: var(--hv-text-sm);
    font-weight: var(--hv-font-medium);
    white-space: nowrap;
    cursor: pointer;
  }
  .hv-ppf-browse:hover {
    background: var(--hv-surface-container);
    color: var(--hv-text-primary);
  }
</style>
