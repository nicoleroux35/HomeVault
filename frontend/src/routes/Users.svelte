<!-- ========================================================================
     File          : Users.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 13/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
     The Users management app: user list with avatars/roles/groups, create
     and edit (profile, password, role, home folder + quota), lock/unlock,
     force-2FA-disable, delete (optionally wiping the home folder), and the
     per-member resource-access editor. Admin-only.

     Dependencies:
       - svelte (>= 5), theme + app/ composites, stores/i18n,
         $lib/services/api, utils/autoRevalidate

     Usage:
     Opened as a desktop window app (admin only).
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { api } from '$lib/services/api';
  import { onAppVisible } from '$lib/utils/autoRevalidate';
  import { t, translate, languageStore } from '$lib/stores/i18n';
  import { isAdmin, authStore } from '$lib/stores/auth';
  import { Modal, Button, Spinner } from '$lib/components/theme';
  import { PageLayout, SectionCard, ConfirmDialog, primaryBtn, ghostBtn, tileBtn, EmptyState } from '$lib/components/app';
  import { createConfirmState } from '$lib/utils/confirmDialog.svelte';
  import { avatarColorFor } from '$lib/utils/avatars';
  import { cn } from '$lib/utils.js';
  import WindowHeader from '$lib/components/desktop/WindowHeader.svelte';
  import FolderPicker from '$lib/components/theme/FolderPicker.svelte';
  import MemberAccessCard from '$lib/components/users/MemberAccessCard.svelte';
  import type { AvailableHomeDisk } from '$lib/types/api';
  import { IconSearch, IconPlus, IconMinus, IconLock, IconLockOpen, IconUsers, IconFolder, IconFolderOpen, IconKey, IconTrash, IconCheck, IconX, IconShield, IconShieldOff, IconGauge, IconArrowLeft, IconEye, IconEyeOff } from '@tabler/icons-svelte';

  interface User {
    username: string;
    fullname: string;
    email: string;
    role: string;
    role_id: number;
    locked: boolean;
    created_at: string;
    groups: string[];
    home_enabled: boolean;
    home_disk: string | null;
    home_path: string | null;
    home_size_bytes: number | null;
    two_fa_enabled: boolean;
  }
  interface Group { name: string; display_name: string; }

  type Filter = 'all' | 'active' | 'locked' | 'admin';
  type QuotaUnit = 'MB' | 'TB' | 'PB';

  const QUOTA_UNIT_BYTES: Record<QuotaUnit, number> = {
    MB: 1024 * 1024,
    TB: 1024 * 1024 * 1024 * 1024,
    PB: 1024 * 1024 * 1024 * 1024 * 1024,
  };

  const ROLE_IDS: Record<string, number> = { user: 1, admin: 2 };
  const roleId = (role: string) => ROLE_IDS[role] ?? 1;

  // Home-directory size slider (only offered for role "user").
  const GIB = 1024 * 1024 * 1024;
  const HOME_SIZE_MIN = 1 * GIB;
  const HOME_SIZE_DEFAULT = 10 * GIB;
  const HOME_SIZE_STEP = GIB / 2; // 512 MiB steps

  // Free space on the disk currently chosen for the home folder.
  function homeDiskAvailable(mountPoint: string): number {
    return availableDisks.find((d) => d.mount_point === mountPoint)?.available_bytes ?? 0;
  }
  // Keep a slider value within [min(1 GiB, free) … free].
  function clampHomeSize(bytes: number, available: number): number {
    const hi = Math.max(0, available);
    const lo = Math.min(HOME_SIZE_MIN, hi);
    return Math.min(Math.max(bytes, lo), hi);
  }

  // The real OS admin/sudo group is managed exclusively through the Role
  // selector (which syncs it automatically). It is kept out of the editable
  // `groups` list — `toggleGroupSel` / `saveGroups` / `createForm.groups` must
  // never touch it — but IS shown as a read-only, role-derived chip so an
  // admin can see who is in it. It stays impossible to grant/revoke by hand.
  const PROTECTED_GROUP = 'homevault_admin';
  // Captured from the group list for the read-only display chip.
  let adminGroup: Group | null = null;

  let users: User[] = [];
  let groups: Group[] = [];
  let availableDisks: AvailableHomeDisk[] = [];
  let loading = true;
  let loadingGroups = false;
  let groupsError = false;
  let loadingDisks = false;

  let search = '';
  let currentFilter: Filter = 'all';

  let selectedUser: User | null = null;
  let creating = false;
  let submitting = false;
  let errors: Record<string, string> = {};

  // ---------- Create form ----------
  let createForm = blankCreate();
  let createPasswordShown = false;

  // ---------- Detail forms (hydrated on select) ----------
  let editForm = { fullname: '', email: '', role: 'user' };
  let groupsSelected: string[] = [];
  let homeForm = {
    home_enabled: false,
    home_disk: '',
    home_path: '',
    home_size_bytes: HOME_SIZE_DEFAULT,
    home_size_unlimited: false,
  };
  // Folder-picker modal state (detail + create home cards).
  let homePickerOpen = false;
  let createHomePickerOpen = false;
  let passwordValue = '';
  let passwordShown = false;
  let deleteHomeFolder = false;
  let quotaForm = {
    uploadEnabled: false, uploadValue: 0, uploadUnit: 'MB' as QuotaUnit,
    downloadEnabled: false, downloadValue: 0, downloadUnit: 'MB' as QuotaUnit,
  };
  let quotaUsage = { uploadUsedBytes: 0, downloadUsedBytes: 0 };
  let loadingQuota = false;

  // "All quotas" overview — reads the existing, already-working
  // GET /api/transfer-quotas/{subject_type} list endpoint to show every
  // configured user AND group quota in one place.
  interface QuotaOverviewRow {
    subjectType: 'user' | 'group';
    subjectName: string;
    uploadEnabled: boolean;
    uploadLimitBytes: number;
    uploadUsedBytes: number;
    downloadEnabled: boolean;
    downloadLimitBytes: number;
    downloadUsedBytes: number;
  }
  let showQuotaOverview = false;
  let loadingQuotaOverview = false;
  let quotaOverviewError = false;
  let quotaOverviewRows: QuotaOverviewRow[] = [];
  const dlg = createConfirmState();

  function blankCreate() {
    return {
      username: '', password: '', fullname: '', email: '',
      role: 'user', groups: [] as string[], home_enabled: false, home_disk: '',
      home_path: '',
      home_size_bytes: HOME_SIZE_DEFAULT, home_size_unlimited: false,
    };
  }

  // Called when a disk is picked for the home folder: seed the slider with a
  // sensible default that fits the chosen disk. Picking a disk and picking an
  // explicit folder are mutually exclusive — choosing one clears the other.
  function pickCreateDisk(mountPoint: string) {
    createForm.home_disk = mountPoint;
    createForm.home_path = '';
    createForm.home_size_bytes = clampHomeSize(HOME_SIZE_DEFAULT, homeDiskAvailable(mountPoint));
  }
  function pickHomeDisk(mountPoint: string) {
    homeForm.home_disk = mountPoint;
    homeForm.home_path = '';
    homeForm.home_size_bytes = clampHomeSize(
      homeForm.home_size_bytes || HOME_SIZE_DEFAULT,
      homeDiskAvailable(mountPoint),
    );
  }
  // The mounted home disk whose mount point is the longest prefix of `path`
  // (used only client-side, for the size slider's free-space math + to show
  // which disk a picked folder lands on). The server does its own resolution.
  function diskContaining(path: string): AvailableHomeDisk | undefined {
    return availableDisks
      .filter((d) => path === d.mount_point || path.startsWith(d.mount_point.replace(/\/+$/, '') + '/'))
      .sort((a, b) => b.mount_point.length - a.mount_point.length)[0];
  }
  // Called when an explicit folder is picked via the FolderPicker: it becomes
  // the home directory; `home_disk` is set to the containing mount so the size
  // slider has a free-space figure (the server re-derives the real disk).
  function pickCreateHomeFolder(path: string) {
    createForm.home_path = path;
    const d = diskContaining(path);
    createForm.home_disk = d?.mount_point ?? '';
    createForm.home_size_bytes = clampHomeSize(HOME_SIZE_DEFAULT, d?.available_bytes ?? 0);
    createHomePickerOpen = false;
  }
  function pickHomeFolder(path: string) {
    homeForm.home_path = path;
    const d = diskContaining(path);
    homeForm.home_disk = d?.mount_point ?? '';
    homeForm.home_size_bytes = clampHomeSize(homeForm.home_size_bytes || HOME_SIZE_DEFAULT, d?.available_bytes ?? 0);
    homePickerOpen = false;
  }

  onMount(() => {
    if (!$isAdmin) {
      window.location.href = '/desktop';
      return;
    }
    Promise.all([loadUsers(), loadGroups(), loadDisks()]);
    // No manual refresh button — revalidate when the window regains attention.
    const teardownRefresh = onAppVisible(refreshAll);

    // Master-detail: `narrow` measures the content column itself (the window
    // floats, so a viewport media query would lie on a small floating window).
    // Below 760px only one pane shows — the rail, or the detail when a user is
    // selected / a create is in progress. In window-width terms that is ~1065px:
    // the rail (256px + 1px border) and the <main> padding (p-6, 48px) are both
    // taken out before the content column is measured.
    const ro = new ResizeObserver((entries) => {
      narrow = (entries[0]?.contentRect.width ?? 0) < 760;
    });
    if (splitEl) ro.observe(splitEl);

    return () => {
      teardownRefresh();
      ro.disconnect();
    };
  });

  // ---------- Loading ----------
  async function loadUsers() {
    loading = true;
    const res = await api.get('/users');
    if (res.success && res.data) users = res.data;
    loading = false;
  }
  async function loadGroups() {
    loadingGroups = true;
    groupsError = false;
    const res = await api.getGroups();
    loadingGroups = false;
    if (res.success && res.data) {
      // The real OS admin/sudo group is managed only through the Role selector
      // and is never an assignable checkbox — it is split out of `groups` here
      // (so an instance whose only group is `homevault_admin` shows an empty
      // editable list) and kept in `adminGroup` for the read-only chip.
      const all = res.data as Group[];
      adminGroup = all.find((g) => g.name === PROTECTED_GROUP) ?? null;
      groups = all.filter((g) => g.name !== PROTECTED_GROUP);
    } else {
      // A failed fetch must not look like "no groups exist" — flag it so the
      // UI can say so and offer a retry.
      groups = [];
      groupsError = true;
    }
  }
  async function loadDisks() {
    loadingDisks = true;
    const res = await api.getAvailableHomeDisks();
    loadingDisks = false;
    availableDisks = res.success && res.data ? res.data : [];
  }

  async function refreshAll() {
    await Promise.all([loadUsers(), loadGroups(), loadDisks()]);
    if (selectedUser) {
      const again = users.find((u) => u.username === selectedUser!.username) || null;
      if (again) hydrateDetail(again);
      else selectedUser = null;
    }
  }

  // ---------- Helpers ----------
  function initials(u: User): string {
    const n = (u.fullname || u.username).trim();
    const p = n.split(/\s+/);
    return (p.length >= 2 ? p[0][0] + p[1][0] : n.slice(0, 2)).toUpperCase();
  }
  function colorFor(u: User): string {
    return avatarColorFor(u.username);
  }
  // Locale-aware byte formatting — units and decimal separator follow the
  // active UI language via Intl (never hard-coded "Go"/"Mo").
  const BYTE_UNITS = ['byte', 'kilobyte', 'megabyte', 'gigabyte', 'terabyte'];
  function formatBytes(bytes: number): string {
    if (!bytes) return '—';
    let i = 0;
    let value = bytes;
    while (value >= 1024 && i < BYTE_UNITS.length - 1) { value /= 1024; i++; }
    try {
      return new Intl.NumberFormat(get(languageStore), {
        style: 'unit', unit: BYTE_UNITS[i], unitDisplay: 'short',
        maximumFractionDigits: 1,
      }).format(value);
    } catch {
      return `${value.toFixed(1)} ${BYTE_UNITS[i]}`;
    }
  }
  // Cryptographically random, ambiguity-free password for the Generate button.
  function generatePassword(): string {
    const abc = 'abcdefghjkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789@#%+=';
    const arr = new Uint32Array(18);
    crypto.getRandomValues(arr);
    let out = '';
    for (const n of arr) out += abc[n % abc.length];
    return out;
  }

  // Largest whole unit that represents `bytes` cleanly, so a saved quota
  // reopens showing e.g. "5 TB" rather than "5242880 MB".
  function bytesToQuotaValue(bytes: number): { value: number; unit: QuotaUnit } {
    if (bytes > 0 && bytes % QUOTA_UNIT_BYTES.PB === 0) return { value: bytes / QUOTA_UNIT_BYTES.PB, unit: 'PB' };
    if (bytes > 0 && bytes % QUOTA_UNIT_BYTES.TB === 0) return { value: bytes / QUOTA_UNIT_BYTES.TB, unit: 'TB' };
    return { value: Math.round(bytes / QUOTA_UNIT_BYTES.MB), unit: 'MB' };
  }
  function quotaBarPct(usedBytes: number, value: number, unit: QuotaUnit): number {
    const limit = Math.max(0, Math.round(value)) * QUOTA_UNIT_BYTES[unit];
    if (limit <= 0) return 0;
    return Math.min(100, Math.round((usedBytes / limit) * 100));
  }
  // Allocated share of the disk the home folder lives on. Deliberately fed by
  // the same two numbers the readout below it shows (allocated vs free) —
  // there is no measured "home directory usage" figure to read, and inventing
  // one would desync the bar from the readout it sits under.
  function homeBarPct(bytes: number, available: number): number {
    if (available <= 0) return 0;
    return Math.min(100, Math.round((bytes / available) * 100));
  }

  $: filteredUsers = users.filter((u) => {
    if (currentFilter === 'active' && u.locked) return false;
    if (currentFilter === 'locked' && !u.locked) return false;
    if (currentFilter === 'admin' && u.role !== 'admin') return false;
    if (search) {
      const hay = `${u.username} ${u.fullname} ${u.email}`.toLowerCase();
      if (!hay.includes(search.toLowerCase())) return false;
    }
    return true;
  });

  $: counts = {
    all: users.length,
    active: users.filter((u) => !u.locked).length,
    locked: users.filter((u) => u.locked).length,
    admin: users.filter((u) => u.role === 'admin').length,
  };

  const FILTERS: { id: Filter; label: string }[] = [
    { id: 'all', label: 'users.tab_all' },
    { id: 'active', label: 'users.tab_active' },
    { id: 'locked', label: 'users.tab_locked' },
    { id: 'admin', label: 'users.tab_admins' },
  ];

  // On a narrow window the two panes stack; the detail pane takes over the
  // whole width whenever a user is selected or we're creating one.
  $: detailOpen = creating || !!selectedUser;

  // Narrow + a pane open → the rail unmounts entirely (PageLayout's
  // `sidebarHidden` → WindowSidebar's `{#if !hidden}`), so exactly one view is
  // in the DOM at a time. Unmounting — not hiding — is what lets an E2E test
  // assert `users-grid` has a count of 0 while the detail is open.
  $: sidebarHidden = narrow && detailOpen;

  // Master-detail split: `narrow` measures the *content column* (the window is
  // a floating window, so a viewport media query would lie). The measured
  // element is always mounted, and its width is a strict function of the
  // window width (rail width is fixed), so the flag can flip back on a resize.
  let splitEl: HTMLDivElement;
  let narrow = false;

  /**
   * Hand off to the read-only Quotas app, which visualises the same transfer
   * quotas this pane edits. Shared `hv-open-app` window event — the same
   * convention Explorer uses to open Shares; `Desktop.svelte` owns `openApp()`
   * (and therefore the icon tile / colour / focus-dedupe), so no app-opening
   * logic is duplicated here.
   */
  function openQuotasApp() {
    window.dispatchEvent(new CustomEvent('hv-open-app', { detail: { id: 'quotas' } }));
  }

  // ---------- Selection ----------
  function hydrateDetail(u: User) {
    selectedUser = u;
    creating = false;
    errors = {};
    editForm = { fullname: u.fullname, email: u.email, role: u.role };
    groupsSelected = [...u.groups];
    // A null `home_disk` with a `home_path` means the admin picked an explicit
    // folder (see apply_home_folder) — reopen it in the picker and re-derive the
    // containing disk locally so the size slider still has a free-space figure.
    const customFolder = u.home_enabled && !!u.home_path && !u.home_disk ? u.home_path : '';
    homeForm = {
      home_enabled: u.home_enabled,
      home_disk: u.home_disk || (customFolder ? diskContaining(customFolder)?.mount_point ?? '' : ''),
      home_path: customFolder,
      home_size_bytes: u.home_size_bytes && u.home_size_bytes > 0 ? u.home_size_bytes : HOME_SIZE_DEFAULT,
      home_size_unlimited: u.home_enabled ? !(u.home_size_bytes && u.home_size_bytes > 0) : false,
    };
    passwordValue = '';
    passwordShown = false;
    deleteHomeFolder = false;
    loadQuota(u);
  }
  function selectUser(u: User) { hydrateDetail(u); }
  function closeDetail() { selectedUser = null; creating = false; errors = {}; }

  function startCreate() {
    createForm = blankCreate();
    createPasswordShown = false;
    errors = {};
    selectedUser = null;
    creating = true;
  }

  async function loadQuota(u: User) {
    loadingQuota = true;
    const res = await api.getTransferQuota('user', u.username);
    loadingQuota = false;
    if (res.success && res.data) {
      const up = bytesToQuotaValue(res.data.upload_limit_bytes);
      const down = bytesToQuotaValue(res.data.download_limit_bytes);
      quotaForm = {
        uploadEnabled: res.data.upload_enabled, uploadValue: up.value, uploadUnit: up.unit,
        downloadEnabled: res.data.download_enabled, downloadValue: down.value, downloadUnit: down.unit,
      };
      quotaUsage = { uploadUsedBytes: res.data.upload_used_bytes || 0, downloadUsedBytes: res.data.download_used_bytes || 0 };
    } else {
      quotaForm = { uploadEnabled: false, uploadValue: 0, uploadUnit: 'MB', downloadEnabled: false, downloadValue: 0, downloadUnit: 'MB' };
      quotaUsage = { uploadUsedBytes: 0, downloadUsedBytes: 0 };
    }
  }

  // ---------- Group selection ----------
  function toggleCreateGroup(name: string) {
    createForm.groups = createForm.groups.includes(name)
      ? createForm.groups.filter((g) => g !== name)
      : [...createForm.groups, name];
  }
  function toggleGroupSel(name: string) {
    groupsSelected = groupsSelected.includes(name)
      ? groupsSelected.filter((g) => g !== name)
      : [...groupsSelected, name];
  }

  // ---------- Create ----------
  async function handleCreate() {
    errors = {};
    if (!createForm.username.trim()) { errors = { username: translate('users.username_required2') }; return; }
    if (!createForm.password) { errors = { password: translate('users.password_required') }; return; }
    if (createForm.password.length < 8) { errors = { password: translate('users.password_min') }; return; }
    submitting = true;
    const res = await api.post('/users', {
      username: createForm.username.trim(),
      password: createForm.password,
      fullname: createForm.fullname,
      email: createForm.email,
      role_id: roleId(createForm.role),
      groups: createForm.groups,
      home_enabled: createForm.home_enabled,
      home_disk: createForm.home_enabled && !createForm.home_path && createForm.home_disk ? createForm.home_disk : null,
      home_path: createForm.home_enabled && createForm.home_path ? createForm.home_path : null,
      home_size_bytes:
        createForm.home_enabled && (createForm.home_disk || createForm.home_path) && !createForm.home_size_unlimited
          ? Math.round(createForm.home_size_bytes)
          : null,
    });
    submitting = false;
    if (res.success) {
      const uname = createForm.username.trim();
      creating = false;
      await loadUsers();
      const created = users.find((u) => u.username === uname);
      if (created) hydrateDetail(created);
    } else {
      errors = { action: res.message || res.error || translate('users.create_failed2') };
    }
  }

  // ---------- Detail actions ----------
  async function saveProfile() {
    if (!selectedUser) return;
    submitting = true;
    const roleChanged = editForm.role !== selectedUser.role;
    // Both calls go through dedicated api.ts wrappers (not the raw put())
    // so the `/users` list cache is actually invalidated — the generic
    // put()'s own invalidation only matches the exact per-user endpoint,
    // never the shorter list cache key, which otherwise left the list row
    // and the freshly re-hydrated detail pane showing stale data for up to
    // 5s after a real, successful save.
    const res = await api.updateUserFull(selectedUser.username, {
      fullname: editForm.fullname,
      email: editForm.email,
      home_enabled: selectedUser.home_enabled,
      home_disk: selectedUser.home_disk,
      // Carry the current home directory + limit through untouched — this form
      // only edits the profile; the folder picker and the home-size slider live
      // in the Home card and save via saveHome(). Omitting either here would
      // reset the home path / clear the quota.
      home_path: selectedUser.home_path,
      home_size_bytes: selectedUser.home_size_bytes,
    });
    if (res.success && roleChanged) {
      await api.updateUserRole(selectedUser.username, roleId(editForm.role));
    }
    submitting = false;
    if (res.success) { await refreshAll(); errors = { saved: translate('users.profile_saved') }; }
    else { errors = { action: res.message || res.error || translate('users.update_failed2') }; }
  }

  async function saveGroups() {
    if (!selectedUser) return;
    errors = {};
    submitting = true;
    const res = await api.updateUserGroups(selectedUser.username, groupsSelected);
    submitting = false;
    if (res.success) { await refreshAll(); errors = { saved: translate('users.groups_saved') }; }
    else { errors = { action: res.message || res.error || translate('users.groups_update_failed') }; }
  }

  /**
   * One Save for the "Role & Groups" card: the role segment and the group
   * chips are the same permission decision, so a single submit persists both.
   * The role change is skipped when untouched, which keeps this identical to
   * the old groups-only save on the common path (one PUT, "Groups saved").
   */
  async function saveRoleAndGroups() {
    if (!selectedUser) return;
    errors = {};
    submitting = true;
    if (editForm.role !== selectedUser.role) {
      await api.updateUserRole(selectedUser.username, roleId(editForm.role));
    }
    const res = await api.updateUserGroups(selectedUser.username, groupsSelected);
    submitting = false;
    if (res.success) { await refreshAll(); errors = { saved: translate('users.groups_saved') }; }
    else { errors = { action: res.message || res.error || translate('users.groups_update_failed') }; }
  }

  async function saveHome() {
    if (!selectedUser) return;
    submitting = true;
    const res = await api.setUserHome(
      selectedUser.username,
      homeForm.home_enabled,
      homeForm.home_enabled && !homeForm.home_path ? homeForm.home_disk : null,
      homeForm.home_enabled && (homeForm.home_disk || homeForm.home_path) && !homeForm.home_size_unlimited
        ? Math.round(homeForm.home_size_bytes)
        : null,
      homeForm.home_enabled && homeForm.home_path ? homeForm.home_path : null
    );
    submitting = false;
    if (res.success) { await refreshAll(); errors = { saved: translate('users.home_saved') }; }
    else { errors = { action: res.message || res.error || translate('users.home_update_failed') }; }
  }

  async function handleDeleteHome() {
    if (!selectedUser) return;
    if (!(await dlg.ask(translate('users.delete_home_confirm', { path: selectedUser.home_path || '' })))) return;
    const res = await api.deleteUserHome(selectedUser.username);
    if (res.success) { await refreshAll(); }
    else { errors = { action: res.message || res.error || translate('users.home_delete_failed') }; }
  }

  async function changePassword() {
    if (!selectedUser) return;
    errors = {};
    if (!passwordValue) { errors = { password: translate('users.password_required') }; return; }
    if (passwordValue.length < 8) { errors = { password: translate('users.password_min') }; return; }
    submitting = true;
    const isSelf = selectedUser.username === $authStore.user?.username;
    const res = await api.updateUserPassword(selectedUser.username, passwordValue);
    submitting = false;
    if (res.success) {
      passwordValue = '';
      if (isSelf) {
        // Changing your own password invalidates every session for this
        // account server-side, including the one used for this very request.
        await authStore.logout();
        window.history.pushState({}, '', '/login');
        window.dispatchEvent(new PopStateEvent('popstate'));
      } else {
        errors = { passwordOk: translate('users.password_updated') };
      }
    } else {
      errors = { password: res.message || res.error || translate('users.password_update_failed') };
    }
  }

  async function saveQuota() {
    if (!selectedUser) return;
    submitting = true;
    const res = await api.setTransferQuota('user', selectedUser.username, {
      upload_enabled: quotaForm.uploadEnabled,
      upload_limit_bytes: Math.max(0, Math.round(quotaForm.uploadValue)) * QUOTA_UNIT_BYTES[quotaForm.uploadUnit],
      download_enabled: quotaForm.downloadEnabled,
      download_limit_bytes: Math.max(0, Math.round(quotaForm.downloadValue)) * QUOTA_UNIT_BYTES[quotaForm.downloadUnit],
    });
    submitting = false;
    if (res.success) { errors = { quotaOk: translate('users.quota_updated') }; }
    else { errors = { action: res.message || res.error || translate('users.quota_update_failed') }; }
  }

  async function toggleLock() {
    if (!selectedUser) return;
    // api.setUserLocked() (not the raw put()) — see saveProfile()'s comment
    // above for why this matters: without it the lock/unlock badge never
    // visibly updated within the cache's 5s TTL.
    const res = await api.setUserLocked(selectedUser.username, !selectedUser.locked);
    if (res.success) { await refreshAll(); }
    else { errors = { action: res.message || res.error || translate('users.status_update_failed') }; }
  }

  async function handleDisable2FA() {
    if (!selectedUser) return;
    if (!(await dlg.ask(translate('users.disable_2fa_confirm', { username: selectedUser.username })))) return;
    const res = await api.adminDisable2FA(selectedUser.username);
    if (res.success) { await refreshAll(); }
    else { errors = { action: res.message || res.error || translate('users.2fa_disable_failed') }; }
  }

  async function handleDelete() {
    if (!selectedUser) return;
    if (!(await dlg.ask(translate('users.delete_confirm', { name: selectedUser.fullname || selectedUser.username })))) return;
    submitting = true;
    const res = await api.deleteUser(selectedUser.username, deleteHomeFolder);
    submitting = false;
    if (res.success) {
      selectedUser = null;
      await loadUsers();
    } else {
      errors = { action: res.message || res.error || translate('users.delete_failed2') };
    }
  }

  // ---------- All quotas overview ----------
  async function openQuotaOverview() {
    showQuotaOverview = true;
    loadingQuotaOverview = true;
    quotaOverviewError = false;
    quotaOverviewRows = [];
    try {
      const [userRes, groupRes] = await Promise.all([
        api.listTransferQuotas('user'),
        api.listTransferQuotas('group'),
      ]);
      if (!userRes.success || !groupRes.success) { quotaOverviewError = true; return; }
      const toRows = (subjectType: 'user' | 'group', data: any[]): QuotaOverviewRow[] =>
        (data || []).map((q) => ({
          subjectType,
          subjectName: q.subject_name,
          uploadEnabled: q.upload_enabled,
          uploadLimitBytes: q.upload_limit_bytes,
          uploadUsedBytes: q.upload_used_bytes,
          downloadEnabled: q.download_enabled,
          downloadLimitBytes: q.download_limit_bytes,
          downloadUsedBytes: q.download_used_bytes,
        }));
      quotaOverviewRows = [
        ...toRows('user', userRes.data),
        ...toRows('group', groupRes.data),
      ].sort((a, b) => a.subjectName.localeCompare(b.subjectName));
    } catch {
      quotaOverviewError = true;
    } finally {
      loadingQuotaOverview = false;
    }
  }
  function closeQuotaOverview() { showQuotaOverview = false; }

  // Shared shapes (appliance tokens only — the per-app CSS vocabulary is gone).
  const inp =
    'w-full box-border h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg ' +
    'bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-strong)] text-sm outline-none ' +
    'transition-colors focus:border-[var(--hv-primary)] disabled:opacity-60';
  const inpMono = cn(inp, 'font-[family-name:var(--hv-font-mono)]');
  const inpSm = cn(inp, 'h-10 text-c-base');
  const field = 'flex flex-col gap-1.5 min-w-0';
  const fieldLabel = 'text-c-label font-semibold text-[var(--hv-shell-text-strong)]';
  const switchTrack =
    'w-14 h-8 flex-none p-1 border-none rounded-full flex items-center transition-colors disabled:opacity-45 disabled:cursor-default';
  const switchKnob = 'w-6 h-6 rounded-full bg-white shadow-[0_2px_6px_rgba(6,9,30,0.4)]';
  // 46px tile variant of the shared `tileBtn` (rail header actions).
  const iconBtn = cn(
    tileBtn,
    'w-[46px] h-[46px] rounded-xl bg-[var(--hv-shell-fill)] disabled:opacity-40 disabled:cursor-default'
  );
  const chip =
    'inline-flex items-center gap-2 min-h-11 px-4 border border-[var(--hv-shell-border)] rounded-lg ' +
    'bg-[var(--hv-shell-fill)] text-[var(--hv-shell-text-strong)] text-c-base font-medium cursor-pointer ' +
    'transition-colors hover:bg-[var(--hv-shell-border-soft)]';
  const chipOn = 'border-[var(--hv-primary)]/55 bg-[var(--hv-primary-light)] font-semibold';
  const hint = 'text-c-label text-[var(--hv-shell-text)]';
  const errTxt = 'text-xs text-[var(--hv-error-text)]';
  const okTxt = 'text-xs text-[var(--hv-success)]';
  const settingIco =
    'flex-none w-[46px] h-[46px] rounded-xl grid place-items-center bg-[var(--hv-shell-fill)] text-[var(--hv-shell-text-2)]';
  const settingIcoPrimary = cn(settingIco, 'bg-[var(--hv-primary-light)] text-[var(--hv-primary)]');
  const settingIcoOk = cn(settingIco, 'bg-[var(--hv-success-bg)] text-[var(--hv-success-text)]');
  const settingIcoDanger = cn(settingIco, 'bg-[var(--hv-error-bg)] text-[var(--hv-error)]');
  const segBtn =
    'flex-1 h-10 border-none bg-transparent rounded-lg text-c-base font-semibold text-[var(--hv-shell-text-2)] ' +
    'inline-flex items-center justify-center gap-1.5 cursor-pointer transition-colors';

  // ── Master/detail chrome (rail + detail cards) — tokens only ──
  // Card header: icon + title, used as every SectionCard's `titleSnippet`.
  const cardTitle =
    'flex items-center gap-2 m-0 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]';
  // `primaryBtn` / `ghostBtn` come from components/app controlClasses.ts —
  // see docs/guides/design-system.md §4.3.
  // One rail row (master list). Selection / lock tinting is layered on top by
  // the row itself so the base shape stays identical for every state.
  const railRow =
    'flex w-full items-center gap-2.5 px-2.5 py-2.5 border border-transparent rounded-xl bg-[var(--hv-shell-card)] ' +
    'cursor-pointer text-left transition-colors';
  const filterBtn =
    'inline-flex items-center gap-1.5 h-8 px-2.5 border-none rounded-lg bg-transparent text-[var(--hv-shell-text-2)] ' +
    'text-c-label font-medium cursor-pointer transition-colors hover:bg-[var(--hv-shell-border-soft)]';
  // The group chips read as one full-width row each in the detail cards.
  const chipRow = cn(chip, 'w-full');
</script>

<!--
  Master/detail shell. The rail (PageLayout's `sidebar`) is the master list;
  everything to the right of it is the detail pane. `sidebarHidden` unmounts
  the rail on a narrow container with a pane open (PageLayout :122 →
  WindowSidebar's `{#if !hidden}`), so exactly one view is ever in the DOM.

  `users-split` is the ResizeObserver's measuring element and is deliberately
  ALWAYS mounted: its width is a strict function of the window width (the rail
  is a fixed width), which is what lets `narrow` flip back on a resize. Gating
  it on the pane state would freeze the flag the first time it went narrow.
-->
<PageLayout
  title={$t('users.heading')}
  data-testid="users-page"
  sidebarLabel={$t('users.list_label')}
  sidebarTestId="users-list"
  {sidebarHidden}
>
  {#snippet icon()}
    <IconUsers stroke={1.7} />
  {/snippet}

  {#snippet sidebarFooter()}
    <button
      type="button"
      data-testid="users-quota-view"
      class={cn(ghostBtn, 'w-full justify-center bg-transparent')}
      onclick={openQuotasApp}
    >
      <IconGauge size={16} stroke={1.9} aria-hidden="true" /> {$t('users.quota_view')}
    </button>
  {/snippet}

  <!-- ───────────── Rail: new-user action, search, filters, list ───────────── -->
  {#snippet sidebar()}
    <div class="flex flex-col gap-2.5 pb-3 border-b border-[var(--hv-shell-border-soft)]">
      <button type="button" data-testid="users-tile-new" class={cn(primaryBtn, 'w-full justify-center')} onclick={startCreate}>
        <IconPlus size={17} stroke={2} aria-hidden="true" /> {$t('users.new_user_btn')}
      </button>

      <div data-testid="users-search" class="flex items-center gap-2.5 h-10 px-3 rounded-lg bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] text-[var(--hv-shell-text)]">
        <IconSearch size={16} stroke={1.9} aria-hidden="true" />
        <input
          type="search"
          class="flex-1 min-w-0 border-none bg-transparent outline-none text-[var(--hv-shell-text-strong)] text-c-base placeholder:text-[var(--hv-shell-text-3)]"
          aria-label={$t('users.search_label')}
          placeholder={$t('users.search_placeholder')}
          bind:value={search}
          autocomplete="off"
        />
      </div>

      <div data-testid="users-filters" class="flex flex-wrap gap-1" role="group" aria-label={$t('users.filter_label')}>
        {#each FILTERS as f (f.id)}
          <button
            type="button"
            data-testid="users-filter"
            class:active={currentFilter === f.id}
            aria-pressed={currentFilter === f.id}
            onclick={() => (currentFilter = f.id)}
            class={cn(filterBtn, currentFilter === f.id && 'bg-[var(--hv-primary-selected-bg)] text-[var(--hv-primary)] font-semibold')}
          >
            {$t(f.label)}
            <span class="text-c-xs font-bold opacity-75">{counts[f.id]}</span>
          </button>
        {/each}
      </div>
    </div>

    <div class="flex flex-col gap-1 pt-3">
      <p class="m-0 pb-1.5 text-c-xs font-semibold uppercase tracking-wide text-[var(--hv-shell-text-3)]">
        {$t('users.list_count', { count: String(filteredUsers.length) })}
      </p>

      {#if loading}
        <div data-testid="users-empty" class="py-10 text-center text-[var(--hv-shell-text)]">
          <Spinner label={$t('users.loading')} />
        </div>
      {:else}
        {#if filteredUsers.length === 0}
          <EmptyState data-testid="users-empty" title={search || currentFilter !== 'all' ? $t('users.empty_filter') : $t('users.empty_none')}>
            {#snippet iconSnippet()}<IconUsers size={32} stroke={1.8} />{/snippet}
          </EmptyState>
        {/if}

        <!--
          Kept named `users-grid` for the E2E contract (41 assertions in
          users.spec.ts): it is now the rail's scrolling row list, not a tile
          grid. Renaming it is a spec change, deliberately out of scope.
        -->
        <div data-testid="users-grid" class="flex flex-col gap-1" aria-busy={loading}>
          {#each filteredUsers as u (u.username)}
            <button
              type="button"
              data-testid="users-tile"
              class:locked={u.locked}
              aria-current={(selectedUser as User | null)?.username === u.username && !creating ? 'true' : undefined}
              onclick={() => selectUser(u)}
              class={cn(
                railRow,
                (selectedUser as User | null)?.username === u.username && !creating
                  ? 'border-[var(--hv-primary)] bg-[var(--hv-primary-selected-bg)]'
                  : 'hover:bg-[var(--hv-primary-selected-bg-hover)]',
                u.locked && 'opacity-70',
              )}
            >
              <span class="grid h-9 w-9 flex-none place-items-center rounded-full text-c-base font-semibold text-white" style="background:{colorFor(u)}" aria-hidden="true">{initials(u)}</span>
              <span class="flex min-w-0 flex-1 flex-col">
                <span class="flex min-w-0 items-center gap-1.5">
                  <span class="truncate text-c-base font-semibold text-[var(--hv-shell-text-strong)]">{u.fullname || u.username}</span>
                  <span data-testid="users-badge" class:admin={u.role === 'admin'} class={cn('inline-flex items-center h-5 flex-none px-2 rounded-full bg-[var(--hv-shell-border)] text-c-xs font-bold text-[var(--hv-shell-text-2)]', u.role === 'admin' && 'bg-[var(--hv-primary-light)] text-[var(--hv-primary)]')}>{u.role === 'admin' ? $t('users.role_admin2') : $t('users.role_user2')}</span>
                </span>
                <span class="truncate font-[family-name:var(--hv-font-mono)] text-c-xs text-[var(--hv-shell-text)]">@{u.username}</span>
              </span>
              {#if u.locked}
                <span data-testid="users-state" class="locked inline-flex flex-none items-center gap-1 text-[var(--hv-warning)]">
                  <IconLock size={13} stroke={2} aria-hidden="true" /><span class="sr-only">{$t('users.locked_label')}</span>
                </span>
              {:else}
                <span data-testid="users-state" class="ok inline-flex flex-none items-center gap-1 text-[var(--hv-shell-text-2)]">
                  <span class="h-2 w-2 rounded-full bg-[var(--hv-success)]" aria-hidden="true"></span><span class="sr-only">{$t('shares.active')}</span>
                </span>
              {/if}
            </button>
          {/each}
        </div>
      {/if}
    </div>
  {/snippet}

  <div data-testid="users-split" bind:this={splitEl} class="flex flex-col flex-1 min-h-0 min-w-0">
    {#if !narrow || detailOpen}
      {#if creating}
        <!-- ───────────── Create form ───────────── -->
        <div data-testid="users-view" class="flex w-full flex-col gap-3">
          <div data-testid="users-detail-topbar" class="flex items-center flex-wrap gap-3">
            <button type="button" data-testid="users-icon-btn" class={iconBtn} onclick={closeDetail} aria-label={$t('common.cancel')}>
              <IconArrowLeft size={22} stroke={2} aria-hidden="true" />
            </button>
            <div class="min-w-0">
              <h2 data-testid="users-detail-h1" class="m-0 truncate text-c-stat font-semibold leading-tight tracking-tight text-[var(--hv-shell-text-strong)]">{$t('users.sheet_create')}</h2>
              <div class="mt-0.5 truncate text-c-base text-[var(--hv-shell-text)]">{$t('users.create_hint')}</div>
            </div>
          </div>

          <form class="flex flex-col gap-3" onsubmit={(e) => { e.preventDefault(); handleCreate(); }}>
            <SectionCard data-testid="users-setting-row" divider>
              {#snippet titleSnippet()}
                <h2 class={cardTitle}><IconUsers size={17} stroke={2} aria-hidden="true" /> {$t('users.identity')}</h2>
              {/snippet}
              <div class="grid grid-cols-1 @min-[768px]:grid-cols-2 gap-3">
                <div class={field}>
                  <label for="c-username" class={fieldLabel}>{$t('users.username_label')} <span class="req text-[var(--hv-error)]" aria-hidden="true">*</span><span class="sr-only">{$t('users.required_field')}</span></label>
                  <input id="c-username" class={cn(inpMono, errors.username && 'border-[var(--hv-error)]')} required aria-invalid={errors.username ? 'true' : undefined} aria-describedby={errors.username ? 'c-username-err' : undefined} bind:value={createForm.username} placeholder={$t('users.username_example')} autocomplete="off" />
                  {#if errors.username}<span id="c-username-err" data-testid="users-err" class={errTxt} role="alert">{errors.username}</span>{/if}
                </div>
                <div class={field}>
                  <label for="c-fullname" class={fieldLabel}>{$t('users.fullname_label')}</label>
                  <input id="c-fullname" class={inp} bind:value={createForm.fullname} placeholder={$t('users.fullname_example')} autocomplete="off" />
                </div>
                <div class={field}>
                  <label for="c-password" class={fieldLabel}>{$t('users.sheet_password')} <span class="req text-[var(--hv-error)]" aria-hidden="true">*</span><span class="sr-only">{$t('users.required_field')}</span></label>
                  <div class={cn('flex items-center gap-2 h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)]', errors.password && 'border-[var(--hv-error)]')}>
                    <input id="c-password" class="flex-1 min-w-0 border-none bg-transparent h-10 font-[family-name:var(--hv-font-mono)] text-sm text-[var(--hv-shell-text-strong)] outline-none" type={createPasswordShown ? 'text' : 'password'} required autocomplete="new-password" aria-invalid={errors.password ? 'true' : undefined} aria-describedby={errors.password ? 'c-password-err' : undefined} bind:value={createForm.password} placeholder={$t('users.password_min')} />
                    <button type="button" data-testid="users-inp-btn" class="ghost grid place-items-center w-7 h-8 p-0 border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:text-[var(--hv-shell-text-strong)]" onclick={() => (createPasswordShown = !createPasswordShown)} aria-pressed={createPasswordShown} aria-label={createPasswordShown ? $t('users.password_hide') : $t('users.password_show')}>
                      {#if createPasswordShown}<IconEyeOff size={16} stroke={2} aria-hidden="true" />{:else}<IconEye size={16} stroke={2} aria-hidden="true" />{/if}
                    </button>
                    <button type="button" data-testid="users-inp-btn" class={cn(primaryBtn, 'h-8 px-3 flex-none text-c-label')} onclick={() => { createForm.password = generatePassword(); createPasswordShown = true; }}>{$t('users.generate')}</button>
                  </div>
                  {#if errors.password}<span id="c-password-err" data-testid="users-err" class={errTxt} role="alert">{errors.password}</span>{/if}
                </div>
                <div class={field}>
                  <label for="c-email" class={fieldLabel}>{$t('users.email_label')}</label>
                  <input id="c-email" class={inp} type="email" bind:value={createForm.email} placeholder={$t('users.email_example')} autocomplete="off" />
                </div>
              </div>
            </SectionCard>

            <SectionCard data-testid="users-setting-row" divider>
              {#snippet titleSnippet()}
                <h2 class={cardTitle}><IconShield size={17} stroke={2} aria-hidden="true" /> {$t('users.role_label')}</h2>
              {/snippet}
              <fieldset class="m-0 min-w-0 border-0 p-0">
                <legend class="sr-only">{$t('users.role_label')}</legend>
                <div class="grid grid-cols-1 @min-[640px]:grid-cols-2 gap-3">
                  <button type="button" data-testid="users-role" class:active={createForm.role === 'user'} aria-pressed={createForm.role === 'user'} onclick={() => (createForm.role = 'user')} class={cn('flex flex-col items-start gap-1.5 px-4 py-4 border rounded-xl bg-[var(--hv-shell-fill)] text-left cursor-pointer transition-colors hover:bg-[var(--hv-shell-border-soft)]', createForm.role === 'user' ? 'border-[var(--hv-primary)]/55 bg-[var(--hv-primary-light)]' : 'border-[var(--hv-shell-border)]')}>
                    <span class="inline-flex items-center gap-2 text-sm font-semibold text-[var(--hv-shell-text-strong)]"><IconUsers size={15} stroke={2} aria-hidden="true" /> {$t('users.role_user2')}</span>
                    <span class="text-c-xs text-[var(--hv-shell-text-2)] leading-relaxed">{$t('users.role_user_desc')}</span>
                  </button>
                  <button type="button" data-testid="users-role" class:active={createForm.role === 'admin'} aria-pressed={createForm.role === 'admin'} onclick={() => (createForm.role = 'admin')} class={cn('flex flex-col items-start gap-1.5 px-4 py-4 border rounded-xl bg-[var(--hv-shell-fill)] text-left cursor-pointer transition-colors hover:bg-[var(--hv-shell-border-soft)]', createForm.role === 'admin' ? 'border-[var(--hv-primary)]/55 bg-[var(--hv-primary-light)]' : 'border-[var(--hv-shell-border)]')}>
                    <span class="inline-flex items-center gap-2 text-sm font-semibold text-[var(--hv-shell-text-strong)]"><IconShield size={15} stroke={2} aria-hidden="true" /> {$t('users.role_admin2')}</span>
                    <span class="text-c-xs text-[var(--hv-shell-text-2)] leading-relaxed">{$t('users.role_admin_desc')}</span>
                  </button>
                </div>
              </fieldset>
            </SectionCard>

            <SectionCard data-testid="users-setting-row" divider>
              {#snippet titleSnippet()}
                <h2 class={cardTitle}><IconUsers size={17} stroke={2} aria-hidden="true" /> {$t('users.groups_label')}</h2>
              {/snippet}
              {#if loadingGroups}
                <div class={hint}><Spinner size="sm" label={$t('users.loading_groups')} /></div>
              {:else if groupsError}
                <div class={hint} role="alert">
                  {$t('users.groups_load_error')}
                  <button type="button" data-testid="users-inp-btn" class={cn(primaryBtn, 'ml-2 h-8 px-3 text-c-label')} onclick={loadGroups}>{$t('users.retry')}</button>
                </div>
              {:else}
                <div data-testid="users-chips" class="flex flex-col gap-2" role="group" aria-label={$t('users.sheet_groups')}>
                  {#if adminGroup}
                    <button type="button" data-testid="users-chip" class:on={createForm.role === 'admin'} disabled aria-disabled="true" title={$t('users.admin_group_managed')} class={cn(chipRow, 'admin-locked cursor-not-allowed border-dashed bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-2)]', createForm.role === 'admin' && 'border-solid border-[var(--hv-primary)]/55 bg-[var(--hv-primary-light)]')}>
                      <IconShield size={15} stroke={2} aria-hidden="true" />
                      {adminGroup.display_name}
                      <span class="text-c-xs font-medium opacity-80">· {$t('users.admin_group_managed')}</span>
                    </button>
                  {/if}
                  {#each groups as g (g.name)}
                    <button
                      type="button"
                      data-testid="users-chip"
                      class:on={createForm.groups.includes(g.display_name)}
                      aria-pressed={createForm.groups.includes(g.display_name)}
                      onclick={() => toggleCreateGroup(g.display_name)}
                      class={cn(chipRow, createForm.groups.includes(g.display_name) && chipOn)}
                    >
                      {#if createForm.groups.includes(g.display_name)}<IconCheck size={15} stroke={2.5} aria-hidden="true" />{:else}<IconPlus size={15} stroke={2} aria-hidden="true" />{/if}
                      {g.display_name}
                    </button>
                  {/each}
                </div>
                {#if groups.length === 0}
                  <div class={cn(hint, 'mt-2.5')}>{$t('users.no_groups_available')} {$t('users.no_groups_hint')}</div>
                {/if}
              {/if}
            </SectionCard>

            <SectionCard data-testid="users-setting-row" divider>
              {#snippet titleSnippet()}
                <h2 class={cardTitle}><IconFolder size={17} stroke={1.8} aria-hidden="true" /> {$t('users.home_toggle_title')}</h2>
              {/snippet}
              {#snippet actions()}
                <button
                  type="button"
                  data-testid="users-switch"
                  class:on={createForm.home_enabled}
                  role="switch"
                  aria-checked={createForm.home_enabled}
                  aria-label={$t('users.home_toggle_title')}
                  onclick={() => (createForm.home_enabled = !createForm.home_enabled)}
                  class={cn(switchTrack, createForm.home_enabled ? 'bg-[var(--hv-primary)] justify-end' : 'bg-[var(--hv-shell-border)] justify-start')}
                ><span class={switchKnob}></span></button>
              {/snippet}
              <p class={hint}>{$t('users.home_toggle_desc')}</p>
              {#if createForm.home_enabled}
                <p class={cn(hint, 'mt-2.5')}>{$t('users.home_disk_or_folder')}</p>
                <div data-testid="users-disk-grid" class="flex flex-wrap gap-2.5 mt-3" role="group" aria-label={$t('users.select_disk')}>
                  {#if loadingDisks}
                    <div class={hint}><Spinner size="sm" label={$t('users.loading_disks')} /></div>
                  {:else if availableDisks.length === 0}
                    <div class={hint}>{$t('users.loading_disks')}</div>
                  {:else}
                    {#each availableDisks as d (d.mount_point)}
                      <button
                        type="button"
                        data-testid="users-disk"
                        class:active={!createForm.home_path && createForm.home_disk === d.mount_point}
                        aria-pressed={!createForm.home_path && createForm.home_disk === d.mount_point}
                        onclick={() => pickCreateDisk(d.mount_point)}
                        class={cn('flex-1 min-w-[150px] flex flex-col items-start gap-0.5 px-3 py-3 border rounded-xl bg-[var(--hv-shell-fill)] text-left cursor-pointer transition-colors hover:bg-[var(--hv-shell-border-soft)]', !createForm.home_path && createForm.home_disk === d.mount_point ? 'border-[var(--hv-primary)]/55 bg-[var(--hv-primary-light)]' : 'border-[var(--hv-shell-border)]')}
                      >
                        <span class="text-c-base font-semibold text-[var(--hv-shell-text-strong)]">{d.mount_point}</span>
                        <span class="text-c-xs text-[var(--hv-shell-text-2)] font-[family-name:var(--hv-font-mono)]">{$t('users.disk_free', { size: formatBytes(d.available_bytes) })} · {d.filesystem}</span>
                      </button>
                    {/each}
                  {/if}
                </div>
                <div class="flex flex-wrap items-center gap-2 mt-2.5">
                  <button type="button" data-testid="users-inp-btn" class={cn(ghostBtn, 'h-10')} onclick={() => (createHomePickerOpen = true)}>
                    <IconFolderOpen size={15} stroke={1.9} aria-hidden="true" /> {$t('users.home_pick_folder')}
                  </button>
                  {#if createForm.home_path}
                    <span class="flex-1 min-w-0 text-xs text-[var(--hv-shell-text-strong)] px-2.5 py-1.5 rounded-lg bg-[var(--hv-primary-light)] border border-[var(--hv-primary)]/30 overflow-hidden text-ellipsis whitespace-nowrap font-[family-name:var(--hv-font-mono)]" title={createForm.home_path}>{createForm.home_path}</span>
                    <button type="button" data-testid="users-inp-btn" class="grid place-items-center w-7 h-8 p-0 border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:text-[var(--hv-shell-text-strong)]" aria-label={$t('common.cancel')} onclick={() => { createForm.home_path = ''; }}>
                      <IconX size={15} stroke={2} aria-hidden="true" />
                    </button>
                  {/if}
                </div>
                {#if createForm.home_disk}
                  {@const avail = homeDiskAvailable(createForm.home_disk)}
                  <div data-testid="users-homesize" class="mt-3 p-3 border border-[var(--hv-shell-border)] rounded-xl bg-[var(--hv-shell-border-soft)] flex flex-col gap-2.5">
                    <div class="flex items-center justify-between gap-3 text-c-base font-semibold text-[var(--hv-shell-text-strong)]">
                      <span id="c-homesize-label">{$t('users.home_size_label')}</span>
                      <label data-testid="users-homesize-unlim" class="inline-flex items-center gap-2 text-c-label font-medium text-[var(--hv-shell-text-2)] cursor-pointer"><input type="checkbox" class="w-4 h-4 cursor-pointer" bind:checked={createForm.home_size_unlimited} />{$t('users.home_size_unlimited')}</label>
                    </div>
                    {#if createForm.home_size_unlimited}
                      <div class={hint}>{$t('users.home_size_unlimited_hint')}</div>
                    {:else}
                      <input
                        data-testid="users-homesize-range"
                        class="w-full accent-[var(--hv-primary)] cursor-pointer"
                        type="range"
                        min={Math.min(HOME_SIZE_MIN, avail)}
                        max={avail}
                        step={HOME_SIZE_STEP}
                        bind:value={createForm.home_size_bytes}
                        aria-labelledby="c-homesize-label"
                        aria-valuetext={formatBytes(createForm.home_size_bytes)}
                      />
                      <div class="h-2 rounded-full bg-[var(--hv-shell-border)] overflow-hidden" role="progressbar" aria-valuenow={homeBarPct(createForm.home_size_bytes, avail)} aria-valuemin="0" aria-valuemax="100">
                        <span class="block h-full rounded-full bg-[var(--hv-primary)]" style="width:{homeBarPct(createForm.home_size_bytes, avail)}%"></span>
                      </div>
                      <div data-testid="users-homesize-readout" class="flex items-baseline gap-2 text-c-base text-[var(--hv-shell-text-2)] font-[family-name:var(--hv-font-mono)]">
                        <strong class="text-c-card-title text-[var(--hv-shell-text-strong)]">{formatBytes(createForm.home_size_bytes)}</strong>
                        <span>{$t('users.home_size_of_free', { free: formatBytes(Math.max(0, avail - createForm.home_size_bytes)) })}</span>
                      </div>
                    {/if}
                  </div>
                {/if}
              {/if}
            </SectionCard>

            {#if errors.action}<div data-testid="users-err" class="block px-3 py-2.5 rounded-lg bg-[var(--hv-error-bg)] border border-[var(--hv-error)]/40 text-xs text-[var(--hv-error-text)]" role="alert">{errors.action}</div>{/if}

            <div class="flex flex-wrap gap-2.5">
              <button type="submit" data-testid="users-btn" class={cn(primaryBtn, 'flex-1 h-12 justify-center text-c-md')} disabled={submitting}>
                <IconCheck size={18} stroke={2.2} aria-hidden="true" /> {$t('users.create_account')}
              </button>
              <button type="button" data-testid="users-btn" class={cn(ghostBtn, 'h-12 justify-center text-c-md')} onclick={closeDetail} disabled={submitting}>{$t('common.cancel')}</button>
            </div>
          </form>
        </div>
      {:else if selectedUser}
        <!-- ───────────── Detail: 6 titled cards ───────────── -->
        <div data-testid="users-view" class="flex w-full flex-col gap-3">
          <div data-testid="users-detail-topbar" class="flex items-center flex-wrap gap-3">
            <button type="button" data-testid="users-icon-btn" class={iconBtn} onclick={closeDetail} aria-label={$t('users.back')}>
              <IconArrowLeft size={22} stroke={2} aria-hidden="true" />
            </button>
            <span data-testid="users-detail-avatar" class="grid h-12 w-12 flex-none place-items-center rounded-full text-c-lg font-semibold text-white" style="background:{colorFor(selectedUser)}" aria-hidden="true">{initials(selectedUser)}</span>
            <div class="min-w-0 flex-1">
              <h2 data-testid="users-detail-h1" class="m-0 truncate text-c-stat font-semibold leading-tight tracking-tight text-[var(--hv-shell-text-strong)]">{selectedUser.fullname || selectedUser.username}</h2>
              <div class="mt-0.5 truncate font-[family-name:var(--hv-font-mono)] text-c-base text-[var(--hv-shell-text)]">@{selectedUser.username}{selectedUser.email ? ' · ' + selectedUser.email : ''}</div>
            </div>
          </div>

          <!--
            A single `users-err` / `users-ok` slot for the whole pane: every
            handler assigns a fresh `errors` object holding at most one key, so
            exactly one of these is ever in the DOM (the E2E assertions use an
            unscoped `toHaveText`, which strict mode would reject otherwise).
          -->
          {#if errors.action}<div data-testid="users-err" class="block px-3 py-2.5 rounded-lg bg-[var(--hv-error-bg)] border border-[var(--hv-error)]/40 text-xs text-[var(--hv-error-text)]" role="alert">{errors.action}</div>{/if}
          {#if errors.saved}<p data-testid="users-ok" class={cn(okTxt, 'm-0')} role="status">{errors.saved}</p>{/if}

          <!-- 1. Identity -->
          <form onsubmit={(e) => { e.preventDefault(); saveProfile(); }}>
            <SectionCard data-testid="users-setting-row" divider>
              {#snippet titleSnippet()}
                <h2 class={cardTitle}><IconUsers size={17} stroke={2} aria-hidden="true" /> {$t('users.identity')}</h2>
              {/snippet}
              {#snippet actions()}
                <button type="submit" data-testid="users-btn" class={primaryBtn} disabled={submitting}><IconCheck size={16} stroke={2.2} aria-hidden="true" /> {$t('users.save_profile')}</button>
              {/snippet}
              <div class="grid grid-cols-1 @min-[768px]:grid-cols-2 gap-3">
                <div class={field}>
                  <span class={fieldLabel}>{$t('users.username_label')}</span>
                  <!-- Read-only: the identifier is the account's primary key and
                       is not editable in this pane (no `#d-username` input). -->
                  <span class="flex h-11 items-center truncate rounded-lg border border-[var(--hv-shell-border)] bg-[var(--hv-shell-fill)] px-3 font-[family-name:var(--hv-font-mono)] text-c-base text-[var(--hv-shell-text-2)]" title={selectedUser.username}>{selectedUser.username}</span>
                </div>
                <div class={field}>
                  <label for="d-fullname" class={fieldLabel}>{$t('users.fullname_label')}</label>
                  <input id="d-fullname" class={inp} bind:value={editForm.fullname} autocomplete="off" />
                </div>
                <div class={field}>
                  <label for="d-email" class={fieldLabel}>{$t('users.email_label')}</label>
                  <input id="d-email" class={inp} type="email" bind:value={editForm.email} autocomplete="off" />
                </div>
              </div>
            </SectionCard>
          </form>

          <!-- 2. Role & groups -->
          <form onsubmit={(e) => { e.preventDefault(); saveRoleAndGroups(); }}>
            <SectionCard data-testid="users-setting-row" divider>
              {#snippet titleSnippet()}
                <h2 class={cardTitle}><IconShield size={17} stroke={2} aria-hidden="true" /> {$t('users.role_groups_title')}</h2>
              {/snippet}
              {#snippet actions()}
                <button type="submit" data-testid="users-btn" class={primaryBtn} disabled={submitting}><IconCheck size={16} stroke={2.2} aria-hidden="true" /> {$t('common.save')}</button>
              {/snippet}
              <fieldset class="m-0 min-w-0 border-0 p-0">
                <legend class="mb-2 block p-0 text-c-label font-semibold text-[var(--hv-shell-text-2)]">{$t('users.role_label')}</legend>
                <div data-testid="users-segment" class="flex gap-1.5 rounded-lg bg-[var(--hv-shell-fill)] p-1.5">
                  <button type="button" data-testid="users-seg-btn" class:active={editForm.role === 'user'} aria-pressed={editForm.role === 'user'} onclick={() => (editForm.role = 'user')} class={cn(segBtn, editForm.role === 'user' && 'bg-[var(--hv-primary-light)] text-[var(--hv-shell-text-strong)]')}><IconUsers size={15} stroke={2} aria-hidden="true" /> {$t('users.role_user2')}</button>
                  <button type="button" data-testid="users-seg-btn" class:active={editForm.role === 'admin'} aria-pressed={editForm.role === 'admin'} onclick={() => (editForm.role = 'admin')} class={cn(segBtn, editForm.role === 'admin' && 'bg-[var(--hv-primary-light)] text-[var(--hv-shell-text-strong)]')}><IconShield size={15} stroke={2} aria-hidden="true" /> {$t('users.role_admin2')}</button>
                </div>
                <p class={cn(hint, 'mt-1.5')}>{editForm.role === 'admin' ? $t('users.role_admin_desc') : $t('users.role_user_desc')}</p>
              </fieldset>

              <div class="mt-4 border-t border-[var(--hv-shell-border-soft)] pt-4">
                {#if loadingGroups}
                  <div class={hint}><Spinner size="sm" label={$t('users.loading_groups')} /></div>
                {:else if groupsError}
                  <div class={hint} role="alert">{$t('users.groups_load_error')}
                    <button type="button" data-testid="users-inp-btn" class={cn(primaryBtn, 'ml-2 h-8 px-3 text-c-label')} onclick={loadGroups}>{$t('users.retry')}</button>
                  </div>
                {:else}
                  <div data-testid="users-chips" class="flex flex-col gap-2" role="group" aria-label={$t('users.sheet_groups')}>
                    {#if adminGroup}
                      <button type="button" data-testid="users-chip" class:on={editForm.role === 'admin'} disabled aria-disabled="true" title={$t('users.admin_group_managed')} class={cn(chipRow, 'admin-locked cursor-not-allowed border-dashed bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-2)]', editForm.role === 'admin' && 'border-solid border-[var(--hv-primary)]/55 bg-[var(--hv-primary-light)]')}>
                        <IconShield size={15} stroke={2} aria-hidden="true" />
                        {adminGroup.display_name}
                        <span class="text-c-xs font-medium opacity-80">· {$t('users.admin_group_managed')}</span>
                      </button>
                    {/if}
                    {#each groups as g (g.name)}
                      <button
                        type="button"
                        data-testid="users-chip"
                        class:on={groupsSelected.includes(g.display_name)}
                        aria-pressed={groupsSelected.includes(g.display_name)}
                        onclick={() => toggleGroupSel(g.display_name)}
                        class={cn(chipRow, groupsSelected.includes(g.display_name) && chipOn)}
                      >
                        {#if groupsSelected.includes(g.display_name)}<IconCheck size={15} stroke={2.5} aria-hidden="true" />{:else}<IconPlus size={15} stroke={2} aria-hidden="true" />{/if}
                        {g.display_name}
                      </button>
                    {/each}
                  </div>
                  {#if groups.length === 0}
                    <div class={cn(hint, 'mt-2.5')}>{$t('users.no_groups_available')} {$t('users.no_groups_hint')}</div>
                  {/if}
                {/if}
              </div>
            </SectionCard>
          </form>

          <!-- 3. Personal folder -->
          <SectionCard data-testid="users-setting-row" divider>
            {#snippet titleSnippet()}
              <h2 class={cardTitle}><IconFolder size={17} stroke={1.8} aria-hidden="true" /> {$t('users.sheet_home')}</h2>
            {/snippet}
            {#snippet actions()}
              <div class="flex items-center gap-2">
                <button type="button" data-testid="users-btn" class={primaryBtn} onclick={saveHome} disabled={submitting}><IconCheck size={16} stroke={2.2} aria-hidden="true" /> {$t('common.save')}</button>
                <button
                  type="button"
                  data-testid="users-switch"
                  class:on={homeForm.home_enabled}
                  role="switch"
                  aria-checked={homeForm.home_enabled}
                  aria-label={$t('users.home_enable_title')}
                  onclick={() => (homeForm.home_enabled = !homeForm.home_enabled)}
                  class={cn(switchTrack, homeForm.home_enabled ? 'bg-[var(--hv-primary)] justify-end' : 'bg-[var(--hv-shell-border)] justify-start')}
                ><span class={switchKnob}></span></button>
              </div>
            {/snippet}
            <p class={hint}>{$t('users.sheet_home')} · {$t('users.home_small_off')}</p>
            {#if selectedUser.home_enabled && selectedUser.home_path}
              <p class={cn(hint, 'mt-1 break-all font-[family-name:var(--hv-font-mono)]')}>{selectedUser.home_path}</p>
            {/if}
            {#if homeForm.home_enabled}
              <p class={cn(hint, 'mt-2.5')}>{$t('users.home_disk_or_folder')}</p>
              <div data-testid="users-disk-grid" class="flex flex-wrap gap-2.5 mt-3" role="group" aria-label={$t('users.select_disk')}>
                {#if loadingDisks}
                  <div class={hint}><Spinner size="sm" label={$t('users.loading_disks')} /></div>
                {:else if availableDisks.length === 0}
                  <div class={hint}>{$t('users.loading_disks')}</div>
                {:else}
                  {#each availableDisks as d (d.mount_point)}
                    <button
                      type="button"
                      data-testid="users-disk"
                      class:active={!homeForm.home_path && homeForm.home_disk === d.mount_point}
                      aria-pressed={!homeForm.home_path && homeForm.home_disk === d.mount_point}
                      onclick={() => pickHomeDisk(d.mount_point)}
                      class={cn('flex-1 min-w-[150px] flex flex-col items-start gap-0.5 px-3 py-3 border rounded-xl bg-[var(--hv-shell-fill)] text-left cursor-pointer transition-colors hover:bg-[var(--hv-shell-border-soft)]', !homeForm.home_path && homeForm.home_disk === d.mount_point ? 'border-[var(--hv-primary)]/55 bg-[var(--hv-primary-light)]' : 'border-[var(--hv-shell-border)]')}
                    >
                      <span class="text-c-base font-semibold text-[var(--hv-shell-text-strong)]">{d.mount_point}</span>
                      <span class="text-c-xs text-[var(--hv-shell-text-2)] font-[family-name:var(--hv-font-mono)]">{$t('users.disk_free', { size: formatBytes(d.available_bytes) })}</span>
                    </button>
                  {/each}
                {/if}
              </div>
              <div class="flex flex-wrap items-center gap-2 mt-2.5">
                <button type="button" data-testid="users-inp-btn" class={cn(ghostBtn, 'h-10')} onclick={() => (homePickerOpen = true)}>
                  <IconFolderOpen size={15} stroke={1.9} aria-hidden="true" /> {$t('users.home_pick_folder')}
                </button>
                {#if homeForm.home_path}
                  <span class="flex-1 min-w-0 text-xs text-[var(--hv-shell-text-strong)] px-2.5 py-1.5 rounded-lg bg-[var(--hv-primary-light)] border border-[var(--hv-primary)]/30 overflow-hidden text-ellipsis whitespace-nowrap font-[family-name:var(--hv-font-mono)]" title={homeForm.home_path}>{homeForm.home_path}</span>
                  <button type="button" data-testid="users-inp-btn" class="grid place-items-center w-7 h-8 p-0 border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:text-[var(--hv-shell-text-strong)]" aria-label={$t('common.cancel')} onclick={() => { homeForm.home_path = ''; }}>
                    <IconX size={15} stroke={2} aria-hidden="true" />
                  </button>
                {/if}
              </div>
              {#if homeForm.home_disk}
                {@const avail = homeDiskAvailable(homeForm.home_disk)}
                <div data-testid="users-homesize" class="mt-3 p-3 border border-[var(--hv-shell-border)] rounded-xl bg-[var(--hv-shell-border-soft)] flex flex-col gap-2.5">
                  <div class="flex items-center justify-between gap-3 text-c-base font-semibold text-[var(--hv-shell-text-strong)]">
                    <span id="d-homesize-label">{$t('users.home_size_label')}</span>
                    <label data-testid="users-homesize-unlim" class="inline-flex items-center gap-2 text-c-label font-medium text-[var(--hv-shell-text-2)] cursor-pointer"><input type="checkbox" class="w-4 h-4 cursor-pointer" bind:checked={homeForm.home_size_unlimited} />{$t('users.home_size_unlimited')}</label>
                  </div>
                  {#if homeForm.home_size_unlimited}
                    <div class={hint}>{$t('users.home_size_unlimited_hint')}</div>
                  {:else}
                    <input
                      data-testid="users-homesize-range"
                      class="w-full accent-[var(--hv-primary)] cursor-pointer"
                      type="range"
                      min={Math.min(HOME_SIZE_MIN, avail)}
                      max={avail}
                      step={HOME_SIZE_STEP}
                      bind:value={homeForm.home_size_bytes}
                      aria-labelledby="d-homesize-label"
                      aria-valuetext={formatBytes(homeForm.home_size_bytes)}
                    />
                    <!-- Allocated share of the disk the folder lives on. Fed by
                         the slider's own numbers (size vs free), never by a
                         "real" home-directory usage figure — that does not
                         exist, and inventing one would desync the readout. -->
                    <div class="h-2 rounded-full bg-[var(--hv-shell-border)] overflow-hidden" role="progressbar" aria-valuenow={homeBarPct(homeForm.home_size_bytes, avail)} aria-valuemin="0" aria-valuemax="100">
                      <span class="block h-full rounded-full bg-[var(--hv-primary)]" style="width:{homeBarPct(homeForm.home_size_bytes, avail)}%"></span>
                    </div>
                    <div data-testid="users-homesize-readout" class="flex items-baseline gap-2 text-c-base text-[var(--hv-shell-text-2)] font-[family-name:var(--hv-font-mono)]">
                      <strong class="text-c-card-title text-[var(--hv-shell-text-strong)]">{formatBytes(homeForm.home_size_bytes)}</strong>
                      <span>{$t('users.home_size_of_free', { free: formatBytes(Math.max(0, avail - homeForm.home_size_bytes)) })}</span>
                    </div>
                  {/if}
                </div>
              {/if}
            {/if}
            {#if selectedUser.home_enabled && selectedUser.home_path}
              <div class="mt-3">
                <button type="button" data-testid="users-btn" class="inline-flex items-center gap-2 h-10 px-4 border border-[var(--hv-error)]/40 rounded-lg bg-transparent text-[var(--hv-error)] text-c-base font-semibold cursor-pointer hover:bg-[var(--hv-error-bg)]" onclick={handleDeleteHome}><IconTrash size={15} stroke={2} aria-hidden="true" /> {$t('users.delete_home_folder')}</button>
              </div>
            {/if}
          </SectionCard>

          <!-- 4. Transfer quotas -->
          <SectionCard data-testid="users-setting-row" divider>
            {#snippet titleSnippet()}
              <h2 class={cardTitle}><IconGauge size={17} stroke={1.8} aria-hidden="true" /> {$t('users.sheet_quota')}</h2>
            {/snippet}
            {#snippet actions()}
              <button type="button" data-testid="users-link-btn" class={cn(ghostBtn, 'h-9 px-3 text-c-label')} onclick={openQuotaOverview}><IconGauge size={14} stroke={2} aria-hidden="true" /> {$t('users.quota_overview_button')}</button>
            {/snippet}
            {#if loadingQuota}
              <div class={hint}><Spinner size="sm" label={$t('users.loading')} /></div>
            {:else}
              <form onsubmit={(e) => { e.preventDefault(); saveQuota(); }}>
                <div data-testid="users-quota-block" class="py-3 border-t border-[var(--hv-shell-border-soft)] first:border-t-0 first:pt-0" role="group" aria-labelledby="q-up-label">
                  <div class="flex items-center gap-3">
                    <div class="min-w-0 flex-1" id="q-up-label"><strong class="block text-sm font-semibold text-[var(--hv-shell-text-strong)]">{$t('users.quota_upload_title')}</strong><small class="text-c-xs text-[var(--hv-shell-text)]">{$t('users.quota_upload_desc')}</small></div>
                    <button type="button" data-testid="users-switch" class:on={quotaForm.uploadEnabled} role="switch" aria-checked={quotaForm.uploadEnabled} aria-label={$t('users.quota_upload_title')} onclick={() => (quotaForm.uploadEnabled = !quotaForm.uploadEnabled)} class={cn(switchTrack, quotaForm.uploadEnabled ? 'bg-[var(--hv-primary)] justify-end' : 'bg-[var(--hv-shell-border)] justify-start')}><span class={switchKnob}></span></button>
                  </div>
                  {#if quotaForm.uploadEnabled}
                    <div class="flex gap-2.5 mt-2.5">
                      <input data-testid="users-quota-input" class={cn(inpSm, 'flex-1')} type="number" min="0" step="1" bind:value={quotaForm.uploadValue} aria-label={$t('users.quota_value_label')} />
                      <select data-testid="users-quota-input" class={cn(inpSm, 'flex-[0_0_90px]')} bind:value={quotaForm.uploadUnit} aria-label={$t('users.quota_unit_label')}><option value="MB">MB</option><option value="TB">TB</option><option value="PB">PB</option></select>
                    </div>
                    <div class="h-2 mt-2.5 rounded-full bg-[var(--hv-shell-border-soft)] overflow-hidden" role="progressbar" aria-valuenow={quotaBarPct(quotaUsage.uploadUsedBytes, quotaForm.uploadValue, quotaForm.uploadUnit)} aria-valuemin="0" aria-valuemax="100"><span class="block h-full rounded-full bg-[var(--hv-primary)]" style="width:{quotaBarPct(quotaUsage.uploadUsedBytes, quotaForm.uploadValue, quotaForm.uploadUnit)}%"></span></div>
                    <p class={hint}>{$t('users.quota_used', { used: formatBytes(quotaUsage.uploadUsedBytes) })}</p>
                  {/if}
                </div>

                <div data-testid="users-quota-block" class="py-3 border-t border-[var(--hv-shell-border-soft)]" role="group" aria-labelledby="q-down-label">
                  <div class="flex items-center gap-3">
                    <div class="min-w-0 flex-1" id="q-down-label"><strong class="block text-sm font-semibold text-[var(--hv-shell-text-strong)]">{$t('users.quota_download_title')}</strong><small class="text-c-xs text-[var(--hv-shell-text)]">{$t('users.quota_download_desc')}</small></div>
                    <button type="button" data-testid="users-switch" class:on={quotaForm.downloadEnabled} role="switch" aria-checked={quotaForm.downloadEnabled} aria-label={$t('users.quota_download_title')} onclick={() => (quotaForm.downloadEnabled = !quotaForm.downloadEnabled)} class={cn(switchTrack, quotaForm.downloadEnabled ? 'bg-[var(--hv-primary)] justify-end' : 'bg-[var(--hv-shell-border)] justify-start')}><span class={switchKnob}></span></button>
                  </div>
                  {#if quotaForm.downloadEnabled}
                    <div class="flex gap-2.5 mt-2.5">
                      <input data-testid="users-quota-input" class={cn(inpSm, 'flex-1')} type="number" min="0" step="1" bind:value={quotaForm.downloadValue} aria-label={$t('users.quota_value_label')} />
                      <select data-testid="users-quota-input" class={cn(inpSm, 'flex-[0_0_90px]')} bind:value={quotaForm.downloadUnit} aria-label={$t('users.quota_unit_label')}><option value="MB">MB</option><option value="TB">TB</option><option value="PB">PB</option></select>
                    </div>
                    <div class="h-2 mt-2.5 rounded-full bg-[var(--hv-shell-border-soft)] overflow-hidden" role="progressbar" aria-valuenow={quotaBarPct(quotaUsage.downloadUsedBytes, quotaForm.downloadValue, quotaForm.downloadUnit)} aria-valuemin="0" aria-valuemax="100"><span class="block h-full rounded-full bg-[var(--hv-primary)]" style="width:{quotaBarPct(quotaUsage.downloadUsedBytes, quotaForm.downloadValue, quotaForm.downloadUnit)}%"></span></div>
                    <p class={hint}>{$t('users.quota_used', { used: formatBytes(quotaUsage.downloadUsedBytes) })}</p>
                  {/if}
                </div>

                {#if errors.quotaOk}<span data-testid="users-ok" class={okTxt} role="status">{errors.quotaOk}</span>{/if}
                <button type="submit" data-testid="users-btn" class={cn(primaryBtn, 'mt-2.5')} disabled={submitting}><IconCheck size={16} stroke={2.2} aria-hidden="true" /> {$t('common.save')}</button>
              </form>
            {/if}
          </SectionCard>

          <!-- 4b. Per-member resource access (apps / folders / shares) -->
          <MemberAccessCard username={selectedUser.username} isAdmin={selectedUser.role === 'admin'} />

          <!-- 5. Security -->
          <SectionCard data-testid="users-setting-row" divider>
            {#snippet titleSnippet()}
              <h2 class={cardTitle}><IconShield size={17} stroke={2} aria-hidden="true" /> {$t('users.security')}</h2>
            {/snippet}
            <div class="flex items-start gap-3">
              <span class={settingIcoOk}><IconShield size={22} stroke={1.7} aria-hidden="true" /></span>
              <div class="flex-1 min-w-0">
                <div class="flex items-baseline gap-1.5 flex-wrap">
                  <strong class="text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">{$t('users.twofa_title')}</strong>
                  <small class="text-xs text-[var(--hv-shell-text)]">{selectedUser.two_fa_enabled ? $t('users.twofa_active') : $t('users.twofa_inactive')}</small>
                </div>
              </div>
              <div class="flex items-center flex-wrap gap-2.5 ml-auto">
                {#if selectedUser.two_fa_enabled}
                  <button type="button" data-testid="users-btn" class={ghostBtn} onclick={handleDisable2FA}><IconShieldOff size={16} stroke={1.9} aria-hidden="true" /> {$t('users.reset_2fa_admin')}</button>
                {/if}
                <button
                  type="button"
                  data-testid="users-switch"
                  class:on={selectedUser.two_fa_enabled}
                  role="switch"
                  aria-checked={selectedUser.two_fa_enabled}
                  aria-label={$t('users.twofa_title')}
                  disabled={!selectedUser.two_fa_enabled}
                  onclick={handleDisable2FA}
                  class={cn(switchTrack, selectedUser.two_fa_enabled ? 'bg-[var(--hv-primary)] justify-end' : 'bg-[var(--hv-shell-border)] justify-start')}
                ><span class={switchKnob}></span></button>
              </div>
            </div>

            <form class="mt-4 border-t border-[var(--hv-shell-border-soft)] pt-4" onsubmit={(e) => { e.preventDefault(); changePassword(); }}>
              <div class="flex items-baseline gap-1.5 flex-wrap">
                <!-- A real <label for>: 8.1 asserts every visible input[id] in
                     this pane has exactly one matching label. -->
                <label for="d-password" class="text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">{$t('users.sheet_password')}</label>
                <small class="text-xs text-[var(--hv-shell-text)]">{$t('users.new_password_label')}</small>
              </div>
              <div class={cn('flex items-center gap-2 h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] mt-2.5', errors.password && 'border-[var(--hv-error)]')}>
                <input id="d-password" class="flex-1 min-w-0 border-none bg-transparent h-10 font-[family-name:var(--hv-font-mono)] text-sm text-[var(--hv-shell-text-strong)] outline-none" type={passwordShown ? 'text' : 'password'} autocomplete="new-password" aria-invalid={errors.password ? 'true' : undefined} aria-describedby={errors.password ? 'd-password-err' : undefined} bind:value={passwordValue} placeholder={$t('users.password_min')} />
                <button type="button" data-testid="users-inp-btn" class="ghost grid place-items-center w-7 h-8 p-0 border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:text-[var(--hv-shell-text-strong)]" onclick={() => (passwordShown = !passwordShown)} aria-pressed={passwordShown} aria-label={passwordShown ? $t('users.password_hide') : $t('users.password_show')}>
                  {#if passwordShown}<IconEyeOff size={16} stroke={2} aria-hidden="true" />{:else}<IconEye size={16} stroke={2} aria-hidden="true" />{/if}
                </button>
                <button type="button" data-testid="users-inp-btn" class={cn(primaryBtn, 'h-8 px-3 flex-none text-c-label')} onclick={() => { passwordValue = generatePassword(); passwordShown = true; }}>{$t('users.generate')}</button>
              </div>
              {#if errors.password}<span id="d-password-err" data-testid="users-err" class={errTxt} role="alert">{errors.password}</span>{/if}
              {#if errors.passwordOk}<span data-testid="users-ok" class={okTxt} role="status">{errors.passwordOk}</span>{/if}
              <button type="submit" data-testid="users-btn" class={cn(ghostBtn, 'mt-2.5')} disabled={submitting}><IconKey size={16} stroke={1.9} aria-hidden="true" /> {$t('users.change_password')}</button>
            </form>
          </SectionCard>

          <!-- 6. Account actions -->
          <SectionCard divider>
            {#snippet titleSnippet()}
              <h2 class={cardTitle}><IconLock size={17} stroke={2} aria-hidden="true" /> {$t('users.actions_title')}</h2>
            {/snippet}
            <div class="flex items-start gap-3">
              <span class={settingIco}>
                {#if selectedUser.locked}<IconLockOpen size={22} stroke={1.8} aria-hidden="true" />{:else}<IconLock size={22} stroke={1.8} aria-hidden="true" />{/if}
              </span>
              <div class="flex-1 min-w-0">
                <div class="text-c-base text-[var(--hv-shell-text-strong)]">{$t('users.lock_hint')}</div>
              </div>
              <!-- The single lock/unlock affordance in this pane. -->
              <button
                type="button"
                data-testid="users-lock-btn"
                class:locked={selectedUser.locked}
                onclick={toggleLock}
                class={cn(ghostBtn, 'flex-none', selectedUser.locked && 'border-[var(--hv-warning)]/50 bg-[var(--hv-warning-bg)] text-[var(--hv-warning)]')}
              >
                {#if selectedUser.locked}
                  <IconLockOpen size={18} stroke={1.9} aria-hidden="true" /> <span>{$t('users.unlock_account')}</span>
                {:else}
                  <IconLock size={18} stroke={1.9} aria-hidden="true" /> <span>{$t('users.lock_account')}</span>
                {/if}
              </button>
            </div>

            <div class="mt-4 flex items-start gap-3 border-t border-[var(--hv-shell-border-soft)] pt-4">
              <span class={settingIcoDanger}><IconTrash size={22} stroke={1.9} aria-hidden="true" /></span>
              <div class="flex-1 min-w-0">
                <div class="flex items-baseline gap-1.5 flex-wrap">
                  <strong class="text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">{$t('users.delete_account')}</strong>
                  <small class="text-xs text-[var(--hv-shell-text)]">{deleteHomeFolder ? $t('users.delete_purge_hint') : $t('users.delete_keep_hint')}</small>
                </div>
              </div>
              <div class="flex items-center flex-wrap gap-2.5">
                {#if selectedUser.home_enabled && selectedUser.home_path}
                  <button
                    type="button"
                    data-testid="users-chip"
                    class:on={deleteHomeFolder}
                    aria-pressed={deleteHomeFolder}
                    onclick={() => (deleteHomeFolder = !deleteHomeFolder)}
                    class={cn(chip, 'danger-chip', deleteHomeFolder ? 'border-[var(--hv-error)]/55 bg-[var(--hv-error-bg)] text-[var(--hv-error-text)]' : '')}
                  >
                    {#if deleteHomeFolder}<IconCheck size={15} stroke={2.5} aria-hidden="true" />{:else}<span class="w-[22px] h-[22px] flex-none rounded-md border-2 border-[var(--hv-shell-border-strong)] grid place-items-center"></span>{/if}
                    {deleteHomeFolder ? $t('users.erase_folder') : $t('users.keep_folder')}
                  </button>
                {/if}
                <button type="button" data-testid="users-btn" class="inline-flex items-center gap-2 h-11 px-5 border-none rounded-lg bg-[var(--hv-error)] text-white text-c-base font-semibold cursor-pointer hover:bg-[var(--hv-error-hover)] disabled:opacity-55 disabled:cursor-default" onclick={handleDelete} disabled={submitting}>{$t('common.delete')}</button>
              </div>
            </div>
          </SectionCard>
        </div>
      {:else}
        <div data-testid="users-noselect" class="flex flex-1 min-h-0 flex-col items-center justify-center gap-3 py-10 text-center text-[var(--hv-shell-text)]">
          <IconUsers size={54} stroke={1.3} class="text-[var(--hv-shell-border-strong)]" aria-hidden="true" />
          <div class="text-c-xl font-semibold text-[var(--hv-shell-text-strong)]">{$t('users.no_selection_title')}</div>
          <p class="m-0 mb-2 max-w-xs text-c-base">{$t('users.no_selection_desc')}</p>
          <button type="button" class={primaryBtn} onclick={startCreate}><IconPlus size={18} stroke={2.2} aria-hidden="true" /> {$t('users.new_user_btn')}</button>
        </div>
      {/if}
    {/if}
  </div>
  <ConfirmDialog
    {dlg}
    onconfirm={() => dlg.settle(true)}
    oncancel={() => dlg.settle(false)}
  />
</PageLayout>

<!-- Home-directory folder pickers (detail + create home cards) -->
<FolderPicker
  open={homePickerOpen}
  value={homeForm.home_path}
  onselect={pickHomeFolder}
  onclose={() => (homePickerOpen = false)}
/>
<FolderPicker
  open={createHomePickerOpen}
  value={createForm.home_path}
  onselect={pickCreateHomeFolder}
  onclose={() => (createHomePickerOpen = false)}
/>

<!-- All quotas overview — kept alongside the read-only Quotas app (the two are
     complementary: this one is the per-user quick check, the app is the whole
     instance's overview). -->
<Modal open={showQuotaOverview} title={$t('users.quota_overview_title')} size="lg" onclose={closeQuotaOverview}>
  {#snippet icon()}
    <IconGauge size={20} stroke={2} aria-hidden="true" />
  {/snippet}
  {#snippet children()}
    {#if loadingQuotaOverview}
      <div class="text-c-label text-[var(--hv-shell-text)]"><Spinner size="sm" label={$t('users.quota_overview_loading')} /></div>
    {:else if quotaOverviewError}
      <div class="text-c-label text-[var(--hv-shell-text)]" role="alert">{$t('users.quota_overview_error')}</div>
    {:else if quotaOverviewRows.length === 0}
      <div class="text-c-label text-[var(--hv-shell-text)]">{$t('users.quota_overview_empty')}</div>
    {:else}
      <div class="overflow-x-auto">
      <table class="w-full border-collapse text-c-base">
        <thead>
          <tr>
            <th class="text-start px-2.5 py-2 font-semibold text-[var(--hv-text-secondary)] border-b border-[var(--hv-border)]">{$t('users.quota_overview_type')}</th>
            <th class="text-start px-2.5 py-2 font-semibold text-[var(--hv-text-secondary)] border-b border-[var(--hv-border)]">{$t('users.quota_overview_name')}</th>
            <th class="text-start px-2.5 py-2 font-semibold text-[var(--hv-text-secondary)] border-b border-[var(--hv-border)]">{$t('users.quota_overview_upload')}</th>
            <th class="text-start px-2.5 py-2 font-semibold text-[var(--hv-text-secondary)] border-b border-[var(--hv-border)]">{$t('users.quota_overview_download')}</th>
          </tr>
        </thead>
        <tbody>
          {#each quotaOverviewRows as row (row.subjectType + ':' + row.subjectName)}
            <tr>
              <td class="px-2.5 py-2 border-b border-[var(--hv-shell-border-soft)] text-[var(--hv-text-primary)]">{row.subjectType === 'user' ? $t('users.quota_overview_type_user') : $t('users.quota_overview_type_group')}</td>
              <td class="px-2.5 py-2 border-b border-[var(--hv-shell-border-soft)] text-[var(--hv-text-primary)]">{row.subjectName}</td>
              <td class="px-2.5 py-2 border-b border-[var(--hv-shell-border-soft)] text-[var(--hv-text-primary)]">{#if row.uploadEnabled}{formatBytes(row.uploadUsedBytes)} / {formatBytes(row.uploadLimitBytes)}{:else}{$t('users.quota_overview_unlimited')}{/if}</td>
              <td class="px-2.5 py-2 border-b border-[var(--hv-shell-border-soft)] text-[var(--hv-text-primary)]">{#if row.downloadEnabled}{formatBytes(row.downloadUsedBytes)} / {formatBytes(row.downloadLimitBytes)}{:else}{$t('users.quota_overview_unlimited')}{/if}</td>
            </tr>
          {/each}
        </tbody>
      </table>
      </div>
    {/if}
  {/snippet}
  {#snippet footer()}
    <Button variant="secondary" onclick={closeQuotaOverview}>{$t('common.close')}</Button>
  {/snippet}
</Modal>
