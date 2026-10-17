<!-- ========================================================================
     File          : FloatingPathPicker.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Non-modal, draggable folder/file picker: browses the server's allowed
       roots (PathGuard-scoped), distinguishes drives/categories via icons,
       and returns the chosen path without blocking the rest of the UI.

     Dependencies:
       - svelte (>= 5), @tabler/icons-svelte, $lib/services/api

     Usage:
       Opened by PathPickerField; see the in-code doc for the full contract.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { t } from '$lib/stores/i18n';
  import {
    IconFolderOpen, IconFolder, IconServer, IconChevronRight,
    IconArrowUp, IconLoader, IconShieldCheckered, IconUsb, IconCloud,
    IconNetwork, IconDeviceDesktop, IconLayoutGrid, IconLayoutList,
    IconGripVertical, IconX, IconFile as FileIcon,
  } from '@tabler/icons-svelte';

  /**
   * Non-modal, draggable folder/file picker.
   *
   * The standard picker for new code: same `mode` contract as the (frozen)
   * modal `FolderPicker.svelte`, but it opens as a floating panel next to its
   * trigger — no screen-blocking backdrop, drag by the header, closes on
   * Escape / ✕ / a selection. Portalled to <body> so a transformed /
   * overflow-clipped ancestor (a desktop window) can't trap it.
   *
   * Layout: a left sidebar of locations (physical disks, USB, RAID virtual
   * disks, cloud, network) + a main pane that lists the current folder as a
   * list or a grid (the choice is remembered in localStorage).
   *
   * Pair it with `PathPickerField.svelte` for the common "read-only field +
   * Browse button" shape, or drive `open`/`onselect`/`onclose` directly.
   */
  let {
    open = false,
    value = '',
    mode = 'folder',
    anchor = null,
    title,
    onselect,
    onclose,
  }: {
    open?: boolean;
    value?: string;
    mode?: 'folder' | 'file';
    /** The element that opened the picker — the panel is placed next to it. */
    anchor?: HTMLElement | null;
    /** Header label; defaults to the mode-appropriate i18n string. */
    title?: string;
    onselect?: (path: string) => void;
    onclose?: () => void;
  } = $props();

  interface FileEntry { name: string; path: string; is_directory: boolean; }
  interface DiskInfo {
    name: string; path: string; mount_point: string; filesystem: string;
    size_bytes: number; used_bytes: number; category: string;
    label?: string; vendor?: string; model?: string; serial?: string;
  }

  const headerTitle = $derived(
    title ?? (mode === 'file' ? $t('folder_picker.title_file') : $t('folder_picker.title')),
  );

  // Runes mode: every value the template reads must be $state or it never
  // re-renders (same bug class as FolderPicker.svelte / CLAUDE.md).
  let currentPath = $state('/');
  let entries: FileEntry[] = $state([]);
  let loading = $state(true);
  let loadedFor = $state<string | null>(null);   // guards the one-time load per open
  let disksData = $state<Record<string, DiskInfo[]>>({});

  // View mode (list | grid) — remembered per browser.
  let viewMode = $state<'list' | 'grid'>('list');
  function loadViewPref() {
    try {
      const v = localStorage.getItem('hv-pathpicker-view');
      if (v === 'grid' || v === 'list') viewMode = v;
    } catch { /* storage blocked — fall back to list */ }
  }
  function setView(m: 'list' | 'grid') {
    viewMode = m;
    try { localStorage.setItem('hv-pathpicker-view', m); } catch { /* ignore */ }
  }

  // De-dupe a disk list by mount point (the kernel exposes several entries for
  // the same path) and, for "physical", drop pseudo-filesystems (devpts, sysfs,
  // cgroup, … all report 0 bytes) so the sidebar shows real drives only.
  function tidy(list: DiskInfo[], realOnly = false): DiskInfo[] {
    const seen = new Set<string>();
    const out: DiskInfo[] = [];
    for (const d of list) {
      if (realOnly && !d.size_bytes) continue;
      const mp = d.mount_point || d.path;
      if (seen.has(mp)) continue;
      seen.add(mp);
      out.push(d);
    }
    return out;
  }

  // Sidebar sections. "Physical" merges the OS disk(s) + internal drives and
  // always leads with a "This device" entry for the filesystem root. The rest
  // only show when the host actually has that class of storage.
  const sidebar = $derived.by(() => {
    const d = disksData;
    const sections: { id: string; label: string; icon: any; color: string; disks: DiskInfo[] }[] = [
      { id: 'physical', label: $t('folder_picker.group_physical'), icon: IconServer,
        color: 'var(--hv-primary)',
        // root "/" is already the "This device" entry above
        disks: tidy([...(d.system ?? []), ...(d.internal ?? [])], true).filter((x) => x.mount_point !== '/') },
      { id: 'external', label: $t('folder_picker.group_external'), icon: IconUsb,
        color: 'var(--hv-warning)', disks: tidy(d.external ?? []) },
      { id: 'raid', label: $t('folder_picker.group_raid'), icon: IconShieldCheckered,
        color: 'var(--hv-success)', disks: tidy(d.raid ?? []) },
      { id: 'cloud', label: $t('folder_picker.group_cloud'), icon: IconCloud,
        color: 'var(--hv-app-dashboard-c)', disks: tidy(d.cloud ?? []) },
      { id: 'network', label: $t('folder_picker.group_network'), icon: IconNetwork,
        color: 'var(--hv-app-journal-c)', disks: tidy(d.network ?? []) },
    ];
    return sections.filter((s) => s.id === 'physical' || s.disks.length > 0);
  });

  // The location whose mount point is the longest prefix of the current path —
  // that's the sidebar row to highlight.
  const activeMount = $derived.by(() => {
    let best = '';
    for (const s of sidebar) {
      for (const disk of s.disks) {
        const mp = disk.mount_point.replace(/\/$/, '') || '/';
        if ((currentPath === mp || currentPath.startsWith(mp === '/' ? '/' : mp + '/')) && mp.length > best.length) {
          best = mp;
        }
      }
    }
    return best || (currentPath === '/' ? '/' : '');
  });

  // ── Floating-panel geometry (non-modal: no backdrop, draggable) ──────────
  let panelW = $state(560);
  let panelH = $state(480);
  let posX = $state(0);
  let posY = $state(0);

  function clampToViewport() {
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    posX = Math.max(8, Math.min(posX, vw - panelW - 8));
    posY = Math.max(8, Math.min(posY, vh - 44));            // keep the drag bar reachable
  }

  function placeNextToAnchor() {
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    panelW = Math.min(560, vw - 16);
    panelH = Math.min(560, Math.round(vh * 0.78));
    if (anchor) {
      const r = anchor.getBoundingClientRect();
      posX = r.left;
      // Prefer below the trigger; flip above if it would overflow the viewport.
      posY = r.bottom + 8 + panelH > vh - 8 && r.top - 8 - panelH > 8
        ? r.top - 8 - panelH
        : r.bottom + 8;
    } else {
      posX = (vw - panelW) / 2;
      posY = Math.max(24, (vh - panelH) / 2);
    }
    clampToViewport();
  }

  // Drag by the header. Window-level listeners (added on pointerdown, removed
  // on pointerup) rather than setPointerCapture — capture would retarget the
  // header's own buttons' click events and break the ✕ button.
  let dragDX = 0;
  let dragDY = 0;
  function onDragMove(e: PointerEvent) {
    posX = e.clientX - dragDX;
    posY = e.clientY - dragDY;
    clampToViewport();
  }
  function onDragEnd() {
    window.removeEventListener('pointermove', onDragMove);
    window.removeEventListener('pointerup', onDragEnd);
  }
  function onDragStart(e: PointerEvent) {
    if ((e.target as HTMLElement).closest('button')) return;   // not the header buttons
    dragDX = e.clientX - posX;
    dragDY = e.clientY - posY;
    window.addEventListener('pointermove', onDragMove);
    window.addEventListener('pointerup', onDragEnd);
  }

  // Load disks + navigate to the current value the first time this opens.
  $effect(() => {
    const openKey = open ? value + '|' + mode : null;
    if (openKey && loadedFor !== openKey) {
      loadedFor = openKey;
      placeNextToAnchor();
      void init();
    } else if (!open) {
      loadedFor = null;
    }
  });

  async function init() {
    loading = true;
    loadViewPref();
    await loadAllDisks();
    // Navigate to the directory containing `value` (or `value` itself if it's
    // a directory); a raw file path still lands us in its folder. No value →
    // start at the filesystem root.
    const start = value && mode === 'file' && !value.endsWith('/')
      ? value.slice(0, value.lastIndexOf('/')) || '/'
      : value || '/';
    await navigateTo(start);
  }

  async function loadAllDisks() {
    try {
      const mountsRes = await api.getDisks();
      if (mountsRes.success && mountsRes.data) {
        disksData = mountsRes.data as Record<string, DiskInfo[]>;
      }
    } catch (e) {
      console.error('FloatingPathPicker: failed to load disks', e);
    }
  }

  async function navigateTo(path: string) {
    loading = true;
    currentPath = path;
    const res = await api.listDirectory(path);
    if (res.success && res.data) {
      entries = mode === 'file'
        ? res.data.entries
        : res.data.entries.filter((e: FileEntry) => e.is_directory);
    } else {
      entries = [];
    }
    loading = false;
  }

  function pickEntry(entry: FileEntry) {
    if (entry.is_directory) navigateTo(entry.path);
    else if (mode === 'file') onselect?.(entry.path);
  }

  function navigateUp() {
    const p = currentPath.replace(/\/$/, '');
    const idx = p.lastIndexOf('/');
    if (idx > 0) navigateTo(p.substring(0, idx));
    else navigateTo('/');
  }

  function confirmSelection() {
    onselect?.(currentPath);
  }

  function close() {
    onclose?.();
  }

  function onKeydown(e: KeyboardEvent) {
    if (open && e.key === 'Escape') {
      e.stopPropagation();
      close();
    }
  }

  function formatSize(bytes: number): string {
    if (!bytes) return '';
    const units = ['B', 'KB', 'MB', 'GB', 'TB'];
    const i = Math.floor(Math.log(bytes) / Math.log(1024));
    return (bytes / Math.pow(1024, i)).toFixed(1) + ' ' + units[i];
  }

  function diskLabel(disk: DiskInfo): string {
    // 1) the filesystem label the user gave the volume ("Backup", "Data", …)
    if (disk.label?.trim()) return disk.label.trim();
    // 2) the physical disk's make + model ("KINGSTON OM3PGP4128P-AH")
    const makeModel = [disk.vendor, disk.model].map((s) => s?.trim()).filter(Boolean).join(' ');
    if (makeModel) return makeModel;
    // 3) the mount-point basename ("efi", "data") — the raw /dev node means
    //    nothing to a non-technical admin
    const parts = disk.mount_point.split('/').filter(Boolean);
    return parts[parts.length - 1] || disk.mount_point;
  }

  function diskSubline(disk: DiskInfo): string {
    const size = disk.size_bytes ? formatSize(disk.size_bytes) : '';
    // With a label, the make/model is the useful secondary detail; without
    // one, the label line already shows make/model so lead with the serial.
    const bits = disk.label?.trim()
      ? [[disk.vendor, disk.model].map((s) => s?.trim()).filter(Boolean).join(' '), size]
      : [disk.serial?.trim() ? `${$t('folder_picker.serial')} ${disk.serial.trim()}` : disk.mount_point, size];
    return bits.filter(Boolean).join(' · ');
  }

  // Portal the panel to <body>: a transformed / overflow:hidden ancestor
  // (a desktop window) would otherwise trap and clip this position:fixed node.
  function portal(node: HTMLElement) {
    document.body.appendChild(node);
    return { destroy() { node.remove(); } };
  }

  onMount(() => {
    window.addEventListener('resize', clampToViewport);
    return () => {
      window.removeEventListener('resize', clampToViewport);
      window.removeEventListener('pointermove', onDragMove);
      window.removeEventListener('pointerup', onDragEnd);
    };
  });
</script>

<svelte:window onkeydown={onKeydown} />

{#if open}
  <div
    class="hv-fpp"
    use:portal
    style="left:{posX}px; top:{posY}px; width:{panelW}px; max-height:{panelH}px;"
    role="dialog"
    aria-label={headerTitle}
  >
    <!-- role="group": the header is a pointer-drag handle (keyboard users
         lose nothing — drag is pointer-only on every panel); the role
         marks the labelled control cluster for AT. -->
    <div class="hv-fpp-header" onpointerdown={onDragStart} role="group">
      <IconGripVertical size={15} class="hv-fpp-grip" />
      <IconFolderOpen size={15} />
      <span class="hv-fpp-title">{headerTitle}</span>
      <div class="hv-fpp-viewtoggle" role="group" aria-label={$t('folder_picker.view_list')}>
        <button
          type="button"
          class="hv-fpp-vbtn"
          class:is-on={viewMode === 'list'}
          onclick={() => setView('list')}
          aria-pressed={viewMode === 'list'}
          aria-label={$t('folder_picker.view_list')}
          title={$t('folder_picker.view_list')}
        >
          <IconLayoutList size={14} />
        </button>
        <button
          type="button"
          class="hv-fpp-vbtn"
          class:is-on={viewMode === 'grid'}
          onclick={() => setView('grid')}
          aria-pressed={viewMode === 'grid'}
          aria-label={$t('folder_picker.view_grid')}
          title={$t('folder_picker.view_grid')}
        >
          <IconLayoutGrid size={14} />
        </button>
      </div>
      <button type="button" class="hv-fpp-x" onclick={close} aria-label={$t('common.close')}>
        <IconX size={15} />
      </button>
    </div>

    <div class="hv-fpp-body">
      <!-- Sidebar: locations -->
      <nav class="hv-fpp-sidebar" aria-label={$t('folder_picker.places')}>
        <button
          type="button"
          class="hv-fpp-place"
          class:is-active={activeMount === '/'}
          onclick={() => navigateTo('/')}
        >
          <IconDeviceDesktop size={15} color="var(--hv-text-secondary)" />
          <span class="hv-fpp-place-name">{$t('folder_picker.this_device')}</span>
        </button>
        {#each sidebar as section (section.id)}
          {@const SIcon = section.icon}
          <div class="hv-fpp-sb-group">{section.label}</div>
          {#if section.disks.length === 0}
            <div class="hv-fpp-sb-empty">—</div>
          {:else}
            {#each section.disks as disk, di (section.id + ':' + disk.mount_point + ':' + di)}
              <button
                type="button"
                class="hv-fpp-place"
                class:is-active={activeMount === (disk.mount_point.replace(/\/$/, '') || '/')}
                onclick={() => navigateTo(disk.mount_point)}
                title={disk.mount_point}
              >
                <SIcon size={15} color={section.color} />
                <span class="hv-fpp-place-main">
                  <span class="hv-fpp-place-name">{diskLabel(disk)}</span>
                  <span class="hv-fpp-place-sub">{diskSubline(disk)}</span>
                </span>
              </button>
            {/each}
          {/if}
        {/each}
      </nav>

      <!-- Main pane -->
      <div class="hv-fpp-main">
        <div class="hv-fpp-crumbs">
          <button
            type="button"
            class="hv-fpp-crumb-btn"
            onclick={navigateUp}
            disabled={currentPath === '/'}
            title={$t('folder_picker.up')}
            aria-label={$t('folder_picker.up')}
          >
            <IconArrowUp size={13} />
          </button>
          <span class="hv-fpp-crumb-path" title={currentPath}>{currentPath}</span>
        </div>

        <div class="hv-fpp-content">
          {#if loading}
            <div class="hv-fpp-loading"><IconLoader size={22} class="hv-fpp-spin" /></div>
          {:else if entries.length === 0}
            <div class="hv-fpp-empty">{$t('folder_picker.empty_folder')}</div>
          {:else if viewMode === 'grid'}
            <div class="hv-fpp-grid">
              {#each entries as entry (entry.path)}
                <button type="button" class="hv-fpp-tile" onclick={() => pickEntry(entry)} title={entry.name}>
                  {#if entry.is_directory}
                    <IconFolder size={30} color="var(--hv-warning)" />
                  {:else}
                    <FileIcon size={30} color="var(--hv-text-tertiary)" />
                  {/if}
                  <span class="hv-fpp-tile-name">{entry.name}</span>
                </button>
              {/each}
            </div>
          {:else}
            <div class="hv-fpp-rows">
              {#each entries as entry (entry.path)}
                <button type="button" class="hv-fpp-row" onclick={() => pickEntry(entry)}>
                  {#if entry.is_directory}
                    <IconFolder size={15} color="var(--hv-warning)" />
                  {:else}
                    <FileIcon size={15} color="var(--hv-text-tertiary)" />
                  {/if}
                  <span class="hv-fpp-row-name">{entry.name}</span>
                  {#if entry.is_directory}
                    <IconChevronRight size={13} class="hv-fpp-chev" />
                  {/if}
                </button>
              {/each}
            </div>
          {/if}
        </div>
      </div>
    </div>

    <div class="hv-fpp-footer">
      <span class="hv-fpp-current" title={currentPath}>{currentPath}</span>
      {#if mode === 'folder'}
        <button type="button" class="hv-fpp-confirm" onclick={confirmSelection} disabled={!currentPath}>
          <IconFolderOpen size={14} />
          <span>{$t('folder_picker.select_this_folder')}</span>
        </button>
      {/if}
    </div>
  </div>
{/if}

<style>
  .hv-fpp {
    position: fixed;
    z-index: var(--hv-z-dropdown);
    display: flex;
    flex-direction: column;
    background: var(--hv-surface-container-lowest);
    border: 1px solid var(--hv-border);
    border-radius: var(--hv-radius-xl);
    box-shadow: 0 18px 50px rgba(10, 14, 40, 0.28);
    overflow: hidden;
    font-size: var(--hv-text-base);
    color: var(--hv-text-primary);
  }

  /* ── Header ── */
  .hv-fpp-header {
    display: flex;
    align-items: center;
    gap: var(--hv-space-2);
    padding: var(--hv-space-2) var(--hv-space-2-5) var(--hv-space-2) var(--hv-space-2);
    border-bottom: 1px solid var(--hv-border);
    background: var(--hv-surface-container-low);
    cursor: grab;
    touch-action: none;
    user-select: none;
    flex-shrink: 0;
  }
  .hv-fpp-header:active { cursor: grabbing; }
  .hv-fpp-header :global(.hv-fpp-grip) { color: var(--hv-text-tertiary); }
  .hv-fpp-title {
    flex: 1;
    font-weight: var(--hv-font-weight-semibold);
    font-size: var(--hv-text-base);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .hv-fpp-viewtoggle {
    display: inline-flex;
    border: 1px solid var(--hv-border);
    border-radius: var(--hv-radius-control);
    overflow: hidden;
    background: var(--hv-surface-container-lowest);
  }
  .hv-fpp-vbtn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: var(--hv-touch-target-min, 44px);
    height: var(--hv-touch-target-min, 44px);
    border: none;
    background: transparent;
    color: var(--hv-text-tertiary);
    cursor: pointer;
  }
  .hv-fpp-vbtn + .hv-fpp-vbtn { border-left: 1px solid var(--hv-border); }
  .hv-fpp-vbtn:hover { background: var(--hv-surface-container); }
  .hv-fpp-vbtn.is-on { background: var(--hv-primary); color: var(--hv-on-primary); }
  .hv-fpp-x {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: var(--hv-touch-target-min, 44px);
    height: var(--hv-touch-target-min, 44px);
    border: none;
    background: transparent;
    border-radius: var(--hv-radius-control);
    color: var(--hv-text-tertiary);
    cursor: pointer;
  }
  .hv-fpp-x:hover { background: var(--hv-surface-container); color: var(--hv-text-primary); }

  /* ── Body: sidebar + main ── */
  .hv-fpp-body {
    flex: 1;
    display: flex;
    min-height: 0;
  }

  .hv-fpp-sidebar {
    width: 152px;
    flex-shrink: 0;
    border-right: 1px solid var(--hv-border);
    background: var(--hv-surface-container-lowest);
    overflow-y: auto;
    padding: 6px;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .hv-fpp-sb-group {
    padding: var(--hv-space-2) var(--hv-space-1-5) var(--hv-space-0-75);
    font-size: var(--hv-font-size-micro);
    font-weight: var(--hv-font-weight-bold);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--hv-text-tertiary);
  }
  .hv-fpp-sb-empty { padding: 2px 8px 4px; color: var(--hv-text-tertiary); font-size: var(--hv-text-xs); }
  .hv-fpp-place {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    min-height: var(--hv-touch-target-min, 44px);
    padding: var(--hv-space-1-5) var(--hv-space-2);
    border: none;
    background: transparent;
    border-radius: var(--hv-radius-control);
    text-align: left;
    cursor: pointer;
    color: var(--hv-text-primary);
    font-size: var(--hv-text-base);
  }
  .hv-fpp-place:hover { background: var(--hv-surface-container); }
  .hv-fpp-place.is-active { background: var(--hv-overlay-hover, rgba(15, 108, 189, 0.14)); color: var(--hv-primary); font-weight: var(--hv-font-weight-semibold); }
  .hv-fpp-place :global(svg) { flex-shrink: 0; }
  .hv-fpp-place-main { flex: 1; display: flex; flex-direction: column; min-width: 0; }
  .hv-fpp-place-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .hv-fpp-place-sub {
    font-size: var(--hv-font-size-micro);
    color: var(--hv-text-tertiary);
    font-weight: var(--hv-font-weight-regular);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .hv-fpp-main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }

  .hv-fpp-crumbs {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: var(--hv-space-1-5) var(--hv-space-2-5);
    border-bottom: 1px solid var(--hv-border);
    background: var(--hv-surface-container-lowest);
    font-size: var(--hv-text-xs);
    color: var(--hv-text-secondary);
    flex-shrink: 0;
  }
  .hv-fpp-crumb-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: var(--hv-touch-target-min, 44px);
    height: var(--hv-touch-target-min, 44px);
    border: none;
    background: transparent;
    border-radius: var(--hv-radius-control);
    color: var(--hv-text-secondary);
    cursor: pointer;
    flex-shrink: 0;
  }
  .hv-fpp-crumb-btn:hover:not(:disabled) { background: var(--hv-surface-container); }
  .hv-fpp-crumb-btn:disabled { opacity: 0.35; cursor: default; }
  .hv-fpp-crumb-path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;
    text-align: left;
  }

  .hv-fpp-content {
    flex: 1;
    overflow-y: auto;
    padding: 6px;
    min-height: 140px;
  }
  .hv-fpp-loading { display: flex; align-items: center; justify-content: center; padding: 48px 0; }
  .hv-fpp-content :global(.hv-fpp-spin) { animation: hv-fpp-spin 1s linear infinite; color: var(--hv-primary); }
  @keyframes hv-fpp-spin { to { transform: rotate(360deg); } }
  .hv-fpp-empty {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 40px 12px;
    color: var(--hv-text-tertiary);
  }

  /* ── List view ── */
  .hv-fpp-rows { display: flex; flex-direction: column; gap: 1px; }
  .hv-fpp-row {
    display: flex;
    align-items: center;
    gap: var(--hv-space-2);
    width: 100%;
    min-height: var(--hv-touch-target-min, 44px);
    padding: var(--hv-space-2) var(--hv-space-2);
    border: none;
    background: transparent;
    border-radius: var(--hv-radius-control);
    text-align: left;
    cursor: pointer;
    color: var(--hv-text-primary);
    font-size: var(--hv-text-base);
  }
  .hv-fpp-row:hover { background: var(--hv-surface-container); }
  .hv-fpp-row :global(svg) { flex-shrink: 0; }
  .hv-fpp-row-name { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .hv-fpp :global(.hv-fpp-chev) { color: var(--hv-text-tertiary) !important; flex-shrink: 0; }

  /* ── Grid view ── */
  .hv-fpp-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(84px, 1fr));
    gap: 3px;
  }
  .hv-fpp-tile {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 5px;
    min-height: var(--hv-touch-target-min, 44px);
    padding: var(--hv-space-2-5) var(--hv-space-1);
    border: none;
    background: transparent;
    border-radius: var(--hv-radius-control);
    cursor: pointer;
    color: var(--hv-text-primary);
  }
  .hv-fpp-tile:hover { background: var(--hv-surface-container); }
  .hv-fpp-tile :global(svg) { flex-shrink: 0; }
  .hv-fpp-tile-name {
    font-size: var(--hv-text-xs);
    line-height: var(--hv-line-height-tight);
    text-align: center;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
    word-break: break-word;
  }

  /* ── Footer ── */
  .hv-fpp-footer {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: var(--hv-space-2) var(--hv-space-2-5);
    border-top: 1px solid var(--hv-border);
    background: var(--hv-surface-container-lowest);
    flex-shrink: 0;
  }
  .hv-fpp-current {
    flex: 1;
    min-width: 0;
    font-size: var(--hv-text-xs);
    color: var(--hv-text-secondary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;
    text-align: left;
  }
  .hv-fpp-confirm {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: var(--hv-touch-target-min, 44px);
    padding: 0 12px;
    border: 1px solid var(--hv-primary);
    border-radius: var(--hv-radius-control);
    background: var(--hv-primary);
    color: var(--hv-on-primary);
    font-size: var(--hv-text-base);
    font-weight: var(--hv-font-weight-semibold);
    white-space: nowrap;
    cursor: pointer;
  }
  .hv-fpp-confirm:disabled { opacity: 0.5; cursor: not-allowed; }

  @media (max-width: 520px) {
    .hv-fpp-sidebar { width: 124px; }
  }
</style>
