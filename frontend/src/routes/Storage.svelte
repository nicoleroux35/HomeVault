<!-- ========================================================================
     File          : Storage.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 10/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
     The Storage app: physical disks, partitions and SMART state, virtual
     disks (mdadm/ZFS/Btrfs) with lifecycle actions, scrub start/status,
     encrypted volumes, snapshots and importable foreign pools. Admin
     mutations; live refresh on visibility.

     Dependencies:
       - svelte (>= 5), theme primitives, app/ composites, utils
         (autoRevalidate), $lib/services/api

     Usage:
     Opened as a desktop window app.
     ======================================================================== -->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { api } from '$lib/services/api';
  import { onAppVisible } from '$lib/utils/autoRevalidate';
  import { Spinner, Badge, Modal, Button, Toggle, Alert } from '$lib/components/theme';
  import { PageLayout, ConfirmDialog, StatCard, EmptyState } from '$lib/components/app';
  import { createConfirmState } from '$lib/utils/confirmDialog.svelte';
  import { cn } from '$lib/utils.js';
  import WindowHeader from '$lib/components/desktop/WindowHeader.svelte';
  import { IconServer, IconDatabase, IconThermometer, IconUsb, IconActivity, IconLock, IconLockOpen, IconShieldCheckered, IconTrash, IconBolt, IconPlus, IconPlayerPlay, IconClock, IconAlertTriangle, IconStack, IconTool, IconCamera } from '@tabler/icons-svelte';
  import { t, translate } from '$lib/stores/i18n';
  import { isAdmin } from '$lib/stores/auth';

  interface PhysicalDisk {
    name: string;
    device: string;
    size: number;
    vendor?: string;
    model: string;
    serial: string;
    type: string;
    role: string;
    connection_type: string;
    smart_status: string;
    temperature?: number | null;
    partition_count: number;
    errors: number;
    partitions?: PartitionInfo[];
  }

  interface PartitionInfo {
    device: string;
    mount_point: string;
    filesystem: string;
    size: number;
    used: number;
    available: number;
    percent: number;
    uuid?: string;
  }

  interface RemovableDisk {
    name: string;
    device: string;
    size: number;
    model: string;
    filesystem?: string;
    mount_point?: string;
    label?: string;
    mounted: boolean;
  }

  interface EncryptedVolume {
    id: string;
    device_path: string;
    mapper_name: string;
    luks_uuid: string;
    filesystem: string;
    label: string;
    mount_point: string;
    created_at: string;
    unlocked: boolean;
  }

  interface AvailableDevice {
    name: string;
    size: number;
    model: string;
    inUse: boolean;
  }

  interface SmartCapabilities {
    available: boolean;
    supports_short_test: boolean;
    supports_long_test: boolean;
    supports_conveyance_test: boolean;
  }

  interface SmartTestStatus {
    in_progress: boolean;
    progress_pct: number;
    remaining_minutes: number;
    last_result: string | null;
  }

  let loading = true;
  let heroVisible = false;

  // --- Snapshot schedules (GFS) state --------------------------------
  interface SnapshotScheduleRow {
    id: string;
    pool: string;
    enabled: boolean;
    cron: string;
    prefix: string;
    recursive: boolean;
    gfs_hourly: number;
    gfs_daily: number;
    gfs_weekly: number;
    gfs_monthly: number;
    gfs_yearly: number;
    last_run_at: string | null;
    last_status: string | null;
    last_error: string | null;
    next_run_at: string | null;
  }
  interface PoolSnapshotRow {
    name: string;
    created: string;
    used: number;
    referenced: number;
    auto: boolean;
  }
  let snapshotSchedules: SnapshotScheduleRow[] = [];
  let snapSchedulesLoaded = false;
  let snapScheduleModal = false;
  let snapForm: {
    id: string | null; pool: string; cron: string; prefix: string; recursive: boolean;
    gfs_hourly: number; gfs_daily: number; gfs_weekly: number; gfs_monthly: number;
    gfs_yearly: number; enabled: boolean;
  } = { id: null, pool: '', cron: '0 */4 * * *', prefix: 'hv-auto', recursive: true,
        gfs_hourly: 24, gfs_daily: 7, gfs_weekly: 4, gfs_monthly: 12, gfs_yearly: 1, enabled: true };
  let snapFormError = '';
  let snapBusyId = '';
  let snapViewPool = '';
  let snapViewRows: PoolSnapshotRow[] = [];
  let snapViewLoading = false;
  let activeTab: 'disks' | 'virtual' | 'partitions' | 'removable' | 'encryption' | 'cache' | 'snapshots' = 'disks';

  interface VirtualDisk {
    name: string;
    pool_type: 'Zfs' | 'Mdadm' | 'Btrfs' | 'Unknown';
    size_bytes: number;
    used_bytes: number;
    available_bytes: number;
    health: string;
    device_count: number;
    mount_point: string | null;
    raid_profile: string | null;
  }
  let virtualDisks: VirtualDisk[] = [];
  let virtualBusy = '';

  let disks: PhysicalDisk[] = [];
  let partitions: PartitionInfo[] = [];
  let removable: RemovableDisk[] = [];
  let encryptedVolumes: EncryptedVolume[] = [];
  let encryptionAvailableDevices: AvailableDevice[] = [];
  let encryptionBusy = false;
  let unlockPassphrase: Record<string, string> = {};

  let showDiskModal = false;
  let selectedDisk: PhysicalDisk | null = null;

  // SMART test state
  let smartCapabilities: SmartCapabilities | null = null;
  let smartTestStatus: SmartTestStatus | null = null;
  let smartTestType = 'short';
  let smartBusy = false;
  let smartPoller: ReturnType<typeof setInterval> | null = null;
  let removableRefreshInterval: ReturnType<typeof setInterval>;

  // Circular diagram state
  let showDiskUsage = false;
  $: diskUsageSlices = selectedDisk ? diskPartitionUsageSlices(selectedDisk) : [];

  let showEncryptModal = false;
  let encryptDevice = '';
  let encryptFilesystem = 'ext4';
  let encryptLabel = '';
  let encryptPassphrase = '';
  let encryptPassphraseConfirm = '';

  // ── Cache tier (HomeVault write caching) ──
  interface CacheSettingsLocal {
    cache_pool: string | null;
    mover_cron: string;
    mover_enabled: boolean;
    min_free_percent: number;
    mover_last_run_at: string | null;
    mover_age_days: number;
    mover_excludes: string | null;
    mover_bwlimit: number;
  }
  interface CachePoolLocal {
    name: string;
    pool_type: string;
    mount_point: string | null;
    size_bytes: number;
    used_bytes: number;
    available_bytes: number;
  }
  interface CacheShareLocal {
    id: string;
    name: string;
    cache_mode: 'yes' | 'no' | 'prefer' | 'only';
    target_pool: string;
    enabled: boolean;
    created_at: string;
    updated_at: string;
  }
  interface CacheMoverRunLocal {
    id: string;
    status: string;
    started_at: string;
    finished_at: string | null;
    files_moved: number;
    bytes_moved: number;
    error_message: string | null;
  }

  const CACHE_MODES = ['yes', 'no', 'prefer', 'only'] as const;

  let cacheSettings: CacheSettingsLocal = { cache_pool: null, mover_cron: '0 3 * * *', mover_enabled: false, min_free_percent: 20, mover_last_run_at: null, mover_age_days: 0, mover_excludes: null, mover_bwlimit: 0 };
  let cachePools: CachePoolLocal[] = [];
  let cacheShares: CacheShareLocal[] = [];
  let cacheRuns: CacheMoverRunLocal[] = [];
  let cacheLoading = false;
  let cacheBusy = false;
  let cacheError = '';
  let showCacheShareModal = false;
  let moverJobId: string | null = null;
  let moverRunning = false;
  let moverProgress = 0;
  let moverProgressMsg = '';
  let newShareName = '';
  let newShareMode: CacheShareLocal['cache_mode'] = 'yes';
  let newShareTarget = '';
  const dlg = createConfirmState();

  // --- Snapshot schedules (GFS) --------------------------------------
  async function loadSnapshotSchedules() {
    try {
      const res = await api.getSnapshotSchedules();
      snapshotSchedules = res.success ? (res.data?.schedules ?? []) : [];
    } catch {
      snapshotSchedules = [];
    } finally {
      snapSchedulesLoaded = true;
    }
  }

  function openSnapCreate() {
    snapForm = { id: null, pool: virtualDisks[0]?.name ?? '', cron: '0 */4 * * *', prefix: 'hv-auto',
                 recursive: true, gfs_hourly: 24, gfs_daily: 7, gfs_weekly: 4,
                 gfs_monthly: 12, gfs_yearly: 1, enabled: true };
    snapFormError = '';
    snapScheduleModal = true;
  }

  function openSnapEdit(row: SnapshotScheduleRow) {
    snapForm = { id: row.id, pool: row.pool, cron: row.cron, prefix: row.prefix,
                 recursive: row.recursive, gfs_hourly: row.gfs_hourly, gfs_daily: row.gfs_daily,
                 gfs_weekly: row.gfs_weekly, gfs_monthly: row.gfs_monthly,
                 gfs_yearly: row.gfs_yearly, enabled: row.enabled };
    snapFormError = '';
    snapScheduleModal = true;
  }

  async function saveSnapSchedule() {
    if (!snapForm.pool.trim()) { snapFormError = $t('tasks.error_name'); return; }
    snapBusyId = 'form';
    snapFormError = '';
    const body = {
      pool: snapForm.pool.trim(), cron: snapForm.cron.trim(), prefix: snapForm.prefix.trim(),
      recursive: snapForm.recursive, gfs_hourly: snapForm.gfs_hourly, gfs_daily: snapForm.gfs_daily,
      gfs_weekly: snapForm.gfs_weekly, gfs_monthly: snapForm.gfs_monthly,
      gfs_yearly: snapForm.gfs_yearly, enabled: snapForm.enabled,
    };
    try {
      const res = snapForm.id
        ? await api.updateSnapshotSchedule(snapForm.id, body)
        : await api.createSnapshotSchedule(body);
      if (!res.success) { snapFormError = res.message || $t('tasks.error_save'); return; }
      snapScheduleModal = false;
      await loadSnapshotSchedules();
    } catch {
      snapFormError = $t('tasks.error_save');
    } finally {
      snapBusyId = '';
    }
  }

  async function runSnapNow(row: SnapshotScheduleRow) {
    snapBusyId = row.id;
    try {
      await api.runSnapshotScheduleNow(row.id);
      await loadSnapshotSchedules();
    } finally {
      snapBusyId = '';
    }
  }

  async function askSnapDelete(row: SnapshotScheduleRow) {
    if (await dlg.ask($t('storage.snapshots.delete_confirm_msg', { pool: row.pool }))) {
      try {
        await api.deleteSnapshotSchedule(row.id);
      } finally {
        await loadSnapshotSchedules();
      }
    }
  }

  async function viewPoolSnapshots(pool: string) {
    snapViewPool = pool;
    snapViewLoading = true;
    try {
      const res = await api.getPoolSnapshots(pool);
      snapViewRows = res.success ? (res.data?.snapshots ?? []) : [];
    } catch {
      snapViewRows = [];
    } finally {
      snapViewLoading = false;
    }
  }

  onMount(async () => {
    await Promise.all([
      loadDisks(),
      loadVirtualDisks(),
      loadPartitions(),
      loadRemovable(),
      loadEncryptedVolumes(),
      loadCache(),
      loadImportablePools(),
      loadSnapshotSchedules(),
    ]);
    loading = false;
    setTimeout(() => heroVisible = true, 100);

    removableRefreshInterval = setInterval(() => {
      if (activeTab === 'removable') loadRemovable();
      if (activeTab === 'snapshots') loadSnapshotSchedules();
    }, 10000);

    if (typeof document !== 'undefined') {
      document.addEventListener('visibilitychange', onVisibilityChange);
    }
    // Replaces the header refresh button: revalidate everything on focus.
    stopAutoRevalidate = onAppVisible(refreshAll);
  });

  let stopAutoRevalidate: (() => void) | null = null;
  onDestroy(() => {
    if (smartPoller !== null) clearInterval(smartPoller);
    if (removableRefreshInterval) clearInterval(removableRefreshInterval);
    if (typeof document !== 'undefined') {
      document.removeEventListener('visibilitychange', onVisibilityChange);
    }
    stopAutoRevalidate?.();
  });

  async function loadDisks() {
    const resp = await api.getStorageDisks();
    if (resp.success && resp.data) disks = resp.data;
  }

  async function loadPartitions() {
    const resp = await api.getStoragePartitions();
    if (resp.success && resp.data) partitions = resp.data;
  }

  // ── Virtual disks (every RAID set: mdadm + ZFS + Btrfs) ──
  async function loadVirtualDisks() {
    const resp = await api.listVirtualDisks();
    if (resp.success && Array.isArray(resp.data)) virtualDisks = resp.data;
  }

  // Live scrub / consistency-check status, keyed by disk name. Populated on
  // demand ("Check status") and shown inline on each card so the user can see
  // whether a scrub is running without leaving the page.
  interface ScrubStatus { status: string; in_progress: boolean; }
  let scrubStatus: Record<string, ScrubStatus> = {};
  let scrubStatusBusy = '';

  async function refreshScrubStatus(v: VirtualDisk) {
    scrubStatusBusy = v.name;
    const r = await api.getVirtualDiskScrubStatus(v.name);
    scrubStatusBusy = '';
    if (r.success && r.data) {
      scrubStatus = { ...scrubStatus, [v.name]: { status: r.data.status, in_progress: !!r.data.in_progress } };
    } else {
      dlg.notify(r.message || r.error || $t('storage.vd_scrub_status_failed'));
    }
  }

  // ── Foreign (importable) pools ──
  interface ImportablePool {
    name: string;
    pool_type: 'Zfs' | 'Btrfs' | 'Mdadm' | 'Unknown';
    id: string | null;
    devices: string[];
    state: string;
    status: string | null;
  }
  let importablePools: ImportablePool[] = [];
  let importableLoading = false;
  let importBusy = '';
  let showImportModal = false;
  let importTarget: ImportablePool | null = null;
  let importNewName = '';

  async function loadImportablePools() {
    importableLoading = true;
    const r = await api.getImportablePools();
    importableLoading = false;
    if (r.success && Array.isArray(r.data)) importablePools = r.data;
  }

  function openImportModal(pool: ImportablePool) {
    importTarget = pool;
    importNewName = '';
    showImportModal = true;
  }

  async function confirmImport() {
    if (!importTarget) return;
    importBusy = importTarget.name;
    const r = await api.importPool(importTarget.name, importTarget.pool_type.toLowerCase(), importNewName.trim() || undefined);
    importBusy = '';
    showImportModal = false;
    if (r.success) {
      await Promise.all([loadImportablePools(), loadVirtualDisks(), loadDisks()]);
    } else {
      dlg.notify(r.message || r.error || $t('storage.import_failed'));
    }
  }

  function vdKindLabel(k: string): string {
    if (k === 'Mdadm') return 'RAID (mdadm)';
    if (k === 'Zfs') return 'ZFS';
    if (k === 'Btrfs') return 'Btrfs';
    return $t('storage.unknown');
  }

  function vdHealthy(v: VirtualDisk): boolean {
    const h = (v.health || '').toLowerCase();
    return h.includes('online') || h.includes('healthy') || h.includes('active') || h.includes('clean') || h.includes('optimal');
  }

  async function scrubVirtualDisk(v: VirtualDisk) {
    if (!(await dlg.ask(translate('storage.vd_scrub_confirm', { name: v.name })))) return;
    virtualBusy = v.name;
    const r = await api.scrubVirtualDisk(v.name);
    virtualBusy = '';
    if (r.success) { await loadVirtualDisks(); }
    else dlg.notify(r.message || r.error || translate('storage.vd_action_failed'));
  }

  async function rebuildVirtualDisk(v: VirtualDisk) {
    if (!(await dlg.ask(translate('storage.vd_rebuild_confirm', { name: v.name })))) return;
    virtualBusy = v.name;
    const r = await api.rebuildRaid(v.name);
    virtualBusy = '';
    if (r.success) { await loadVirtualDisks(); }
    else dlg.notify(r.message || r.error || translate('storage.vd_action_failed'));
  }

  async function destroyVirtualDisk(v: VirtualDisk) {
    if (!(await dlg.ask(translate('storage.vd_destroy_confirm', { name: v.name })))) return;
    virtualBusy = v.name;
    const r = await api.deleteVirtualDisk(v.name);
    virtualBusy = '';
    if (r.success) { await Promise.all([loadVirtualDisks(), loadDisks()]); }
    else dlg.notify(r.message || r.error || translate('storage.vd_action_failed'));
  }

  // ── Cache tier ──
  async function loadCache() {
    cacheLoading = true;
    cacheError = '';
    try {
      const [settingsResp, poolsResp, sharesResp, runsResp] = await Promise.all([
        api.getCacheSettings(),
        api.getCachePools(),
        api.getCacheShares(),
        api.getCacheMoverRuns(),
      ]);
      if (settingsResp.success && settingsResp.data) cacheSettings = settingsResp.data;
      if (poolsResp.success && poolsResp.data) cachePools = poolsResp.data;
      if (sharesResp.success && sharesResp.data) cacheShares = sharesResp.data;
      if (runsResp.success && runsResp.data) cacheRuns = runsResp.data;
    } catch (e) {
      cacheError = $t('cache.load_failed');
    } finally {
      cacheLoading = false;
    }
  }

  async function saveCacheSettings() {
    cacheBusy = true;
    try {
      const resp = await api.updateCacheSettings({
        cache_pool: cacheSettings.cache_pool,
        mover_cron: cacheSettings.mover_cron,
        mover_enabled: cacheSettings.mover_enabled,
        min_free_percent: cacheSettings.min_free_percent,
        mover_age_days: cacheSettings.mover_age_days,
        mover_excludes: cacheSettings.mover_excludes,
        mover_bwlimit: cacheSettings.mover_bwlimit,
      });
      if (resp.success) {
        await loadCache();
      } else {
        cacheError = resp.message || $t('cache.config_save_failed');
      }
    } catch {
      cacheError = $t('cache.config_save_failed');
    } finally {
      cacheBusy = false;
    }
  }

  async function runMover() {
    cacheBusy = true;
    cacheError = '';
    try {
      const resp = await api.runCacheMover();
      if (resp.success && resp.data?.job_id) {
        const jobId = resp.data.job_id;
        moverJobId = jobId;
        moverRunning = true;
        moverProgress = 0;
        moverProgressMsg = $t('cache.mover_started');
        const stop = api.subscribeToProgress(
          jobId,
          (event) => {
            moverProgress = Math.round(event.progress || 0);
            moverProgressMsg = event.message || '';
          },
          async () => {
            moverRunning = false;
            moverJobId = null;
            moverProgress = 100;
            stop();
            await loadCache();
          },
          () => {
            moverRunning = false;
            moverJobId = null;
            stop();
          }
        );
      } else {
        cacheError = resp.message || $t('cache.mover_failed');
      }
    } catch {
      cacheError = $t('cache.mover_failed');
    } finally {
      cacheBusy = false;
    }
  }

  function openCacheShareModal() {
    newShareName = '';
    newShareMode = 'yes';
    newShareTarget = cachePools.length > 0 ? cachePools[0].name : '';
    showCacheShareModal = true;
  }

  async function createCacheShare() {
    if (!newShareName.trim() || !newShareTarget) return;
    cacheBusy = true;
    try {
      const resp = await api.createCacheShare({
        name: newShareName.trim(),
        cache_mode: newShareMode,
        target_pool: newShareTarget,
      });
      if (resp.success) {
        showCacheShareModal = false;
        await loadCache();
      } else {
        cacheError = resp.message || $t('cache.share_create_failed');
      }
    } catch {
      cacheError = $t('cache.share_create_failed');
    } finally {
      cacheBusy = false;
    }
  }

  async function updateShareMode(share: CacheShareLocal, mode: string) {
    cacheBusy = true;
    try {
      const resp = await api.updateCacheShare(share.name, { cache_mode: mode });
      if (resp.success) {
        await loadCache();
      } else {
        cacheError = resp.message || $t('cache.share_update_failed');
      }
    } catch {
      cacheError = $t('cache.share_update_failed');
    } finally {
      cacheBusy = false;
    }
  }

  async function deleteCacheShare(share: CacheShareLocal) {
    if (!(await dlg.ask($t('cache.delete_confirm').replace('%name%', share.name)))) return;
    cacheBusy = true;
    try {
      const resp = await api.deleteCacheShare(share.name);
      if (resp.success) {
        await loadCache();
      } else {
        cacheError = resp.message || $t('cache.share_delete_failed');
      }
    } catch {
      cacheError = $t('cache.share_delete_failed');
    } finally {
      cacheBusy = false;
    }
  }

  async function loadRemovable() {
    const resp = await api.getStorageRemovable();
    if (resp.success && resp.data) removable = resp.data;
  }

  async function loadEncryptedVolumes() {
    const resp = await api.getEncryptedVolumes();
    if (resp.success && resp.data) encryptedVolumes = resp.data;
  }

  async function loadEncryptionAvailableDevices() {
    const resp = await api.getEncryptionAvailableDevices();
    if (resp.success && resp.data) encryptionAvailableDevices = resp.data.filter((d: AvailableDevice) => !d.inUse);
  }

  async function refreshAll() {
    loading = true;
    await Promise.all([loadDisks(), loadVirtualDisks(), loadPartitions(), loadRemovable(), loadEncryptedVolumes(), loadCache(), loadImportablePools()]);
    loading = false;
  }

  function openEncryptModal() {
    encryptDevice = '';
    encryptFilesystem = 'ext4';
    encryptLabel = '';
    encryptPassphrase = '';
    encryptPassphraseConfirm = '';
    showEncryptModal = true;
    loadEncryptionAvailableDevices();
  }

  async function confirmEncryptVolume() {
    if (!encryptDevice || !encryptLabel.trim()) {
      dlg.notify(translate('encryption.error_missing_fields'));
      return;
    }
    if (encryptPassphrase.length < 8) {
      dlg.notify(translate('encryption.error_passphrase_length'));
      return;
    }
    if (encryptPassphrase !== encryptPassphraseConfirm) {
      dlg.notify(translate('encryption.error_passphrase_mismatch'));
      return;
    }
    encryptionBusy = true;
    const resp = await api.createEncryptedVolume({
      device: encryptDevice,
      filesystem: encryptFilesystem,
      label: encryptLabel.trim(),
      passphrase: encryptPassphrase,
    });
    encryptionBusy = false;
    if (resp.success) {
      showEncryptModal = false;
      await Promise.all([loadDisks(), loadPartitions(), loadEncryptedVolumes()]);
    } else {
      dlg.notify(resp.message || resp.error || translate('encryption.error_create_failed'));
    }
  }

  async function unlockVolume(volume: EncryptedVolume) {
    const passphrase = unlockPassphrase[volume.id];
    if (!passphrase) return;
    encryptionBusy = true;
    const resp = await api.unlockEncryptedVolume(volume.id, passphrase);
    encryptionBusy = false;
    if (resp.success) {
      unlockPassphrase[volume.id] = '';
      await Promise.all([loadDisks(), loadPartitions(), loadEncryptedVolumes()]);
    } else {
      dlg.notify(resp.message || resp.error || translate('encryption.error_unlock_failed'));
    }
  }

  async function lockVolume(volume: EncryptedVolume) {
    encryptionBusy = true;
    const resp = await api.lockEncryptedVolume(volume.id);
    encryptionBusy = false;
    if (resp.success) await Promise.all([loadDisks(), loadPartitions(), loadEncryptedVolumes()]);
    else dlg.notify(resp.message || resp.error || translate('encryption.error_lock_failed'));
  }

  async function deleteVolume(volume: EncryptedVolume) {
    if (!(await dlg.ask(translate('encryption.delete_confirm', { label: volume.label })))) return;
    encryptionBusy = true;
    const resp = await api.deleteEncryptedVolume(volume.id);
    encryptionBusy = false;
    if (resp.success) await Promise.all([loadDisks(), loadPartitions(), loadEncryptedVolumes()]);
    else dlg.notify(resp.message || resp.error || translate('encryption.error_delete_failed'));
  }

  function onVisibilityChange() {
    if (document.visibilityState === 'visible') {
      loadRemovable();
    }
  }

  $: totalDisks = disks.length;
  $: healthyDisks = disks.filter(d => d.smart_status === 'Healthy').length;
  $: failedDisks = disks.filter(d => d.smart_status === 'Failed').length;
  $: mountedPartitions = partitions.filter(p => p.mount_point && p.mount_point !== '').length;

  $: totalCapacity = disks.reduce((sum, d) => sum + d.size, 0);

  function formatBytes(bytes: number): string {
    if (!bytes) return '0 B';
    const units = ['B', 'KB', 'MB', 'GB', 'TB'];
    let i = 0;
    let val = bytes;
    while (val >= 1024 && i < units.length - 1) { val /= 1024; i++; }
    return `${val.toFixed(1)} ${units[i]}`;
  }

  // ── SMART test functions ──
  let smartPollerActive = false;

  async function loadSmartCapabilities() {
    if (!selectedDisk) return;
    smartBusy = true;
    try {
      const resp = await api.getDiskSmartCapabilities(selectedDisk.name);
      if (resp.success && resp.data) {
        smartCapabilities = resp.data.capabilities;
        smartTestStatus = resp.data.test_status;
        if (smartTestStatus?.in_progress) {
          startSmartPolling();
        }
      }
    } catch {
      // silent
    } finally {
      smartBusy = false;
    }
  }

  async function handleSmartTestStart() {
    if (!selectedDisk || smartBusy) return;
    smartBusy = true;
    try {
      const resp = await api.startDiskSmartTest(selectedDisk.name, smartTestType);
      if (resp.success) {
        // Start polling for status
        smartTestStatus = { in_progress: true, progress_pct: 99, remaining_minutes: resp.data?.estimated_remaining_minutes || 0, last_result: null };
        startSmartPolling();
      } else {
        // silent error
      }
    } catch {
      // silent
    } finally {
      smartBusy = false;
    }
  }

  function startSmartPolling() {
    smartPollerActive = true;
    smartPoller = setInterval(async () => {
      if (!selectedDisk || !smartPollerActive) return;
      try {
        const resp = await api.getDiskSmartTestStatus(selectedDisk.name);
        if (resp.success && resp.data) {
          const st = resp.data.test_status;
          smartTestStatus = st;
          if (!st.in_progress && smartPollerActive) {
            stopSmartPolling();
          }
        }
      } catch {
        // if network error, keep polling
      }
    }, 3000);
  }

  function stopSmartPolling() {
    smartPollerActive = false;
    if (smartPoller !== null) {
      clearInterval(smartPoller);
      smartPoller = null;
    }
  }

  // ── Circular diagram: compute partition usage for a disk ──
  function diskPartitions(disk: PhysicalDisk): PartitionInfo[] {
    return partitions.filter(p => p.device.startsWith(`/dev/${disk.name}`));
  }

  function diskPartitionUsageSlices(disk: PhysicalDisk): { label: string; value: number; color: string; percent: number }[] {
    const slices: { label: string; value: number; color: string; percent: number }[] = [];
    const parts = diskPartitions(disk);
    const totalSize = disk.size;

    if (totalSize === 0) return slices;

    let accounted = 0;
    // Ring strokes + legend swatches sit on white: every stop must clear
    // 3:1 as a graphical object (WCAG 1.4.11) — #14b8a6 (2.49:1) →
    // #0F766E (5.47:1), #f97316 (2.80:1) → #C2410C (5.18:1).
    const colors = ['var(--hv-primary)', 'var(--hv-success)', 'var(--hv-warning)', 'var(--hv-app-groups-c)', 'var(--hv-app-widgets-c)', 'var(--hv-app-chat-c)', 'var(--hv-app-backup-c)', 'var(--hv-app-dashboard-c)'];

    parts.forEach((p, i) => {
      const mountLabel = p.mount_point || p.device;
      const percent = (p.size / totalSize) * 100;
      slices.push({
        label: mountLabel,
        value: p.size,
        color: colors[i % colors.length],
        percent,
      });
      accounted += p.size;
    });

    // Remaining unpartitioned space
    const unaccounted = totalSize - accounted;
    if (unaccounted > 0) {
      slices.push({
        label: $t('storage.unpartitioned'),
        value: unaccounted,
        color: 'var(--hv-border)',
        percent: (unaccounted / totalSize) * 100,
      });
    }

    return slices;
  }

  function smartLabel(status: string): { text: string; variant: string } {
    switch (status) {
      case 'Healthy': return { text: 'Healthy', variant: 'success' };
      case 'Failed': return { text: 'Failed', variant: 'error' };
      default: return { text: status || 'Unknown', variant: 'neutral' };
    }
  }

  function connLabel(type: string): string {
    return type?.toUpperCase() || '-';
  }

  function openDiskDetail(disk: PhysicalDisk) {
    stopSmartPolling();
    selectedDisk = disk;
    showDiskModal = true;
    smartCapabilities = null;
    smartTestStatus = null;
    showDiskUsage = false;
    loadSmartCapabilities();
  }

  function usageColor(percent: number): string {
    if (percent >= 90) return 'var(--hv-error)';
    if (percent >= 75) return 'var(--hv-warning, var(--hv-warning))';
    return 'var(--hv-success, var(--hv-success))';
  }

  function capacityStr(bytes: number): string {
    const tb = bytes / (1024 * 1024 * 1024 * 1024);
    if (tb >= 0.9) return `${tb.toFixed(1)} TB`;
    const gb = bytes / (1024 * 1024 * 1024);
    return `${gb.toFixed(0)} GB`;
  }

  function capacityMain(bytes: number): string {
    const tb = bytes / (1024 * 1024 * 1024 * 1024);
    if (tb >= 0.9) return tb.toFixed(1);
    const gb = bytes / (1024 * 1024 * 1024);
    return gb.toFixed(0);
  }

  function capacityUnit(bytes: number): string {
    const tb = bytes / (1024 * 1024 * 1024 * 1024);
    return tb >= 0.9 ? 'TB' : 'GB';
  }


// Shared shapes (appliance tokens only — the per-app CSS vocabulary is gone).
  const usageTrack = 'h-2 rounded-full bg-[var(--hv-shell-border-soft)] overflow-hidden';
  const inpSel = 'w-full box-border h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-strong)] text-sm outline-none transition-colors focus:border-[var(--hv-primary)]';
  const encInput = cn(inpSel, 'mb-3');
</script>
<PageLayout
  title={$t('storage.title')}
  data-testid="storage-page"
  tabs={[
    { id: 'disks', label: $t('storage.tab_disks'), icon: IconServer, count: totalDisks },
    { id: 'virtual', label: $t('storage.tab_virtual_disks'), icon: IconStack, count: virtualDisks.length || undefined },
    { id: 'partitions', label: $t('storage.tab_partitions'), icon: IconActivity, count: partitions.length },
    { id: 'removable', label: $t('storage.tab_removable'), icon: IconUsb, count: removable.length },
    { id: 'encryption', label: $t('encryption.tab'), icon: IconLock, count: encryptedVolumes.length },
    { id: 'cache', label: $t('cache.tab'), icon: IconBolt, count: cacheShares.length || undefined },
    { id: 'snapshots', label: $t('storage.tabs.snapshots'), icon: IconCamera, count: snapshotSchedules.length || undefined }
  ]}
  bind:activeTab
  onTabChange={(id) => { if (id === 'snapshots') loadSnapshotSchedules(); }}
  footerMode="status"
>
  {#snippet icon()}
    <IconServer stroke={1.8} />
  {/snippet}

  {#if loading}
    <div data-testid="storage-loading" class="flex flex-col gap-3" role="status">
      <span class="sr-only">{$t('storage.analyzing_devices')}</span>
      <div class="grid grid-cols-2 @min-[880px]:grid-cols-4 gap-3" aria-hidden="true">
        {#each Array(4) as _, i}
          <div class="flex flex-col gap-2 rounded-xl border border-border bg-card px-5 py-4 shadow-[var(--hv-shell-shadow-card)]">
            <span class="hv-skeleton-text block" style="width: {55 + ((i * 31) % 25)}%"></span>
            <span class="hv-skeleton-text block" style="width: {35 + ((i * 17) % 30)}%; height: 1.5em"></span>
          </div>
        {/each}
      </div>
      <div class="flex flex-col gap-3 rounded-xl border border-border bg-card px-5 py-4 shadow-[var(--hv-shell-shadow-card)]" aria-hidden="true">
        {#each Array(4) as _, i}
          <span class="hv-skeleton-text block" style="width: {45 + ((i * 31 + 17) % 36)}%"></span>
        {/each}
      </div>
    </div>
  {:else}
    <div data-testid="storage-stats" class="grid grid-cols-2 @min-[880px]:grid-cols-4 gap-3 mb-5">
      <StatCard data-testid="storage-stat-card" label={$t('storage.total_capacity')}>
        {#snippet value()}{capacityMain(totalCapacity)}<span class="text-sm text-[var(--hv-shell-text)]">{capacityUnit(totalCapacity)}</span>{/snippet}
      </StatCard>
      <StatCard data-testid="storage-stat-card" label={$t('storage.installed_disks')} value={`${totalDisks}`} />
      <StatCard data-testid="storage-stat-card" label={$t('storage.healthy_total')}>
        {#snippet value()}<span class={failedDisks > 0 ? 'text-[var(--hv-warning)]' : ''}>{healthyDisks}<span class="text-sm text-[var(--hv-shell-text)]">/{totalDisks}</span></span>{/snippet}
      </StatCard>
      <StatCard data-testid="storage-stat-card" label={$t('storage.mounted_partitions')} value={`${mountedPartitions}`} />
    </div>

    <div data-testid="storage-tab-panel" class="flex flex-col gap-4">
      {#if activeTab === 'disks'}
        {#if disks.length === 0}
          <EmptyState data-testid="storage-empty" title={$t('storage.no_disks_detected')} description={$t('storage.no_disks_hint')}>
            {#snippet iconSnippet()}<IconServer size={32} stroke={1.8} />{/snippet}
          </EmptyState>
        {:else}
          <div data-testid="storage-disk-grid" class="grid gap-4 [grid-template-columns:repeat(auto-fill,minmax(260px,1fr))]">
            {#each disks as disk}
              <button data-testid="storage-disk-card" class="flex flex-col gap-4 px-4 py-4 rounded-lg bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)] text-left cursor-pointer transition-colors hover:border-[var(--hv-shell-border-strong)]" onclick={() => openDiskDetail(disk)}>
                <div class="flex items-center gap-3">
                  <span class={cn('w-2.5 h-2.5 rounded-full flex-none', disk.smart_status === 'Healthy' ? 'bg-[var(--hv-success)]' : disk.smart_status === 'Failed' ? 'bg-[var(--hv-error)]' : 'bg-[var(--hv-warning)]')}></span>
                  <div class="flex-1 min-w-0">
                    <div class="text-sm font-bold text-[var(--hv-shell-text-strong)] font-[family-name:var(--hv-font-mono)]">{disk.device}</div>
                    <div class="text-xs text-[var(--hv-shell-text)] truncate">{disk.model || disk.serial || $t('health.unknown')}</div>
                  </div>
                  <span class="text-micro font-bold uppercase tracking-wide px-2 py-1 rounded-md bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-2)]">{connLabel(disk.connection_type)}</span>
                </div>
                <div class="flex flex-col gap-2">
                  <div class="flex items-center justify-between text-xs">
                    <span class="text-[var(--hv-shell-text-2)]">{$t('storage.capacity')}</span>
                    <span class="font-semibold text-[var(--hv-shell-text-strong)]">{capacityStr(disk.size)}</span>
                  </div>
                  {#if disk.temperature != null}
                    <div class="flex items-center justify-between text-xs">
                      <span class="text-[var(--hv-shell-text-2)]">{$t('storage.temperature')}</span>
                      <span class="inline-flex items-center gap-1 font-semibold text-[var(--hv-shell-text-strong)]"><IconThermometer size={12} />{disk.temperature.toFixed(0)}&deg;C</span>
                    </div>
                  {/if}
                  <div class="flex items-center justify-between text-xs">
                    <span class="text-[var(--hv-shell-text-2)]">{$t('storage.smart')}</span>
                    <span class={cn('font-semibold', disk.smart_status === 'Healthy' ? 'text-[var(--hv-success)]' : disk.smart_status === 'Failed' ? 'text-[var(--hv-error)]' : 'text-[var(--hv-warning)]')}>{disk.smart_status}</span>
                  </div>
                  <div class="flex items-center justify-between text-xs">
                    <span class="text-[var(--hv-shell-text-2)]">{$t('storage.partitions')}</span>
                    <span class="font-semibold text-[var(--hv-shell-text-strong)]">{disk.partition_count}</span>
                  </div>
                </div>
              </button>
            {/each}
          </div>
        {/if}

      {:else if activeTab === 'virtual'}
        {#if importablePools.length > 0}
          <div class="mb-4 px-5 py-4 rounded-2xl bg-[var(--hv-warning-bg)] border border-[var(--hv-warning)]" data-testid="storage-importable-pools">
            <div class="flex items-center gap-2 mb-2">
              <IconAlertTriangle size={18} />
              <h3 class="text-c-card-title font-bold text-[var(--hv-shell-text-strong)]">{$t('storage.import_pools_title')}</h3>
            </div>
            <p class="text-xs text-[var(--hv-shell-text-2)] mb-3">{$t('storage.import_pools_hint')}</p>
            <div class="flex flex-col gap-2">
              {#each importablePools as p (p.pool_type + ':' + p.name)}
                <div class="flex items-center gap-3 flex-wrap" data-testid="storage-importable-pool">
                  <span class="text-c-base font-bold font-[family-name:var(--hv-font-mono)] text-[var(--hv-shell-text-strong)]">{p.name}</span>
                  <span class="text-micro font-bold uppercase px-2 py-0.5 rounded-full bg-[var(--hv-shell-fill)] text-[var(--hv-shell-text-2)]">{vdKindLabel(p.pool_type)}</span>
                  <span class="text-xs text-[var(--hv-shell-text-2)]">{p.devices.length} {$t('storage.unit_disks')}{#if p.state && p.state !== 'unknown'} · {p.state}{/if}</span>
                  <span class="flex-1"></span>
                  {#if isAdmin}
                    <Button variant="primary" disabled={importBusy === p.name} onclick={() => openImportModal(p)}><IconPlus size={16} /> {$t('storage.import_pool')}</Button>
                  {/if}
                </div>
              {/each}
            </div>
          </div>
        {/if}

        {#if virtualDisks.length === 0}
          <EmptyState data-testid="storage-empty" title={$t('storage.no_virtual_disks')} description={$t('storage.no_virtual_disks_hint')}>
            {#snippet iconSnippet()}<IconStack size={32} stroke={1.8} />{/snippet}
          </EmptyState>
        {:else}
          <div class="flex flex-col gap-3">
            {#each virtualDisks as v (v.name)}
              <div data-testid="storage-vd-card" class="px-5 py-4 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)]">
                <div class="flex items-center gap-3 flex-wrap">
                  <span class={cn('grid place-items-center w-10 h-10 rounded-lg flex-none', vdHealthy(v) ? 'bg-[var(--hv-primary-light)] text-[var(--hv-primary)]' : 'bg-[var(--hv-warning-bg)] text-[var(--hv-warning)]')}><IconStack size={20} /></span>
                  <div class="flex-1 min-w-0">
                    <div class="flex items-center gap-2 flex-wrap">
                      <span class="text-c-card-title font-bold text-[var(--hv-shell-text-strong)] font-[family-name:var(--hv-font-mono)]">{v.name}</span>
                      <span class="text-micro font-bold uppercase px-2 py-0.5 rounded-full bg-[var(--hv-primary-light)] text-[var(--hv-primary)]">{vdKindLabel(v.pool_type)}</span>
                      {#if v.raid_profile}<span class="text-micro font-bold uppercase px-2 py-0.5 rounded-full bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-2)]">{v.raid_profile}</span>{/if}
                    </div>
                    <span class="text-xs text-[var(--hv-shell-text)]">
                      {v.device_count} {$t('storage.unit_disks')}
                      {#if v.mount_point} · {v.mount_point}{/if}
                    </span>
                  </div>
                  <span class={cn('inline-flex items-center gap-1.5 text-c-xs font-semibold', vdHealthy(v) ? 'text-[var(--hv-success)]' : 'text-[var(--hv-warning)]')}>
                    {#if vdHealthy(v)}<IconShieldCheckered size={13} />{:else}<IconAlertTriangle size={13} />{/if}
                    {v.health || $t('storage.unknown')}
                  </span>
                </div>
                <div class="mt-3">
                  <div class={usageTrack}>
                    <div class="h-full rounded-full" style="width: {v.size_bytes > 0 ? Math.min(100, Math.max(2, (v.used_bytes / v.size_bytes) * 100)) : 0}%; background: {vdHealthy(v) ? 'var(--hv-primary)' : 'var(--hv-warning)'};"></div>
                  </div>
                  <span class="mt-1.5 block text-xs text-[var(--hv-shell-text-2)]">{capacityMain(v.used_bytes)} {capacityUnit(v.used_bytes)} / {capacityMain(v.size_bytes)} {capacityUnit(v.size_bytes)}</span>
                </div>
                <div class="flex gap-2.5 mt-3">
                  {#if v.pool_type === 'Mdadm'}
                    <Button variant="ghost" disabled={virtualBusy === v.name} onclick={() => rebuildVirtualDisk(v)}><IconTool size={16} /> {$t('storage.vd_rebuild')}</Button>
                  {:else}
                    <Button variant="ghost" disabled={virtualBusy === v.name} onclick={() => scrubVirtualDisk(v)}><IconShieldCheckered size={16} /> {$t('storage.vd_scrub')}</Button>
                  {/if}
                  <Button variant="ghost" disabled={scrubStatusBusy === v.name} onclick={() => refreshScrubStatus(v)}><IconActivity size={16} /> {$t('storage.vd_scrub_status')}</Button>
                  <Button variant="danger" disabled={virtualBusy === v.name} onclick={() => destroyVirtualDisk(v)}><IconTrash size={16} /> {$t('storage.vd_destroy')}</Button>
                </div>
                {#if scrubStatus[v.name]}
                  <div class="mt-2 flex items-center gap-2 text-xs" data-testid="storage-vd-scrub-status">
                    {#if scrubStatus[v.name].in_progress}<IconActivity size={13} class="text-[var(--hv-primary)]" />{:else}<IconShieldCheckered size={13} class="text-[var(--hv-success)]" />{/if}
                    <span class="text-[var(--hv-shell-text-2)]">{scrubStatus[v.name].status}</span>
                  </div>
                {/if}
              </div>
            {/each}
          </div>
          <p class="text-xs text-center text-[var(--hv-shell-text)]">{$t('storage.vd_create_note')}</p>
        {/if}

      {:else if activeTab === 'partitions'}
        {#if partitions.length === 0}
          <EmptyState data-testid="storage-empty" title={$t('storage.no_partitions')} description={$t('storage.no_partitions_hint')}>
            {#snippet iconSnippet()}<IconActivity size={32} stroke={1.8} />{/snippet}
          </EmptyState>
        {:else}
          <div class="flex flex-col gap-2.5">
            {#each partitions as p}
              <div class="px-4 py-3 rounded-xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)]">
                <div class="flex items-center gap-2.5 flex-wrap">
                  <span class="text-sm font-bold text-[var(--hv-shell-text-strong)] font-[family-name:var(--hv-font-mono)]">{p.device}</span>
                  {#if p.filesystem}<span class="text-micro font-bold uppercase px-2 py-0.5 rounded-full bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-2)]">{p.filesystem}</span>{/if}
                  <span class={cn('text-xs', p.mount_point ? 'text-[var(--hv-shell-text-2)]' : 'text-[var(--hv-shell-text-3)]')}>{p.mount_point || $t('storage.not_mounted')}</span>
                </div>
                <div class="mt-2.5">
                  <div class={usageTrack}>
                    <div class="h-full rounded-full" style="width: {Math.max(2, p.percent)}%; background: {usageColor(p.percent)};"></div>
                  </div>
                  <span class="mt-1.5 block text-xs text-[var(--hv-shell-text-2)]"><strong class="text-[var(--hv-shell-text-strong)]">{p.percent}%</strong> · {formatBytes(p.available)} {$t('common.free')} / {formatBytes(p.size)}</span>
                </div>
              </div>
            {/each}
          </div>
        {/if}

      {:else if activeTab === 'removable'}
        {#if removable.length === 0}
          <EmptyState data-testid="storage-empty" title={$t('storage.no_removable')} description={$t('storage.no_removable_hint')}>
            {#snippet iconSnippet()}<IconUsb size={32} stroke={1.8} />{/snippet}
          </EmptyState>
        {:else}
          <div class="grid gap-3 [grid-template-columns:repeat(auto-fill,minmax(220px,1fr))]">
            {#each removable as r}
              <div data-testid="storage-rem-card" class="px-4 py-4 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)]">
                <div class="flex items-center gap-2.5">
                  <span class={cn('w-2.5 h-2.5 rounded-full flex-none', r.mounted ? 'bg-[var(--hv-success)]' : 'bg-[var(--hv-shell-text-3)]')}></span>
                  <span class="flex-1 text-sm font-bold text-[var(--hv-shell-text-strong)] font-[family-name:var(--hv-font-mono)]">{r.device}</span>
                  <span class={cn('text-micro font-bold px-2 py-0.5 rounded-full', r.mounted ? 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]' : 'bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-2)]')}>{r.mounted ? $t('storage.mounted') : $t('storage.not_mounted')}</span>
                </div>
                <div class="mt-2 text-xs text-[var(--hv-shell-text)] truncate">{r.model || r.name || $t('health.unknown')}</div>
                <div class="mt-1 flex gap-2 text-xs text-[var(--hv-shell-text-2)]">
                  <span>{capacityStr(r.size)}</span>
                  {#if r.filesystem}<span>{r.filesystem}</span>{/if}
                </div>
                {#if r.mount_point}<div class="mt-2 text-xs text-[var(--hv-shell-text-2)] font-[family-name:var(--hv-font-mono)]">{r.mount_point}</div>{/if}
              </div>
            {/each}
          </div>
        {/if}

      {:else if activeTab === 'encryption'}
        <div class="flex items-center gap-3 justify-between">
          <p class="m-0 text-xs text-[var(--hv-shell-text)]">{$t('encryption.desc')}</p>
          <Button variant="primary" onclick={openEncryptModal} disabled={encryptionBusy} data-testid="storage-btn-primary">
            <IconLock size={16} /> {$t('encryption.encrypt_device')}
          </Button>
        </div>
        {#if encryptedVolumes.length === 0}
          <EmptyState data-testid="storage-empty" title={$t('encryption.no_volumes')} description={$t('encryption.no_volumes_hint')}>
            {#snippet iconSnippet()}<IconShieldCheckered size={32} stroke={1.8} />{/snippet}
          </EmptyState>
        {:else}
          <div class="grid gap-3 [grid-template-columns:repeat(auto-fill,minmax(220px,1fr))]">
            {#each encryptedVolumes as v (v.id)}
              <div data-testid="storage-enc-card" class="px-4 py-4 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)]">
                <div class="flex items-center gap-2.5">
                  <span class={cn('w-2.5 h-2.5 rounded-full flex-none', v.unlocked ? 'bg-[var(--hv-success)]' : 'bg-[var(--hv-shell-text-3)]')}></span>
                  <span class="flex-1 text-sm font-bold text-[var(--hv-shell-text-strong)]">{v.label}</span>
                  <span class={cn('text-micro font-bold px-2 py-0.5 rounded-full', v.unlocked ? 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]' : 'bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-2)]')}>{v.unlocked ? $t('encryption.unlocked') : $t('encryption.locked')}</span>
                </div>
                <div class="mt-2 text-xs text-[var(--hv-shell-text)]">{v.device_path} &middot; {v.filesystem}</div>
                <div class="mt-1 text-xs text-[var(--hv-shell-text-2)]">{v.mount_point}</div>
                {#if v.unlocked}
                  <div class="flex gap-2.5 mt-3">
                    <Button variant="ghost" onclick={() => lockVolume(v)} disabled={encryptionBusy}><IconLock size={14} /> {$t('encryption.lock')}</Button>
                    <Button variant="danger" onclick={() => deleteVolume(v)} disabled={encryptionBusy}>{$t('common.delete')}</Button>
                  </div>
                {:else}
                  <div class="flex gap-2 mt-3">
                    <input type="password" class={cn(inpSel, 'flex-1')} placeholder={$t('encryption.passphrase')} bind:value={unlockPassphrase[v.id]} />
                    <Button variant="primary" onclick={() => unlockVolume(v)} disabled={encryptionBusy || !unlockPassphrase[v.id]}><IconLockOpen size={14} /> {$t('encryption.unlock')}</Button>
                  </div>
                {/if}
              </div>
            {/each}
          </div>
        {/if}

      {:else if activeTab === 'cache'}
        <div data-testid="storage-cache" class="flex flex-col gap-4 max-w-[720px]">
          {#if cacheError}
            <Alert variant="error">{cacheError}</Alert>
          {/if}

          <div data-testid="storage-cache-card" class="px-5 py-4 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)]">
            <div class="flex items-center gap-2 mb-3"><IconBolt size={18} class="text-[var(--hv-primary)]" /><h3 class="m-0 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">{$t('cache.config_title')}</h3></div>
            <div class="flex flex-col gap-3">
              <label class="flex flex-col gap-1.5 text-c-label text-[var(--hv-shell-text-2)]">
                <span>{$t('cache.cache_pool_label')}</span>
                <select class={inpSel} bind:value={cacheSettings.cache_pool}>
                  <option value={null}>{$t('cache.cache_pool_none')}</option>
                  {#each cachePools as p}
                    <option value={p.name}>{p.name}{p.mount_point ? ` (${p.mount_point})` : ''}</option>
                  {/each}
                </select>
              </label>
              <Toggle bind:checked={cacheSettings.mover_enabled} label={$t('cache.mover_enabled')} />
              <div class="grid grid-cols-2 gap-3">
                <label class="flex flex-col gap-1.5 text-c-label text-[var(--hv-shell-text-2)]"><span>{$t('cache.mover_schedule_label')}</span><input class={inpSel} type="text" bind:value={cacheSettings.mover_cron} placeholder="0 3 * * *" /></label>
                <label class="flex flex-col gap-1.5 text-c-label text-[var(--hv-shell-text-2)]"><span>{$t('cache.min_free_label')}</span><input class={inpSel} type="number" min="1" max="95" bind:value={cacheSettings.min_free_percent} /></label>
                <label class="flex flex-col gap-1.5 text-c-label text-[var(--hv-shell-text-2)]"><span>{$t('cache.mover_age_label')}</span><input class={inpSel} type="number" min="0" max="3650" bind:value={cacheSettings.mover_age_days} /></label>
                <label class="flex flex-col gap-1.5 text-c-label text-[var(--hv-shell-text-2)]"><span>{$t('cache.mover_bwlimit_label')}</span><input class={inpSel} type="number" min="0" max="10000000" bind:value={cacheSettings.mover_bwlimit} /></label>
              </div>
              <label class="flex flex-col gap-1.5 text-c-label text-[var(--hv-shell-text-2)]"><span>{$t('cache.mover_excludes_label')}</span><input class={inpSel} type="text" bind:value={cacheSettings.mover_excludes} placeholder="*.partial, .fuse_hidden*" /></label>
            </div>
            <div class="mt-3">
              <Button variant="primary" onclick={saveCacheSettings} disabled={cacheBusy}>{$t('cache.save_config')}</Button>
            </div>
          </div>

          <div data-testid="storage-cache-card" class="px-5 py-4 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)]">
            <div class="flex items-center gap-2 mb-2"><IconPlayerPlay size={18} class="text-[var(--hv-primary)]" /><h3 class="m-0 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">{$t('cache.mover_title')}</h3></div>
            <p class="m-0 text-xs text-[var(--hv-shell-text)]">{$t('cache.mover_hint')}</p>
            <div class="mt-3">
              <Button variant="primary" onclick={runMover} disabled={cacheBusy || moverRunning}><IconPlayerPlay size={14} /> {$t('cache.run_mover')}</Button>
            </div>
            {#if moverRunning || moverProgress >= 100}
              <div class="mt-3">
                <div class={usageTrack}><div class="h-full rounded-full bg-[var(--hv-primary)]" style="width: {moverProgress}%;"></div></div>
                <span class="mt-1.5 block text-xs text-[var(--hv-shell-text-2)]"><strong class="text-[var(--hv-shell-text-strong)]">{moverProgress}%</strong> · {moverProgressMsg}</span>
              </div>
            {/if}
            {#if cacheRuns.length > 0}
              <div class="flex flex-col gap-2 mt-3">
                {#each cacheRuns.slice(0, 10) as run}
                  <div class="flex items-center gap-2.5 px-3 py-2 rounded-lg bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] text-xs">
                    <span class={cn('px-2 py-0.5 rounded-full font-bold uppercase', run.status === 'success' ? 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]' : run.status === 'failed' ? 'bg-[var(--hv-error-bg)] text-[var(--hv-error)]' : 'bg-[var(--hv-primary-light)] text-[var(--hv-primary)]')}>{run.status}</span>
                    <span class="text-[var(--hv-shell-text-2)]">{new Date(run.started_at).toLocaleString()}</span>
                    <span class="ml-auto text-[var(--hv-shell-text-2)]">{run.files_moved} {$t('cache.files')} · {capacityStr(run.bytes_moved)}</span>
                  </div>
                {/each}
              </div>
            {/if}
          </div>

          <div data-testid="storage-cache-card" class="px-5 py-4 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)]">
            <div class="flex items-center gap-2 mb-3">
              <IconDatabase size={18} class="text-[var(--hv-primary)]" /><h3 class="m-0 flex-1 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">{$t('cache.shares_title')}</h3>
              <Button variant="primary" onclick={openCacheShareModal}><IconPlus size={14} /> {$t('cache.add_share')}</Button>
            </div>
            {#if cacheShares.length === 0}
              <EmptyState data-testid="storage-empty" title={$t('cache.no_shares')} description={$t('cache.no_shares_hint')}>
                {#snippet iconSnippet()}<IconBolt size={32} stroke={1.8} />{/snippet}
              </EmptyState>
            {:else}
              <div class="flex flex-col gap-2">
                {#each cacheShares as share (share.id)}
                  <div class="flex items-center gap-3 px-3 py-2.5 rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]">
                    <div class="flex-1 min-w-0">
                      <div class="text-sm font-semibold text-[var(--hv-shell-text-strong)]">{share.name}</div>
                      <div class="text-xs text-[var(--hv-shell-text-2)]">→ {share.target_pool}</div>
                    </div>
                    <select
                      class={cn(inpSel, 'w-auto flex-none')}
                      value={share.cache_mode}
                      onchange={(e) => updateShareMode(share, e.currentTarget.value)}
                      disabled={cacheBusy}
                      aria-label={$t('cache.mode')}
                    >
                      {#each CACHE_MODES as m}
                        <option value={m}>{$t(`cache.mode_${m}`)}</option>
                      {/each}
                    </select>
                    <button class="grid place-items-center w-11 h-11 flex-none rounded-lg border border-[var(--hv-shell-border-strong)] bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-strong)] cursor-pointer hover:bg-[var(--hv-error-bg)] hover:text-[var(--hv-error)]" onclick={() => deleteCacheShare(share)} disabled={cacheBusy} aria-label={$t('common.delete')}>
                      <IconTrash size={16} />
                    </button>
                  </div>
                {/each}
              </div>
            {/if}
          </div>
        </div>
      {:else if activeTab === 'snapshots'}
        <div data-testid="storage-snapshots" class="flex flex-col gap-4 max-w-[900px]">
          <div class="flex items-center justify-between gap-3">
            <p class="m-0 text-c-label text-[var(--hv-shell-text)]">{$t('storage.snapshots.heading')}</p>
            <Button variant="primary" data-testid="snapshots-new-btn" onclick={openSnapCreate}>
              <IconPlus size={16} stroke={2} /> {$t('storage.snapshots.new')}
            </Button>
          </div>

          {#if !snapSchedulesLoaded}
            <EmptyState title={$t('storage.analyzing_devices')} />
          {:else if snapshotSchedules.length === 0}
            <EmptyState data-testid="snapshots-empty" title={$t('storage.snapshots.empty_title')} description={$t('storage.snapshots.empty_desc')} />
          {:else}
            <div class="flex flex-col gap-3">
              {#each snapshotSchedules as row (row.id)}
                <div data-testid="snapshots-schedule" class="px-5 py-4 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)]">
                  <div class="flex items-start justify-between gap-3">
                    <div class="flex flex-col gap-1">
                      <span class="text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">{row.pool}</span>
                      <span class="text-xs text-[var(--hv-shell-text-2)]"><code>{row.cron}</code> · {$t('storage.snapshots.prefix')}: {row.prefix}{row.recursive ? ' · ' + $t('storage.snapshots.recursive') : ''}</span>
                      <span class="text-xs text-[var(--hv-shell-text-2)]">
                        {$t('storage.snapshots.gfs_help', { h: String(row.gfs_hourly), d: String(row.gfs_daily), w: String(row.gfs_weekly), m: String(row.gfs_monthly), y: String(row.gfs_yearly) })}
                      </span>
                      {#if row.last_status === 'failed'}
                        <span class="text-xs text-[var(--hv-danger)]" data-testid="snapshots-schedule-error">{row.last_error}</span>
                      {/if}
                    </div>
                    <div class="flex items-center gap-1">
                      <button type="button" class="grid place-items-center w-9 h-9 flex-none rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:bg-[var(--hv-subtle-hover)] hover:text-[var(--hv-shell-text-strong)] hv-ic" data-testid="snapshots-run-{row.id}" aria-label={$t('storage.snapshots.run_now')} disabled={snapBusyId === row.id} onclick={() => runSnapNow(row)}>
                        <IconPlayerPlay size={18} stroke={1.8} />
                      </button>
                      <button type="button" class="grid place-items-center w-9 h-9 flex-none rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:bg-[var(--hv-subtle-hover)] hover:text-[var(--hv-shell-text-strong)] hv-ic" data-testid="snapshots-view-{row.id}" aria-label={$t('storage.snapshots.snapshots_title')} onclick={() => viewPoolSnapshots(row.pool)}>
                        <IconCamera size={18} stroke={1.8} />
                      </button>
                      <button type="button" class="grid place-items-center w-9 h-9 flex-none rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:bg-[var(--hv-subtle-hover)] hover:text-[var(--hv-shell-text-strong)] hv-ic" data-testid="snapshots-edit-{row.id}" aria-label={$t('tasks.edit', { name: row.pool })} onclick={() => openSnapEdit(row)}>
                        <IconTool size={18} stroke={1.8} />
                      </button>
                      <button type="button" class="grid place-items-center w-9 h-9 flex-none rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:bg-[var(--hv-subtle-hover)] hover:text-[var(--hv-shell-text-strong)] hv-ic" data-testid="snapshots-delete-{row.id}" aria-label={$t('tasks.delete', { name: row.pool })} onclick={() => askSnapDelete(row)}>
                        <IconTrash size={18} stroke={1.8} />
                      </button>
                    </div>
                  </div>
                  <div class="flex items-center gap-3 mt-2 text-xs text-[var(--hv-shell-text-2)]">
                    <span class="inline-flex items-center gap-1"><IconClock size={14} /> {row.last_status ?? '—'} {row.last_run_at ? new Date(row.last_run_at).toLocaleString() : ''}</span>
                    {#if row.next_run_at}<span>→ {new Date(row.next_run_at).toLocaleString()}</span>{/if}
                    <span class={row.enabled ? 'text-[var(--hv-success)]' : 'text-[var(--hv-shell-text-2)]'}>{row.enabled ? $t('storage.snapshots.enabled') : $t('tasks.enabled_off')}</span>
                  </div>
                </div>
              {/each}
            </div>
          {/if}

          {#if snapViewPool}
            <div data-testid="snapshots-pool-list" class="px-5 py-4 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)]">
              <div class="flex items-center justify-between gap-2 mb-2">
                <h3 class="m-0 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">{$t('storage.snapshots.snapshots_title')} — {snapViewPool}</h3>
                <button type="button" class="grid place-items-center w-9 h-9 flex-none rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:bg-[var(--hv-subtle-hover)] hover:text-[var(--hv-shell-text-strong)] hv-ic" aria-label={$t('common.cancel')} onclick={() => (snapViewPool = '')}>✕</button>
              </div>
              {#if snapViewLoading}
                <p class="m-0 text-xs text-[var(--hv-shell-text-2)]">{$t('storage.analyzing_devices')}</p>
              {:else if snapViewRows.length === 0}
                <p class="m-0 text-xs text-[var(--hv-shell-text-2)]">{$t('storage.snapshots.snapshots_empty')}</p>
              {:else}
                <ul class="m-0 p-0 list-none flex flex-col gap-1 max-h-64 overflow-auto">
                  {#each snapViewRows as snap (snap.name)}
                    <li class="flex items-center justify-between gap-2 text-xs">
                      <span class="font-mono truncate">{snap.name}</span>
                      {#if snap.auto}<span class="text-[var(--hv-primary)] text-t-caption uppercase">{$t('storage.snapshots.snapshots_auto')}</span>{/if}
                    </li>
                  {/each}
                </ul>
              {/if}
            </div>
          {/if}
        </div>
      {/if}
    </div>
  {/if}

  {#snippet footer()}
    {#if !loading}
      <span data-testid="storage-footer" class="flex items-center gap-2 min-w-0 flex-wrap">
        <span>{totalDisks} {totalDisks > 1 ? $t('storage.unit_disks') : $t('storage.unit_disk')}</span>
        <span class="text-[var(--hv-shell-text-3)]">&middot;</span>
        <span>{virtualDisks.length} {$t('storage.tab_virtual_disks')}</span>
        <span class="text-[var(--hv-shell-text-3)]">&middot;</span>
        <span>{partitions.length} {$t('storage.tab_partitions')}</span>
        <span class="text-[var(--hv-shell-text-3)]">&middot;</span>
        <span>{removable.length} {$t('storage.tab_removable')}</span>
      </span>
    {/if}
  {/snippet}
  <ConfirmDialog
    {dlg}
    onconfirm={() => dlg.settle(true)}
    oncancel={() => dlg.settle(false)}
  />
</PageLayout>

<!-- SNAPSHOT SCHEDULE MODAL -->
<Modal open={snapScheduleModal} title={snapForm.id ? $t('storage.snapshots.edit_title') : $t('storage.snapshots.new_title')} size="md" onclose={() => (snapScheduleModal = false)}>
  <div class="flex flex-col gap-4 p-1">
    {#if snapFormError}
      <p data-testid="snapshots-form-error" role="alert" class="m-0 text-c-label text-[var(--hv-danger)]">{snapFormError}</p>
    {/if}
    <label class="flex flex-col gap-1.5 text-c-label text-[var(--hv-shell-text-2)]">
      <span>{$t('storage.snapshots.col_pool')}</span>
      <input class={inpSel} type="text" bind:value={snapForm.pool} placeholder="tank/data" />
    </label>
    <label class="flex flex-col gap-1.5 text-c-label text-[var(--hv-shell-text-2)]">
      <span>{$t('storage.snapshots.col_cron')}</span>
      <input class={inpSel} type="text" bind:value={snapForm.cron} placeholder="0 3 * * *" />
    </label>
    <label class="flex flex-col gap-1.5 text-c-label text-[var(--hv-shell-text-2)]">
      <span>{$t('storage.snapshots.prefix')}</span>
      <input class={inpSel} type="text" bind:value={snapForm.prefix} placeholder="hv-auto" />
    </label>
    <Toggle bind:checked={snapForm.recursive} label={$t('storage.snapshots.recursive')} />
    <div class="grid grid-cols-2 gap-3" data-testid="snapshots-gfs-fields">
      <label class="flex flex-col gap-1.5 text-c-label text-[var(--hv-shell-text-2)]"><span>H</span><input class={inpSel} type="number" min="0" max="1024" bind:value={snapForm.gfs_hourly} /></label>
      <label class="flex flex-col gap-1.5 text-c-label text-[var(--hv-shell-text-2)]"><span>D</span><input class={inpSel} type="number" min="0" max="1024" bind:value={snapForm.gfs_daily} /></label>
      <label class="flex flex-col gap-1.5 text-c-label text-[var(--hv-shell-text-2)]"><span>W</span><input class={inpSel} type="number" min="0" max="1024" bind:value={snapForm.gfs_weekly} /></label>
      <label class="flex flex-col gap-1.5 text-c-label text-[var(--hv-shell-text-2)]"><span>M</span><input class={inpSel} type="number" min="0" max="1024" bind:value={snapForm.gfs_monthly} /></label>
      <label class="flex flex-col gap-1.5 text-c-label text-[var(--hv-shell-text-2)]"><span>Y</span><input class={inpSel} type="number" min="0" max="1024" bind:value={snapForm.gfs_yearly} /></label>
    </div>
    <p class="m-0 text-xs text-[var(--hv-shell-text-2)]">{$t('storage.snapshots.gfs_help', { h: String(snapForm.gfs_hourly), d: String(snapForm.gfs_daily), w: String(snapForm.gfs_weekly), m: String(snapForm.gfs_monthly), y: String(snapForm.gfs_yearly) })}</p>
    <Toggle bind:checked={snapForm.enabled} label={$t('storage.snapshots.enabled')} />
  </div>
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={() => (snapScheduleModal = false)}>{$t('common.cancel')}</Button>
      <Button variant="primary" data-testid="snapshots-save-btn" onclick={saveSnapSchedule} disabled={snapBusyId === 'form'}>{$t('common.save')}</Button>
    </div>
  {/snippet}
</Modal>

<!-- CACHE SHARE MODAL -->
<Modal open={showCacheShareModal} title={$t('cache.add_share')} size="md" onclose={() => showCacheShareModal = false}>
  <div class="flex flex-col gap-3">
    <label class="flex flex-col gap-1.5 text-c-label text-[var(--hv-shell-text-2)]"><span>{$t('cache.share_name')}</span><input class={inpSel} type="text" bind:value={newShareName} placeholder="photos" /></label>
    <label class="flex flex-col gap-1.5 text-c-label text-[var(--hv-shell-text-2)]"><span>{$t('cache.mode')}</span><select class={inpSel} bind:value={newShareMode}>{#each CACHE_MODES as m}<option value={m}>{$t(`cache.mode_${m}`)}</option>{/each}</select></label>
    <label class="flex flex-col gap-1.5 text-c-label text-[var(--hv-shell-text-2)]"><span>{$t('cache.target_pool')}</span><select class={inpSel} bind:value={newShareTarget}>{#each cachePools as p}{#if p.name !== cacheSettings.cache_pool}<option value={p.name}>{p.name}</option>{/if}{/each}</select></label>
    {#if !cacheSettings.cache_pool}
      <Alert variant="error">{$t('cache.no_cache_pool')}</Alert>
    {/if}
  </div>
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={() => showCacheShareModal = false}>{$t('common.cancel')}</Button>
      <Button variant="primary" onclick={createCacheShare} disabled={cacheBusy || !newShareName.trim() || !newShareTarget || !cacheSettings.cache_pool}>{$t('common.create')}</Button>
    </div>
  {/snippet}
</Modal>

<!-- DISK DETAIL MODAL -->
<Modal open={showDiskModal && !!selectedDisk} title={selectedDisk?.device || $t('storage.disk_details')} size="md" onclose={() => showDiskModal = false}>
  {#if selectedDisk}
    <div>
      <div class="flex items-center gap-3 mb-4">
        <span class="grid place-items-center w-11 h-11 rounded-xl bg-[var(--hv-primary-light)] text-[var(--hv-primary)] flex-none"><IconServer size={20} /></span>
        <div class="min-w-0">
          <div class="text-sm font-semibold text-[var(--hv-shell-text-strong)] truncate">{selectedDisk.model || $t('health.unknown')}</div>
          <div class="text-xs text-[var(--hv-shell-text-2)] font-[family-name:var(--hv-font-mono)]">{selectedDisk.serial}</div>
        </div>
      </div>

      <div class="grid grid-cols-2 gap-3">
        <div class="flex flex-col gap-0.5"><span class="text-micro font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('storage.device')}</span><span class="text-sm font-medium text-[var(--hv-shell-text-strong)] font-[family-name:var(--hv-font-mono)]">{selectedDisk.device}</span></div>
        <div class="flex flex-col gap-0.5"><span class="text-micro font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('common.size')}</span><span class="text-sm font-medium text-[var(--hv-shell-text-strong)]">{formatBytes(selectedDisk.size)}</span></div>
        <div class="flex flex-col gap-0.5"><span class="text-micro font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('storage.connection')}</span><span class="text-sm font-medium text-[var(--hv-shell-text-strong)]">{connLabel(selectedDisk.connection_type)}</span></div>
        <div class="flex flex-col gap-0.5"><span class="text-micro font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('storage.role')}</span><span class="text-sm font-medium text-[var(--hv-shell-text-strong)]">{selectedDisk.role || $t('storage.not_assigned')}</span></div>
        <div class="flex flex-col gap-0.5"><span class="text-micro font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('storage.smart')}</span><span class={cn('text-sm font-semibold', selectedDisk.smart_status === 'Healthy' ? 'text-[var(--hv-success)]' : 'text-[var(--hv-error)]')}>{selectedDisk.smart_status}</span></div>
        <div class="flex flex-col gap-0.5"><span class="text-micro font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('storage.temperature')}</span><span class="text-sm font-medium text-[var(--hv-shell-text-strong)]">{#if selectedDisk.temperature != null}{selectedDisk.temperature.toFixed(0)}&deg;C{:else}&mdash;{/if}</span></div>
        <div class="flex flex-col gap-0.5"><span class="text-micro font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('storage.partitions')}</span><span class="text-sm font-medium text-[var(--hv-shell-text-strong)]">{selectedDisk.partition_count}</span></div>
        <div class="flex flex-col gap-0.5"><span class="text-micro font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('storage.errors')}</span><span class={cn('text-sm font-medium', selectedDisk.errors > 0 ? 'text-[var(--hv-error)]' : 'text-[var(--hv-shell-text-strong)]')}>{selectedDisk.errors}</span></div>
      </div>

      <div class="mt-5 pt-4 border-t border-[var(--hv-shell-border-soft)]">
        <span class="text-micro font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('storage.smart')} Test</span>
        <div class="mt-2">
          {#if smartCapabilities === null}
            <div class="py-3"><Spinner size="sm" /></div>
          {:else if !smartCapabilities.available}
            <div class="flex items-center gap-1.5 text-xs text-[var(--hv-shell-text)]"><IconAlertTriangle size={14} /> <span>{$t('storage.smart_test_not_available')}</span></div>
          {:else if !$isAdmin}
            <div class="flex items-center gap-1.5 text-xs text-[var(--hv-shell-text-2)]"><IconShieldCheckered size={14} /> <span>{$t('storage.smart_test_admin_only')}</span></div>
          {:else if smartTestStatus?.in_progress}
            <div>
              <div class={usageTrack}><div class="h-full rounded-full bg-[var(--hv-primary)]" style="width:{100 - (smartTestStatus?.progress_pct ?? 0)}%"></div></div>
              <span class="mt-1.5 block text-xs text-[var(--hv-shell-text-2)]">{$t('storage.smart_test_running', { pct: String(100 - (smartTestStatus?.progress_pct ?? 0)), min: String(smartTestStatus?.remaining_minutes || '?') })}</span>
            </div>
          {:else}
            <div class="flex gap-2.5">
              <select class={cn(inpSel, 'w-auto flex-none')} bind:value={smartTestType}>
                <option value="short">{$t('storage.smart_test_short')}</option>
                <option value="long">{$t('storage.smart_test_long')}</option>
                {#if smartCapabilities.supports_conveyance_test}
                  <option value="conveyance">{$t('storage.smart_test_conveyance')}</option>
                {/if}
              </select>
              <Button variant="secondary" size="sm" onclick={handleSmartTestStart} disabled={smartBusy}>{smartBusy ? $t('storage.smart_test_starting') : $t('storage.smart_test_start')}</Button>
            </div>
          {/if}
          {#if smartTestStatus?.last_result && !smartTestStatus?.in_progress}
            <div class="flex items-center gap-1.5 mt-2 text-xs text-[var(--hv-shell-text-2)]"><IconClock size={12} /> <span>{$t('storage.smart_test_result', { result: smartTestStatus.last_result })}</span></div>
          {/if}
        </div>
      </div>

      {#if showDiskUsage}
        <div class="mt-5 pt-4 border-t border-[var(--hv-shell-border-soft)]">
          <span class="text-micro font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('storage.disk_usage')}</span>
          <div class="flex gap-4 items-center mt-2">
            <svg viewBox="0 0 100 100" class="w-[140px] h-[140px]">
              {#each diskUsageSlices as slice, i}
                <circle cx="50" cy="50" r="42" fill="none" stroke-width="12" stroke-dasharray="{slice.percent * 2.639} {100 * 2.639 - slice.percent * 2.639}" transform="rotate(-90, 50, 50)" style="stroke: {slice.color}; transition: stroke-dasharray 0.3s" />
              {/each}
              <text x="50" y="46" text-anchor="middle" font-size="12" font-weight="bold" fill="var(--hv-text-primary)">{formatBytes(selectedDisk.size)}</text>
              <text x="50" y="60" text-anchor="middle" font-size="7" fill="var(--hv-text-tertiary)">{$t('common.size')}</text>
            </svg>
            <div class="flex flex-col gap-1.5 flex-1">
              {#each diskUsageSlices as slice}
                <div class="flex items-center gap-2 text-xs">
                  <span class="w-2.5 h-2.5 rounded-[2px] flex-none" style="background:{slice.color}"></span>
                  <span class="text-[var(--hv-shell-text-2)]">{slice.label}</span>
                  <span class="ml-auto text-[var(--hv-shell-text-2)]">{formatBytes(slice.value)}</span>
                  <span class="w-10 text-right font-semibold text-[var(--hv-shell-text-strong)]">{slice.percent.toFixed(1)}%</span>
                </div>
              {/each}
            </div>
          </div>
        </div>
      {/if}
    </div>
  {/if}
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={() => showDiskUsage = !showDiskUsage}>{$t('storage.disk_usage')}</Button>
      <Button variant="ghost" onclick={() => showDiskModal = false}>{$t('common.close')}</Button>
    </div>
  {/snippet}
</Modal>

<!-- ENCRYPT DEVICE MODAL -->
<Modal open={showEncryptModal} title={$t('encryption.encrypt_device')} size="md" onclose={() => showEncryptModal = false}>
  <div>
    <p class="m-0 mb-3 px-3 py-2.5 rounded-lg bg-[var(--hv-warning-light)] border border-[var(--hv-warning)]/40 text-xs text-[var(--hv-warning-text)]">{$t('encryption.warning_unrecoverable')}</p>
    <label class="block text-c-label font-semibold text-[var(--hv-shell-text-strong)] mb-1" for="encDevice">{$t('encryption.device')}</label>
    <select id="encDevice" class={encInput} bind:value={encryptDevice}><option value="">{$t('encryption.select_device')}</option>{#each encryptionAvailableDevices as d}<option value={d.name}>{d.name} &mdash; {d.model} ({formatBytes(d.size)})</option>{/each}</select>
    <label class="block text-c-label font-semibold text-[var(--hv-shell-text-strong)] mb-1" for="encFs">{$t('encryption.filesystem')}</label>
    <select id="encFs" class={encInput} bind:value={encryptFilesystem}><option value="ext4">ext4</option><option value="xfs">xfs</option><option value="btrfs">btrfs</option></select>
    <label class="block text-c-label font-semibold text-[var(--hv-shell-text-strong)] mb-1" for="encLabel">{$t('encryption.label')}</label>
    <input id="encLabel" class={encInput} type="text" bind:value={encryptLabel} placeholder={$t('encryption.label_placeholder')} />
    <label class="block text-c-label font-semibold text-[var(--hv-shell-text-strong)] mb-1" for="encPass">{$t('encryption.passphrase')}</label>
    <input id="encPass" class={encInput} type="password" bind:value={encryptPassphrase} />
    <label class="block text-c-label font-semibold text-[var(--hv-shell-text-strong)] mb-1" for="encPassConfirm">{$t('encryption.passphrase_confirm')}</label>
    <input id="encPassConfirm" class={encInput} type="password" bind:value={encryptPassphraseConfirm} />
  </div>
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={() => showEncryptModal = false}>{$t('common.cancel')}</Button>
      <Button variant="primary" onclick={confirmEncryptVolume} disabled={encryptionBusy}>{$t('encryption.encrypt_device')}</Button>
    </div>
  {/snippet}
</Modal>

<Modal open={showImportModal && !!importTarget} title={$t('storage.import_pool')} size="md" onclose={() => showImportModal = false}>
  {#if importTarget}
    <div data-testid="storage-import-modal">
      <p class="m-0 mb-3 text-sm text-[var(--hv-shell-text-2)]">{$t('storage.import_pool_hint', { name: importTarget.name })}</p>
      <p class="m-0 mb-3 flex items-center gap-2 text-c-label font-semibold text-[var(--hv-shell-text-strong)]">
        <span>{importTarget.name}</span>
        <span class="text-micro font-bold uppercase px-2 py-0.5 rounded-full bg-[var(--hv-shell-fill)] text-[var(--hv-shell-text-2)]">{vdKindLabel(importTarget.pool_type)}</span>
      </p>
      {#if importTarget.pool_type === 'Zfs'}
        <label class="block text-c-label font-semibold text-[var(--hv-shell-text-strong)] mb-1" for="importNewName">{$t('storage.import_pool_new_name')}</label>
        <input id="importNewName" class={encInput} type="text" bind:value={importNewName} placeholder={importTarget.name} />
      {/if}
    </div>
  {/if}
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={() => showImportModal = false}>{$t('common.cancel')}</Button>
      <Button variant="primary" onclick={confirmImport} disabled={!!importBusy}>{$t('storage.import_pool')}</Button>
    </div>
  {/snippet}
</Modal>