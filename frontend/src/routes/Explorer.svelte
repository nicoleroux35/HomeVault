<!-- ========================================================================
     File          : Explorer.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 13/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
     The File Explorer app: browse/search within the user's allowed roots,
     create/rename/delete/copy/move, uploads (multipart + resumable chunks),
     downloads via tickets, previews and thumbnails, archive extract/compress,
     trash with restore/purge, share-by-link, and per-folder permissions —
     with the a11y row-activation helpers and confirm dialogs.

     Dependencies:
       - svelte (>= 5), theme primitives, app/ composites, utils
         (confirmDialog, activateOnEnterOrSpace), $lib/services/api

     Usage:
     Opened as a desktop window app.
     ======================================================================== -->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { Icon, Button, Modal } from '$lib/components/theme';
  import { PageLayout, ConfirmDialog } from '$lib/components/app';
  import { createConfirmState } from '$lib/utils/confirmDialog.svelte';
  import { activateOnEnterOrSpace } from '$lib/utils/activateOnEnterOrSpace';
  import WindowSidebarItem from '$lib/components/desktop/WindowSidebarItem.svelte';
  import WindowSidebarSection from '$lib/components/desktop/WindowSidebarSection.svelte';
  import { cn } from '$lib/utils.js';
  import { api } from '$lib/services/api';
  import WindowHeader from '$lib/components/desktop/WindowHeader.svelte';
  import { t, translate } from '$lib/stores/i18n';
  import { explorerShareIntent } from '$lib/stores/explorerShareIntent';
  import FolderPicker from '$lib/components/theme/FolderPicker.svelte';
  import { IconAlertCircle, IconArchive, IconArrowBackUp, IconArrowUp, IconClock, IconCloud, IconCloudCheck, IconCloudUpload, IconCopy, IconDatabase, IconDownload, IconHistory, IconPlayerEject, IconPencil, IconEye, IconFileZip, IconFolderMinus, IconFolderOpen, IconFolderPlus, IconServer, IconHome, IconLayoutGrid, IconList, IconLoader, IconLock, IconRefresh, IconSearch, IconShare, IconShield, IconShieldCheckered, IconStar, IconStarFilled, IconTrash, IconUpload, IconUsb, IconX } from '@tabler/icons-svelte';
  import ShareLinkModal from '$lib/components/explorer/ShareLinkModal.svelte';
  import ExplorerWidget from '$lib/components/explorer/ExplorerWidget.svelte';

  interface FileEntry {
    name: string;
    path: string;
    is_directory: boolean;
    size_bytes: number;
    modified: string;
    permissions: string;
    owner: string;
    filesystem: string | null;
  }

  interface DirectoryListing {
    path: string;
    parent: string | null;
    entries: FileEntry[];
    total_size: number;
    file_count: number;
    directory_count: number;
    // Set by the backend when the directory held more than 10,000 items and
    // only the first 10,000 (dirs first, then alphabetical) were returned.
    truncated?: boolean;
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

  interface StorageDisk {
    name: string;
    device: string;
    // Backend's DiskInfo serializes this field as "size", not "size_bytes"
    // (src/disk.rs has #[serde(rename = "size")]) — mismatched here before,
    // which silently rendered every physical disk's size as "NaN undefined".
    size: number;
    vendor: string;
    model: string;
    role: string;
    connection_type: string;
    mount_point?: string;
  }

  let listing = $state<DirectoryListing | null>(null);
  let loading = $state(true);
  let error = $state('');
  let currentPath = $state('/');
  let selected = $state<Set<string>>(new Set());
  let lastSelectedIndex = -1;
  let view = $state<'list' | 'grid'>('list');
  let sortBy = $state<'name' | 'size' | 'modified'>('name');
  let sortDir = $state<'asc' | 'desc'>('asc');
  let disks = $state<{
    system: DiskInfo[];
    internal: DiskInfo[];
    raid: DiskInfo[];
    external: DiskInfo[];
    cloud: DiskInfo[];
    network: DiskInfo[];
  } | null>(null);
  let storageDisks = $state<StorageDisk[]>([]);
  let showNewFolder = $state(false);
  let showRename = $state(false);
  let showPermissions = $state(false);
  let showPreview = $state(false);
  let newFolderName = $state('');
  let renameName = $state('');
  let renameTarget = $state<FileEntry | null>(null);
  let chmodMode = $state('755');
  let chmodTarget = $state<FileEntry | null>(null);
  let previewContent = $state<{ name: string; content: string; is_truncated: boolean; size_bytes: number; mime_type: string; path: string; previewable: boolean } | null>(null);
  let uploading = $state(false);
  let uploadProgress = $state(0);
  let searchQuery = $state('');
  let searchResults = $state<FileEntry[] | null>(null);
  let searching = $state(false);
  let searchDebounceTimer: ReturnType<typeof setTimeout> | null = null;
  // Directory-scoped thumbnail ticket (see `api.getThumbnailTicket`) — lets
  // `<img>` thumbnails authenticate without the session token in the URL.
  let thumbnailTicket = $state('');
  let fileInputEl: HTMLInputElement | null = $state(null);
  let folderPickerAction = $state<'copy' | 'move' | 'extract' | null>(null);
  let extractTarget = $state<FileEntry | null>(null);

  // Labelled sidebar roots ("My files", "Shares", …) + real-time filesystem watcher.
  let roots = $state<{ path: string; kind: string; name: string }[]>([]);
  let watchEventSource: EventSource | null = null;
  let watchTicket = '';

  // Pinned folders (sidebar "Favorites"), synced server-side.
  let favorites = $state<{ path: string; label: string }[]>([]);

  // Recently opened locations (sidebar "Recents"), synced server-side.
  let recents = $state<{ path: string; label: string }[]>([]);

  // Fuzzy (subsequence) search toggle next to the search box.
  let fuzzySearch = $state(false);

  // Whether the current session may format drives (admin-only endpoint).
  let isAdmin = $state(false);

  // Read-only browsing of a past ZFS snapshot ("time travel").
  let snapshots = $state<{ name: string; created: string; browse_path: string }[]>([]);
  let showSnapshots = $state(false);
  let snapshotsLoading = $state(false);
  let snapshotMode = $state(false);
  /** The live path we were looking at before entering a snapshot. */
  let snapshotLivePath = $state('');

  // Inline text/code editor (inside the preview modal).
  let editing = $state(false);
  let editContent = $state('');
  let editSaving = $state(false);
  let editError = $state('');
  // True once the user has typed in the editor since the last save — gates
  // closing the preview so unsaved edits are never silently discarded.
  let editDirty = $state(false);
  let showDiscardConfirm = $state(false);

  // "Share by link" modal (file rows).
  let showShareLink = $state(false);
  let shareLinkTarget = $state<FileEntry | null>(null);

  // Copy/move/upload collision prompt (Replace / Keep both / Skip).
  let collisionPrompt = $state<{ name: string } | null>(null);
  let collisionResolve: ((v: 'overwrite' | 'keep_both' | 'skip' | null) => void) | null = null;

  type Strategy = 'overwrite' | 'keep_both' | 'skip';

  function askCollision(name: string): Promise<Strategy | null> {
    return new Promise((resolve) => {
      collisionPrompt = { name };
      collisionResolve = resolve;
    });
  }

  function resolveCollision(choice: Strategy | null) {
    collisionPrompt = null;
    const r = collisionResolve;
    collisionResolve = null;
    r?.(choice);
  }

  /** Given the names about to land in `destDir`, ask the user how to handle
   *  any name clash and return the chosen strategy (applied to the whole
   *  batch). `null` means the user cancelled the operation. */
  async function resolveStrategy(names: string[], destDir: string): Promise<Strategy | null> {
    let existing = new Set<string>();
    if (destDir === currentPath && listing) {
      existing = new Set(listing.entries.map((e) => e.name));
    } else {
      try {
        const r = await api.listDirectory(destDir);
        if (r.success && r.data?.entries) {
          existing = new Set(r.data.entries.map((e: FileEntry) => e.name));
        }
      } catch {
        /* can't list the destination — proceed and let the backend decide */
      }
    }
    const clashes = names.filter((n) => existing.has(n));
    if (clashes.length === 0) return 'overwrite'; // no conflict — value is moot
    return await askCollision(clashes.join(', '));
  }

  interface TrashEntry {
    id: string;
    name: string;
    original_path: string;
    deleted_at: string;
    is_directory: boolean;
    size_bytes: number;
  }

  let showTrash = $state(false);
  let trashItems = $state<TrashEntry[]>([]);
  let trashLoading = $state(false);

  let showCompress = $state(false);
  let compressName = $state('archive.tar.gz');
  let compressFormat = $state<'tar.gz' | 'zip'>('tar.gz');

  let jobProgress = $state<{ message: string; progress: number } | null>(null);
  let jobUnsubscribe: (() => void) | null = null;
  const dlg = createConfirmState();

  const IMAGE_EXTS = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'svg']);
  const ARCHIVE_EXTS = new Set(['zip', 'tar', 'gz', 'tgz', 'bz2', 'xz', '7z']);
  const TEXT_EXTS = new Set(['txt', 'md', 'json', 'xml', 'csv', 'log', 'yml', 'yaml', 'ini', 'conf', 'html', 'css', 'js', 'ts']);

  function isImage(name: string) {
    const ext = name.split('.').pop()?.toLowerCase() || '';
    return IMAGE_EXTS.has(ext);
  }

  function isArchive(name: string) {
    const lower = name.toLowerCase();
    if (ARCHIVE_EXTS.has(lower.split('.').pop() || '')) return true;
    if (lower.endsWith('.tar.gz') || lower.endsWith('.tgz')) return true;
    return false;
  }

  function isText(name: string) {
    const ext = name.split('.').pop()?.toLowerCase() || '';
    return TEXT_EXTS.has(ext);
  }

  function getFileIcon(entry: FileEntry): string {
    if (entry.is_directory) return 'folder';
    if (isImage(entry.name)) return 'image';
    if (isArchive(entry.name)) return 'archive';
    if (isText(entry.name)) return 'description';
    return 'insert_drive_file';
  }

  function formatSize(bytes: number): string {
    if (bytes === 0) return '0 B';
    const units = ['B', 'KB', 'MB', 'GB', 'TB'];
    const i = Math.floor(Math.log(bytes) / Math.log(1024));
    return (bytes / Math.pow(1024, i)).toFixed(1) + ' ' + units[i];
  }

  function joinPath(base: string, name: string): string {
    if (base === '/' || base === '') return '/' + name;
    if (base.endsWith('/')) return base + name;
    return base + '/' + name;
  }

  /** A ZFS snapshot's `creation` is Unix epoch seconds (as a string). */
  function formatSnapDate(epoch: string): string {
    const n = Number(epoch);
    if (!Number.isFinite(n) || n <= 0) return epoch;
    try {
      return new Date(n * 1000).toLocaleString();
    } catch {
      return epoch;
    }
  }

  function parentPath(p: string): string {
    if (!p || p === '/') return '/';
    const parts = p.split('/').filter(Boolean);
    parts.pop();
    return '/' + parts.join('/');
  }

  let diskRefreshInterval: ReturnType<typeof setInterval>;
  let watchSeq = 0;
  let loadSeq = 0;

  async function connectWatcher(path: string) {
    const seq = ++watchSeq;
    // Close any existing watcher
    if (watchEventSource) {
      watchEventSource.close();
      watchEventSource = null;
    }
    try {
      const ticketRes = await api.getExplorerWatchTicket(path);
      if (seq !== watchSeq || currentPath !== path) return;
      if (!ticketRes.success || !ticketRes.data?.ticket) return;
      watchTicket = ticketRes.data.ticket;
      const url = api.getExplorerWatchUrl(watchTicket, path);
      const es = new EventSource(url);
      es.addEventListener('change', () => {
        if (seq === watchSeq && currentPath === path) {
          refreshCurrentDir();
        }
      });
      es.onerror = () => {
        // Reconnect after a delay
        es.close();
        if (watchEventSource === es) watchEventSource = null;
        setTimeout(() => {
          if (seq === watchSeq && !watchEventSource && currentPath === path) {
            connectWatcher(path);
          }
        }, 5000);
      };
      watchEventSource = es;
    } catch {
      // Ticket fetch failed — fall back to a delayed retry
      setTimeout(() => {
        if (seq === watchSeq && currentPath === path) connectWatcher(path);
      }, 5000);
    }
  }

  onMount(async () => {
    await loadFavorites();
    await loadRecents();
    await loadDisks();
    await loadRoots();
    try {
      const me = await api.getCurrentUser();
      isAdmin = (me as any)?.data?.is_admin === true || (me as any)?.is_admin === true;
    } catch {
      isAdmin = false;
    }
    await loadDirectory('/');

    // Deep link: ?rewind=open starts the Rewind drawer for the live folder.
    if (typeof window !== 'undefined' && new URLSearchParams(window.location.search).get('rewind') === 'open') {
      openRewindFor(currentPath, currentPath);
    }

    diskRefreshInterval = setInterval(loadDisks, 30000);

    if (typeof document !== 'undefined') {
      document.addEventListener('visibilitychange', onVisibilityChange);
      window.addEventListener('mousemove', onMarqueeMove);
      window.addEventListener('mouseup', onMarqueeUp);
    }
  });

  onDestroy(() => {
    if (diskRefreshInterval) clearInterval(diskRefreshInterval);
    if (watchEventSource) watchEventSource.close();
    if (typeof document !== 'undefined') {
      document.removeEventListener('visibilitychange', onVisibilityChange);
    }
    if (typeof window !== 'undefined') {
      window.removeEventListener('mousemove', onMarqueeMove);
      window.removeEventListener('mouseup', onMarqueeUp);
    }
  });

  function onVisibilityChange() {
    if (document.visibilityState === 'visible') {
      loadDisks();
      loadRoots();
      // Re-list the current directory (replaces the toolbar refresh button —
      // the live watcher normally keeps it fresh, this covers a stale reconnect).
      if (!snapshotMode && !showTrash) loadDirectory(currentPath);
      // Reconnect the watcher if it was closed while the tab was hidden
      if (!watchEventSource) {
        connectWatcher(currentPath);
      }
    }
  }

  async function loadRoots() {
    try {
      const r = await api.getExplorerRoots();
      if (r.success && Array.isArray(r.data)) roots = r.data;
    } catch (e) {
      console.error('Failed to load explorer roots:', e);
    }
  }

  // --- Favorites (server-side) ---------------------------------------------

  async function loadFavorites() {
    try {
      const r = await api.getExplorerFavorites();
      if (r.success && Array.isArray(r.data)) {
        favorites = r.data.map((f: any) => ({ path: f.path, label: f.label || f.path.split('/').filter(Boolean).pop() || f.path }));
      }
    } catch {
      favorites = [];
    }
  }

  function isFavorite(path: string): boolean {
    return favorites.some((f) => f.path === path);
  }

  async function toggleFavorite(path: string) {
    if (!path || path === '/') return;
    if (isFavorite(path)) {
      // Optimistic removal
      favorites = favorites.filter((f) => f.path !== path);
      try { await api.removeExplorerFavorite(path); } catch { /* keep optimistic */ }
    } else {
      const label = path.split('/').filter(Boolean).pop() || path;
      favorites = [...favorites, { path, label }];
      try { await api.addExplorerFavorite(path, label); } catch { /* keep optimistic */ }
    }
  }

  // --- Recents (server-side) -----------------------------------------------

  async function loadRecents() {
    try {
      const r = await api.getExplorerRecents();
      if (r.success && Array.isArray(r.data)) {
        recents = r.data.map((e: any) => ({ path: e.path, label: e.label || e.path.split('/').filter(Boolean).pop() || e.path }));
      }
    } catch {
      recents = [];
    }
  }

  /** Record a visit (fire-and-forget) and mirror it at the top locally. */
  function recordRecent(path: string) {
    if (!path || path === '/') return;
    const label = path.split('/').filter(Boolean).pop() || path;
    recents = [{ path, label }, ...recents.filter((e) => e.path !== path)].slice(0, 50);
    api.touchExplorerRecent(path).catch(() => {});
  }

  async function clearRecents() {
    recents = [];
    try { await api.clearExplorerRecents(); } catch { /* keep optimistic */ }
  }

  // --- Rewind (per-file version timeline) -----------------------------------

  interface RewindVersion { name: string; created: string; browse_path: string; }

  let showRewind = $state(false);
  let rewindLabel = $state('');
  let rewindLivePath = $state('');
  let rewindVersions = $state<RewindVersion[]>([]);
  let rewindLoading = $state(false);

  async function openRewindFor(path: string, label: string) {
    rewindLabel = label;
    rewindLivePath = path;
    rewindVersions = [];
    rewindLoading = true;
    showRewind = true;
    try {
      const r = await api.getExplorerHistory(path);
      if (r.success && r.data && Array.isArray(r.data.versions)) {
        rewindVersions = r.data.versions;
      }
    } catch {
      rewindVersions = [];
    } finally {
      rewindLoading = false;
    }
  }

  function closeRewind() {
    showRewind = false;
    // Drop `rewind=open` from the URL so a reload doesn't reopen the drawer.
    if (typeof window !== 'undefined' && new URLSearchParams(window.location.search).get('rewind') === 'open') {
      const url = new URL(window.location.href);
      url.searchParams.delete('rewind');
      window.history.replaceState(null, '', url.toString());
    }
  }

  /** Copy a past version back into the live tree (a "restore"). */
  async function restoreRewindVersion(v: RewindVersion) {
    const destDir = parentPath(rewindLivePath) === rewindLivePath ? rewindLivePath : parentPath(rewindLivePath);
    const liveName = rewindLivePath.split('/').filter(Boolean).pop() || v.name;
    const strategy = await resolveStrategy([liveName], destDir);
    if (strategy === null) return;
    const r = await api.copyItem(v.browse_path, joinPath(destDir, liveName), strategy);
    if (r.success && r.data?.job_id) {
      if (await watchJob(r.data.job_id)) {
        closeRewind();
        await loadDirectory(currentPath);
      }
    } else if (r.success) {
      closeRewind();
      await loadDirectory(currentPath);
    }
  }

  // --- Network discovery + drive formatting ---------------------------------

  interface Neighbour { host: string; address?: string; share?: string; source: string; }

  let showNetwork = $state(false);
  let neighbours = $state<Neighbour[]>([]);
  let networkLoading = $state(false);

  async function openNetwork() {
    showNetwork = true;
    networkLoading = true;
    try {
      const r = await api.getNetworkDiscovery();
      if (r.success && r.data && Array.isArray(r.data.neighbours)) {
        neighbours = r.data.neighbours;
      } else {
        neighbours = [];
      }
    } catch {
      neighbours = [];
    } finally {
      networkLoading = false;
    }
  }

  let showFormat = $state(false);
  let formatDevice = $state('');
  let formatFstype = $state<'ext4' | 'exfat'>('ext4');
  let formatLabel = $state('');
  let formatBusy = $state(false);
  let formatError = $state('');

  async function handleFormat() {
    if (!formatDevice.trim() || formatBusy) return;
    formatBusy = true;
    formatError = '';
    try {
      const r = await api.formatDrive(formatDevice.trim(), formatFstype, formatLabel.trim() || undefined);
      if (r.success) {
        showFormat = false;
        formatDevice = '';
        formatLabel = '';
        await loadDisks();
      } else {
        formatError = r.message || (r as any).error || translate('explorer.eject_failed');
      }
    } catch (e: any) {
      formatError = String(e?.message || e);
    } finally {
      formatBusy = false;
    }
  }

  // --- Virtualised rendering (windowing) ------------------------------------

  const VIRTUAL_PAGE = 200;
  let visibleLimit = $state(VIRTUAL_PAGE);
  // `$derived.by` (not `$derived`) so the slice is evaluated lazily inside a
  // closure: `sortedEntries` is declared further down and a direct reference
  // here would read it before its declaration.
  let visibleEntries = $derived.by(() => sortedEntries.slice(0, visibleLimit));
  let sentinelEl: HTMLElement | null = $state(null);

  function resetPaging() {
    visibleLimit = VIRTUAL_PAGE;
  }

  $effect(() => {
    const el = sentinelEl;
    if (!el || typeof IntersectionObserver === 'undefined') return;
    const obs = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) {
        visibleLimit += VIRTUAL_PAGE;
      }
    }, { root: fileAreaEl });
    obs.observe(el);
    return () => obs.disconnect();
  });

  // --- Marquee (rubber-band) multi-select -----------------------------------
  // Mouse enhancement only: every row stays a keyboard-operable button, and
  // shift/ctrl-click selection is unchanged.

  let fileAreaEl: HTMLElement | null = $state(null);
  let marquee = $state<{ sx: number; sy: number; cx: number; cy: number } | null>(null);

  function marqueeStyle(): string {
    if (!marquee || !fileAreaEl) return 'display:none';
    const r = fileAreaEl.getBoundingClientRect();
    const x0 = Math.min(marquee.sx, marquee.cx) - r.left + fileAreaEl.scrollLeft;
    const x1 = Math.max(marquee.sx, marquee.cx) - r.left + fileAreaEl.scrollLeft;
    const y0 = Math.min(marquee.sy, marquee.cy) - r.top + fileAreaEl.scrollTop;
    const y1 = Math.max(marquee.sy, marquee.cy) - r.top + fileAreaEl.scrollTop;
    return `left:${x0}px;top:${y0}px;width:${x1 - x0}px;height:${y1 - y0}px`;
  }

  function onFileAreaMouseDown(e: MouseEvent) {
    if (e.button !== 0 || e.shiftKey || e.ctrlKey || e.metaKey || !fileAreaEl) return;
    const t = e.target as HTMLElement;
    if (t.closest('button, a, input, select, textarea, [role="button"], [data-testid="explorer-file-row"], [data-testid="explorer-grid-item"]')) return;
    marquee = { sx: e.clientX, sy: e.clientY, cx: e.clientX, cy: e.clientY };
  }

  function onMarqueeMove(e: MouseEvent) {
    if (marquee) marquee = { ...marquee, cx: e.clientX, cy: e.clientY };
  }

  function onMarqueeUp(e: MouseEvent) {
    if (!marquee || !fileAreaEl) return;
    const m = marquee;
    marquee = null;
    const moved = Math.hypot(m.cx - m.sx, m.cy - m.sy);
    if (moved < 4) return; // plain click on empty space — leave selection alone
    const x0 = Math.min(m.sx, m.cx), x1 = Math.max(m.sx, m.cx);
    const y0 = Math.min(m.sy, m.cy), y1 = Math.max(m.sy, m.cy);
    const next = new Set<string>();
    const rows = fileAreaEl.querySelectorAll('[data-path]');
    rows.forEach((row) => {
      const r = (row as HTMLElement).getBoundingClientRect();
      const overlap = r.left < x1 && r.right > x0 && r.top < y1 && r.bottom > y0;
      if (overlap) {
        const p = (row as HTMLElement).dataset.path;
        if (p) next.add(p);
      }
    });
    if (next.size > 0) {
      selected = next;
      lastSelectedIndex = -1;
    }
  }

  // --- Internal drag & drop (move onto a folder) -----------------------------

  let dragPaths = $state<string[]>([]);

  function onRowDragStart(e: DragEvent, entry: FileEntry) {
    if (snapshotMode || showTrash) return;
    dragPaths = selected.has(entry.path) ? [...selected] : [entry.path];
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = 'move';
      e.dataTransfer.setData('text/plain', entry.path);
    }
  }

  function onFolderDragOver(e: DragEvent) {
    if (snapshotMode || showTrash || dragPaths.length === 0) return;
    e.preventDefault();
  }

  async function onFolderDrop(e: DragEvent, destDir: string) {
    e.preventDefault();
    e.stopPropagation();
    if (snapshotMode || showTrash || dragPaths.length === 0) return;
    const names = dragPaths.map((p) => p.split('/').filter(Boolean).pop() || p);
    const strategy = await resolveStrategy(names, destDir);
    const paths = dragPaths;
    dragPaths = [];
    if (strategy === null) return;
    let moved = 0;
    for (const src of paths) {
      const name = src.split('/').filter(Boolean).pop() || src;
      if (src === joinPath(destDir, name)) continue; // dropped onto its own folder
      const r = await api.moveItem(src, joinPath(destDir, name), strategy);
      if (r.success) moved++;
    }
    if (moved > 0) {
      selected = new Set();
      await loadDirectory(currentPath);
    }
  }

  function endInternalDrag() {
    dragPaths = [];
  }

  // --- Snapshot browsing --------------------------------------------------

  async function toggleSnapshots() {
    showSnapshots = !showSnapshots;
    if (showSnapshots && !snapshotMode) {
      snapshotsLoading = true;
      try {
        const r = await api.getExplorerSnapshots(currentPath);
        snapshots = r.success && Array.isArray(r.data) ? r.data : [];
      } catch {
        snapshots = [];
      } finally {
        snapshotsLoading = false;
      }
    }
  }

  async function enterSnapshot(snap: { name: string; browse_path: string }) {
    showSnapshots = false;
    if (!snapshotMode) snapshotLivePath = currentPath;
    snapshotMode = true;
    await loadDirectory(snap.browse_path);
  }

  async function exitSnapshot() {
    const back = snapshotLivePath || '/';
    snapshotMode = false;
    snapshotLivePath = '';
    snapshots = [];
    await loadDirectory(back);
  }

  /** Every mutating action bails while browsing a read-only snapshot. */
  function blockedInSnapshot(): boolean {
    return snapshotMode;
  }

  // --- Share this folder (hand off to the Shares app) ---------------------

  function shareFolder(entry: FileEntry) {
    explorerShareIntent.set({ path: entry.path });
    if (typeof window !== 'undefined') {
      window.dispatchEvent(new CustomEvent('hv-open-app', { detail: { id: 'shares' } }));
    }
  }

  // --- Safe-eject a USB / external drive --------------------------------

  async function ejectDrive(mountPoint: string) {
    if (!(await dlg.ask(translate('explorer.confirm_eject', { name: mountPoint })))) return;
    const r = await api.ejectDrive(mountPoint);
    if (r.success) {
      if (currentPath === mountPoint || currentPath.startsWith(mountPoint + '/')) {
        await loadDirectory('/');
      }
      await loadDisks();
    } else {
      dlg.notify(r.message || r.error || translate('explorer.eject_failed'));
    }
  }

  /** Quiet re-list of the current directory for the background watcher —
   *  no spinner, keeps the user's selection (minus items that vanished),
   *  and never runs while the user is mid-task. */
  async function refreshCurrentDir() {
    if (
      loading ||
      uploading ||
      searchResults !== null ||
      showTrash ||
      folderPickerAction !== null ||
      showNewFolder ||
      showRename ||
      showPermissions ||
      editing ||
      collisionPrompt !== null
    ) {
      return;
    }
    const pathAtStart = currentPath;
    const seqAtStart = loadSeq;
    try {
      const result = await api.listDirectory(pathAtStart);
      if (seqAtStart !== loadSeq || currentPath !== pathAtStart) return;
      if (result.success && result.data) {
        listing = result.data;
        const paths = new Set(result.data.entries.map((e: FileEntry) => e.path));
        const next = new Set([...selected].filter((p) => paths.has(p)));
        if (next.size !== selected.size) selected = next;
      }
    } catch {
      /* ignore transient errors on a background refresh */
    }
  }

  async function loadDisks() {
    try {
      const [mountsResult, storageResult] = await Promise.all([
        api.getDisks(),
        api.getStorageDisks()
      ]);
      if (mountsResult.success && mountsResult.data) {
        disks = mountsResult.data;
      }
      if (storageResult.success && storageResult.data) {
        storageDisks = storageResult.data;
      }
    } catch (e) {
      console.error('Failed to load disks:', e);
    }
  }

  function getStorageDiskByName(name: string): StorageDisk | undefined {
    return storageDisks.find(d => d.name === name);
  }

  function storageDiskLabel(disk: StorageDisk): string {
    const clean = (s?: string) => (s && s.trim() && s.trim().toLowerCase() !== 'unknown' ? s.trim() : '');
    return clean(disk.model) || clean(disk.vendor) || disk.name;
  }

  async function loadDirectory(path: string) {
    const seq = ++loadSeq;
    currentPath = path;
    loading = true;
    error = '';
    selected = new Set();
    searchQuery = '';
    searchResults = null;
    showTrash = false;
    try {
      // Fetch the directory-scoped thumbnail ticket together with the
      // listing so the grid's `<img>`s are authenticated from the very first
      // render (a ticket arriving *after* the images would leave them hidden,
      // because their `onerror` hides them on the initial 401).
      const [result, ticket] = await Promise.all([
        api.listDirectory(path),
        api.getThumbnailTicket(path),
      ]);
      if (seq !== loadSeq) return;
      if (result.success && result.data) {
        listing = result.data;
        if (ticket) thumbnailTicket = ticket;
        resetPaging();
        recordRecent(path);
        // Reconnect the real-time watcher for the new directory
        connectWatcher(path);
      } else {
        error = result.error || result.message || translate('explorer.load_failed');
      }
    } catch (e: any) {
      if (seq !== loadSeq) return;
      error = translate('explorer.error_prefix', { error: String(e?.message || e) });
    }
    if (seq === loadSeq) {
      loading = false;
    }
  }

  function handleSearchInput(value: string) {
    searchQuery = value;
    if (searchDebounceTimer) clearTimeout(searchDebounceTimer);
    if (!value.trim()) {
      searchResults = null;
      searching = false;
      return;
    }
    searchDebounceTimer = setTimeout(() => runSearch(value.trim()), 300);
  }

  async function runSearch(query: string) {
    searching = true;
    try {
      const result = await api.searchFiles(currentPath, query, 200, fuzzySearch);
      if (result.success && result.data) {
        searchResults = result.data.results;
      } else {
        searchResults = [];
      }
    } finally {
      searching = false;
      resetPaging();
    }
  }

  function clearSearch() {
    searchQuery = '';
    searchResults = null;
    resetPaging();
  }

  let sortedEntries = $derived.by(() => {
    // While a recursive search is active, its results replace the current
    // directory's listing entirely (searching across subfolders, not just
    // filtering what's already loaded).
    const source = searchResults !== null ? searchResults : (listing?.entries ?? []);
    if (source.length === 0) return [];
    let entries = [...source];
    entries.sort((a, b) => {
      if (a.is_directory !== b.is_directory) return a.is_directory ? -1 : 1;
      let cmp = 0;
      if (sortBy === 'name') cmp = a.name.localeCompare(b.name, undefined, { sensitivity: 'base' });
      else if (sortBy === 'size') cmp = a.size_bytes - b.size_bytes;
      else if (sortBy === 'modified') cmp = a.modified.localeCompare(b.modified);
      return sortDir === 'asc' ? cmp : -cmp;
    });
    return entries;
  });

  function toggleSelect(entry: FileEntry, index: number, e: MouseEvent) {
    const newSet = new Set(selected);
    if (e.shiftKey && lastSelectedIndex >= 0) {
      const start = Math.min(lastSelectedIndex, index);
      const end = Math.max(lastSelectedIndex, index);
      for (let i = start; i <= end; i++) {
        newSet.add(sortedEntries[i].path);
      }
    } else if (e.ctrlKey || e.metaKey) {
      if (newSet.has(entry.path)) newSet.delete(entry.path);
      else newSet.add(entry.path);
    } else {
      if (newSet.has(entry.path) && newSet.size === 1) {
        newSet.clear();
      } else {
        newSet.clear();
        newSet.add(entry.path);
      }
    }
    selected = newSet;
    lastSelectedIndex = index;
  }

  async function navigateToEntry(entry: FileEntry) {
    if (entry.is_directory) {
      await loadDirectory(entry.path);
    }
  }

  // Reflow-proof double-click: selecting a row inserts the selection toolbar
  // above the list, which shifts every row down between the two native
  // clicks of a `dblclick` — the second click (and the resulting `dblclick`
  // event) then lands on the wrong row or misses entirely. Instead, detect
  // the double-click ourselves from consecutive `click` events on the *same*
  // entry (matched by path, not by screen position, so a reflow can't break
  // it) and drive navigation from there.
  let lastRowClick: { path: string; at: number } = { path: '', at: 0 };
  const DOUBLE_CLICK_MS = 400;

  function handleRowClick(entry: FileEntry, index: number, e: MouseEvent) {
    const now = Date.now();
    if (
      lastRowClick.path === entry.path &&
      now - lastRowClick.at < DOUBLE_CLICK_MS &&
      !e.shiftKey &&
      !e.ctrlKey &&
      !e.metaKey
    ) {
      lastRowClick = { path: '', at: 0 };
      navigateToEntry(entry);
      return;
    }
    lastRowClick = { path: entry.path, at: now };
    toggleSelect(entry, index, e);
  }

  function navigateToParent() {
    if (!listing) return;
    if (listing.parent && listing.parent !== listing.path) {
      loadDirectory(listing.parent);
    } else if (currentPath !== '/') {
      loadDirectory(parentPath(currentPath));
    }
  }

  async function handleMkdir() {
    if (blockedInSnapshot()) return;
    if (!newFolderName.trim()) {
      return;
    }
    const fullPath = joinPath(currentPath, newFolderName.trim());
    const result = await api.mkdir(fullPath);
    if (result.success) {
      showNewFolder = false;
      newFolderName = '';
      await loadDirectory(currentPath);
    } else {
      dlg.notify(result.message || result.error || translate('explorer.mkdir_failed'));
    }
  }

  function openRename(entry: FileEntry) {
    if (blockedInSnapshot()) return;
    renameTarget = entry;
    renameName = entry.name;
    showRename = true;
  }

  async function handleRename() {
    if (blockedInSnapshot()) return;
    if (!renameTarget || !renameName.trim()) return;
    const newPath = joinPath(parentPath(renameTarget.path), renameName.trim());
    const result = await api.renameItem(renameTarget.path, newPath);
    if (result.success) {
      showRename = false;
      renameTarget = null;
      await loadDirectory(currentPath);
    } else {
      dlg.notify(result.message || result.error || translate('explorer.rename_failed'));
    }
  }

  async function handleDelete(entry?: FileEntry) {
    if (blockedInSnapshot()) return;
    const targets = entry ? [entry] : sortedEntries.filter(e => selected.has(e.path));
    if (targets.length === 0) return;
    const msg = targets.length === 1
      ? translate('explorer.confirm_trash_one', { name: targets[0].name })
      : translate('explorer.confirm_trash_many', { count: String(targets.length) });
    if (!(await dlg.ask(msg))) return;

    let successCount = 0;
    let failCount = 0;
    for (const t of targets) {
      const result = await api.trashItem(t.path);
      if (result.success) successCount++;
      else failCount++;
    }
    if (successCount > 0) {
      selected = new Set();
      await loadDirectory(currentPath);
    }
    // Batch delete never fails silently — the user sees what did not move.
    if (failCount > 0) {
      dlg.notify(
        failCount === targets.length
          ? translate('explorer.delete_failed')
          : translate('explorer.delete_partial', { done: String(successCount), total: String(targets.length) })
      );
    }
  }

  async function loadTrash() {
    trashLoading = true;
    const result = await api.getTrash();
    if (result.success && result.data) {
      trashItems = result.data;
    } else {
      dlg.notify(result.message || result.error || translate('explorer.trash_load_failed'));
    }
    trashLoading = false;
  }

  function openTrash() {
    showTrash = true;
    loadTrash();
  }

  function closeTrash() {
    showTrash = false;
  }

  async function restoreTrash(item: TrashEntry) {
    const result = await api.restoreTrashItem(item.id);
    if (result.success) {
      await loadTrash();
    } else {
      dlg.notify(result.message || result.error || translate('explorer.restore_failed'));
    }
  }

  async function purgeTrash(item: TrashEntry) {
    if (!(await dlg.ask(translate('explorer.confirm_purge', { name: item.name })))) return;
    const result = await api.purgeTrashItem(item.id);
    if (result.success) {
      await loadTrash();
    } else {
      dlg.notify(result.message || result.error || translate('explorer.purge_failed'));
    }
  }

  async function emptyTrash() {
    if (trashItems.length === 0) return;
    if (!(await dlg.ask(translate('explorer.confirm_empty_trash', { count: String(trashItems.length) })))) return;
    const result = await api.emptyTrash();
    if (result.success) {
      await loadTrash();
    } else {
      dlg.notify(result.message || result.error || translate('explorer.empty_trash_failed'));
    }
  }

  async function handleDownload(entry: FileEntry) {
    if (entry.is_directory) {
      return;
    }
    const result = await api.downloadFile(entry.path, entry.name);
    if (!result.success) {
      dlg.notify(result.error || translate('explorer.download_failed'));
    }
  }

  function handleExtract(entry: FileEntry) {
    if (blockedInSnapshot()) return;
    extractTarget = entry;
    folderPickerAction = 'extract';
  }

  async function extractTo(dest: string) {
    if (blockedInSnapshot()) return;
    if (!extractTarget) return;
    const result = await api.extractArchive(extractTarget.path, dest);
    if (result.success) {
      if (dest === currentPath) await loadDirectory(currentPath);
    } else {
      dlg.notify(result.message || result.error || translate('explorer.extract_failed'));
    }
    extractTarget = null;
  }

  function openPermissions(entry: FileEntry) {
    if (blockedInSnapshot()) return;
    chmodTarget = entry;
    chmodMode = entry.permissions ? parseOctalFromRwx(entry.permissions).toString(8).padStart(3, '0') : '755';
    showPermissions = true;
  }

  function parseOctalFromRwx(rwx: string): number {
    let m = 0;
    if (rwx[0] === 'r') m += 4;
    if (rwx[1] === 'w') m += 2;
    if (rwx[2] === 'x') m += 1;
    return m;
  }

  async function handleChmod() {
    if (blockedInSnapshot()) return;
    if (!chmodTarget) return;
    if (!/^[0-7]{3,4}$/.test(chmodMode)) {
      return;
    }
    const result = await api.setPermissions(chmodTarget.path, chmodMode);
    if (result.success) {
      showPermissions = false;
      chmodTarget = null;
      await loadDirectory(currentPath);
    } else {
      dlg.notify(result.message || result.error || translate('explorer.permission_failed'));
    }
  }

  async function handlePreview(entry: FileEntry) {
    if (entry.is_directory) return;
    const result = await api.previewFile(entry.path);
    if (result.success && result.data) {
      previewContent = result.data;
      editing = false;
      editDirty = false;
      editError = '';
      showPreview = true;
    } else {
      dlg.notify(result.message || result.error || translate('explorer.preview_unavailable'));
    }
  }

  /** Guarded close: refuses to drop unsaved editor changes without asking. */
  function closePreview() {
    if (editing && editDirty) {
      // The confirm modal is layered on top — the preview closes so its
      // Dialog's own close state can't fight the reopen, and "continue
      // editing" restores it with the in-progress editContent intact.
      showPreview = false;
      showDiscardConfirm = true;
      return;
    }
    closePreviewForce();
  }

  /** Unconditional close, used by the discard/save-confirm paths. */
  function closePreviewForce() {
    showPreview = false;
    previewContent = null;
    editing = false;
    editDirty = false;
    editError = '';
  }

  function startEdit() {
    if (!previewContent) return;
    editContent = previewContent.content;
    editError = '';
    editDirty = false;
    editing = true;
  }

  function discardEdit() {
    showDiscardConfirm = false;
    closePreviewForce();
  }

  async function saveEditAndClose() {
    await saveEdit();
    if (!editing) {
      showDiscardConfirm = false;
      closePreviewForce();
    }
  }

  async function saveEdit() {
    if (blockedInSnapshot()) return;
    const pc = previewContent;
    if (!pc) return;
    editSaving = true;
    editError = '';
    const r = await api.saveFile(pc.path, editContent);
    editSaving = false;
    if (r.success) {
      previewContent = { ...pc, content: editContent, size_bytes: new Blob([editContent]).size };
      editing = false;
      editDirty = false;
      await refreshCurrentDir();
    } else {
      editError = r.message || r.error || translate('explorer.save_failed');
    }
  }

  function openShareLink(entry: FileEntry) {
    if (blockedInSnapshot() || entry.is_directory) return;
    shareLinkTarget = entry;
    showShareLink = true;
  }

  // Subscribes to an SSE progress job (used for directory copy/move, see
  // `spawn_tracked_copy_job` on the backend) and resolves once it's done.
  // The backend's "done" SSE event only fires on success (a pre-existing
  // quirk — a Failed job just stops sending events), so failure is also
  // detected directly from a `state === 'failed'` progress event.
  function watchJob(jobId: string): Promise<boolean> {
    return new Promise((resolve) => {
      let settled = false;
      jobProgress = { message: '', progress: 0 };
      const finish = (ok: boolean) => {
        if (settled) return;
        settled = true;
        jobProgress = null;
        unsubscribe();
        resolve(ok);
      };
      const unsubscribe = api.subscribeToProgress(
        jobId,
        (event: any) => {
          jobProgress = { message: event.message, progress: event.progress };
          if (event.state === 'failed') {
            finish(false);
          }
        },
        () => finish(true),
        () => finish(false)
      );
    });
  }

  async function copySelectedTo(dest: string) {
    // Copying OUT of a snapshot is allowed — it's how you restore an old
    // version. (Copying into a snapshot dir just fails at the backend.)
    const items = sortedEntries.filter(e => selected.has(e.path));
    const strategy = await resolveStrategy(items.map(i => i.name), dest);
    if (strategy === null) return; // cancelled at the collision prompt
    let success = 0;
    let failCount = 0;
    for (const it of items) {
      const targetPath = joinPath(dest, it.name);
      const r = await api.copyItem(it.path, targetPath, strategy);
      if (r.success && r.data?.job_id) {
        if (await watchJob(r.data.job_id)) success++;
      } else if (r.success) {
        success++;
      } else {
        failCount++;
      }
    }
    if (failCount > 0) {
      dlg.notify(
        failCount === items.length
          ? translate('explorer.copy_failed')
          : translate('explorer.copy_partial', { done: String(success), total: String(items.length) })
      );
    }
    if (success > 0) {
      selected = new Set();
      if (dest === currentPath) await loadDirectory(currentPath);
    }
  }

  async function moveSelectedTo(dest: string) {
    if (blockedInSnapshot()) return;
    const items = sortedEntries.filter(e => selected.has(e.path));
    const strategy = await resolveStrategy(items.map(i => i.name), dest);
    if (strategy === null) return;
    let success = 0;
    let failCount = 0;
    for (const it of items) {
      const targetPath = joinPath(dest, it.name);
      const r = await api.moveItem(it.path, targetPath, strategy);
      if (r.success && r.data?.job_id) {
        if (await watchJob(r.data.job_id)) success++;
      } else if (r.success) {
        success++;
      } else {
        failCount++;
      }
    }
    if (failCount > 0) {
      dlg.notify(
        failCount === items.length
          ? translate('explorer.move_failed')
          : translate('explorer.move_partial', { done: String(success), total: String(items.length) })
      );
    }
    if (success > 0) {
      selected = new Set();
      if (dest === currentPath || dest === parentPath(currentPath)) await loadDirectory(currentPath);
    }
  }

  function openCompress() {
    if (blockedInSnapshot()) return;
    const items = sortedEntries.filter(e => selected.has(e.path));
    if (items.length === 0) return;
    compressName = items.length === 1 ? `${items[0].name}.tar.gz` : 'archive.tar.gz';
    compressFormat = 'tar.gz';
    showCompress = true;
  }

  async function handleCompress() {
    if (blockedInSnapshot()) return;
    const items = sortedEntries.filter(e => selected.has(e.path));
    if (items.length === 0 || !compressName.trim()) return;
    const destination = joinPath(currentPath, compressName.trim());
    const result = await api.compressItems(items.map(i => i.path), destination, compressFormat);
    if (result.success) {
      showCompress = false;
      selected = new Set();
      await loadDirectory(currentPath);
    } else {
      dlg.notify(result.message || result.error || translate('explorer.compress_failed'));
    }
  }

  function promptDestination(action: 'copy' | 'move'): void {
    folderPickerAction = action;
  }

  function handleFolderPickerSelect(dest: string) {
    const action = folderPickerAction;
    folderPickerAction = null;
    if (action === 'copy') copySelectedTo(dest);
    else if (action === 'move') moveSelectedTo(dest);
    else if (action === 'extract') extractTo(dest);
  }

  async function triggerUpload() {
    fileInputEl?.click();
  }

  async function handleUploadFile(e: Event) {
    const target = e.target as HTMLInputElement;
    const files = target.files;
    if (!files || files.length === 0) return;
    await uploadFiles(Array.from(files));
    if (fileInputEl) fileInputEl.value = '';
  }

  async function uploadFiles(files: File[]) {
    await uploadFilesTo(files, currentPath);
  }

  /** Upload to an arbitrary directory — used by the drop zone (current dir)
   *  and by the sidebar destinations (drop an OS file onto a location to
   *  upload it there). */
  async function uploadFilesTo(files: File[], dest: string) {
    if (blockedInSnapshot()) return;
    if (files.length === 0) return;
    const strategy = await resolveStrategy(files.map(f => f.name), dest);
    if (strategy === null) return; // cancelled at the collision prompt
    uploading = true;
    uploadProgress = 0;
    let uploaded = 0;
    let failedName = '';
    try {
      for (const file of files) {
        const result = await api.uploadFile(dest, file, (loaded: number, total: number) => {
          uploadProgress = total > 0 ? Math.round((loaded / total) * 100) : 0;
        }, strategy);
        if (result.success) {
          uploaded++;
        } else if (!failedName) {
          failedName = file.name;
        }
      }
      if (failedName) {
        // Partial uploads are reported with what succeeded and what failed —
        // never swallowed.
        dlg.notify(
          files.length === 1
            ? translate('explorer.upload_failed', { name: failedName })
            : translate('explorer.upload_partial', { done: String(uploaded), total: String(files.length) })
        );
      }
      if (dest === currentPath) await loadDirectory(currentPath);
    } finally {
      uploading = false;
      uploadProgress = 0;
    }
  }

  // --- Sidebar destinations accept an OS file drag to upload there ----------
  let sidebarDropTarget = $state('');

  function sidebarDragOver(e: DragEvent, path: string) {
    if (snapshotMode || showTrash) return;
    if (!e.dataTransfer?.types.includes('Files')) return;
    e.preventDefault();
    sidebarDropTarget = path;
  }

  function sidebarDragLeave(e: DragEvent) {
    e.preventDefault();
    sidebarDropTarget = '';
  }

  async function sidebarDrop(e: DragEvent, path: string) {
    e.preventDefault();
    sidebarDropTarget = '';
    const files = e.dataTransfer?.files;
    if (!files || files.length === 0) return;
    await uploadFilesTo(Array.from(files), path);
  }

  let isDraggingOver = $state(false);
  let dragDepth = 0;

  // Icon-rail collapse of the sidebar (Aurora-style): the rail narrows to a
  // 64px icon strip instead of vanishing, and the per-disk sub-labels hide.
  let sidebarCollapsed = $state(false);

  function handleDragOver(e: DragEvent) {
    e.preventDefault();
  }

  function handleDragEnter(e: DragEvent) {
    e.preventDefault();
    if (snapshotMode) return;
    if (!e.dataTransfer?.types.includes('Files')) return;
    dragDepth++;
    isDraggingOver = true;
  }

  function handleDragLeave(e: DragEvent) {
    e.preventDefault();
    dragDepth = Math.max(0, dragDepth - 1);
    if (dragDepth === 0) isDraggingOver = false;
  }

  async function handleDrop(e: DragEvent) {
    e.preventDefault();
    dragDepth = 0;
    isDraggingOver = false;
    const files = e.dataTransfer?.files;
    if (!files || files.length === 0) return;
    await uploadFiles(Array.from(files));
  }

  function navigateToDisk(d: DiskInfo) {
    loadDirectory(d.mount_point);
  }

  // The deepest configured root that contains the current path, so the
  // breadcrumb can read "My files / photos / 2024" instead of the raw
  // "home / nicolas / photos / 2024".
  let breadcrumbRoot = $derived.by(() => {
    let best: { path: string; kind: string; name: string } | null = null;
    for (const r of roots) {
      if (currentPath === r.path || currentPath.startsWith(r.path.replace(/\/$/, '') + '/')) {
        if (!best || r.path.length > best.path.length) best = r;
      }
    }
    return best;
  });

  let breadcrumbParts = $derived.by(() => {
    if (!currentPath || currentPath === '/') return [];
    const base = breadcrumbRoot ? breadcrumbRoot.path.replace(/\/$/, '') : '';
    return currentPath.slice(base.length).split('/').filter(Boolean);
  });

  function rootLabel(root: { path?: string; kind: string; name: string }): string {
    const key = `explorer.root_${root.kind}`;
    const label = translate(key);
    if (label === key) return root.name;
    // When multiple roots share the same translated kind (e.g. `/media` and
    // `/mnt` both carry `kind === "media"`), disambiguate with the root path
    // so the sidebar and breadcrumb do not render two identical labels.
    if (roots.filter((r) => r.kind === root.kind).length > 1 && root.path) {
      return `${label} (${root.path})`;
    }
    return label;
  }

  // The inline editor is offered only for text/code previews that were not
  // truncated (saving a truncated preview would chop the real file).
  let canEditPreview = $derived(
    !!previewContent &&
      previewContent.previewable &&
      !previewContent.mime_type.startsWith('image/') &&
      !previewContent.is_truncated,
  );
  const toolbarBtn = 'grid place-items-center w-9 h-9 flex-none rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:bg-[var(--hv-subtle-hover)] hover:text-[var(--hv-shell-text-strong)] hv-ic';
  const efRowAction = 'grid place-items-center w-8 h-8 flex-none rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:bg-[var(--hv-subtle-hover)] hover:text-[var(--hv-shell-text-strong)] hv-ic';
  const efRowDanger = 'hover:bg-[var(--hv-error-bg)] hover:text-[var(--hv-error)]';
  const stateMessage = 'flex flex-col items-center gap-2 py-14 text-center text-[var(--hv-shell-text-2)]';
</script>
<PageLayout
  title={$t('explorer.title')}
  data-testid="explorer-page"
  sidebarLabel={$t('explorer.locations')}
  sidebarTestId="explorer-sidebar"
  sidebarCollapseable
  bind:sidebarCollapsed
  footerMode="status"
>
  {#snippet icon()}
    <IconFolderOpen stroke={1.8} />
  {/snippet}

  {#snippet sidebar()}
    {#if roots.length > 0}
      <WindowSidebarSection label={$t('explorer.locations')}>
        {#each roots as root (root.path)}
          <WindowSidebarItem
            label={rootLabel(root)}
            class="disk-btn"
            active={!showTrash && currentPath === root.path}
            dropActive={sidebarDropTarget === root.path}
            onclick={() => loadDirectory(root.path)}
            ondragover={(e) => sidebarDragOver(e, root.path)}
            ondragleave={sidebarDragLeave}
            ondrop={(e) => sidebarDrop(e, root.path)}
          >
            {#snippet icon()}
              {#if root.kind === 'home'}<IconHome size={18} class="text-[var(--hv-shell-text-2)]" />{:else if root.kind === 'media'}<IconUsb size={18} class="text-[var(--hv-shell-text-2)]" />{:else if root.kind === 'appdata'}<IconServer size={18} class="text-[var(--hv-shell-text-2)]" />{:else if root.kind === 'virtualdisk'}<IconDatabase size={18} class="text-[var(--hv-primary)]" />{:else}<IconFolderOpen size={18} class="text-[var(--hv-shell-text-2)]" />{/if}
            {/snippet}
          </WindowSidebarItem>
        {/each}
      </WindowSidebarSection>
    {/if}

    {#if favorites.length > 0}
      <WindowSidebarSection label={$t('explorer.favorites')}>
        {#each favorites as fav (fav.path)}
          <WindowSidebarItem
            label={fav.label}
            labelClass="disk-name"
            class="disk-btn"
            active={!showTrash && currentPath === fav.path}
            dropActive={sidebarDropTarget === fav.path}
            onclick={() => loadDirectory(fav.path)}
            ondragover={(e) => sidebarDragOver(e, fav.path)}
            ondragleave={sidebarDragLeave}
            ondrop={(e) => sidebarDrop(e, fav.path)}
          >
            {#snippet icon()}<IconStarFilled size={15} class="text-[var(--hv-warning)]" />{/snippet}
            {#snippet action()}
              <button class="is-danger hv-ic" title={$t('explorer.unpin')} aria-label={$t('explorer.unpin')} onclick={() => toggleFavorite(fav.path)}><IconX size={13} /></button>
            {/snippet}
          </WindowSidebarItem>
        {/each}
      </WindowSidebarSection>
    {/if}

    {#if recents.length > 0}
      <WindowSidebarSection label={$t('explorer.recents')}>
        {#each recents as recent (recent.path)}
          <WindowSidebarItem
            label={recent.label}
            labelClass="disk-name"
            class="disk-btn"
            active={!showTrash && currentPath === recent.path}
            dropActive={sidebarDropTarget === recent.path}
            onclick={() => loadDirectory(recent.path)}
            ondragover={(e) => sidebarDragOver(e, recent.path)}
            ondragleave={sidebarDragLeave}
            ondrop={(e) => sidebarDrop(e, recent.path)}
          >
            {#snippet icon()}<IconClock size={15} class="text-[var(--hv-shell-text-2)]" />{/snippet}
          </WindowSidebarItem>
        {/each}
        <button data-testid="explorer-recents-clear" class="mx-3 mt-1 border-none bg-transparent text-c-xs text-[var(--hv-shell-text-2)] cursor-pointer hover:text-[var(--hv-shell-text-strong)] hv-ic" onclick={clearRecents}>{$t('explorer.clear_recents')}</button>
      </WindowSidebarSection>
    {/if}

    <WindowSidebarSection>
      <WindowSidebarItem
        label={$t('explorer.trash')}
        class="disk-btn"
        active={showTrash}
        onclick={openTrash}
      >
        {#snippet icon()}<IconTrash size={18} class="text-[var(--hv-shell-text-2)]" />{/snippet}
      </WindowSidebarItem>
    </WindowSidebarSection>

    <WindowSidebarSection label={$t('explorer.storage_section_title')}>
      {#if storageDisks.length > 0}
        {#if !sidebarCollapsed}
          <div class="flex items-center gap-1.5 px-3 py-1 text-c-xs text-[var(--hv-shell-text-2)]"><IconServer size={14} /> <span>{$t('explorer.category_physical_disks')}</span></div>
        {/if}
        {#each storageDisks as disk}
          {@const mountPoint = disk.mount_point}
          <WindowSidebarItem
            label={storageDiskLabel(disk)}
            detail={formatSize(disk.size)}
            class="disk-btn"
            dropActive={!!mountPoint && sidebarDropTarget === mountPoint}
            onclick={() => mountPoint ? loadDirectory(mountPoint) : null}
            ondragover={mountPoint ? (e) => sidebarDragOver(e, mountPoint) : undefined}
            ondragleave={sidebarDragLeave}
            ondrop={mountPoint ? (e) => sidebarDrop(e, mountPoint) : undefined}
          >
            {#snippet icon()}<Icon name={disk.connection_type === 'nvme' ? 'memory' : 'hard_drive'} size={18} class="text-[var(--hv-primary)]" />{/snippet}
          </WindowSidebarItem>
        {/each}
      {/if}
      {#if disks}
        {#if disks.external && disks.external.length > 0}
          {#if !sidebarCollapsed}
            <div class="flex items-center gap-1.5 px-3 py-1 text-c-xs text-[var(--hv-shell-text-2)]"><IconUsb size={14} /> <span>{$t('explorer.category_external_media')}</span></div>
          {/if}
          {#each disks.external as disk}
            <WindowSidebarItem
              label={disk.mount_point}
              detail={formatSize(disk.size_bytes)}
              class="disk-btn"
              dropActive={sidebarDropTarget === disk.mount_point}
              onclick={() => navigateToDisk(disk)}
              ondragover={(e) => sidebarDragOver(e, disk.mount_point)}
              ondragleave={sidebarDragLeave}
              ondrop={(e) => sidebarDrop(e, disk.mount_point)}
            >
              {#snippet icon()}<IconUsb size={18} class="text-[var(--hv-warning)]" />{/snippet}
              {#snippet action()}
                <button class="is-danger hv-ic" title={$t('explorer.eject')} aria-label={$t('explorer.eject')} onclick={() => ejectDrive(disk.mount_point)}><IconPlayerEject size={14} /></button>
              {/snippet}
            </WindowSidebarItem>
          {/each}
        {/if}
        {#if disks.network && disks.network.length > 0}
          {#if !sidebarCollapsed}
            <div class="flex items-center gap-1.5 px-3 py-1 text-c-xs text-[var(--hv-shell-text-2)]"><IconCloud size={14} /> <span>{$t('explorer.category_network_storage')}</span></div>
          {/if}
          {#each disks.network as disk}
            <WindowSidebarItem label={disk.mount_point} detail={formatSize(disk.size_bytes)} class="disk-btn" dropActive={sidebarDropTarget === disk.mount_point} onclick={() => navigateToDisk(disk)} ondragover={(e) => sidebarDragOver(e, disk.mount_point)} ondragleave={sidebarDragLeave} ondrop={(e) => sidebarDrop(e, disk.mount_point)}>
              {#snippet icon()}<IconCloudUpload size={18} class="text-[var(--hv-app-dashboard-c)]" />{/snippet}
            </WindowSidebarItem>
          {/each}
        {/if}
        {#if disks.cloud && disks.cloud.length > 0}
          {#if !sidebarCollapsed}
            <div class="flex items-center gap-1.5 px-3 py-1 text-c-xs text-[var(--hv-shell-text-2)]"><IconCloudCheck size={14} /> <span>{$t('explorer.category_cloud_services')}</span></div>
          {/if}
          {#each disks.cloud as disk}
            <WindowSidebarItem label={disk.mount_point} detail={formatSize(disk.size_bytes)} class="disk-btn" dropActive={sidebarDropTarget === disk.mount_point} onclick={() => navigateToDisk(disk)} ondragover={(e) => sidebarDragOver(e, disk.mount_point)} ondragleave={sidebarDragLeave} ondrop={(e) => sidebarDrop(e, disk.mount_point)}>
              {#snippet icon()}<IconCloud size={18} class="text-[var(--hv-info)]" />{/snippet}
            </WindowSidebarItem>
          {/each}
        {/if}
        {#if disks.raid && disks.raid.length > 0}
          {#if !sidebarCollapsed}
            <div class="flex items-center gap-1.5 px-3 py-1 text-c-xs text-[var(--hv-shell-text-2)]"><IconShield size={14} /> <span>{$t('explorer.category_raid_arrays')}</span></div>
          {/if}
          {#each disks.raid as disk}
            <WindowSidebarItem label={disk.mount_point} detail={formatSize(disk.size_bytes)} class="disk-btn" dropActive={sidebarDropTarget === disk.mount_point} onclick={() => navigateToDisk(disk)} ondragover={(e) => sidebarDragOver(e, disk.mount_point)} ondragleave={sidebarDragLeave} ondrop={(e) => sidebarDrop(e, disk.mount_point)}>
              {#snippet icon()}<IconShieldCheckered size={18} class="text-[var(--hv-success)]" />{/snippet}
            </WindowSidebarItem>
          {/each}
        {/if}
      {:else}
        <div class="flex flex-col gap-2 px-3 py-2 text-xs text-[var(--hv-shell-text-2)]" role="status">
          <span class="sr-only">{$t('explorer.loading')}</span>
          <span class="hv-skeleton-text block" style="width: 70%" aria-hidden="true"></span>
          <span class="hv-skeleton-text block" style="width: 55%" aria-hidden="true"></span>
          <span class="hv-skeleton-text block" style="width: 62%" aria-hidden="true"></span>
        </div>
      {/if}
    </WindowSidebarSection>
  {/snippet}

  <div data-testid="explorer-toolbar" class="flex items-center gap-1.5 px-3 py-2 border-b border-[var(--hv-shell-border)] bg-[var(--hv-shell-fill)] flex-wrap">
    <button data-testid="explorer-back" class={toolbarBtn} onclick={navigateToParent} disabled={!listing || (!listing.parent && currentPath === '/')} title={$t('explorer.back')} aria-label={$t('explorer.back')}><IconArrowUp size={16} /></button>
    {#if !snapshotMode && !showTrash && listing}
      <button class={cn(toolbarBtn, isFavorite(currentPath) && 'text-[var(--hv-warning)]')} data-testid="explorer-pin" class:active={isFavorite(currentPath)} onclick={() => toggleFavorite(currentPath)} disabled={currentPath === '/'} title={isFavorite(currentPath) ? $t('explorer.unpin') : $t('explorer.pin')}>{#if isFavorite(currentPath)}<IconStarFilled size={16} />{:else}<IconStar size={16} />{/if}</button>
    {/if}
    {#if !showTrash}
      <div data-testid="explorer-history" class="relative">
        <button class={cn(toolbarBtn, (showSnapshots || snapshotMode) && 'text-[var(--hv-primary)]')} data-testid="explorer-history-btn" class:active={showSnapshots || snapshotMode} onclick={toggleSnapshots} title={$t('explorer.history')} aria-label={$t('explorer.history')}><IconHistory size={16} /></button>
        {#if showSnapshots}
          <div data-testid="explorer-history-menu" class="absolute top-full left-0 mt-1 z-[var(--hv-z-dropdown)] w-56 rounded-xl bg-[var(--hv-shell-glass)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)] p-1.5">
            {#if snapshotsLoading}
              <div data-testid="explorer-history-empty" class="px-3 py-4 text-xs text-[var(--hv-shell-text-2)]">{$t('explorer.loading')}</div>
            {:else if snapshots.length === 0}
              <div data-testid="explorer-history-empty" class="px-3 py-4 text-xs text-[var(--hv-shell-text-2)]">{$t('explorer.no_snapshots')}</div>
            {:else}
              {#each snapshots as snap (snap.name)}
                <button class="w-full flex items-center justify-between gap-2 px-3 py-2 rounded-lg text-left border-none bg-transparent cursor-pointer hover:bg-[var(--hv-subtle-hover)]" onclick={() => enterSnapshot(snap)}>
                  <span class="text-c-base font-medium text-[var(--hv-shell-text-strong)] truncate">{snap.name}</span>
                  <span class="text-c-xs text-[var(--hv-shell-text-2)] flex-none">{formatSnapDate(snap.created)}</span>
                </button>
              {/each}
            {/if}
          </div>
        {/if}
      </div>
    {/if}
    {#if !showTrash}
      <button data-testid="explorer-rewind-btn" class={cn(toolbarBtn, showRewind && 'text-[var(--hv-primary)]')} class:active={showRewind} onclick={() => openRewindFor(currentPath, currentPath)} title={$t('explorer.rewind')} aria-label={$t('explorer.rewind')}><IconClock size={16} /></button>
      <button data-testid="explorer-network-btn" class={cn(toolbarBtn, showNetwork && 'text-[var(--hv-primary)]')} onclick={openNetwork} title={$t('explorer.network')} aria-label={$t('explorer.network')}><IconServer size={16} /></button>
      {#if isAdmin}
        <button data-testid="explorer-format-btn" class={toolbarBtn} onclick={() => { formatError = ''; showFormat = true; }} title={$t('explorer.format_drive')} aria-label={$t('explorer.format_drive')}><IconDatabase size={16} /></button>
      {/if}
    {/if}
    <div data-testid="explorer-breadcrumb" class="flex items-center gap-0.5 flex-1 min-w-0 px-1 overflow-hidden">
      {#if breadcrumbRoot}
        <button class="px-2 py-1 rounded-lg border-none bg-transparent text-c-base font-medium text-[var(--hv-shell-text-2)] cursor-pointer hover:bg-[var(--hv-subtle-hover)]" onclick={() => breadcrumbRoot && loadDirectory(breadcrumbRoot.path)}>{rootLabel(breadcrumbRoot)}</button>
      {:else}
        <button class="px-2 py-1 rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:bg-[var(--hv-subtle-hover)]" onclick={() => loadDirectory('/')}><IconHome size={14} /></button>
      {/if}
      {#each breadcrumbParts as part, i}
        <span class="text-[var(--hv-shell-text-3)]">/</span>
        <button class="px-2 py-1 rounded-lg border-none bg-transparent text-c-base font-medium text-[var(--hv-shell-text-2)] cursor-pointer hover:bg-[var(--hv-subtle-hover)]" onclick={() => { const base = breadcrumbRoot ? breadcrumbRoot.path.replace(/\/$/, '') : ''; loadDirectory(base + '/' + breadcrumbParts.slice(0, i + 1).join('/')); }}>{part}</button>
      {/each}
    </div>
    <!-- Page-level actions live in the file toolbar, never in the window
         header bar (which is icon + title + window controls only). -->
    {#if !snapshotMode}
      <Button onclick={() => showNewFolder = true}>
        <IconFolderPlus size={16} />
        <span>{$t('explorer.new_folder')}</span>
      </Button>
      <Button variant="ghost" onclick={triggerUpload} disabled={uploading}>
        <IconUpload size={16} />
        <span>{$t('explorer.upload')}</span>
      </Button>
    {/if}
    <div class="flex items-center gap-1.5 ml-auto">
      <div class="relative flex items-center">
        <IconSearch size={14} class="absolute left-2.5 text-[var(--hv-shell-text-3)]" />
        <input type="search" data-testid="explorer-search-input" class="h-9 w-44 pl-8 pr-7 rounded-lg border border-[var(--hv-shell-border)] bg-[var(--hv-shell-glass)] text-c-base outline-none focus:border-[var(--hv-primary)]" placeholder={$t('explorer.search_placeholder')} value={searchQuery} oninput={(e) => handleSearchInput((e.target as HTMLInputElement).value)} />
        {#if searching}
          <IconLoader size={14} class="absolute right-2 animate-spin text-[var(--hv-shell-text-3)]" />
        {:else if searchQuery}
          <button data-testid="explorer-search-clear" class="absolute right-1.5 border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hv-ic-abs" onclick={clearSearch} title={$t('explorer.search_clear')} aria-label={$t('explorer.search_clear')}><IconX size={14} /></button>
        {/if}
      </div>
      {#if searchResults !== null}
        <span class="text-c-xs text-[var(--hv-shell-text-2)]">{$t('explorer.search_results_count', { count: String(searchResults.length) })}</span>
      {/if}
      <button data-testid="explorer-fuzzy-toggle" class={cn(toolbarBtn, fuzzySearch && 'bg-[var(--hv-primary-light)] text-[var(--hv-primary)]')} class:active={fuzzySearch} aria-pressed={fuzzySearch} onclick={() => { fuzzySearch = !fuzzySearch; if (searchQuery.trim()) runSearch(searchQuery.trim()); }} title={$t('explorer.fuzzy_search')} aria-label={$t('explorer.fuzzy_search')}><span class="text-sm font-bold" aria-hidden="true">≈</span></button>
      <div data-testid="explorer-view-toggle" class="flex gap-0.5">
        <button class={cn(toolbarBtn, view === 'list' && 'bg-[var(--hv-primary-light)] text-[var(--hv-primary)]')} class:active={view === 'list'} data-testid="explorer-view-list" onclick={() => view = 'list'} title={$t('explorer.view_list')}><IconList size={16} /></button>
        <button class={cn(toolbarBtn, view === 'grid' && 'bg-[var(--hv-primary-light)] text-[var(--hv-primary)]')} class:active={view === 'grid'} data-testid="explorer-view-grid" onclick={() => view = 'grid'} title={$t('explorer.view_grid')}><IconLayoutGrid size={16} /></button>
      </div>
      {#if selected.size > 0}
        <span class="text-c-xs text-[var(--hv-shell-text-2)]">{$t('explorer.selected_count', { count: String(selected.size) })}</span>
        <button class={toolbarBtn} onclick={() => promptDestination('copy')} title={snapshotMode ? $t('explorer.restore_here') : $t('explorer.copy')}><IconCopy size={16} /></button>
        {#if !snapshotMode}
          <button class={toolbarBtn} onclick={() => promptDestination('move')} title={$t('explorer.move')}><IconFolderMinus size={16} /></button>
          <button data-testid="explorer-compress" class={toolbarBtn} onclick={openCompress} title={$t('explorer.compress_selection')} aria-label={$t('explorer.compress_selection')}><IconFileZip size={16} /></button>
          <button class={cn(toolbarBtn, 'hover:bg-[var(--hv-error-bg)] hover:text-[var(--hv-error)]')} onclick={() => handleDelete()} title={$t('explorer.delete_to_trash')}><IconTrash size={16} /></button>
        {/if}
      {/if}
      <input type="file" multiple bind:this={fileInputEl} onchange={handleUploadFile} style="display: none;" />
    </div>
  </div>

  {#if snapshotMode}
    <div class="flex items-center gap-2 px-4 py-2 bg-[var(--hv-primary-light)] border-b border-[var(--hv-primary)]/30 text-sm text-[var(--hv-primary)]">
      <IconHistory size={16} />
      <span>{$t('explorer.snapshot_readonly')}</span>
      <Button variant="ghost" onclick={exitSnapshot} class="ml-auto"><IconArrowBackUp size={14} /> {$t('explorer.snapshot_exit')}</Button>
    </div>
  {/if}

  {#if uploading || jobProgress}
    <div class="flex items-center gap-2 px-4 py-2 bg-[var(--hv-shell-fill)] border-b border-[var(--hv-shell-border)] text-sm text-[var(--hv-shell-text-2)]">
      <IconLoader size={16} class="animate-spin" />
      <span>{jobProgress ? (jobProgress.message || $t('explorer.working')) : $t('explorer.uploading', { percent: String(uploadProgress) })}</span>
      <div class="flex-1 h-1.5 rounded-full bg-[var(--hv-shell-border-soft)] overflow-hidden">
        <div class="h-full bg-[var(--hv-primary)]" style="width: {jobProgress ? jobProgress.progress : uploadProgress}%"></div>
      </div>
    </div>
  {/if}

  <!-- The marquee (rubber-band) selection is a pointer-only enhancement on
       top of the file rows/buttons: onmousedown starts a drag rectangle over
       empty space and never replaces a keyboard path (rows/grids are
       buttons, reachable and selectable with Tab + Enter). The region stays
       announced along with its drag listeners. -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div data-testid="explorer-file-area" class:drag-active={isDraggingOver} class="relative flex-1 min-h-0 overflow-y-auto" bind:this={fileAreaEl} onmousedown={onFileAreaMouseDown} ondragover={handleDragOver} ondragenter={handleDragEnter} ondragleave={handleDragLeave} ondrop={handleDrop} role="region" aria-label={$t('explorer.drop_zone_label')}>
    {#if marquee}
      <div data-testid="explorer-marquee" class="pointer-events-none absolute z-20 border border-[var(--hv-primary)] bg-[var(--hv-primary-light)]/60" style={marqueeStyle()} aria-hidden="true"></div>
    {/if}
    {#if showTrash}
      <div data-testid="explorer-trash-toolbar" class="flex items-center justify-between gap-2 px-4 py-3">
        <h3 class="m-0 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">{$t('explorer.trash')}</h3>
        <Button variant="ghost" onclick={emptyTrash} disabled={trashItems.length === 0}><IconTrash size={14} /> {$t('explorer.empty_trash')}</Button>
      </div>
      {#if trashLoading}
        <div data-testid="explorer-state-message" class={stateMessage} role="status">
          <span class="sr-only">{$t('explorer.loading')}</span>
          {#each Array(5) as _, i}
            <span class="hv-skeleton-text block" style="width: {45 + ((i * 31 + 17) % 36)}%" aria-hidden="true"></span>
          {/each}
        </div>
      {:else if trashItems.length === 0}
        <div data-testid="explorer-state-message" class={stateMessage}><IconTrash size={40} /><p class="m-0">{$t('explorer.trash_empty')}</p></div>
      {:else}
        <div data-testid="explorer-file-table" class="px-4">
          {#each trashItems as item (item.id)}
            <div data-testid="explorer-file-row" class="grid grid-cols-[minmax(0,1fr)_auto_auto_auto] items-center gap-3 px-3 py-2 border-b border-[var(--hv-shell-border-soft)]">
              <span class="flex items-center gap-2 min-w-0"><Icon name={item.is_directory ? 'folder' : 'insert_drive_file'} size={18} class="text-[var(--hv-shell-text-2)]" /><span class="min-w-0"><span class="block text-c-base font-medium text-[var(--hv-shell-text-strong)] truncate">{item.name}</span><span class="block text-c-xs text-[var(--hv-shell-text-2)] truncate">{item.original_path}</span></span></span>
              <span class="text-xs text-[var(--hv-shell-text-2)]">{item.is_directory ? '—' : formatSize(item.size_bytes)}</span>
              <span class="text-xs text-[var(--hv-shell-text-2)]">{item.deleted_at}</span>
              <span class="flex gap-1"><button class={efRowAction} onclick={() => restoreTrash(item)} title={$t('explorer.restore')} aria-label={$t('explorer.restore')}><IconRefresh size={14} /></button><button class={cn(efRowAction, efRowDanger)} onclick={() => purgeTrash(item)} title={$t('explorer.purge')} aria-label={$t('explorer.purge')}><IconTrash size={14} /></button></span>
            </div>
          {/each}
        </div>
      {/if}
    {:else}
    {#if !loading && !error && listing && searchResults === null && !showTrash && !snapshotMode && currentPath === '/'}
      <ExplorerWidget onopen={(p) => loadDirectory(p)} />
    {/if}
    {#if isDraggingOver}
      <div class="absolute inset-0 z-10 flex flex-col items-center justify-center gap-2 bg-[var(--hv-primary-light)]/70">
        <IconUpload size={40} />
        <p>{$t('explorer.drop_here')}</p>
      </div>
    {/if}
    {#if loading}
      <div data-testid="explorer-state-message" class={stateMessage} role="status">
        <span class="sr-only">{$t('explorer.loading')}</span>
        {#each Array(5) as _, i}
          <span class="hv-skeleton-text block" style="width: {45 + ((i * 31 + 17) % 36)}%" aria-hidden="true"></span>
        {/each}
      </div>
    {:else if error}
      <div data-testid="explorer-state-error" class={stateMessage}><IconAlertCircle size={32} /><p class="m-0">{error}</p><Button variant="ghost" onclick={() => loadDirectory(currentPath)}>{$t('explorer.retry')}</Button></div>
    {:else if listing}
      {#if view === 'list'}
        <div data-testid="explorer-file-table" class="px-4">
          <div data-testid="explorer-file-header" class="grid grid-cols-[minmax(0,1fr)_auto_auto_auto_auto] items-center gap-3 px-3 py-2 border-b border-[var(--hv-shell-border)]">
            <button class="sort-btn border-none bg-transparent rounded-lg text-c-xs font-bold uppercase tracking-wide text-[var(--hv-shell-text-2)] cursor-pointer" onclick={() => { sortBy = 'name'; sortDir = sortDir === 'asc' ? 'desc' : 'asc'; }}>{$t('explorer.col_name')} {sortBy === 'name' ? (sortDir === 'asc' ? '\u25B2' : '\u25BC') : ''}</button>
            <button class="sort-btn border-none bg-transparent rounded-lg text-c-xs font-bold uppercase tracking-wide text-[var(--hv-shell-text-2)] cursor-pointer" onclick={() => { sortBy = 'size'; sortDir = sortDir === 'asc' ? 'desc' : 'asc'; }}>{$t('explorer.col_size')} {sortBy === 'size' ? (sortDir === 'asc' ? '\u25B2' : '\u25BC') : ''}</button>
            <button class="sort-btn border-none bg-transparent rounded-lg text-c-xs font-bold uppercase tracking-wide text-[var(--hv-shell-text-2)] cursor-pointer" onclick={() => { sortBy = 'modified'; sortDir = sortDir === 'asc' ? 'desc' : 'asc'; }}>{$t('explorer.modified')} {sortBy === 'modified' ? (sortDir === 'asc' ? '\u25B2' : '\u25BC') : ''}</button>
            <span class="text-c-xs font-bold uppercase tracking-wide text-[var(--hv-shell-text-2)]">{$t('explorer.permissions')}</span>
            <span class="text-c-xs font-bold uppercase tracking-wide text-[var(--hv-shell-text-2)]">{$t('explorer.col_actions')}</span>
          </div>
          {#each visibleEntries as entry, vi (entry.path)}
            <div data-testid="explorer-file-row" data-path={entry.path} draggable="true" ondragstart={(e) => onRowDragStart(e, entry)} ondragend={endInternalDrag} ondragover={entry.is_directory ? onFolderDragOver : undefined} ondrop={entry.is_directory ? (e) => onFolderDrop(e, entry.path) : undefined} class:selected={selected.has(entry.path)} class="grid grid-cols-[minmax(0,1fr)_auto_auto_auto_auto] items-center gap-3 px-3 py-2 border-b border-[var(--hv-shell-border-soft)] cursor-pointer hover:bg-[var(--hv-subtle-hover)]" onclick={(e) => handleRowClick(entry, sortedEntries.indexOf(entry), e)} onkeydown={(e) => activateOnEnterOrSpace(e, () => navigateToEntry(entry))} role="button" tabindex="0" aria-label={entry.name}>
              <span class="flex items-center gap-2 min-w-0"><Icon name={getFileIcon(entry)} size={18} className={entry.is_directory ? 'folder-icon' : ''} class="text-[var(--hv-shell-text-2)]" /><span class="text-c-base text-[var(--hv-shell-text-strong)] truncate">{entry.name}</span></span>
              <span class="text-xs text-[var(--hv-shell-text-2)]">{entry.is_directory ? '\u2014' : formatSize(entry.size_bytes)}</span>
              <span class="text-xs text-[var(--hv-shell-text-2)]">{entry.modified}</span>
              <span class="text-xs text-[var(--hv-shell-text-2)]">{entry.permissions}</span>
              <span class="flex gap-1">
                {#if !entry.is_directory}
                  <button class={efRowAction} data-testid="explorer-row-download" onclick={(e) => { e.stopPropagation(); handleDownload(entry); }} title={$t('explorer.download')} aria-label={$t('explorer.download')}><IconDownload size={14} /></button>
                  {#if !snapshotMode}
                    <button class={efRowAction} data-testid="explorer-row-share-link" onclick={(e) => { e.stopPropagation(); openShareLink(entry); }} title={$t('explorer.share_link')} aria-label={$t('explorer.share_link')}><IconShare size={14} /></button>
                  {/if}
                  {#if isText(entry.name) || isImage(entry.name)}
                    <button class={efRowAction} data-testid="explorer-row-preview" onclick={(e) => { e.stopPropagation(); handlePreview(entry); }} title={$t('explorer.preview')} aria-label={$t('explorer.preview')}><IconEye size={14} /></button>
                  {/if}
                  <button class={efRowAction} data-testid="explorer-row-rewind" onclick={(e) => { e.stopPropagation(); openRewindFor(entry.path, entry.name); }} title={$t('explorer.rewind')} aria-label={$t('explorer.rewind')}><IconClock size={14} /></button>
                  {#if isArchive(entry.name) && !snapshotMode}
                    <button class={efRowAction} data-testid="explorer-row-extract" onclick={(e) => { e.stopPropagation(); handleExtract(entry); }} title={$t('explorer.extract')} aria-label={$t('explorer.extract')}><IconArchive size={14} /></button>
                  {/if}
                {/if}
                {#if entry.is_directory && !snapshotMode}
                  <button class={efRowAction} data-testid="explorer-row-share" onclick={(e) => { e.stopPropagation(); shareFolder(entry); }} title={$t('explorer.share')} aria-label={$t('explorer.share')}><IconShare size={14} /></button>
                {/if}
                {#if !snapshotMode}
                  <button class={efRowAction} data-testid="explorer-row-rename" onclick={(e) => { e.stopPropagation(); openRename(entry); }} title={$t('explorer.rename')} aria-label={$t('explorer.rename')}><IconPencil size={14} /></button>
                  <button class={efRowAction} data-testid="explorer-row-permissions" onclick={(e) => { e.stopPropagation(); openPermissions(entry); }} title={$t('explorer.permissions')} aria-label={$t('explorer.permissions')}><IconLock size={14} /></button>
                  <button class={cn(efRowAction, efRowDanger)} data-testid="explorer-row-delete" onclick={(e) => { e.stopPropagation(); handleDelete(entry); }} title={$t('explorer.delete_to_trash')} aria-label={$t('explorer.delete_to_trash')}><IconTrash size={14} /></button>
                {/if}
              </span>
            </div>
          {/each}
          {#if sortedEntries.length === 0}
            <div data-testid="explorer-state-message" class={stateMessage}><IconFolderOpen size={40} /><p class="m-0">{$t('explorer.empty_folder')}</p></div>
          {/if}
          {#if visibleLimit < sortedEntries.length}
            <div bind:this={sentinelEl} class="flex justify-center py-3">
              <Button variant="ghost" onclick={() => visibleLimit += VIRTUAL_PAGE}>{$t('explorer.show_more', { remaining: String(sortedEntries.length - visibleLimit) })}</Button>
            </div>
          {/if}
        </div>
      {:else}
        <div data-testid="explorer-file-grid" class="grid gap-2 p-4 [grid-template-columns:repeat(auto-fill,minmax(110px,1fr))]">
          {#each visibleEntries as entry, vi (entry.path)}
            <button data-testid="explorer-grid-item" data-path={entry.path} draggable="true" ondragstart={(e) => onRowDragStart(e, entry)} ondragend={endInternalDrag} ondragover={entry.is_directory ? onFolderDragOver : undefined} ondrop={entry.is_directory ? (e) => onFolderDrop(e, entry.path) : undefined} class:selected={selected.has(entry.path)} class="flex flex-col items-center gap-1.5 p-3 rounded-lg border border-transparent bg-transparent cursor-pointer hover:bg-[var(--hv-subtle-hover)] text-center" onclick={(e) => handleRowClick(entry, sortedEntries.indexOf(entry), e)}>
              {#if !entry.is_directory && isImage(entry.name)}
                <img class="w-12 h-12 object-cover rounded-lg" src={api.getThumbnailUrl(entry.path, thumbnailTicket)} alt={entry.name} loading="lazy" onerror={(e) => { (e.target as HTMLImageElement).style.display = 'none'; (e.target as HTMLImageElement).nextElementSibling?.classList.remove('explorer-grid-icon--hidden'); }} />
                <span class="explorer-grid-icon explorer-grid-icon--hidden"><Icon name="image" size={48} /></span>
              {:else}
                <Icon name={getFileIcon(entry)} size={48} className={entry.is_directory ? 'folder-icon' : ''} />
              {/if}
              <span class="text-c-sm text-[var(--hv-shell-text-strong)] leading-tight break-words">{entry.name}</span>
              <span class="text-micro text-[var(--hv-shell-text-2)]">{entry.is_directory ? $t('explorer.folder_label') : formatSize(entry.size_bytes)}</span>
            </button>
          {/each}
          {#if sortedEntries.length === 0}
            <div data-testid="explorer-state-message" class={stateMessage}><IconFolderOpen size={40} /><p class="m-0">{$t('explorer.empty_folder')}</p></div>
          {/if}
          {#if visibleLimit < sortedEntries.length}
            <div bind:this={sentinelEl} class="flex justify-center py-3 [grid-column:1/-1]">
              <Button variant="ghost" onclick={() => visibleLimit += VIRTUAL_PAGE}>{$t('explorer.show_more', { remaining: String(sortedEntries.length - visibleLimit) })}</Button>
            </div>
          {/if}
        </div>
      {/if}
    {/if}
    {/if}
  </div>

  {#snippet footer()}
    {#if listing && !showTrash}
      <span data-testid="explorer-footer" class="flex items-center gap-2 min-w-0">
        <span>{$t('explorer.footer_summary', { dirs: String(listing.directory_count), files: String(listing.file_count) })}</span>
        {#if listing.total_size > 0}<span>&middot; {formatSize(listing.total_size)}</span>{/if}
        {#if listing.truncated}<span class="text-[var(--hv-warning)]">&middot; {$t('explorer.listing_truncated')}</span>{/if}
      </span>
      {#if selected.size > 0}
        <span data-testid="explorer-selection-summary">{$t('explorer.selected_count', { count: String(selected.size) })}</span>
      {/if}
    {/if}
  {/snippet}
  <ConfirmDialog
    {dlg}
    onconfirm={() => dlg.settle(true)}
    oncancel={() => dlg.settle(false)}
  />
</PageLayout><!-- New folder modal -->
<Modal open={showNewFolder} title={$t('explorer.new_folder')} size="sm" onclose={() => { showNewFolder = false; newFolderName = ''; }}>
  <p class="m-0 mb-2 text-xs text-[var(--hv-shell-text-2)]">{$t('explorer.in_folder', { path: currentPath })}</p>
  <input type="text" class="w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]" data-testid="explorer-new-folder-input" bind:value={newFolderName} placeholder={$t('explorer.folder_name_placeholder')} onkeydown={(e) => { if (e.key === 'Enter') handleMkdir(); }} />
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={() => { showNewFolder = false; newFolderName = ''; }}>{$t('common.cancel')}</Button>
      <Button variant="primary" onclick={handleMkdir}>{$t('common.create')}</Button>
    </div>
  {/snippet}
</Modal>

<!-- Compress modal -->
<Modal open={showCompress} title={$t('explorer.compress_selection')} size="sm" onclose={() => showCompress = false}>
  <label class="block text-c-label font-semibold text-[var(--hv-shell-text-strong)] mb-1" for="compressName">{$t('explorer.archive_name')}</label>
  <input id="compressName" type="text" class="w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]" bind:value={compressName} onkeydown={(e) => { if (e.key === 'Enter') handleCompress(); }} />
  <label class="block text-c-label font-semibold text-[var(--hv-shell-text-strong)] mt-3 mb-1" for="compressFormat">{$t('explorer.archive_format')}</label>
  <select id="compressFormat" class="w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]" bind:value={compressFormat}>
    <option value="tar.gz">tar.gz</option>
    <option value="zip">zip</option>
  </select>
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={() => showCompress = false}>{$t('common.cancel')}</Button>
      <Button variant="primary" onclick={handleCompress}>{$t('explorer.compress_selection')}</Button>
    </div>
  {/snippet}
</Modal>

<!-- Rename modal -->
<Modal open={showRename && !!renameTarget} title={$t('explorer.rename')} size="sm" onclose={() => showRename = false}>
  {#if renameTarget}
    <p class="m-0 mb-2 text-xs text-[var(--hv-shell-text-2)]">{renameTarget.path}</p>
    <input type="text" class="w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]" bind:value={renameName} onkeydown={(e) => { if (e.key === 'Enter') handleRename(); }} />
  {/if}
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={() => showRename = false}>{$t('common.cancel')}</Button>
      <Button variant="primary" onclick={handleRename}>{$t('explorer.rename')}</Button>
    </div>
  {/snippet}
</Modal>

<!-- Permissions modal -->
<Modal open={showPermissions && !!chmodTarget} title={$t('explorer.change_permissions')} size="sm" onclose={() => showPermissions = false}>
  {#if chmodTarget}
    <p class="m-0 mb-2 text-xs text-[var(--hv-shell-text-2)]">{chmodTarget.path}</p>
    <p class="m-0 mb-2 text-xs text-[var(--hv-shell-text-2)]">{$t('explorer.current_mode', { mode: chmodTarget.permissions })}</p>
    <input type="text" class="w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]" bind:value={chmodMode} placeholder="755" onkeydown={(e) => { if (e.key === 'Enter') handleChmod(); }} />
    <p class="m-0 mt-2 text-xs text-[var(--hv-shell-text-2)]">{$t('explorer.octal_mode_hint')}</p>
  {/if}
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={() => showPermissions = false}>{$t('common.cancel')}</Button>
      <Button variant="primary" onclick={handleChmod}>{$t('common.apply')}</Button>
    </div>
  {/snippet}
</Modal>

<!-- Preview modal -->
<Modal open={showPreview && !!previewContent} title={previewContent?.name ?? ''} size="lg" onclose={closePreview}>
  {#if previewContent}
    <p class="m-0 mb-3 text-xs text-[var(--hv-shell-text-2)]">
      {formatSize(previewContent.size_bytes)} &middot; {previewContent.mime_type}
      {#if previewContent.is_truncated}<span class="ml-1 px-1.5 py-0.5 rounded bg-[var(--hv-warning-bg)] text-[var(--hv-warning)] text-micro font-bold uppercase">{$t('explorer.truncated')}</span>{/if}
    </p>
    {#if previewContent.mime_type.startsWith('image/')}
      <img src={api.getThumbnailUrl(previewContent.path, thumbnailTicket)} alt={previewContent.name} class="max-w-full max-h-[50vh] object-contain mx-auto" />
    {:else if previewContent.previewable}
      {#if editing}
        <textarea data-testid="explorer-preview-editor" class="w-full min-h-[40vh] p-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm font-[family-name:var(--hv-font-mono)] outline-none focus:border-[var(--hv-primary)]" bind:value={editContent} spellcheck="false" oninput={() => (editDirty = true)}></textarea>
        {#if editError}<p class="mt-2 text-xs text-[var(--hv-error)]">{editError}</p>{/if}
      {:else}
        <pre data-testid="explorer-preview-text" class="m-0 p-3 rounded-lg bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] text-c-base font-[family-name:var(--hv-font-mono)] overflow-auto max-h-[50vh]">{previewContent.content}</pre>
      {/if}
    {:else}
      <div class="flex flex-col items-center gap-2 py-10 text-center text-[var(--hv-shell-text-2)]">
        <p class="m-0">{ $t('explorer.preview_unavailable')}</p>
        <Button variant="ghost" onclick={() => handleDownload({ path: previewContent!.path, name: previewContent!.name, is_directory: false } as FileEntry)}><IconDownload size={14} /> {$t('explorer.download')}</Button>
      </div>
    {/if}
  {/if}
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      {#if canEditPreview && editing}
        <Button variant="ghost" onclick={() => { editing = false; editError = ''; }}>{$t('common.cancel')}</Button>
        <Button variant="primary" disabled={editSaving} onclick={saveEdit}>{$t('explorer.save')}</Button>
      {:else}
        {#if canEditPreview}
          <Button variant="ghost" onclick={startEdit}><IconPencil size={14} /> {$t('explorer.edit')}</Button>
        {/if}
        <Button variant="primary" onclick={closePreview}>{$t('common.close')}</Button>
      {/if}
    </div>
  {/snippet}
</Modal>

<!-- Copy/move/upload name-collision prompt -->
<Modal open={!!collisionPrompt} title={$t('explorer.collision_title')} size="md" onclose={() => resolveCollision(null)}>
  {#if collisionPrompt}
    <p class="m-0 text-sm text-[var(--hv-shell-text)]">{$t('explorer.collision_body', { name: collisionPrompt.name })}</p>
  {/if}
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end flex-wrap">
      <Button variant="ghost" onclick={() => resolveCollision(null)}>{$t('common.cancel')}</Button>
      <Button variant="ghost" onclick={() => resolveCollision('skip')}>{$t('explorer.collision_skip')}</Button>
      <Button variant="ghost" onclick={() => resolveCollision('keep_both')}>{$t('explorer.collision_keep_both')}</Button>
      <Button variant="primary" onclick={() => resolveCollision('overwrite')}>{$t('explorer.collision_replace')}</Button>
    </div>
  {/snippet}
</Modal>

<!-- Share by link -->
<ShareLinkModal open={showShareLink && !!shareLinkTarget} initialPath={shareLinkTarget?.path ?? ''} onclose={() => { showShareLink = false; shareLinkTarget = null; }} />

<!-- Rewind: per-file version timeline -->
<Modal open={showRewind} title={$t('explorer.rewind_title')} size="md" onclose={closeRewind}>
  <p class="m-0 mb-1 text-xs text-[var(--hv-shell-text-2)]">{rewindLabel}</p>
  {#if rewindLoading}
    <div class="flex items-center gap-2 py-6 text-sm text-[var(--hv-shell-text-2)]" role="status"><IconLoader size={16} class="animate-spin" /> {$t('explorer.loading')}</div>
  {:else if rewindVersions.length === 0}
    <p class="m-0 py-6 text-sm text-[var(--hv-shell-text-2)]">{$t('explorer.rewind_empty')}</p>
  {:else}
    <div class="flex flex-col gap-1.5">
      {#each rewindVersions as v (v.browse_path)}
        <div class="flex items-center gap-2 rounded-lg border border-[var(--hv-shell-border-soft)] px-3 py-2">
          <IconHistory size={16} class="shrink-0 text-[var(--hv-shell-text-2)]" />
          <span class="min-w-0 flex-1">
            <span class="block truncate text-c-base font-medium text-[var(--hv-shell-text-strong)]">{v.name}</span>
            <span class="block text-c-xs text-[var(--hv-shell-text-2)]">{formatSnapDate(v.created)}</span>
          </span>
          <Button variant="ghost" onclick={() => restoreRewindVersion(v)}>{$t('explorer.rewind_restore')}</Button>
        </div>
      {/each}
    </div>
  {/if}
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={closeRewind}>{$t('common.close')}</Button>
    </div>
  {/snippet}
</Modal>

<!-- Network discovery -->
<Modal open={showNetwork} title={$t('explorer.network')} size="md" onclose={() => showNetwork = false}>
  {#if networkLoading}
    <div class="flex items-center gap-2 py-6 text-sm text-[var(--hv-shell-text-2)]" role="status"><IconLoader size={16} class="animate-spin" /> {$t('explorer.loading')}</div>
  {:else if neighbours.length === 0}
    <p class="m-0 py-6 text-sm text-[var(--hv-shell-text-2)]">{$t('explorer.network_empty')}</p>
  {:else}
    <div class="flex flex-col gap-1.5">
      {#each neighbours as n (`${n.source}:${n.host}:${n.share ?? ''}`)}
        <div class="flex items-center gap-2 rounded-lg border border-[var(--hv-shell-border-soft)] px-3 py-2">
          <IconServer size={16} class="shrink-0 text-[var(--hv-shell-text-2)]" />
          <span class="min-w-0 flex-1">
            <span class="block truncate text-c-base font-medium text-[var(--hv-shell-text-strong)]">{n.share ? `${n.host} / ${n.share}` : n.host}</span>
            <span class="block truncate text-c-xs text-[var(--hv-shell-text-2)]">{[n.address, n.source].filter(Boolean).join(' · ')}</span>
          </span>
        </div>
      {/each}
    </div>
  {/if}
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={() => showNetwork = false}>{$t('common.close')}</Button>
      <Button variant="primary" onclick={openNetwork}>{$t('explorer.network_scan')}</Button>
    </div>
  {/snippet}
</Modal>

<!-- Drive formatting (admin only, destructive) -->
<Modal open={showFormat} title={$t('explorer.format_drive')} size="sm" onclose={() => showFormat = false}>
  <label class="mb-1.5 block text-c-label font-medium text-[var(--hv-shell-text-strong)]" for="formatDevice">/dev/…</label>
  <input id="formatDevice" type="text" class="h-11 w-full rounded-lg border border-[var(--hv-shell-border-strong)] bg-[var(--hv-shell-border-soft)] px-3 text-sm outline-none focus:border-[var(--hv-primary)]" bind:value={formatDevice} placeholder="/dev/sdb1" autocomplete="off" />
  <label class="mb-1.5 mt-4 block text-c-label font-medium text-[var(--hv-shell-text-strong)]" for="formatFstype">ext4 / exFAT</label>
  <select id="formatFstype" class="h-11 w-full rounded-lg border border-[var(--hv-shell-border-strong)] bg-[var(--hv-shell-border-soft)] px-3 text-sm outline-none focus:border-[var(--hv-primary)]" bind:value={formatFstype}>
    <option value="ext4">ext4</option>
    <option value="exfat">exFAT</option>
  </select>
  <label class="mb-1.5 mt-4 block text-c-label font-medium text-[var(--hv-shell-text-strong)]" for="formatLabel">{$t('explorer.format_label')}</label>
  <input id="formatLabel" type="text" class="h-11 w-full rounded-lg border border-[var(--hv-shell-border-strong)] bg-[var(--hv-shell-border-soft)] px-3 text-sm outline-none focus:border-[var(--hv-primary)]" bind:value={formatLabel} maxlength="16" autocomplete="off" />
  {#if formatDevice.trim()}
    <p class="m-0 mt-3 text-xs font-medium text-[var(--hv-error)]">{$t('explorer.format_warning', { device: formatDevice.trim() })}</p>
  {/if}
  {#if formatError}
    <p class="m-0 mt-2 text-xs text-[var(--hv-error)]" role="alert">{formatError}</p>
  {/if}
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={() => showFormat = false}>{$t('common.cancel')}</Button>
      <Button variant="danger" onclick={handleFormat} disabled={formatBusy || !formatDevice.trim()}>{#if formatBusy}<IconLoader size={16} class="animate-spin" />{/if} {$t('explorer.format_confirm')}</Button>
    </div>
  {/snippet}
</Modal>

<!-- Unsaved editor changes — never drop them silently -->
<Modal open={showDiscardConfirm} title={$t('explorer.unsaved_title')} size="sm" closeOnBackdrop={false} onclose={() => (showDiscardConfirm = false)}>
  <p class="m-0 text-sm text-[var(--hv-shell-text)]">{$t('explorer.unsaved_body')}</p>
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={() => { showDiscardConfirm = false; showPreview = true; }}>{$t('common.cancel')}</Button>
      <Button variant="danger" onclick={discardEdit}>{$t('explorer.unsaved_discard')}</Button>
      <Button variant="primary" disabled={editSaving} onclick={saveEditAndClose}>{$t('explorer.save')}</Button>
    </div>
  {/snippet}
</Modal>

<FolderPicker open={folderPickerAction !== null} value={currentPath} onselect={handleFolderPickerSelect} onclose={() => { folderPickerAction = null; extractTarget = null; }} />