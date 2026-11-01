<!-- ========================================================================
     File          : Shares.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
     The Shares app: share list across SMB/NFS/FTP/SFTP/WebDAV, create/edit
     forms, per-protocol service configuration and control, per-share
     permissions, and the create-share form pre-fill coming from the
     Explorer's share intent.

     Dependencies:
       - svelte (>= 5), theme + app/ composites, stores/explorerShareIntent,
         $lib/services/api

     Usage:
     Opened as a desktop window app.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { api } from '$lib/services/api';
  import { t, translate } from '$lib/stores/i18n';
  import { explorerShareIntent } from '$lib/stores/explorerShareIntent';
  import { onAppVisible } from '$lib/utils/autoRevalidate';
  import { Spinner, Button, Toggle, Modal, Alert } from '$lib/components/theme';
  import PathField from '$lib/components/theme/PathField.svelte';
  import { PageLayout, ConfirmDialog, tileBtn, dangerHover, StatCard, EmptyState } from '$lib/components/app';
  import { createConfirmState } from '$lib/utils/confirmDialog.svelte';
  import { notifyError, notifySuccess } from '$lib/utils/notify';
  import { cn } from '$lib/utils.js';
  import { IconShare, IconPlus, IconSettings, IconPlayerPlay, IconSquare, IconRefresh, IconShield, IconTrash, IconUser, IconUsers, IconGauge } from '@tabler/icons-svelte';

  interface Share {
    name: string;
    path: string;
    share_type: string;
    enabled: boolean;
    comment: string;
  }

  /**
   * The per-share quota, as the API answers it. `supported: false` means the
   * filesystem holding the share cannot cap one folder — the card then shows
   * the reason instead of a field that would not hold.
   */
  interface ShareQuotaState {
    share_name: string;
    share_type: string;
    backend: string;
    supported: boolean;
    filesystem: string;
    reason: string | null;
    quota_bytes: number;
    used_bytes: number;
    free_bytes: number;
    percent: number;
    quota_human: string;
    used_human: string;
    min_bytes: number;
    max_bytes: number;
  }

  /** Per-share Time Machine state (`GET /shares/smb/time-machine`). */
  interface SmbTimeMachine {
    enabled: boolean;
    quota_gb: number;
  }

  /** One rsync module as the API renders it (never its password). */
  interface RsyncModule {
    name: string;
    path: string;
    comment: string;
    read_only: boolean;
    users: string[];
    enabled: boolean;
  }

  interface RsyncSettings {
    enabled: boolean;
    port: number;
    max_connections: number;
    chroot: boolean;
  }

  interface RsyncState {
    settings: RsyncSettings;
    modules: RsyncModule[];
    share_candidates: { name: string; path: string; used: boolean }[];
    users_with_secret: string[];
    available_users: string[];
    service_active: boolean;
    binary_available: boolean;
    config_path: string;
    module_limit: number;
    default_port: number;
    connection_limit: number;
    ssh_path: string;
    ssh_hint: string;
  }

  interface SmbConfig {
    workgroup: string;
    security_mode: string;
    protocol_version: string;
    allow_guest: boolean;
    create_mask: string;
    directory_mask: string;
    additional_params: string[];
  }

  interface NfsConfig {
    networks: string[];
    default_access: string;
    version: string;
    root_squash: string;
    custom_options: string[];
  }

  interface FtpConfig {
    port: number;
    allowed_users: string[];
    passive_ports: string;
    max_clients: number;
    force_tls: boolean;
    allow_anonymous: boolean;
    chroot: boolean;
    welcome_message: string;
  }

  interface SftpConfig {
    port: number;
  }

  interface SftpGrantee {
    username: string;
    shares: string[];
    has_key: boolean;
  }

  interface WebdavConfig {
    port: number;
    path_prefix: string;
    require_ssl: boolean;
    require_auth: boolean;
    compression: boolean;
    locking: boolean;
    allowed_methods: string[];
  }

  interface SvcStatus {
    name: string;
    display_name: string;
    running: boolean;
  }

  interface SharePermission {
    id: number;
    share_name: string;
    share_type: string;
    grantee_type: string;
    grantee_name: string;
    permission: string;
    recursive: boolean;
  }

  let shares = $state<Share[]>([]);
  // Per-share quota state, keyed `<name>:<type>` (the API inspects the
  // filesystem per share, so an unsupported one carries its reason).
  let quotas = $state<Record<string, ShareQuotaState>>({});
  // Which share's quota editor is open (`<name>:<type>`), and its GB value.
  let quotaEditor = $state<string | null>(null);
  let quotaInput = $state('');
  let quotaBusy = $state(false);
  let quotaNotice = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);
  let loading = $state(true);
  let showCreateModal = $state(false);
  let serviceStatus = $state<SvcStatus[]>([]);
  let configLoading = $state(false);
  const dlg = createConfirmState();

  let newShare = $state({
    name: '',
    path: '',
    share_type: 'smb',
    comment: ''
  });

  let activeTab = $state<'list' | 'smb' | 'nfs' | 'ftp' | 'webdav' | 'sftp' | 'rsync'>('list');

  let smbMode = $state<'simple' | 'advanced'>('simple');
  let nfsMode = $state<'simple' | 'advanced'>('simple');
  let ftpMode = $state<'simple' | 'advanced'>('simple');
  let webdavMode = $state<'simple' | 'advanced'>('simple');

  let sftpGrantees = $state<SftpGrantee[]>([]);
  let sftpGranteesLoading = $state(false);
  let sftpKeyDrafts = $state<Record<string, string>>({});
  let sftpRefreshing = $state(false);
  let newSftpUsername = $state('');
  let newSftpKey = $state('');

  let showPermissionsModal = $state(false);
  let permissionsShare = $state<Share | null>(null);
  let sharePermissions = $state<SharePermission[]>([]);
  let permissionsLoading = $state(false);
  let permissionsSyncing = $state(false);
  let newGrant = $state({ grantee_type: 'user', grantee_name: '', permission: 'r', recursive: true });

  /**
   * Fallbacks for the SMB form. The server always answers with every field,
   * but the form merges the response over these: a partial or older payload
   * must degrade to one field showing a default, never blank the whole tab
   * (`allow_guest: undefined` used to break the guest switch's binding).
   */
  const SMB_CONFIG_DEFAULTS: SmbConfig = { workgroup: 'WORKGROUP', security_mode: 'user', protocol_version: 'SMB3', allow_guest: false, create_mask: '0755', directory_mask: '0755', additional_params: [] };

  let smbConfig: SmbConfig = $state({ ...SMB_CONFIG_DEFAULTS });
  let smbRecycle = $state<Record<string, boolean>>({});
  let recycleSaving = $state<string | null>(null);
  let smbTimeMachine = $state<Record<string, SmbTimeMachine>>({});
  let tmSaving = $state<string | null>(null);
  let tmQuotaDrafts = $state<Record<string, string>>({});

  // Rsync server (Shares → Rsync)
  let rsync = $state<RsyncState | null>(null);
  let rsyncLoading = $state(false);
  let rsyncSaving = $state(false);
  let rsyncError = $state('');
  let rsyncModuleModal = $state(false);
  let rsyncModuleForm = $state({ name: '', path: '', comment: '', read_only: false, users: [] as string[], enabled: true });
  let rsyncModuleEditing = $state(false);
  let rsyncBusyName = $state<string | null>(null);
  let rsyncSecretDrafts = $state<Record<string, string>>({});
  let rsyncSecretBusy = $state<string | null>(null);
  let nfsConfig: NfsConfig = $state({ networks: ['*'], default_access: 'rw', version: '4', root_squash: 'root_squash', custom_options: [] });
  let ftpConfig: FtpConfig = $state({ port: 21, allowed_users: [], passive_ports: '30000-31000', max_clients: 10, force_tls: false, allow_anonymous: false, chroot: true, welcome_message: 'Welcome to HomeVault FTP' });
  let webdavConfig: WebdavConfig = $state({ port: 8082, path_prefix: '/', require_ssl: false, require_auth: true, compression: false, locking: true, allowed_methods: [] });
  let sftpConfig: SftpConfig = $state({ port: 2222 });

  let nfsNetworks = $state('*');

  let totalShares = $derived(shares.length);
  let activeShares = $derived(shares.filter(s => s.enabled).length);
  let smbShares = $derived(shares.filter(s => s.share_type.toLowerCase() === 'smb').length);
  let nfsShares = $derived(shares.filter(s => s.share_type.toLowerCase() === 'nfs').length);
  let ftpShares = $derived(shares.filter(s => s.share_type.toLowerCase() === 'ftp').length);
  let webdavShares = $derived(shares.filter(s => s.share_type.toLowerCase() === 'webdav').length);
  let sftpShares = $derived(shares.filter(s => s.share_type.toLowerCase() === 'sftp').length);

  let tabCounts = $derived<Record<string, number>>({
    list: totalShares, smb: smbShares, nfs: nfsShares, ftp: ftpShares, webdav: webdavShares, sftp: sftpShares,
    rsync: rsync?.modules.length ?? 0,
  });

  const TABS: { id: typeof activeTab; label: string }[] = [
    { id: 'list', label: 'shares.tab_list' },
    { id: 'smb', label: 'SMB' },
    { id: 'nfs', label: 'NFS' },
    { id: 'ftp', label: 'FTP' },
    { id: 'webdav', label: 'WebDAV' },
    { id: 'sftp', label: 'SFTP' },
    { id: 'rsync', label: 'Rsync' },
  ];

  onMount(() => {
    (async () => {
      await loadShares();
      await loadServiceStatus();
    })();

    // Opened from the File Explorer's "Share this folder" action: pre-fill
    // and open the create-share form for that path.
    const intent = get(explorerShareIntent);
    if (intent?.path) {
      explorerShareIntent.set(null);
      newShare = {
        name: (intent.path.split('/').filter(Boolean).pop() || '').toLowerCase().replace(/[^a-z0-9_-]+/g, '-'),
        path: intent.path,
        share_type: 'smb',
        comment: '',
      };
      showCreateModal = true;
    }

    // No manual refresh button — revalidate when the window regains attention.
    return onAppVisible(refreshCurrent);
  });

  function getServiceRunning(name: string): boolean {
    const s = serviceStatus.find(s => s.name === name);
    return s?.running ?? false;
  }

  async function loadShares() {
    loading = true;
    const response = await api.get('/shares');
    if (response.success && response.data) {
      shares = response.data;
      await loadQuotas();
    }
    loading = false;
  }

  // Quotas are read per share (the backend inspects the filesystem that holds
  // it, which is what makes "supported" truthful). One request per share, all
  // in parallel, and a failure leaves the bar out rather than blanking the
  // screen.
  async function loadQuotas() {
    const results = await Promise.all(
      shares.map((share) => api.getShareQuota(share.name, share.share_type)),
    );
    const next: Record<string, ShareQuotaState> = {};
    results.forEach((res, index) => {
      if (res.success && res.data) {
        const share = shares[index];
        next[`${share.name}:${share.share_type}`] = res.data as ShareQuotaState;
      }
    });
    quotas = next;
  }

  function quotaKey(share: Share): string {
    return `${share.name}:${share.share_type}`;
  }

  function quotaOf(share: Share): ShareQuotaState | undefined {
    return quotas[quotaKey(share)];
  }

  function openQuota(share: Share) {
    const current = quotaOf(share);
    quotaEditor = quotaKey(share);
    // The editor speaks GB (what an admin thinks in); the API takes bytes.
    quotaInput = current && current.quota_bytes > 0
      ? String(Math.round(current.quota_bytes / 1_000_000_000))
      : '';
    quotaNotice = null;
  }

  function closeQuota() {
    quotaEditor = null;
    quotaInput = '';
    quotaNotice = null;
  }

  async function saveQuota(share: Share) {
    const gigabytes = Number(quotaInput);
    if (!Number.isFinite(gigabytes) || gigabytes < 0) {
      quotaNotice = { kind: 'error', text: $t('shares.quota_invalid') };
      return;
    }
    quotaBusy = true;
    quotaNotice = null;
    const bytes = gigabytes === 0 ? 0 : Math.round(gigabytes * 1_000_000_000);
    const res = await api.setShareQuota(share.name, share.share_type, bytes);
    quotaBusy = false;
    if (res.success && res.data) {
      quotas = { ...quotas, [quotaKey(share)]: res.data as ShareQuotaState };
      quotaNotice = { kind: 'ok', text: $t('shares.quota_saved') };
    } else {
      quotaNotice = { kind: 'error', text: res.message || $t('shares.quota_save_error') };
    }
  }

  async function clearQuota(share: Share) {
    quotaBusy = true;
    quotaNotice = null;
    const res = await api.clearShareQuota(share.name, share.share_type);
    quotaBusy = false;
    if (res.success && res.data) {
      quotas = { ...quotas, [quotaKey(share)]: res.data as ShareQuotaState };
      quotaNotice = { kind: 'ok', text: $t('shares.quota_cleared') };
    } else {
      quotaNotice = { kind: 'error', text: res.message || $t('shares.quota_save_error') };
    }
  }

  async function loadServiceStatus() {
    const resp = await api.get('/network/services');
    if (resp.success && resp.data) {
      serviceStatus = resp.data.services ?? resp.data;
    }
  }

  async function startService(st: string) {
    const resp = await api.post(`/shares/${st}/start`);
    if (resp.success) {
      await loadServiceStatus();
    } else {
      notifyError(resp);
    }
  }

  async function stopService(st: string) {
    const resp = await api.post(`/shares/${st}/stop`);
    if (resp.success) {
      await loadServiceStatus();
    } else {
      notifyError(resp);
    }
  }

  async function restartService(st: string) {
    const resp = await api.post(`/shares/${st}/restart`);
    if (resp.success) {
      await loadServiceStatus();
    } else {
      notifyError(resp);
    }
  }

  async function loadSmbConfig() {
    configLoading = true;
    const resp = await api.get('/shares/smb/config');
    if (resp.success && resp.data) smbConfig = { ...SMB_CONFIG_DEFAULTS, ...resp.data };
    const recycle = await api.get('/shares/smb/recycle');
    if (recycle.success && recycle.data) smbRecycle = recycle.data;
    await loadSmbTimeMachine();
    configLoading = false;
  }

  async function loadSmbTimeMachine() {
    const resp = await api.get('/shares/smb/time-machine');
    if (resp.success && resp.data) {
      smbTimeMachine = resp.data as Record<string, SmbTimeMachine>;
      const drafts: Record<string, string> = {};
      for (const [name, tm] of Object.entries(smbTimeMachine)) {
        drafts[name] = tm.quota_gb ? String(tm.quota_gb) : '';
      }
      tmQuotaDrafts = drafts;
    }
  }

  /** Turn Time Machine on/off for one SMB share. The server validates the
   *  quota (0 = unlimited) before it touches `smb.conf`. */
  async function toggleSmbTimeMachine(name: string, enabled: boolean) {
    const previous = smbTimeMachine[name] ?? { enabled: false, quota_gb: 0 };
    const quota = Number(tmQuotaDrafts[name] ?? '') || 0;
    smbTimeMachine = { ...smbTimeMachine, [name]: { enabled, quota_gb: enabled ? quota : 0 } };
    tmSaving = name;
    const resp = await api.put(`/shares/${encodeURIComponent(name)}/smb/time-machine`, {
      enabled,
      quota_gb: enabled ? quota : 0,
    });
    tmSaving = null;
    if (resp.success) notifySuccess('common.saved');
    else {
      smbTimeMachine = { ...smbTimeMachine, [name]: previous };
      notifyError(resp);
    }
  }

  /** Save the maximum size macOS must respect; 0 (empty) means unlimited. */
  async function saveSmbTimeMachineQuota(name: string) {
    const enabled = smbTimeMachine[name]?.enabled ?? false;
    const quota = Number(tmQuotaDrafts[name] ?? '') || 0;
    tmSaving = name;
    const resp = await api.put(`/shares/${encodeURIComponent(name)}/smb/time-machine`, {
      enabled,
      quota_gb: enabled ? quota : 0,
    });
    tmSaving = null;
    if (resp.success) {
      smbTimeMachine = { ...smbTimeMachine, [name]: { enabled, quota_gb: enabled ? quota : 0 } };
      notifySuccess('common.saved');
    } else {
      notifyError(resp);
    }
  }

  async function loadRsync() {
    rsyncLoading = true;
    rsyncError = '';
    const resp = await api.get('/shares/rsync');
    if (resp.success && resp.data) rsync = resp.data as RsyncState;
    else rsyncError = resp.message || translate('shares.rsync_load_failed');
    rsyncLoading = false;
  }

  async function saveRsyncSettings() {
    if (!rsync) return;
    rsyncSaving = true;
    rsyncError = '';
    const resp = await api.put('/shares/rsync/config', rsync.settings);
    rsyncSaving = false;
    if (resp.success && resp.data) {
      rsync = resp.data as RsyncState;
      notifySuccess('common.saved');
    } else {
      rsyncError = resp.message || translate('shares.rsync_save_failed');
    }
  }

  function openRsyncModule(module?: RsyncModule) {
    rsyncModuleEditing = !!module;
    rsyncModuleForm = module
      ? { ...module, users: [...module.users] }
      : { name: '', path: '', comment: '', read_only: false, users: [], enabled: true };
    rsyncModuleModal = true;
  }

  async function saveRsyncModule() {
    if (!rsync) return;
    const form = rsyncModuleForm;
    // The server refuses all three, but a round trip that fails after the
    // dialog has already looked acceptable is the wrong order: the reason
    // belongs next to the field, before anything is sent.
    if (!rsyncModuleEditing && !form.path) {
      rsyncError = translate('shares.rsync_need_share');
      return;
    }
    if (!rsyncModuleEditing && !/^[a-z0-9][a-z0-9._-]*$/.test(form.name)) {
      rsyncError = translate('shares.rsync_name_lowercase');
      return;
    }
    if (form.users.length === 0) {
      rsyncError = translate('shares.rsync_need_account');
      return;
    }
    rsyncSaving = true;
    rsyncError = '';
    const resp = rsyncModuleEditing
      ? await api.put(`/shares/rsync/modules/${encodeURIComponent(form.name)}`, {
          read_only: form.read_only, users: form.users, comment: form.comment, enabled: form.enabled,
        })
      : await api.post('/shares/rsync/modules', {
          name: form.name, path: form.path, comment: form.comment,
          read_only: form.read_only, users: form.users, enabled: form.enabled,
        });
    rsyncSaving = false;
    if (resp.success && resp.data) {
      rsync = resp.data as RsyncState;
      rsyncModuleModal = false;
      notifySuccess('common.saved');
    } else {
      rsyncError = resp.message || translate('shares.rsync_save_failed');
    }
  }

  async function toggleRsyncModule(module: RsyncModule, enabled: boolean) {
    rsyncBusyName = module.name;
    rsyncError = '';
    const resp = await api.put(`/shares/rsync/modules/${encodeURIComponent(module.name)}`, { enabled });
    rsyncBusyName = null;
    if (resp.success && resp.data) rsync = resp.data as RsyncState;
    else rsyncError = resp.message || translate('shares.rsync_save_failed');
  }

  async function deleteRsyncModule(module: RsyncModule) {
    if (!(await dlg.ask(translate('shares.rsync_delete_confirm', { name: module.name })))) return;
    rsyncBusyName = module.name;
    const resp = await api.delete(`/shares/rsync/modules/${encodeURIComponent(module.name)}`);
    rsyncBusyName = null;
    if (resp.success && resp.data) rsync = resp.data as RsyncState;
    else notifyError(resp);
  }

  async function saveRsyncSecret(user: string) {
    const password = (rsyncSecretDrafts[user] ?? '').trim();
    if (!password) return;
    rsyncSecretBusy = user;
    rsyncError = '';
    const resp = await api.put(`/shares/rsync/users/${encodeURIComponent(user)}/secret`, { password });
    rsyncSecretBusy = null;
    if (resp.success) {
      rsyncSecretDrafts = { ...rsyncSecretDrafts, [user]: '' };
      await loadRsync();
      notifySuccess('common.saved');
    } else {
      rsyncError = resp.message || translate('shares.rsync_save_failed');
    }
  }

  function toggleRsyncUser(username: string) {
    const users = rsyncModuleForm.users.includes(username)
      ? rsyncModuleForm.users.filter((u) => u !== username)
      : [...rsyncModuleForm.users, username];
    rsyncModuleForm = { ...rsyncModuleForm, users };
  }

  /** Turn the recycle bin on or off for one SMB share. Optimistic: the switch
   *  moves immediately and reverts if the server rejects the change. */
  async function toggleSmbRecycle(name: string, enabled: boolean) {
    const previous = smbRecycle[name] ?? false;
    smbRecycle = { ...smbRecycle, [name]: enabled };
    recycleSaving = name;
    const resp = await api.put(`/shares/${encodeURIComponent(name)}/smb/recycle`, { enabled });
    recycleSaving = null;
    if (resp.success) {
      notifySuccess('common.saved');
    } else {
      smbRecycle = { ...smbRecycle, [name]: previous };
      notifyError(resp);
    }
  }

  async function saveSmbConfig() {
    const resp = await api.put('/shares/smb/config', smbConfig);
    if (resp.success) notifySuccess('common.saved');
    else notifyError(resp);
  }

  async function loadNfsConfig() {
    configLoading = true;
    const resp = await api.get('/shares/nfs/config');
    if (resp.success && resp.data) {
      nfsConfig = resp.data;
      nfsNetworks = (nfsConfig.networks || ['*']).join('\n');
    } else {
      nfsNetworks = '*';
    }
    configLoading = false;
  }

  function saveNetworks() {
    nfsConfig.networks = nfsNetworks.split('\n').map(s => s.trim()).filter(s => s.length > 0);
  }

  async function saveNfsConfig() {
    saveNetworks();
    const resp = await api.put('/shares/nfs/config', nfsConfig);
    if (resp.success) notifySuccess('common.saved');
    else notifyError(resp);
  }

  async function loadFtpConfig() {
    configLoading = true;
    const resp = await api.get('/shares/ftp/config');
    if (resp.success && resp.data) ftpConfig = resp.data;
    configLoading = false;
  }

  async function saveFtpConfig() {
    const resp = await api.put('/shares/ftp/config', ftpConfig);
    if (resp.success) notifySuccess('common.saved');
    else notifyError(resp);
  }

  async function loadWebdavConfig() {
    configLoading = true;
    const resp = await api.get('/shares/webdav/config');
    if (resp.success && resp.data) webdavConfig = resp.data;
    configLoading = false;
  }

  async function saveWebdavConfig() {
    const resp = await api.put('/shares/webdav/config', webdavConfig);
    if (resp.success) notifySuccess('common.saved');
    else notifyError(resp);
  }

  async function loadSftpConfig() {
    configLoading = true;
    const resp = await api.get('/shares/sftp/config');
    if (resp.success && resp.data) sftpConfig = resp.data;
    configLoading = false;
  }

  async function saveSftpConfig() {
    const resp = await api.put('/shares/sftp/config', sftpConfig);
    if (resp.success) notifySuccess('common.saved');
    else notifyError(resp);
  }

  async function loadSftpGrantees() {
    sftpGranteesLoading = true;
    const resp = await api.get('/shares/sftp/users');
    if (resp.success && resp.data) {
      sftpGrantees = resp.data;
      for (const g of sftpGrantees) {
        if (!(g.username in sftpKeyDrafts)) sftpKeyDrafts[g.username] = '';
      }
    }
    sftpGranteesLoading = false;
  }

  async function handleSaveSftpKey(username: string) {
    const key = (sftpKeyDrafts[username] || '').trim();
    if (!key) {
      return;
    }
    const resp = await api.put('/shares/sftp/users/key', { username, public_key: key });
    if (resp.success) {
      sftpKeyDrafts[username] = '';
      await loadSftpGrantees();
    } else {
      notifyError(resp);
    }
  }

  async function handleAddSftpUser() {
    const username = newSftpUsername.trim();
    const key = newSftpKey.trim();
    if (!username || !key) {
      return;
    }
    const resp = await api.put('/shares/sftp/users/key', { username, public_key: key });
    if (resp.success) {
      newSftpUsername = '';
      newSftpKey = '';
      await loadSftpGrantees();
    } else {
      notifyError(resp);
    }
  }

  async function handleRefreshSftpAccess() {
    sftpRefreshing = true;
    const resp = await api.post('/shares/sftp/rebuild');
    sftpRefreshing = false;
    if (!resp.success) notifyError(resp);
  }

  function switchTab(tab: typeof activeTab) {
    activeTab = tab;
    if (tab === 'smb') loadSmbConfig();
    else if (tab === 'nfs') loadNfsConfig();
    else if (tab === 'ftp') loadFtpConfig();
    else if (tab === 'webdav') loadWebdavConfig();
    else if (tab === 'sftp') { loadSftpConfig(); loadSftpGrantees(); }
    else if (tab === 'rsync') loadRsync();
  }

  // Arrow-key navigation across the tab strip (WAI-ARIA tabs pattern).
  function onTabKeydown(e: KeyboardEvent, index: number) {
    const last = TABS.length - 1;
    let next = -1;
    if (e.key === 'ArrowRight' || e.key === 'ArrowDown') next = index === last ? 0 : index + 1;
    else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') next = index === 0 ? last : index - 1;
    else if (e.key === 'Home') next = 0;
    else if (e.key === 'End') next = last;
    else return;
    e.preventDefault();
    switchTab(TABS[next].id as typeof activeTab);
    const strip = (e.currentTarget as HTMLElement).parentElement;
    (strip?.children[next] as HTMLElement | undefined)?.focus();
  }

  async function refreshCurrent() {
    if (activeTab === 'list') { await loadShares(); await loadServiceStatus(); }
    else switchTab(activeTab);
  }

  async function handleCreateShare() {
    if (!newShare.name || !newShare.path) {
      return;
    }
    const response = await api.post('/shares', {
      name: newShare.name,
      path: newShare.path,
      share_type: newShare.share_type,
      comment: newShare.comment,
      options: []
    });
    if (response.success) {
      showCreateModal = false;
      newShare = { name: '', path: '', share_type: 'smb', comment: '' };
      await loadShares();
    } else {
      notifyError(response);
    }
  }

  async function handleDeleteShare(name: string, shareType: string) {
    if (!(await dlg.ask(translate('shares.delete_confirm', { name })))) return;
    const response = await api.delete(`/shares/${name}/${shareType}`);
    if (response.success) {
      await loadShares();
    } else {
      notifyError(response);
    }
  }

  async function handleToggleShare(name: string, shareType: string, enabled: boolean) {
    const response = await api.put(`/shares/${name}/${shareType}/toggle/${enabled}`);
    if (response.success) {
      await loadShares();
    } else {
      notifyError(response);
    }
  }

  async function openPermissions(share: Share) {
    permissionsShare = share;
    newGrant = { grantee_type: 'user', grantee_name: '', permission: 'r', recursive: true };
    showPermissionsModal = true;
    await loadSharePermissions();
  }

  function closePermissions() {
    showPermissionsModal = false;
    permissionsShare = null;
    sharePermissions = [];
  }

  async function loadSharePermissions() {
    if (!permissionsShare) return;
    permissionsLoading = true;
    const resp = await api.getSharePermissions(permissionsShare.name, permissionsShare.share_type);
    sharePermissions = (resp.success && resp.data) ? resp.data : [];
    permissionsLoading = false;
  }

  async function handleGrantPermission() {
    if (!permissionsShare) return;
    if (!newGrant.grantee_name.trim()) {
      return;
    }
    const resp = await api.grantSharePermission(permissionsShare.name, permissionsShare.share_type, {
      grantee_type: newGrant.grantee_type,
      grantee_name: newGrant.grantee_name.trim(),
      permission: newGrant.permission,
      recursive: newGrant.recursive,
    });
    if (resp.success) {
      newGrant = { grantee_type: 'user', grantee_name: '', permission: 'r', recursive: true };
      await loadSharePermissions();
    } else {
      notifyError(resp);
    }
  }

  async function handleRevokePermission(perm: SharePermission) {
    if (!permissionsShare) return;
    const resp = await api.revokeSharePermission(permissionsShare.name, permissionsShare.share_type, {
      grantee_type: perm.grantee_type,
      grantee_name: perm.grantee_name,
    });
    if (resp.success) {
      await loadSharePermissions();
    } else {
      notifyError(resp);
    }
  }

  async function handleSyncPermissions() {
    if (!permissionsShare) return;
    permissionsSyncing = true;
    const resp = await api.syncSharePermissions(permissionsShare.name, permissionsShare.share_type);
    if (resp.success) {
      await loadSharePermissions();
    } else {
      notifyError(resp);
    }
    permissionsSyncing = false;
  }

  let webdavUrl = $derived(`http://${window.location.hostname}:${webdavConfig.port}${webdavConfig.path_prefix || '/'}`);

  // One role colour per protocol from the shared semantic palette; used as
  // the tile/badge fill with white glyphs/text on top.
  function getProtocolColor(protocol: string): string {
    switch (protocol.toLowerCase()) {
      case 'smb': return 'var(--hv-primary)';
      case 'nfs': return 'var(--hv-success)';
      case 'ftp': return 'var(--hv-warning)';
      case 'webdav': return 'var(--hv-primary)';
      case 'sftp': return 'var(--hv-info)';
      default: return 'var(--hv-shell-text)';
    }
  }

  function closeCreate() {
    showCreateModal = false;
    newShare = { name: '', path: '', share_type: 'smb', comment: '' };
  }

  // Shared shapes (appliance tokens only — the per-app CSS vocabulary is gone).
  const btn =
    'inline-flex items-center justify-center gap-2 h-12 px-5 border border-[var(--hv-shell-border-strong)] ' +
    'rounded-lg bg-[var(--hv-shell-fill)] text-[var(--hv-shell-text-strong)] text-sm font-semibold cursor-pointer ' +
    'transition-colors hover:bg-[var(--hv-shell-border)] disabled:opacity-50 disabled:cursor-default';
  // 48px tile variant of the shared `tileBtn`; the delete action adds
  // `dangerHover` at its call site (components/app controlClasses.ts).
  const iconBtn = cn(tileBtn, 'w-12 h-12');
  const inp =
    'w-full box-border h-12 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] ' +
    'text-[var(--hv-shell-text-strong)] text-c-card-title outline-none transition-colors focus:border-[var(--hv-primary)] ' +
    'disabled:opacity-60';
  const field = 'flex flex-col gap-2';
  const fieldLabel = 'text-c-base font-semibold text-[var(--hv-shell-text-strong)]';
  const segBtn = 'flex-1 h-11 border-none rounded-lg bg-transparent text-c-base font-semibold text-[var(--hv-shell-text-2)] cursor-pointer transition-colors';
</script>

<PageLayout
  title={$t('shares.heading')}
  data-testid="shares-page"
  footerMode="status"
>
  {#snippet icon()}
    <IconShare stroke={1.7} />
  {/snippet}

  <!-- Page-level primary action: a WindowToolbar row under the header, never
       inside it (the header bar is icon + title + window controls only). -->
  {#snippet toolbarRight()}
    {#if activeTab === 'list'}
      <Button variant="primary" onclick={() => (showCreateModal = true)} data-testid="shares-new">
        <IconPlus size={18} stroke={2} /> {$t('shares.new_share')}
      </Button>
    {/if}
  {/snippet}

  <div data-testid="shares-tabs" class="flex gap-1.5 px-4 py-3 flex-none border-b border-[var(--hv-shell-border-soft)] bg-[var(--hv-shell-fill)] overflow-x-auto" role="tablist" aria-label={$t('shares.heading')}>
    {#each TABS as tb, i (tb.id)}
      <button
        data-testid="shares-tab"
        class:active={activeTab === tb.id}
        role="tab"
        aria-selected={activeTab === tb.id}
        tabindex={activeTab === tb.id ? 0 : -1}
        onclick={() => switchTab(tb.id)}
        onkeydown={(e) => onTabKeydown(e, i)}
        class={cn(
          'inline-flex items-center gap-2 h-11 px-4 flex-none border-none rounded-lg bg-[var(--hv-shell-fill)] text-[var(--hv-shell-text-2)] text-sm font-medium cursor-pointer transition-colors hover:bg-[var(--hv-shell-border-soft)] hover:text-[var(--hv-shell-text-strong)] focus-visible:outline-2 focus-visible:outline-[var(--hv-primary)]',
          activeTab === tb.id && 'bg-[var(--hv-primary-light)] text-[var(--hv-primary)] font-semibold',
        )}
      >
        {tb.id === 'list' ? $t(tb.label) : tb.label}
        {#if tabCounts[tb.id] > 0}
          <span class="min-w-5 h-5 px-1.5 grid place-items-center rounded-full bg-[var(--hv-shell-border)] text-c-xs font-bold">{tabCounts[tb.id]}</span>
        {/if}
      </button>
    {/each}
  </div>

  {#if activeTab === 'list'}
    {#if loading}
      <div data-testid="shares-loading" class="flex items-center justify-center gap-3 py-12 text-[var(--hv-shell-text)]"><Spinner /><span>{$t('shares.loading2')}</span></div>
    {:else}
      <div data-testid="shares-stats" class="grid grid-cols-2 gap-3 mb-5 max-w-[520px] @max-[1024px]:max-w-none">
        <StatCard data-testid="shares-stat" class="border-[var(--hv-primary)]/20" label={$t('shares.stat_total')} value={`${totalShares}`} />
        <StatCard data-testid="shares-stat" class="border-[var(--hv-primary)]/20" label={$t('shares.stat_active')} value={`${activeShares}`} />
      </div>

      {#if shares.length === 0}
        <EmptyState data-testid="shares-empty" title={$t('shares.empty_title')} description={$t('shares.empty_desc')}>
          {#snippet iconSnippet()}<IconShare size={32} stroke={1.8} />{/snippet}
          {#snippet action()}
            <Button variant="primary" size="lg" onclick={() => (showCreateModal = true)}>
              <IconPlus size={19} stroke={2.2} /> {$t('shares.new_share')}
            </Button>
          {/snippet}
        </EmptyState>
      {:else}
        <div class="flex flex-col gap-3 max-w-[640px]">
          {#each shares as share (share.name + share.share_type)}
            <div data-testid="shares-card" class="p-4 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)]">
              <div class="flex items-center justify-between gap-2.5 mb-3">
                <span class="px-2.5 py-1.5 rounded-md text-c-xs font-extrabold tracking-wide text-white" style="background:{getProtocolColor(share.share_type)}">{share.share_type.toUpperCase()}</span>
                <span class:on={share.enabled} class={cn('inline-flex items-center gap-2 text-c-label font-semibold', share.enabled ? 'text-[var(--hv-success)]' : 'text-[var(--hv-shell-text)]')}>
                  <span class={cn('w-2 h-2 rounded-full', share.enabled ? 'bg-[var(--hv-success)] shadow-[0_0_6px_var(--hv-success)]' : 'bg-[var(--hv-shell-text-3)]')}></span>
                  {share.enabled ? $t('shares.active') : $t('shares.inactive')}
                </span>
              </div>
              <div class="text-c-xl font-semibold mb-2 text-[var(--hv-shell-text-strong)]">{share.name}</div>
              <code class="block font-[family-name:var(--hv-font-mono)] text-c-base text-[var(--hv-shell-text-2)] bg-[var(--hv-shell-border-soft)] px-2.5 py-2 rounded-lg overflow-x-auto whitespace-nowrap">{share.path}</code>
              {#if share.comment}<p class="text-c-base text-[var(--hv-shell-text)] leading-relaxed mt-2.5 mb-0">{share.comment}</p>{/if}

              <!-- Quota (QW-07): a bar when a ceiling is set, the reason when
                   the filesystem cannot enforce one, and an editor behind a
                   button so the card stays readable. -->
              {#if quotaOf(share)?.supported === false}
                <p class="m-0 mt-2.5 text-c-sm text-[var(--hv-shell-text-2)]" data-testid="shares-quota-unsupported">
                  {$t('shares.quota_unsupported', { fs: quotaOf(share)?.filesystem ?? '' })} — {quotaOf(share)?.reason ?? ''}
                </p>
              {:else if quotaOf(share) && (quotaOf(share)!.quota_bytes > 0)}
                <div class="mt-2.5" data-testid="shares-quota">
                  <div class="flex items-center justify-between gap-2 text-c-sm text-[var(--hv-shell-text-2)]">
                    <span>{$t('shares.quota_usage', { used: quotaOf(share)!.used_human, quota: quotaOf(share)!.quota_human })}</span>
                    <span data-testid="shares-quota-percent">{quotaOf(share)!.percent}%</span>
                  </div>
                  <div
                    class="mt-1 h-2 rounded-full bg-[var(--hv-shell-border-soft)] overflow-hidden"
                    role="progressbar"
                    aria-valuemin="0"
                    aria-valuemax="100"
                    aria-valuenow={quotaOf(share)!.percent}
                    aria-label={$t('shares.quota_bar_label', { name: share.name })}
                  >
                    <div
                      class="h-full rounded-full"
                      class:bg-[var(--hv-error)]={quotaOf(share)!.percent >= 95}
                      class:bg-[var(--hv-warning)]={quotaOf(share)!.percent >= 80 && quotaOf(share)!.percent < 95}
                      class:bg-[var(--hv-primary)]={quotaOf(share)!.percent < 80}
                      style="width:{quotaOf(share)!.percent}%"
                    ></div>
                  </div>
                </div>
              {/if}

              {#if quotaEditor === quotaKey(share)}
                <div class="mt-3 p-3 rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]" data-testid="shares-quota-editor">
                  {#if quotaNotice}
                    <p
                      class="m-0 mb-2 text-c-sm"
                      class:text-[var(--hv-error)]={quotaNotice.kind === 'error'}
                      class:text-[var(--hv-success)]={quotaNotice.kind === 'ok'}
                      role={quotaNotice.kind === 'error' ? 'alert' : 'status'}
                      data-testid="shares-quota-notice"
                    >
                      {quotaNotice.text}
                    </p>
                  {/if}
                  <label class="block text-c-label font-semibold text-[var(--hv-shell-text-strong)]" for={`quota-${quotaKey(share)}`}>
                    {$t('shares.quota_field')}
                    <input
                      id={`quota-${quotaKey(share)}`}
                      class="mt-1 w-full h-11 px-3 rounded-lg border border-[var(--hv-shell-border-strong)] bg-[var(--hv-shell-card)] text-[var(--hv-shell-text-strong)]"
                      type="number"
                      min="0"
                      step="1"
                      placeholder="0"
                      bind:value={quotaInput}
                      data-testid="shares-quota-input"
                    />
                  </label>
                  <p class="m-0 mt-1 text-c-sm text-[var(--hv-shell-text-2)]">{$t('shares.quota_hint')}</p>
                  <div class="flex flex-wrap gap-2 mt-3">
                    <button class={cn(btn, 'fill')} disabled={quotaBusy} onclick={() => saveQuota(share)} data-testid="shares-quota-save">
                      {$t('common.save')}
                    </button>
                    <button class={cn(btn, 'ghost')} disabled={quotaBusy} onclick={() => clearQuota(share)} data-testid="shares-quota-clear">
                      {$t('shares.quota_clear')}
                    </button>
                    <button class={cn(btn, 'ghost')} disabled={quotaBusy} onclick={closeQuota} data-testid="shares-quota-cancel">
                      {$t('common.cancel')}
                    </button>
                  </div>
                </div>
              {/if}

              <div class="flex gap-2.5 mt-4">
                <button data-testid="shares-btn" class={cn(btn, 'fill flex-1')} onclick={() => handleToggleShare(share.name, share.share_type, !share.enabled)}>
                  {#if share.enabled}<IconSquare size={16} />{:else}<IconPlayerPlay size={16} />{/if}
                  {share.enabled ? $t('shares.stop') : $t('shares.start')}
                </button>
                <button data-testid="shares-icon-btn" class={iconBtn} onclick={() => openPermissions(share)} title={$t('shares.permissions_button')} aria-label={$t('shares.permissions_button')}>
                  <IconShield size={18} />
                </button>
                <button
                  data-testid="shares-quota-btn"
                  class={iconBtn}
                  onclick={() => openQuota(share)}
                  title={$t('shares.quota_button')}
                  aria-label={$t('shares.quota_button')}
                >
                  <IconGauge size={18} />
                </button>
                <button data-testid="shares-icon-btn" class={cn(iconBtn, dangerHover)} onclick={() => handleDeleteShare(share.name, share.share_type)} title={$t('common.delete')} aria-label={$t('common.delete')}>
                  <IconTrash size={18} />
                </button>
              </div>
            </div>
          {/each}
        </div>
      {/if}
    {/if}

  {:else if activeTab === 'smb'}
    <div data-testid="shares-config" class="flex flex-col gap-4 max-w-[640px]">
      {@render svcBar('smb', 'SMB / CIFS')}
      {@render modeSwitch(smbMode, (m) => (smbMode = m))}
      {#if configLoading}<div data-testid="shares-loading" class="flex items-center justify-center py-12"><Spinner /></div>{:else}
        <div data-testid="shares-form" class="flex flex-col gap-4 p-5 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)]">
          <label class={field}><span class={fieldLabel}>{$t('shares.workgroup')}</span><input class={inp} bind:value={smbConfig.workgroup} /></label>
          {@render toggleField($t('shares.guest'), smbConfig.allow_guest, (v) => (smbConfig.allow_guest = v))}
          {#if smbMode === 'advanced'}
            <label class={field}><span class={fieldLabel}>{$t('shares.security_mode')}</span><select class={inp} bind:value={smbConfig.security_mode}><option value="user">{$t('shares.opt_user')}</option><option value="share">{$t('shares.opt_share')}</option><option value="domain">{$t('shares.opt_domain')}</option><option value="ads">{$t('shares.opt_ads')}</option></select></label>
            <label class={field}><span class={fieldLabel}>{$t('shares.version')}</span><select class={inp} bind:value={smbConfig.protocol_version}><option value="SMB2">SMB2</option><option value="SMB3">SMB3</option><option value="NT1">NT1 (CIFS)</option></select></label>
            <label class={field}><span class={fieldLabel}>{$t('shares.create_mask')}</span><input class={inp} bind:value={smbConfig.create_mask} /></label>
            <label class={field}><span class={fieldLabel}>{$t('shares.directory_mask')}</span><input class={inp} bind:value={smbConfig.directory_mask} /></label>
          {/if}
          <Button variant="primary" size="lg" onclick={saveSmbConfig}>{$t('common.save')}</Button>
        </div>
      {/if}

      {#if smbShares > 0}
        <div data-testid="shares-recycle" class="flex flex-col gap-4 p-5 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)]">
          <div>
            <h3 class="m-0 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">{$t('shares.recycle_title')}</h3>
            <p class="m-0 mt-1 text-c-base text-[var(--hv-shell-text)] leading-relaxed">{$t('shares.recycle_desc')}</p>
          </div>
          <div class="flex flex-col gap-3">
            {#each shares.filter(s => s.share_type.toLowerCase() === 'smb') as share (share.name)}
              {@const holder = { get checked() { return smbRecycle[share.name] ?? false; }, set checked(v) { toggleSmbRecycle(share.name, v); } }}
              <div data-testid="shares-recycle-row" class="flex items-center justify-between gap-3 flex-wrap p-3 rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]">
                <div class="flex flex-col min-w-0">
                  <span id={`shares-recycle-name-${share.name}`} class="font-semibold text-c-card-title text-[var(--hv-shell-text-strong)] truncate">{share.name}</span>
                  <code class="font-[family-name:var(--hv-font-mono)] text-c-label text-[var(--hv-shell-text-2)] truncate">{share.path}</code>
                </div>
                <Toggle bind:checked={holder.checked} disabled={recycleSaving === share.name} ariaLabelledby={`shares-recycle-name-${share.name}`} />
              </div>
            {/each}
          </div>
        </div>

        <div data-testid="shares-time-machine" class="flex flex-col gap-4 p-5 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)]">
          <div>
            <h3 class="m-0 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">{$t('shares.tm_title')}</h3>
            <p class="m-0 mt-1 text-c-base text-[var(--hv-shell-text)] leading-relaxed">{$t('shares.tm_desc')}</p>
            <p class="m-0 mt-1 text-c-label text-[var(--hv-shell-text-2)] leading-relaxed">{$t('shares.tm_bonjour_hint')}</p>
          </div>
          <div class="flex flex-col gap-3">
            {#each shares.filter(s => s.share_type.toLowerCase() === 'smb') as share (share.name)}
              {@const tm = smbTimeMachine[share.name] ?? { enabled: false, quota_gb: 0 }}
              {@const tmHolder = { get checked() { return tm.enabled; }, set checked(v: boolean) { toggleSmbTimeMachine(share.name, v); } }}
              <div data-testid="shares-time-machine-row" class="flex flex-col gap-2 p-3 rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]">
                <div class="flex items-center justify-between gap-3 flex-wrap">
                  <div class="flex flex-col min-w-0">
                    <span id={`shares-tm-name-${share.name}`} class="font-semibold text-c-card-title text-[var(--hv-shell-text-strong)] truncate">{share.name}</span>
                    <code class="font-[family-name:var(--hv-font-mono)] text-c-label text-[var(--hv-shell-text-2)] truncate">{share.path}</code>
                  </div>
                  <Toggle
                    bind:checked={tmHolder.checked}
                    disabled={tmSaving === share.name}
                    ariaLabelledby={`shares-tm-name-${share.name}`}
                  />
                </div>
                {#if tm.enabled}
                  <div class="flex items-end gap-2 flex-wrap">
                    <label class="flex flex-col gap-1 flex-1 min-w-[160px]">
                      <span class="text-c-label font-semibold text-[var(--hv-shell-text)]">{$t('shares.tm_quota')}</span>
                      <input
                        class={inp}
                        type="number"
                        min="0"
                        step="1"
                        inputmode="numeric"
                        placeholder={$t('shares.tm_quota_unlimited')}
                        bind:value={tmQuotaDrafts[share.name]}
                        data-testid="shares-tm-quota"
                      />
                    </label>
                    <Button variant="ghost" onclick={() => saveSmbTimeMachineQuota(share.name)} disabled={tmSaving === share.name}>
                      {$t('common.save')}
                    </Button>
                  </div>
                  <p class="m-0 text-c-label text-[var(--hv-shell-text-2)]">{$t('shares.tm_quota_hint')}</p>
                {/if}
              </div>
            {/each}
          </div>
        </div>
      {/if}
    </div>

  {:else if activeTab === 'nfs'}
    <div data-testid="shares-config" class="flex flex-col gap-4 max-w-[640px]">
      {@render svcBar('nfs', 'NFS')}
      {@render modeSwitch(nfsMode, (m) => (nfsMode = m))}
      {#if configLoading}<div data-testid="shares-loading" class="flex items-center justify-center py-12"><Spinner /></div>{:else}
        <div data-testid="shares-form" class="flex flex-col gap-4 p-5 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)]">
          <label class={field}><span class={fieldLabel}>{$t('shares.networks')}</span><textarea class={cn(inp, 'h-auto min-h-[76px] py-3 font-[family-name:var(--hv-font-mono)] text-c-base leading-relaxed resize-y')} bind:value={nfsNetworks} rows="3"></textarea></label>
          <label class={field}><span class={fieldLabel}>{$t('shares.access')}</span><select class={inp} bind:value={nfsConfig.default_access}><option value="rw">{$t('shares.read_write')}</option><option value="ro">{$t('shares.read_only_opt')}</option></select></label>
          {#if nfsMode === 'advanced'}
            <label class={field}><span class={fieldLabel}>{$t('shares.version')}</span><select class={inp} bind:value={nfsConfig.version}><option value="3">NFS v3</option><option value="4">NFS v4</option><option value="4.2">NFS v4.2</option></select></label>
            <label class={field}><span class={fieldLabel}>{$t('shares.root_squash')}</span><select class={inp} bind:value={nfsConfig.root_squash}><option value="root_squash">{$t('shares.root_squash')}</option><option value="no_root_squash">{$t('shares.no_root_squash')}</option><option value="all_squash">{$t('shares.all_squash')}</option></select></label>
          {/if}
          <Button variant="primary" size="lg" onclick={saveNfsConfig}>{$t('common.save')}</Button>
        </div>
      {/if}
    </div>

  {:else if activeTab === 'ftp'}
    <div data-testid="shares-config" class="flex flex-col gap-4 max-w-[640px]">
      {@render svcBar('ftp', 'FTP')}
      {@render modeSwitch(ftpMode, (m) => (ftpMode = m))}
      {#if configLoading}<div data-testid="shares-loading" class="flex items-center justify-center py-12"><Spinner /></div>{:else}
        <div data-testid="shares-form" class="flex flex-col gap-4 p-5 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)]">
          {@render toggleField($t('shares.force_tls'), ftpConfig.force_tls, (v) => (ftpConfig.force_tls = v))}
          {@render toggleField($t('shares.anonymous'), ftpConfig.allow_anonymous, (v) => (ftpConfig.allow_anonymous = v))}
          <label class={field}><span class={fieldLabel}>{$t('shares.message')}</span><input class={inp} bind:value={ftpConfig.welcome_message} /></label>
          {#if ftpMode === 'advanced'}
            <label class={field}><span class={fieldLabel}>{$t('shares.ftp_port')}</span><input class={inp} type="number" bind:value={ftpConfig.port} /></label>
            <label class={field}><span class={fieldLabel}>{$t('shares.passive_ports')}</span><input class={inp} bind:value={ftpConfig.passive_ports} /></label>
            <label class={field}><span class={fieldLabel}>{$t('shares.max_clients')}</span><input class={inp} type="number" bind:value={ftpConfig.max_clients} /></label>
            {@render toggleField($t('shares.chroot'), ftpConfig.chroot, (v) => (ftpConfig.chroot = v))}
          {/if}
          <Button variant="primary" size="lg" onclick={saveFtpConfig}>{$t('common.save')}</Button>
        </div>
      {/if}
    </div>

  {:else if activeTab === 'webdav'}
    <div data-testid="shares-config" class="flex flex-col gap-4 max-w-[640px]">
      {@render svcBar('webdav', 'WebDAV')}
      {@render modeSwitch(webdavMode, (m) => (webdavMode = m))}
      {#if configLoading}<div data-testid="shares-loading" class="flex items-center justify-center py-12"><Spinner /></div>{:else}
        <div data-testid="shares-form" class="flex flex-col gap-4 p-5 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)]">
          <label class={field}><span class={fieldLabel}>{$t('shares.access_url')}</span><input class={inp} value={webdavUrl} disabled /></label>
          {@render toggleField($t('shares.auth_required'), webdavConfig.require_auth, (v) => (webdavConfig.require_auth = v))}
          {@render toggleField($t('shares.ssl_tls'), webdavConfig.require_ssl, (v) => (webdavConfig.require_ssl = v))}
          {#if webdavMode === 'advanced'}
            <label class={field}><span class={fieldLabel}>Port</span><input class={inp} type="number" bind:value={webdavConfig.port} /></label>
            {@render toggleField($t('shares.locking'), webdavConfig.locking, (v) => (webdavConfig.locking = v))}
          {/if}
          <Button variant="primary" size="lg" onclick={saveWebdavConfig}>{$t('common.save')}</Button>
        </div>
      {/if}
    </div>

  {:else if activeTab === 'sftp'}
    <div data-testid="shares-config" class="flex flex-col gap-4 max-w-[640px]">
      {@render svcBar('sftp', 'SFTP')}
      {#if configLoading}<div data-testid="shares-loading" class="flex items-center justify-center py-12"><Spinner /></div>{:else}
        <div data-testid="shares-form" class="flex flex-col gap-4 p-5 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)]">
          <p class="m-0 text-c-base text-[var(--hv-shell-text)] leading-relaxed">{$t('shares.sftp_hint')}</p>
          <label class={field}><span class={fieldLabel}>{$t('shares.sftp_port')}</span><input class={inp} type="number" bind:value={sftpConfig.port} /></label>
          <Button variant="primary" size="lg" onclick={saveSftpConfig}>{$t('common.save')}</Button>
        </div>
      {/if}

      <div data-testid="shares-form" class="flex flex-col gap-4 p-5 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)]">
        <div data-testid="shares-form-head" class="flex items-center justify-between gap-3 flex-wrap">
          <h3 class="m-0 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">{$t('shares.sftp_users_title')}</h3>
          <Button variant="ghost" onclick={handleRefreshSftpAccess} disabled={sftpRefreshing}>
            <IconRefresh size={15} /> {sftpRefreshing ? $t('shares.syncing') : $t('shares.sftp_refresh_button')}
          </Button>
        </div>
        <p class="m-0 text-c-base text-[var(--hv-shell-text)] leading-relaxed">{$t('shares.sftp_users_desc')}</p>

        <label class={field}><span class={fieldLabel}>{$t('shares.sftp_username_placeholder')}</span>
          <input class={inp} bind:value={newSftpUsername} placeholder={$t('shares.sftp_username_placeholder')} />
        </label>
        <label class={field}><span class={fieldLabel}>{$t('shares.sftp_key_placeholder')}</span>
          <textarea class={cn(inp, 'h-auto min-h-[64px] py-3 font-[family-name:var(--hv-font-mono)] text-c-base resize-y')} bind:value={newSftpKey} rows="2" placeholder="ssh-ed25519 AAAA..."></textarea>
        </label>
        <Button variant="primary" size="lg" onclick={handleAddSftpUser}>{$t('shares.sftp_add_user_button')}</Button>

        {#if sftpGranteesLoading}
          <div data-testid="shares-loading" class="flex items-center justify-center py-8"><Spinner /></div>
        {:else if sftpGrantees.length === 0}
          <p class="m-0 text-c-base text-[var(--hv-shell-text)] py-4 text-center">{$t('shares.sftp_no_users')}</p>
        {:else}
          <div class="flex flex-col gap-3">
            {#each sftpGrantees as grantee (grantee.username)}
              <div class="flex flex-col gap-2.5 p-3 rounded-2xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]">
                <div class="flex items-center gap-2.5">
                  <span class="text-[var(--hv-shell-text)] flex-none flex"><IconUser size={16} /></span>
                  <span class="flex-1 font-semibold text-c-card-title text-[var(--hv-shell-text-strong)]">{grantee.username}</span>
                  {#if grantee.has_key}<span class="text-c-xs text-[var(--hv-success)] flex-none">{ $t('shares.sftp_key_set')}</span>{/if}
                </div>
                <div class="text-c-label text-[var(--hv-shell-text)]">{grantee.shares.length > 0 ? grantee.shares.join(', ') : $t('shares.sftp_no_shares_yet')}</div>
                <textarea
                  class={cn(inp, 'h-auto min-h-[64px] py-3 font-[family-name:var(--hv-font-mono)] text-c-base resize-y')}
                  rows="2"
                  placeholder={$t('shares.sftp_key_placeholder')}
                  bind:value={sftpKeyDrafts[grantee.username]}
                ></textarea>
                <button data-testid="shares-btn" class={cn(btn, 'self-start')} onclick={() => handleSaveSftpKey(grantee.username)}>{$t('common.save')}</button>
              </div>
            {/each}
          </div>
        {/if}
      </div>
    </div>

  {:else if activeTab === 'rsync'}
    <div data-testid="shares-rsync" class="flex flex-col gap-4 max-w-[760px]">
      {#if rsyncLoading}
        <div data-testid="shares-loading" class="flex items-center justify-center py-12"><Spinner /></div>
      {:else if rsync}
        {#if rsyncError}
          <Alert variant="error">{rsyncError}</Alert>
        {/if}

        {#if !rsync.binary_available}
          <Alert variant="warning">{$t('shares.rsync_missing_binary')}</Alert>
        {/if}

        <div data-testid="shares-rsync-settings" class="flex flex-col gap-4 p-5 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)]">
          <div>
            <h3 class="m-0 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">{$t('shares.rsync_title')}</h3>
            <p class="m-0 mt-1 text-c-base text-[var(--hv-shell-text)] leading-relaxed">{$t('shares.rsync_desc')}</p>
          </div>
          <div class="flex items-center justify-between gap-4 flex-wrap">
            <span class="text-c-base font-semibold text-[var(--hv-shell-text-strong)]">{$t('shares.rsync_enable')}</span>
            <Toggle bind:checked={rsync.settings.enabled} label={$t('shares.rsync_enable')} />
          </div>
          <div class="grid gap-3 sm:grid-cols-2">
            <label class={field}>
              <span class={fieldLabel}>{$t('shares.rsync_port')}</span>
              <input class={inp} type="number" min="1" max="65535" bind:value={rsync.settings.port} />
            </label>
            <label class={field}>
              <span class={fieldLabel}>{$t('shares.rsync_max_connections')}</span>
              <input class={inp} type="number" min="1" max={rsync.connection_limit} bind:value={rsync.settings.max_connections} />
            </label>
          </div>
          <p class="m-0 text-c-label text-[var(--hv-shell-text-2)]">
            {$t('shares.rsync_service_state', { state: rsync.service_active ? $t('shares.rsync_running') : $t('shares.rsync_stopped') })}
          </p>
          <Button variant="primary" size="lg" onclick={saveRsyncSettings} disabled={rsyncSaving}>
            {rsyncSaving ? $t('shares.syncing') : $t('common.save')}
          </Button>
          <p class="m-0 text-c-label text-[var(--hv-shell-text-2)]">{$t('shares.rsync_ssh_hint')}</p>
          <code class="text-c-label font-[family-name:var(--hv-font-mono)] break-all text-[var(--hv-shell-text-2)]">{rsync.ssh_path}</code>
        </div>

        <div data-testid="shares-rsync-modules" class="flex flex-col gap-3 p-5 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)]">
          <div class="flex items-center justify-between gap-3 flex-wrap">
            <h3 class="m-0 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">{$t('shares.rsync_modules_title')}</h3>
            <Button variant="primary" onclick={() => openRsyncModule()} disabled={rsync.modules.length >= rsync.module_limit}>
              <IconPlus size={16} stroke={2} /> {$t('shares.rsync_add_module')}
            </Button>
          </div>
          <p class="m-0 text-c-base text-[var(--hv-shell-text)] leading-relaxed">{$t('shares.rsync_modules_desc')}</p>

          {#if rsync.modules.length === 0}
            <EmptyState title={$t('shares.rsync_empty_title')} description={$t('shares.rsync_empty_desc')} />
          {:else}
            <div class="flex flex-col gap-3">
              {#each rsync.modules as module (module.name)}
                {@const moduleHolder = { get checked() { return module.enabled; }, set checked(v: boolean) { toggleRsyncModule(module, v); } }}
                <div data-testid="shares-rsync-module" class="flex flex-col gap-2 p-3 rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]">
                  <div class="flex items-center justify-between gap-3 flex-wrap">
                    <div class="flex flex-col min-w-0">
                      <span id={`shares-rsync-module-${module.name}`} class="font-semibold text-c-card-title text-[var(--hv-shell-text-strong)] truncate">{module.name}</span>
                      <code class="font-[family-name:var(--hv-font-mono)] text-c-label text-[var(--hv-shell-text-2)] truncate">{module.path}</code>
                      <span class="text-c-label text-[var(--hv-shell-text-2)]">
                        {module.users.length > 0 ? module.users.join(', ') : $t('shares.rsync_no_users')}
                        {module.read_only ? ` · ${$t('shares.rsync_read_only')}` : ''}
                      </span>
                    </div>
                    <div class="flex items-center gap-1">
                      <Toggle
                        bind:checked={moduleHolder.checked}
                        disabled={rsyncBusyName === module.name}
                        ariaLabelledby={`shares-rsync-module-${module.name}`}
                      />
                      <button type="button" class="grid place-items-center w-9 h-9 flex-none rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:bg-[var(--hv-subtle-hover)] hover:text-[var(--hv-shell-text-strong)] hv-ic" data-testid="shares-rsync-edit" aria-label={$t('tasks.edit', { name: module.name })} onclick={() => openRsyncModule(module)}>
                        <IconUser size={18} stroke={1.8} />
                      </button>
                      <button type="button" class="grid place-items-center w-9 h-9 flex-none rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:bg-[var(--hv-subtle-hover)] hover:text-[var(--hv-shell-text-strong)] hv-ic" data-testid="shares-rsync-delete" aria-label={$t('tasks.delete', { name: module.name })} disabled={rsyncBusyName === module.name} onclick={() => deleteRsyncModule(module)}>
                        <IconTrash size={18} stroke={1.8} />
                      </button>
                    </div>
                  </div>
                </div>
              {/each}
            </div>
          {/if}
        </div>

        <div data-testid="shares-rsync-passwords" class="flex flex-col gap-3 p-5 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)]">
          <h3 class="m-0 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">{$t('shares.rsync_passwords_title')}</h3>
          <p class="m-0 text-c-base text-[var(--hv-shell-text)] leading-relaxed">{$t('shares.rsync_passwords_desc')}</p>
          {#if rsync.available_users.length === 0}
            <p class="m-0 text-c-base text-[var(--hv-shell-text)]">{$t('shares.rsync_no_accounts')}</p>
          {:else}
            {#each rsync.available_users as user (user)}
              <div class="flex items-end gap-2 flex-wrap p-3 rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]">
                <div class="flex flex-col min-w-[140px]">
                  <span class="font-semibold text-c-card-title text-[var(--hv-shell-text-strong)]">{user}</span>
                  <span class="text-c-label text-[var(--hv-shell-text-2)]">
                    {rsync.users_with_secret.includes(user) ? $t('shares.rsync_password_set') : $t('shares.rsync_password_missing')}
                  </span>
                </div>
                <label class="flex flex-col gap-1 flex-1 min-w-[160px]">
                  <span class="text-c-label font-semibold text-[var(--hv-shell-text)]">{$t('shares.rsync_password')}</span>
                  <input
                    class={inp}
                    type="password"
                    autocomplete="new-password"
                    placeholder={$t('shares.rsync_password_placeholder')}
                    bind:value={rsyncSecretDrafts[user]}
                    data-testid="shares-rsync-password"
                  />
                </label>
                <Button variant="ghost" onclick={() => saveRsyncSecret(user)} disabled={rsyncSecretBusy === user || !(rsyncSecretDrafts[user] ?? '').trim()}>
                  {$t('common.save')}
                </Button>
              </div>
            {/each}
          {/if}
        </div>
      {:else}
        <Alert variant="error">{rsyncError || $t('shares.rsync_load_failed')}</Alert>
      {/if}
    </div>
  {/if}

  {#snippet footer()}
    <span data-testid="shares-footer" class="flex items-center gap-2 min-w-0 flex-wrap">
      <span>{totalShares} {$t('shares.stat_total')}</span>
      <span class="text-[var(--hv-shell-text-3)]">&middot;</span>
      <span>{smbShares} SMB</span>
      <span class="text-[var(--hv-shell-text-3)]">&middot;</span>
      <span>{nfsShares} NFS</span>
      <span class="text-[var(--hv-shell-text-3)]">&middot;</span>
      <span>{ftpShares} FTP</span>
      <span class="text-[var(--hv-shell-text-3)]">&middot;</span>
      <span>{webdavShares} WebDAV</span>
      <span class="text-[var(--hv-shell-text-3)]">&middot;</span>
      <span>{sftpShares} SFTP</span>
    </span>
  {/snippet}
  <ConfirmDialog
    {dlg}
    onconfirm={() => dlg.settle(true)}
    oncancel={() => dlg.settle(false)}
  />
</PageLayout>

{#snippet svcBar(svc: string, label: string)}
  <div data-testid="shares-svcbar" class="flex items-center gap-3 flex-wrap px-4 py-4 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)]">
    <span class="grid place-items-center w-[46px] h-[46px] flex-none rounded-lg text-white" style="background:{getProtocolColor(svc)}"><IconSettings size={20} stroke={1.9} /></span>
    <div class="flex-1 min-w-[120px]">
      <h2 class="m-0 mb-0.5 text-base font-semibold text-[var(--hv-shell-text-strong)]">{label}</h2>
      <span class:on={getServiceRunning(svc)} class={cn('inline-flex items-center gap-2 text-c-label font-semibold', getServiceRunning(svc) ? 'text-[var(--hv-success)]' : 'text-[var(--hv-shell-text)]')}>
        <span class={cn('w-2 h-2 rounded-full', getServiceRunning(svc) ? 'bg-[var(--hv-success)]' : 'bg-[var(--hv-shell-text-3)]')}></span>
        {getServiceRunning(svc) ? $t('shares.active') : $t('shares.inactive')}
      </span>
    </div>
    <div class="flex gap-2.5">
      {#if getServiceRunning(svc)}
        <button data-testid="shares-btn" class={btn} onclick={() => stopService(svc)}><IconSquare size={15} /> {$t('shares.stop')}</button>
        <button data-testid="shares-icon-btn" class={iconBtn} onclick={() => restartService(svc)} title={$t('shares.restart')} aria-label={$t('shares.restart')}><IconRefresh size={17} /></button>
      {:else}
        <Button variant="primary" onclick={() => startService(svc)}><IconPlayerPlay size={15} /> {$t('shares.start')}</Button>
      {/if}
    </div>
  </div>
{/snippet}

{#snippet modeSwitch(mode: 'simple' | 'advanced', set: (m: 'simple' | 'advanced') => void)}
  <div data-testid="shares-mode" class="flex p-1.5 gap-1.5 rounded-xl bg-[var(--hv-shell-border-soft)]">
    <button data-testid="shares-mode-btn" class:active={mode === 'simple'} class={cn(segBtn, mode === 'simple' && 'bg-[var(--hv-primary-light)] text-[var(--hv-shell-text-strong)]')} onclick={() => set('simple')}>{$t('shares.mode_simple')}</button>
    <button data-testid="shares-mode-btn" class:active={mode === 'advanced'} class={cn(segBtn, mode === 'advanced' && 'bg-[var(--hv-primary-light)] text-[var(--hv-shell-text-strong)]')} onclick={() => set('advanced')}>{$t('shares.mode_advanced')}</button>
  </div>
{/snippet}

{#snippet toggleField(label: string, value: boolean, set: (v: boolean) => void)}
  {@const holder = { get checked() { return value; }, set checked(v) { set(v); } }}
  <Toggle bind:checked={holder.checked} label={label} />
{/snippet}

<!-- Create share -->
<Modal open={showCreateModal} title={$t('shares.modal_title')} size="md" onclose={closeCreate}>
  {#snippet icon()}
    <IconPlus size={22} stroke={2} />
  {/snippet}
  {#snippet children()}
    <div class="flex flex-col gap-4">
      <label data-testid="shares-field" class={field}><span class={fieldLabel}>{$t('shares.name_label')} <span data-testid="shares-req" class="text-[var(--hv-error)]">*</span></span><input data-testid="shares-name-input" class={inp} bind:value={newShare.name} placeholder="mon-partage" /></label>
      <div data-testid="shares-field" class={field}><span class={fieldLabel}>{$t('shares.path')} <span data-testid="shares-req" class="text-[var(--hv-error)]">*</span></span><PathField id="hv-shares-path" bind:value={newShare.path} mode="folder" placeholder="/mnt/donnees" /></div>
      <label data-testid="shares-field" class={field}><span class={fieldLabel}>{$t('shares.protocol')}</span><select class={inp} bind:value={newShare.share_type}><option value="smb">{$t('shares.opt_smb')}</option><option value="nfs">{$t('shares.opt_nfs')}</option><option value="ftp">{$t('shares.opt_ftp')}</option><option value="webdav">{$t('shares.opt_webdav')}</option><option value="sftp">{$t('shares.opt_sftp')}</option></select></label>
      <label data-testid="shares-field" class={field}><span class={fieldLabel}>{$t('shares.comment_label')}</span><input class={inp} bind:value={newShare.comment} placeholder={$t('shares.optional')} /></label>
    </div>
  {/snippet}
  {#snippet footer()}
    <Button variant="ghost" size="lg" onclick={closeCreate}>{$t('common.cancel')}</Button>
    <Button variant="primary" size="lg" onclick={handleCreateShare}>{$t('common.create')}</Button>
  {/snippet}
</Modal>

<!-- Rsync module -->
<Modal
  open={rsyncModuleModal}
  title={rsyncModuleEditing ? $t('shares.rsync_edit_module') : $t('shares.rsync_add_module')}
  size="md"
  onclose={() => (rsyncModuleModal = false)}
>
  {#snippet icon()}
    <IconGauge size={22} stroke={1.9} />
  {/snippet}
  {#snippet children()}
    <div class="flex flex-col gap-4">
      {#if !rsyncModuleEditing}
        <label class={field}>
          <span class={fieldLabel}>{$t('shares.rsync_module_name')}</span>
          <input class={inp} bind:value={rsyncModuleForm.name} placeholder="media" data-testid="shares-rsync-module-name" />
        </label>
        <label class={field}>
          <span class={fieldLabel}>{$t('shares.rsync_module_share')}</span>
          <select class={inp} bind:value={rsyncModuleForm.path} data-testid="shares-rsync-module-path">
            <option value="">{$t('shares.rsync_pick_share')}</option>
            {#each rsync?.share_candidates ?? [] as candidate (candidate.path)}
              <option value={candidate.path} disabled={candidate.used}>
                {candidate.name} — {candidate.path}
              </option>
            {/each}
          </select>
        </label>
      {:else}
        <p class="m-0 text-c-base text-[var(--hv-shell-text)]">
          <strong>{rsyncModuleForm.name}</strong> — <code class="font-[family-name:var(--hv-font-mono)] text-c-label">{rsyncModuleForm.path}</code>
        </p>
      {/if}
      <label class={field}>
        <span class={fieldLabel}>{$t('shares.comment_label')}</span>
        <input class={inp} bind:value={rsyncModuleForm.comment} placeholder={$t('shares.optional')} />
      </label>
      <label class="flex items-center justify-between gap-4 flex-wrap">
        <span class="text-c-base font-semibold text-[var(--hv-shell-text-strong)]">{$t('shares.rsync_read_only')}</span>
        <Toggle bind:checked={rsyncModuleForm.read_only} label={$t('shares.rsync_read_only')} />
      </label>
      <label class="flex items-center justify-between gap-4 flex-wrap">
        <span class="text-c-base font-semibold text-[var(--hv-shell-text-strong)]">{$t('shares.rsync_module_enabled')}</span>
        <Toggle bind:checked={rsyncModuleForm.enabled} label={$t('shares.rsync_module_enabled')} />
      </label>
      <fieldset class="m-0 p-0 border-0 flex flex-col gap-2">
        <legend class="p-0 text-c-label font-semibold text-[var(--hv-shell-text)]">{$t('shares.rsync_module_users')}</legend>
        {#if (rsync?.available_users ?? []).length === 0}
          <p class="m-0 text-c-base text-[var(--hv-shell-text)]">{$t('shares.rsync_no_accounts')}</p>
        {:else}
          {#each rsync?.available_users ?? [] as user (user)}
            <label class="flex items-center gap-3 min-h-[44px] cursor-pointer">
              <input
                type="checkbox"
                class="h-4 w-4 flex-none"
                checked={rsyncModuleForm.users.includes(user)}
                onchange={() => toggleRsyncUser(user)}
                data-testid="shares-rsync-module-user"
              />
              <span class="text-c-base text-[var(--hv-shell-text-strong)]">{user}</span>
              {#if !(rsync?.users_with_secret ?? []).includes(user)}
                <span class="text-c-label text-[var(--hv-warning,#B45309)]">{$t('shares.rsync_no_password_yet')}</span>
              {/if}
            </label>
          {/each}
        {/if}
      </fieldset>
      {#if rsyncError}
        <Alert variant="error">{rsyncError}</Alert>
      {/if}
    </div>
  {/snippet}
  {#snippet footer()}
    <Button variant="ghost" size="lg" onclick={() => (rsyncModuleModal = false)}>{$t('common.cancel')}</Button>
    <Button variant="primary" size="lg" onclick={saveRsyncModule} disabled={rsyncSaving}>
      {rsyncSaving ? $t('shares.syncing') : $t('common.save')}
    </Button>
  {/snippet}
</Modal>

<!-- Permissions -->
<Modal open={showPermissionsModal} title={$t('shares.permissions_title', { name: permissionsShare?.name ?? '' })} size="md" onclose={closePermissions}>
  {#snippet icon()}
    <IconShield size={22} stroke={1.9} />
  {/snippet}
  {#snippet children()}
    {#if permissionsLoading}
      <div class="flex items-center justify-center py-8"><Spinner /></div>
    {:else}
      {#if sharePermissions.length === 0}
        <p class="m-0 text-c-base text-[var(--hv-shell-text)] py-4 text-center">{$t('shares.no_permissions')}</p>
      {:else}
        <div class="flex flex-col gap-2.5">
          {#each sharePermissions as perm (perm.id)}
            <div class="flex items-center gap-2.5 px-3 py-3 rounded-lg bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] min-h-[56px]">
              <span class="text-[var(--hv-shell-text)] flex flex-none">{#if perm.grantee_type === 'group'}<IconUsers size={16} />{:else}<IconUser size={16} />{/if}</span>
              <span class="flex-1 min-w-[60px] font-semibold text-c-md text-[var(--hv-shell-text-strong)] overflow-hidden text-ellipsis">{perm.grantee_name}</span>
              <span class="bg-[var(--hv-shell-border)] text-c-xs font-bold px-2 py-1 rounded-md uppercase flex-none text-[var(--hv-shell-text-2)]">{perm.permission}</span>
              {#if perm.recursive}<span class="text-c-xs text-[var(--hv-shell-text)] flex-none">{$t('shares.recursive_label')}</span>{/if}
              <button data-testid="shares-icon-btn" class={cn('grid place-items-center w-11 h-11 flex-none rounded-lg text-[var(--hv-shell-text-strong)] cursor-pointer transition-colors', 'hover:bg-[var(--hv-error-bg)] hover:text-[var(--hv-error)]')} onclick={() => handleRevokePermission(perm)} title={$t('shares.revoke_button')} aria-label={$t('shares.revoke_button')}><IconTrash size={16} /></button>
            </div>
          {/each}
        </div>
      {/if}

      <div data-testid="shares-grant" class="flex flex-col gap-3 mt-4 pt-4 border-t border-[var(--hv-shell-border-soft)]">
        <div class="text-sm font-semibold text-[var(--hv-shell-text-strong)]">{$t('shares.grant_button')}</div>
        <div class="flex gap-1.5 p-1.5 rounded-lg bg-[var(--hv-shell-border-soft)]">
          <button class:active={newGrant.grantee_type === 'user'} class={cn(segBtn, newGrant.grantee_type === 'user' && 'bg-[var(--hv-primary-light)] text-[var(--hv-shell-text-strong)]')} onclick={() => (newGrant.grantee_type = 'user')}>{$t('shares.grantee_type_user')}</button>
          <button class:active={newGrant.grantee_type === 'group'} class={cn(segBtn, newGrant.grantee_type === 'group' && 'bg-[var(--hv-primary-light)] text-[var(--hv-shell-text-strong)]')} onclick={() => (newGrant.grantee_type = 'group')}>{$t('shares.grantee_type_group')}</button>
        </div>
        <input class={inp} bind:value={newGrant.grantee_name} placeholder={$t('shares.grantee_name_placeholder')} />
        <div class="flex gap-1.5 p-1.5 rounded-lg bg-[var(--hv-shell-border-soft)]">
          <button class:active={newGrant.permission === 'r'} class={cn(segBtn, newGrant.permission === 'r' && 'bg-[var(--hv-primary-light)] text-[var(--hv-shell-text-strong)]')} onclick={() => (newGrant.permission = 'r')}>{$t('shares.permission_r')}</button>
          <button class:active={newGrant.permission === 'rw'} class={cn(segBtn, newGrant.permission === 'rw' && 'bg-[var(--hv-primary-light)] text-[var(--hv-shell-text-strong)]')} onclick={() => (newGrant.permission = 'rw')}>{$t('shares.permission_rw')}</button>
          <button class:active={newGrant.permission === 'rwx'} class={cn(segBtn, newGrant.permission === 'rwx' && 'bg-[var(--hv-primary-light)] text-[var(--hv-shell-text-strong)]')} onclick={() => (newGrant.permission = 'rwx')}>{$t('shares.permission_rwx')}</button>
        </div>
        {@render toggleField($t('shares.recursive_label'), newGrant.recursive, (v) => (newGrant.recursive = v))}
        <Button variant="primary" size="lg" onclick={handleGrantPermission}>{$t('shares.grant_button')}</Button>
      </div>

      <div class="flex flex-col gap-3 items-start mt-4 pt-4 border-t border-[var(--hv-shell-border-soft)]">
        <p class="m-0 text-c-base text-[var(--hv-shell-text)] leading-relaxed">{$t('shares.sync_desc')}</p>
        <Button variant="ghost" onclick={handleSyncPermissions} disabled={permissionsSyncing}>
          <IconRefresh size={15} /> {permissionsSyncing ? $t('shares.syncing') : $t('shares.sync_button')}
        </Button>
      </div>
    {/if}
  {/snippet}
</Modal>