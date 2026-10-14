<!-- ========================================================================
     File          : FolderPicker.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 10/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Modal folder/file picker built on focusTrap: browses the server's
       allowed roots with the server-side protection against escaping them,
       and reports the selection through onselect. The modal counterpart of
       FloatingPathPicker.

     Dependencies:
       - svelte (>= 5), @tabler/icons-svelte, $lib/utils/focusTrap,
         $lib/services/api

     Usage:
       `import { FolderPicker } from '$lib/components/theme'`.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { focusTrap } from '$lib/utils/focusTrap';
  import { t, translate } from '$lib/stores/i18n';
  import { IconFolderOpen, IconFolder, IconServer, IconChevronRight, IconHome, IconArrowUp, IconLoader, IconShieldCheckered, IconUsb, IconCloud, IconX, IconFile as FileIcon } from '@tabler/icons-svelte';

  let {
    open = false,
    value = '',
    mode = 'folder',
    onselect,
    onclose,
  }: {
    open?: boolean;
    value?: string;
    mode?: 'folder' | 'file';
    onselect?: (path: string) => void;
    onclose?: () => void;
  } = $props();

  interface FileEntry {
    name: string;
    path: string;
    is_directory: boolean;
  }

  interface DiskInfo {
    name: string;
    path: string;
    mount_point: string;
    filesystem: string;
    size_bytes: number;
    used_bytes: number;
    category: string;
  }

  interface DisksResponse {
    system: DiskInfo[];
    internal: DiskInfo[];
    raid: DiskInfo[];
    external: DiskInfo[];
    cloud: DiskInfo[];
    network: DiskInfo[];
  }

  interface StorageDisk {
    name: string;
    device: string;
    size_bytes: number;
    vendor: string;
    model: string;
    role: string;
    connection_type: string;
    mount_point?: string;
  }

  interface DiskGroup {
    id: string;
    icon: any;
    color: string;
    disks: DiskInfo[];
  }

  // These must be $state — this file uses $props() so it's in Svelte 5 runes
  // mode, where plain `let` reassignment never triggers a re-render (same bug
  // class as Shares.svelte/Downloader.svelte, documented in CLAUDE.md).
  // Confirmed live: navigation/selection logic ran correctly against these
  // vars, but the DOM never visually updated after the initial mount.
  let currentPath = $state('/');
  let entries: FileEntry[] = $state([]);
  let disksData: DisksResponse | null = $state(null);
  let storageDisks: StorageDisk[] = $state([]);
  let loading = $state(true);
  let browsing = $state(false);

  // Labels are looked up reactively via `folder_picker.group_<id>` so they
  // follow the selected language, rather than being baked in here once.
  // $state: each group's `.disks` array is mutated in place in loadAllDisks(),
  // which needs deep reactivity to be picked up by the {#each} below.
  const diskGroups: DiskGroup[] = $state([
    { id: 'system', icon: IconServer, color: 'var(--hv-info)', disks: [] },
    { id: 'internal', icon: IconServer, color: 'var(--hv-primary)', disks: [] },
    { id: 'raid', icon: IconShieldCheckered, color: 'var(--hv-success)', disks: [] },
    { id: 'external', icon: IconUsb, color: 'var(--hv-warning)', disks: [] },
    { id: 'cloud', icon: IconCloud, color: 'var(--hv-app-dashboard-c)', disks: [] },
    { id: 'network', icon: IconCloud, color: 'var(--hv-app-journal-c)', disks: [] },
  ]);

  onMount(async () => {
    await loadAllDisks();
    if (value) {
      await navigateTo(value);
    } else {
      loading = false;
    }
  });

  async function loadAllDisks() {
    try {
      const [mountsRes, storageRes] = await Promise.all([
        api.getDisks(),
        api.getStorageDisks(),
      ]);
      if (mountsRes.success && mountsRes.data) {
        disksData = mountsRes.data;
        for (const group of diskGroups) {
          group.disks = (disksData as any)[group.id] || [];
        }
      }
      if (storageRes.success && storageRes.data) {
        storageDisks = storageRes.data;
      }
    } catch (e) {
      console.error('Failed to load disks:', e);
    }
  }

  async function navigateTo(path: string) {
    browsing = true;
    loading = true;
    currentPath = path;
    const res = await api.listDirectory(path);
    if (res.success && res.data) {
      // Folder mode only ever shows directories (unchanged from before);
      // file mode shows both, so the user can navigate in *and* pick a file.
      entries = mode === 'file'
        ? res.data.entries
        : res.data.entries.filter((e: FileEntry) => e.is_directory);
    } else {
      entries = [];
    }
    loading = false;
  }

  function pickEntry(entry: FileEntry) {
    if (entry.is_directory) {
      navigateTo(entry.path);
    } else {
      selectPath(entry.path);
    }
  }

  function navigateUp() {
    const p = currentPath.replace(/\/$/, '');
    const idx = p.lastIndexOf('/');
    if (idx > 0) {
      navigateTo(p.substring(0, idx));
    } else if (idx === 0) {
      navigateTo('/');
    } else {
      goToRoots();
    }
  }

  function goToRoots() {
    browsing = false;
    currentPath = '/';
    entries = [];
  }

  function selectPath(path: string) {
    onselect?.(path);
  }

  function close() {
    onclose?.();
  }

  // Portal the overlay to <body>: it is `position: fixed`, and a desktop
  // window is a transformed / `overflow: hidden` ancestor — inside one the
  // overlay resolves against the window box instead of the viewport, so a
  // modal taller than the window was clipped at the top and its close button
  // became unreachable (confirmed live: `.hv-docker-monitor` intercepted the
  // click). Same rationale as `FloatingPathPicker` / `VmConsole`.
  function portal(node: HTMLElement) {
    document.body.appendChild(node);
    return { destroy() { node.remove(); } };
  }

  function formatSize(bytes: number): string {
    if (bytes === 0) return '';
    const units = ['B', 'KB', 'MB', 'GB', 'TB'];
    const i = Math.floor(Math.log(bytes) / Math.log(1024));
    return (bytes / Math.pow(1024, i)).toFixed(1) + ' ' + units[i];
  }

  function getDiskLabel(disk: DiskInfo): string {
    const parts = disk.mount_point.split('/').filter(Boolean);
    return parts[parts.length - 1] || disk.mount_point;
  }
</script>

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="hv-fp-overlay"
    onclick={close}
    use:portal
    use:focusTrap={{ escape: close }}
    role="dialog"
    tabindex="-1"
    aria-modal="true"
  >
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="hv-fp-modal" onclick={(e) => e.stopPropagation()}>
      <!-- Header -->
      <div class="hv-fp-header">
        <div class="hv-fp-title">
          <IconFolderOpen size={18} />
          <span>{mode === 'file' ? $t('folder_picker.title_file') : $t('folder_picker.title')}</span>
        </div>
        <button class="hv-fp-close" onclick={close} aria-label={$t('common.close')}><IconX size={16} /></button>
      </div>

      <!-- Breadcrumb -->
      <div class="hv-fp-breadcrumb">
        <button class="hv-fp-bc-btn hv-ic" onclick={goToRoots} title={$t('folder_picker.root')} aria-label={$t('folder_picker.root')}>
          <IconServer size={14} />
        </button>
        {#if browsing}
          <button class="hv-fp-bc-btn hv-ic" onclick={navigateUp} title={$t('folder_picker.up')} aria-label={$t('folder_picker.up')}>
            <IconArrowUp size={14} />
          </button>
          <span class="hv-fp-bc-sep">/</span>
          <span class="hv-fp-bc-path">{currentPath}</span>
        {/if}
      </div>

      <!-- Content -->
      <div class="hv-fp-content">
        {#if loading}
          <div class="hv-fp-loading">
            <IconLoader size={24} class="hv-spin" />
          </div>
        {:else if !browsing}
          <!-- Root view: grouped disk categories -->
          <div class="hv-fp-roots">
            {#each diskGroups as group}
              {#if group.disks.length > 0}
                {@const GroupIcon = group.icon}
                <div class="hv-fp-group-label">
                  <GroupIcon size={14} color={group.color} />
                  <span>{$t(`folder_picker.group_${group.id}`)}</span>
                </div>
                {#each group.disks as disk}
                  <button class="hv-fp-item" onclick={() => navigateTo(disk.mount_point)}>
                    <GroupIcon size={16} color={group.color} />
                    <div class="hv-fp-item-info">
                      <span class="hv-fp-item-name">{getDiskLabel(disk)}</span>
                      <span class="hv-fp-item-path">{disk.mount_point}</span>
                    </div>
                    <span class="hv-fp-item-size">{formatSize(disk.size_bytes)}</span>
                    <IconChevronRight size={14} class="hv-fp-chevron" />
                  </button>
                {/each}
              {/if}
            {/each}
            <div class="hv-fp-group-label">
              <IconFolder size={14} color="var(--hv-text-tertiary)" />
              <span>{$t('folder_picker.group_root')}</span>
            </div>
            <button class="hv-fp-item" onclick={() => navigateTo('/')}>
              <IconHome size={16} color="var(--hv-text-secondary)" />
              <div class="hv-fp-item-info">
                <span class="hv-fp-item-name">{$t('folder_picker.system_root')}</span>
                <span class="hv-fp-item-path">/</span>
              </div>
              <IconChevronRight size={14} class="hv-fp-chevron" />
            </button>
          </div>
        {:else}
          <!-- Folder (and, in file mode, file) browsing view -->
          <div class="hv-fp-folders">
            {#if entries.length === 0}
              <div class="hv-fp-empty">{$t('folder_picker.empty_folder')}</div>
            {:else}
              {#each entries as entry}
                <button class="hv-fp-item" onclick={() => pickEntry(entry)}>
                  {#if entry.is_directory}
                    <IconFolder size={16} color="var(--hv-warning)" />
                  {:else}
                    <FileIcon size={16} color="var(--hv-text-tertiary)" />
                  {/if}
                  <span class="hv-fp-item-name">{entry.name}</span>
                  {#if entry.is_directory}
                    <IconChevronRight size={14} class="hv-fp-chevron" />
                  {/if}
                </button>
              {/each}
            {/if}
          </div>
        {/if}
      </div>

      <!-- Footer -->
      <div class="hv-fp-footer">
        <button class="hv-btn hv-btn-secondary hv-btn-sm" onclick={close}>{$t('common.cancel')}</button>
        {#if mode === 'folder'}
        <button
          class="hv-btn hv-btn-primary hv-btn-sm"
          onclick={() => selectPath(currentPath)}
          disabled={currentPath === '/' && !browsing}
        >
          <IconFolderOpen size={14} />
          <span>{$t('folder_picker.select_this_folder')}</span>
        </button>
        {/if}
      </div>
    </div>
  </div>
{/if}

<style>
  .hv-fp-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: var(--hv-z-modal);
  }

  .hv-fp-modal {
    width: 560px;
    max-width: calc(100vw - 32px);
    max-height: 640px;
    background: var(--hv-surface);
    border-radius: var(--hv-radius-xl);
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.3);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .hv-fp-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: var(--hv-space-4) var(--hv-space-5);
    border-bottom: 1px solid var(--hv-border);
  }

  .hv-fp-title {
    display: flex;
    align-items: center;
    gap: var(--hv-space-2);
    font-size: var(--hv-font-size-md);
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-text-primary);
  }

  .hv-fp-title :global(svg) {
    color: var(--hv-primary);
  }

  .hv-fp-close {
    width: 28px;
    height: 28px;
    border: none;
    background: transparent;
    border-radius: var(--hv-radius-md);
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--hv-text-tertiary);
    transition:
      color var(--hv-transition-base) var(--hv-ease-default),
      background-color var(--hv-transition-base) var(--hv-ease-default);
  }

  .hv-fp-close:hover {
    background: var(--hv-surface-container);
    color: var(--hv-text-primary);
  }

  .hv-fp-breadcrumb {
    display: flex;
    align-items: center;
    gap: var(--hv-space-1);
    padding: var(--hv-space-2) var(--hv-space-5);
    background: var(--hv-surface-container-lowest);
    border-bottom: 1px solid var(--hv-border);
    font-size: var(--hv-font-size-xs);
  }

  .hv-fp-bc-btn {
    width: 28px;
    height: 28px;
    border: none;
    background: transparent;
    border-radius: var(--hv-radius-md);
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--hv-text-secondary);
    transition:
      color var(--hv-transition-base) var(--hv-ease-default),
      background-color var(--hv-transition-base) var(--hv-ease-default);
  }

  .hv-fp-bc-btn:hover {
    background: var(--hv-surface-container);
    color: var(--hv-text-primary);
  }

  .hv-fp-bc-sep {
    color: var(--hv-text-tertiary);
  }

  .hv-fp-bc-path {
    color: var(--hv-text-secondary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .hv-fp-content {
    flex: 1;
    overflow-y: auto;
    padding: var(--hv-space-2);
    min-height: 260px;
  }

  .hv-fp-loading {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 80px 20px;
  }

  .hv-fp-loading :global(.hv-spin) {
    animation: spin 1s linear infinite;
    color: var(--hv-primary);
  }

  .hv-fp-roots, .hv-fp-folders {
    display: flex;
    flex-direction: column;
    gap: var(--hv-space-1);
  }

  .hv-fp-group-label {
    display: flex;
    align-items: center;
    gap: var(--hv-space-2);
    padding: var(--hv-space-3) var(--hv-space-3) var(--hv-space-1);
    font-size: var(--hv-font-size-xs);
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-text-tertiary);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .hv-fp-item {
    display: flex;
    align-items: center;
    gap: var(--hv-space-3);
    padding: var(--hv-space-2) var(--hv-space-3);
    border: none;
    background: transparent;
    border-radius: var(--hv-radius-md);
    cursor: pointer;
    color: var(--hv-text-primary);
    font-size: var(--hv-font-size-sm);
    text-align: left;
    transition:
      color var(--hv-transition-base) var(--hv-ease-default),
      background-color var(--hv-transition-base) var(--hv-ease-default);
    width: 100%;
  }

  .hv-fp-item:hover {
    background: var(--hv-surface-container);
  }

  .hv-fp-item :global(svg) {
    flex-shrink: 0;
  }

  .hv-fp-item-info {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .hv-fp-item-name {
    font-weight: var(--hv-font-weight-medium);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .hv-fp-item-path {
    font-size: var(--hv-font-size-xs);
    color: var(--hv-text-tertiary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .hv-fp-item-size {
    font-size: var(--hv-font-size-xs);
    color: var(--hv-text-tertiary);
    flex-shrink: 0;
  }

  :global(.hv-fp-chevron) {
    color: var(--hv-text-tertiary) !important;
    flex-shrink: 0;
  }

  .hv-fp-empty {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 40px 20px;
    color: var(--hv-text-tertiary);
    font-size: var(--hv-font-size-sm);
  }

  .hv-fp-footer {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: var(--hv-space-2);
    padding: var(--hv-space-3) var(--hv-space-5);
    border-top: 1px solid var(--hv-border);
    background: var(--hv-surface-container-lowest);
  }

  @keyframes spin {
    from { transform: rotate(0deg); }
    to { transform: rotate(360deg); }
  }
</style>
