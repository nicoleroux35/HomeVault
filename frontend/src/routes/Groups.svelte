<!-- ========================================================================
     File          : Groups.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
     The Groups management app: Unix group list with members, create/delete
     groups, add/remove member users (admin group protected).

     Dependencies:
       - svelte (>= 5), theme + app/ composites, $lib/services/api

     Usage:
     Opened as a desktop window app.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { t, translate } from '$lib/stores/i18n';
  import { isAdmin } from '$lib/stores/auth';
  import { onAppVisible } from '$lib/utils/autoRevalidate';
  import { Button } from '$lib/components/theme';
  import { PageLayout, SectionCard, FormRow, ConfirmDialog, tileBtn, EmptyState } from '$lib/components/app';
  import { createConfirmState } from '$lib/utils/confirmDialog.svelte';
  import { avatarColorFor } from '$lib/utils/avatars';
  import { cn } from '$lib/utils.js';
  import { IconSearch, IconPlus, IconUsers, IconUserCog, IconTrash, IconCheck, IconX, IconShield, IconGauge, IconArrowLeft } from '@tabler/icons-svelte';

  interface Group {
    name: string;
    display_name: string;
    gid: number;
    member_count: number;
    /** Free text from the `hv_groups` mirror; the OS group has no such field. */
    description?: string | null;
  }

  type QuotaUnit = 'MB' | 'TB' | 'PB';
  type QuotaPeriod = 'lifetime' | 'day' | 'week' | 'month';

  // ROADMAP 23.2 — the window the transfer counters are counted in. Same
  // order and wording as the Users card and the Quotas screen.
  const QUOTA_PERIODS: { id: QuotaPeriod; label: string }[] = [
    { id: 'lifetime', label: 'quotas.period_lifetime' },
    { id: 'day', label: 'quotas.period_day' },
    { id: 'week', label: 'quotas.period_week' },
    { id: 'month', label: 'quotas.period_month' },
  ];

  const QUOTA_UNIT_BYTES: Record<QuotaUnit, number> = {
    MB: 1024 * 1024,
    TB: 1024 * 1024 * 1024 * 1024,
    PB: 1024 * 1024 * 1024 * 1024 * 1024,
  };

  // The admin group is protected server-side (see ADMIN_GROUP_NAME): its
  // deletion is blocked and its membership is driven by the Users page's
  // role selector, never edited here.
  const PROTECTED_GROUP = 'homevault_admin';

  let groups: Group[] = [];
  let loading = true;

  let search = '';

  let selectedGroup: Group | null = null;
  let creating = false;
  let submitting = false;
  let errors: Record<string, string> = {};

  let createForm = { name: '', members: [] as string[], description: '' };

  // Rename + description editor for the selected group. `name` is the display
  // name; the server migrates every reference on a rename.
  let metaForm = { name: '', description: '' };
  let metaBusy = false;
  let metaOk = '';
  let createSearch = '';

  let membersSelected: string[] = [];
  let membersBefore: string[] = [];
  let memberSearch = '';
  let loadingMembers = false;

  let allUsernames: string[] | null = null;
  let loadingUsers = false;

  let quotaForm = {
    uploadEnabled: false, uploadValue: 0, uploadUnit: 'MB' as QuotaUnit,
    downloadEnabled: false, downloadValue: 0, downloadUnit: 'MB' as QuotaUnit,
    period: 'lifetime' as QuotaPeriod,
  };
  let quotaUsage = { uploadUsedBytes: 0, downloadUsedBytes: 0 };
  /** The period and reset instant of the *saved* row — never of the form. */
  let quotaSaved = { period: 'lifetime' as QuotaPeriod, resetsAt: null as string | null };
  let loadingQuota = false;
  const dlg = createConfirmState();

  // True below the 860px breakpoint (the old `@media (max-width: 860px)`):
  // the two panes stack, the list hides while a detail/create pane is open,
  // and the back arrow shows. Viewport-driven via matchMedia so PageLayout's
  // `sidebarHidden` can react to it.
  let narrow = false;

  function formatBytes(bytes: number): string {
    if (!bytes) return '—';
    const units = ['o', 'Ko', 'Mo', 'Go', 'To'];
    let i = 0;
    while (bytes >= 1024 && i < units.length - 1) { bytes /= 1024; i++; }
    return `${bytes.toFixed(1)} ${units[i]}`;
  }

  // Largest whole unit that represents `bytes` cleanly, so a saved quota
  // reopens showing e.g. "5 TB" rather than "5242880 MB".
  function bytesToQuotaValue(bytes: number): { value: number; unit: QuotaUnit } {
    if (bytes > 0 && bytes % QUOTA_UNIT_BYTES.PB === 0) return { value: bytes / QUOTA_UNIT_BYTES.PB, unit: 'PB' };
    if (bytes > 0 && bytes % QUOTA_UNIT_BYTES.TB === 0) return { value: bytes / QUOTA_UNIT_BYTES.TB, unit: 'TB' };
    return { value: Math.round(bytes / QUOTA_UNIT_BYTES.MB), unit: 'MB' };
  }
  // Locale-aware date, used for the "resets on …" line only.
  function formatResetDate(iso: string): string {
    if (!iso) return '—';
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return '—';
    try {
      return d.toLocaleString(document.documentElement.lang || undefined, {
        day: 'numeric', month: 'short', year: 'numeric',
      });
    } catch {
      return iso.slice(0, 10);
    }
  }
  function quotaBarPct(usedBytes: number, value: number, unit: QuotaUnit): number {
    const limit = Math.max(0, Math.round(value)) * QUOTA_UNIT_BYTES[unit];
    if (limit <= 0) return 0;
    return Math.min(100, Math.round((usedBytes / limit) * 100));
  }

  // Shared control shape for this screen's native inputs/selects (appliance
  // tokens only — the per-app CSS vocabulary is gone).
  const inp =
    'w-full box-border h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] ' +
    'text-[var(--hv-shell-text-strong)] text-sm outline-none transition-colors placeholder:text-[var(--hv-shell-text-3)] ' +
    'focus:border-[var(--hv-primary)]';

  const userRow =
    'flex items-center gap-2.5 min-h-[46px] px-3 w-full border rounded-lg text-left text-c-base font-medium ' +
    'text-[var(--hv-shell-text-strong)] transition-all';

  const check =
    'w-[22px] h-[22px] flex-none rounded-md border-2 grid place-items-center text-[var(--hv-shell-text-strong)]';

  onMount(() => {
    if (!$isAdmin) {
      window.location.href = '/desktop';
      return;
    }
    Promise.all([loadGroups(), loadAvailableUsers()]);
    // No manual refresh button — revalidate when the window regains attention.
    const teardown = onAppVisible(refreshAll);

    const mq = window.matchMedia('(max-width: 860px)');
    narrow = mq.matches;
    const onMq = () => (narrow = mq.matches);
    mq.addEventListener('change', onMq);

    return () => {
      teardown();
      mq.removeEventListener('change', onMq);
    };
  });

  async function loadGroups() {
    loading = true;
    const res = await api.getGroups();
    if (res.success && res.data) groups = res.data as Group[];
    loading = false;
  }

  async function loadAvailableUsers() {
    loadingUsers = true;
    const res = await api.getAvailableUsers();
    loadingUsers = false;
    allUsernames = res.success && res.data ? (res.data as string[]) : [];
  }

  async function refreshAll() {
    await loadGroups();
    if (selectedGroup) {
      const again = groups.find((g) => g.name === selectedGroup!.name) || null;
      if (again) hydrateDetail(again);
      else selectedGroup = null;
    }
  }

  // ---------- Helpers ----------
  function initials(g: Group): string {
    const n = (g.display_name || g.name).trim();
    const p = n.split(/[\s_-]+/).filter(Boolean);
    return (p.length >= 2 ? p[0][0] + p[1][0] : n.slice(0, 2)).toUpperCase();
  }
  function colorFor(g: Group): string {
    return avatarColorFor(g.name);
  }
  function isProtected(g: Group | null): boolean {
    return !!g && g.name === PROTECTED_GROUP;
  }

  $: filteredGroups = groups.filter((g) => {
    if (!search) return true;
    const hay = `${g.name} ${g.display_name}`.toLowerCase();
    return hay.includes(search.toLowerCase());
  });

  $: detailOpen = creating || !!selectedGroup;

  // Union of every known HomeVault user and the group's actual current
  // members — a group's real Unix members can include accounts with no
  // matching HomeVault user row (a system account, one added via usermod
  // outside the app, or a HomeVault user later deleted from the DB) — those
  // must stay visible so this list agrees with the member count.
  $: memberUserPool = Array.from(new Set([...(allUsernames ?? []), ...membersSelected]))
    .sort((a, b) => a.localeCompare(b));
  $: filteredMemberUsers = memberUserPool
    .filter((u) => !memberSearch || u.toLowerCase().includes(memberSearch.toLowerCase()));
  $: filteredCreateUsers = (allUsernames ?? [])
    .filter((u) => !createSearch || u.toLowerCase().includes(createSearch.toLowerCase()));

  // Narrow + a detail/create pane open → the list rail collapses (matches
  // the old stacked-pane `@media (max-width: 860px)` rule).
  $: sidebarHidden = narrow && detailOpen;

  // ---------- Selection ----------
  function hydrateDetail(g: Group) {
    selectedGroup = g;
    creating = false;
    errors = {};
    memberSearch = '';
    metaOk = '';
    metaForm = { name: g.display_name || g.name, description: g.description ?? '' };
    loadMembers(g);
    loadQuota(g);
  }
  function selectGroup(g: Group) { hydrateDetail(g); }
  function closeDetail() { selectedGroup = null; creating = false; errors = {}; }

  function startCreate() {
    createForm = { name: '', members: [], description: '' };
    createSearch = '';
    errors = {};
    selectedGroup = null;
    creating = true;
  }

  async function loadMembers(g: Group) {
    loadingMembers = true;
    const res = await api.getGroup(g.display_name);
    loadingMembers = false;
    membersBefore = res.success && res.data ? (res.data as { members: string[] }).members : [];
    membersSelected = [...membersBefore];
  }

  async function loadQuota(g: Group) {
    loadingQuota = true;
    const res = await api.getTransferQuota('group', g.display_name);
    loadingQuota = false;
    if (res.success && res.data) {
      const up = bytesToQuotaValue(res.data.upload_limit_bytes);
      const down = bytesToQuotaValue(res.data.download_limit_bytes);
      const period = (res.data.period || 'lifetime') as QuotaPeriod;
      quotaForm = {
        uploadEnabled: res.data.upload_enabled, uploadValue: up.value, uploadUnit: up.unit,
        downloadEnabled: res.data.download_enabled, downloadValue: down.value, downloadUnit: down.unit,
        period,
      };
      quotaUsage = { uploadUsedBytes: res.data.upload_used_bytes || 0, downloadUsedBytes: res.data.download_used_bytes || 0 };
      quotaSaved = { period, resetsAt: res.data.period_resets_at || null };
    } else {
      quotaForm = { uploadEnabled: false, uploadValue: 0, uploadUnit: 'MB', downloadEnabled: false, downloadValue: 0, downloadUnit: 'MB', period: 'lifetime' };
      quotaUsage = { uploadUsedBytes: 0, downloadUsedBytes: 0 };
      quotaSaved = { period: 'lifetime', resetsAt: null };
    }
  }

  function toggleCreateMember(username: string) {
    createForm.members = createForm.members.includes(username)
      ? createForm.members.filter((u) => u !== username)
      : [...createForm.members, username];
  }
  function toggleMember(username: string) {
    // Defense in depth: the admin group's checklist is rendered read-only,
    // but guard the mutator too — editing it here would desync from the
    // Users page's DB-role-driven admin status.
    if (isProtected(selectedGroup)) return;
    membersSelected = membersSelected.includes(username)
      ? membersSelected.filter((u) => u !== username)
      : [...membersSelected, username];
  }

  // ---------- Actions ----------
  async function handleCreate() {
    errors = {};
    const name = createForm.name.trim();
    if (!name) { errors = { name: translate('groups.name_required') }; return; }
    if (!/^[a-z0-9_-]+$/.test(name)) { errors = { name: translate('groups.name_invalid') }; return; }
    if (groups.some((g) => g.display_name === name)) { errors = { name: translate('groups.name_exists') }; return; }
    submitting = true;
    const res = await api.createGroup(name, createForm.members, createForm.description.trim());
    submitting = false;
    if (res.success) {
      creating = false;
      await loadGroups();
      const created = groups.find((g) => g.display_name === name);
      if (created) hydrateDetail(created);
    } else {
      errors = { action: res.message || res.error || translate('groups.create_failed') };
    }
  }

  async function saveMembers() {
    if (!selectedGroup || isProtected(selectedGroup)) return;
    errors = {};
    const before = new Set(membersBefore);
    const after = new Set(membersSelected);
    const added = membersSelected.filter((u) => !before.has(u));
    const removed = membersBefore.filter((u) => !after.has(u));
    if (added.length === 0 && removed.length === 0) { errors = { membersOk: translate('groups.members_updated') }; return; }

    submitting = true;
    let ok = true;
    for (const username of added) {
      const res = await api.addMemberToGroup(selectedGroup.display_name, username);
      if (!res.success) {
        ok = false;
        errors = { action: `${translate('groups.member_action_failed', { username })} — ${res.message || res.error || translate('common.request_failed')}` };
        break;
      }
    }
    if (ok) {
      for (const username of removed) {
        const res = await api.removeMemberFromGroup(selectedGroup.display_name, username);
        if (!res.success) {
          ok = false;
          errors = { action: `${translate('groups.member_action_failed', { username })} — ${res.message || res.error || translate('common.request_failed')}` };
          break;
        }
      }
    }
    submitting = false;
    // refreshAll() -> hydrateDetail() unconditionally resets `errors = {}`
    // as its first line (so a stale error/success from a *previous*
    // selection never bleeds into a freshly opened one) — setting the
    // success message before that call let it immediately erase itself
    // before ever rendering. Setting it after is the fix: nothing else
    // touches `errors` past this point.
    if (ok) { await refreshAll(); errors = { membersOk: translate('groups.members_updated') }; }
  }

  async function saveMeta() {
    if (!selectedGroup || isProtected(selectedGroup)) return;
    errors = {};
    metaOk = '';
    const name = metaForm.name.trim();
    const description = metaForm.description.trim();
    if (!name) { errors = { meta: translate('groups.rename_required') }; return; }
    if (!/^[a-z0-9_-]+$/.test(name)) { errors = { meta: translate('groups.name_invalid') }; return; }
    const renamed = name !== (selectedGroup.display_name || selectedGroup.name);
    if (renamed && groups.some((g) => g.display_name === name)) {
      errors = { meta: translate('groups.name_exists') };
      return;
    }
    const descriptionChanged = description !== (selectedGroup.description ?? '');
    if (!renamed && !descriptionChanged) { metaOk = translate('groups.meta_unchanged'); return; }

    metaBusy = true;
    const res = await api.updateGroup(selectedGroup.display_name || selectedGroup.name, {
      name: renamed ? name : undefined,
      description: descriptionChanged ? description : undefined,
    });
    metaBusy = false;
    if (res.success) {
      const notice = renamed ? translate('groups.renamed_ok') : translate('groups.description_saved');
      await loadGroups();
      // The detail pane must follow the rename, or the next save would target
      // the old name. `hydrateDetail` resets the notices, so set it after.
      const again = groups.find((g) => g.display_name === name) || null;
      if (again) hydrateDetail(again);
      metaOk = notice;
    } else {
      errors = { meta: res.message || res.error || translate('groups.meta_failed') };
    }
  }

  async function saveQuota() {
    if (!selectedGroup) return;
    submitting = true;
    const res = await api.setTransferQuota('group', selectedGroup.display_name, {
      upload_enabled: quotaForm.uploadEnabled,
      upload_limit_bytes: Math.max(0, Math.round(quotaForm.uploadValue)) * QUOTA_UNIT_BYTES[quotaForm.uploadUnit],
      download_enabled: quotaForm.downloadEnabled,
      download_limit_bytes: Math.max(0, Math.round(quotaForm.downloadValue)) * QUOTA_UNIT_BYTES[quotaForm.downloadUnit],
      period: quotaForm.period,
    });
    submitting = false;
    if (res.success) {
      errors = { quotaOk: translate('groups.quota_updated') };
      // Re-read the row so the reset date shown is the one the server saved.
      await loadQuota(selectedGroup);
    } else {
      errors = { action: res.message || res.error || translate('groups.quota_update_failed') };
    }
  }

  async function handleDelete() {
    if (!selectedGroup || isProtected(selectedGroup)) return;
    let msg = translate('groups.delete_confirm', { name: selectedGroup.display_name || selectedGroup.name });
    if (selectedGroup.member_count > 0) {
      msg += ' ' + translate('groups.delete_members_warn', { count: String(selectedGroup.member_count) });
    }
    if (!(await dlg.ask(msg))) return;
    submitting = true;
    const res = await api.deleteGroup(selectedGroup.display_name);
    submitting = false;
    if (res.success) {
      selectedGroup = null;
      await loadGroups();
    } else {
      errors = { action: res.message || res.error || translate('groups.delete_failed') };
    }
  }

  </script>

<PageLayout
  title={$t('groups.heading')}
  data-testid="groups-page"
  sidebarLabel={$t('groups.title')}
  sidebarTestId="groups-list"
  {sidebarHidden}
>
  {#snippet icon()}
    <IconUsers stroke={1.7} />
  {/snippet}

  <!-- Page-level primary action: a WindowToolbar row under the header, never
       inside it (the header bar is icon + title + window controls only). -->
  {#snippet toolbarRight()}
    <Button variant="primary" data-testid="groups-new" onclick={startCreate}>
      <IconPlus size={17} stroke={2} /> {$t('groups.new')}
    </Button>
  {/snippet}

  {#snippet sidebar()}
    <div class="px-4 py-3 border-b border-[var(--hv-shell-border-soft)]">
      <div data-testid="groups-search" class="flex items-center gap-2.5 h-11 px-3 rounded-lg bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] text-[var(--hv-shell-text)]">
        <IconSearch size={18} stroke={1.9} />
        <input
          type="search"
          placeholder={$t('groups.search_placeholder')}
          bind:value={search}
          autocomplete="off"
          class="flex-1 min-w-0 bg-transparent border-none outline-none text-[var(--hv-shell-text-strong)] text-sm placeholder:text-[var(--hv-shell-text-3)]"
        />
      </div>
    </div>

    <div class="p-3 flex flex-col gap-2 overflow-y-auto">
      {#if loading}
        <div data-testid="groups-empty" class="text-center py-12 text-[var(--hv-shell-text)]">
          <p class="text-c-base">{$t('users.loading')}</p>
        </div>
      {:else if filteredGroups.length === 0}
        <EmptyState data-testid="groups-empty" title={search ? $t('groups.empty_filter') : $t('groups.no_groups')}>
          {#snippet iconSnippet()}<IconUsers size={32} stroke={1.8} />{/snippet}
        </EmptyState>
      {:else}
        {#each filteredGroups as g (g.name)}
          <button
            data-testid="groups-row"
            class:selected={selectedGroup?.name === g.name && !creating}
            class={cn(
              'flex items-center gap-3 w-full p-3 rounded-lg bg-[var(--hv-shell-card)] border text-left cursor-pointer transition-all',
              selectedGroup?.name === g.name && !creating
                ? 'border-[var(--hv-primary)] bg-[var(--hv-primary-selected-bg)]'
                : 'border-[var(--hv-shell-border)] hover:border-[var(--hv-primary)] hover:bg-[var(--hv-primary-selected-bg-hover)]',
            )}
            onclick={() => selectGroup(g)}
          >
            <span
              data-testid="groups-row-avatar"
              class="w-11 h-11 rounded-lg grid place-items-center font-bold text-sm text-white flex-none"
              style="background:{colorFor(g)}"
            >{initials(g)}</span>
            <span class="flex-1 min-w-0 flex flex-col gap-0.5">
              <span data-testid="groups-row-name" class="text-c-card-title font-semibold text-[var(--hv-shell-text-strong)] truncate">{g.display_name || g.name}</span>
              <span class="font-[family-name:var(--hv-font-mono)] text-c-xs text-[var(--hv-shell-text)] truncate">@{g.display_name} · GID {g.gid}</span>
              {#if g.description}
                <span data-testid="groups-row-description" class="text-c-xs text-[var(--hv-shell-text)] truncate">{g.description}</span>
              {/if}
            </span>
            <span class="flex flex-col items-end gap-1.5 flex-none">
              <span data-testid="groups-badge" class="inline-flex items-center h-6 px-2.5 rounded-full bg-[var(--hv-primary-selected-bg)] text-[var(--hv-primary)] text-c-xs font-bold">
                {$t('groups.member_count', { count: String(g.member_count) })}
              </span>
              {#if isProtected(g)}
                <span data-testid="groups-state" class="sys inline-flex items-center gap-1 text-c-xs text-[var(--hv-shell-text)]">
                  <IconShield size={13} stroke={2} /> {$t('groups.system_badge')}
                </span>
              {/if}
            </span>
          </button>
        {/each}
      {/if}
    </div>
  {/snippet}

  {#if !narrow || detailOpen}
    <div data-testid="groups-detail" class="flex flex-col min-h-0">
      {#if creating}
        <div class="flex items-center gap-3 mb-4">
          {#if narrow}
            <button data-testid="groups-icon-btn" class={cn('only-narrow', tileBtn)} onclick={closeDetail} aria-label={$t('common.cancel')}>
              <IconArrowLeft size={20} stroke={2} />
            </button>
          {/if}
          <div class="min-w-0">
            <div data-testid="groups-detail-h1" class="text-c-2xl font-semibold tracking-tight text-[var(--hv-shell-text-strong)] truncate">{$t('groups.new')}</div>
            <div class="text-c-base text-[var(--hv-shell-text)] mt-0.5 truncate">{$t('groups.create_sub')}</div>
          </div>
        </div>

        {#if errors.action}
          <div data-testid="groups-err" class="px-3 py-2.5 rounded-lg bg-[var(--hv-error-bg)] border border-[var(--hv-error)] mb-3 text-xs text-[var(--hv-error-text)]">{errors.action}</div>
        {/if}

        <SectionCard data-testid="groups-card" class="mb-4">
          <FormRow
            label={$t('groups.name_label')}
            for="c-name"
            required={true}
            error={errors.name}
            help={errors.name ? undefined : $t('groups.name_hint')}
          >
            <input
              id="c-name"
              class={cn(inp, 'font-[family-name:var(--hv-font-mono)]', errors.name && 'border-[var(--hv-error)]')}
              bind:value={createForm.name}
              placeholder="ex. developpeurs"
              autocomplete="off"
            />
          </FormRow>

          <FormRow
            label={$t('groups.description_label')}
            for="c-desc"
            help={$t('groups.description_hint')}
          >
            <input
              id="c-desc"
              data-testid="groups-create-description"
              class={inp}
              bind:value={createForm.description}
              maxlength="200"
              autocomplete="off"
            />
          </FormRow>

          <div class="mt-3">
            <span class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('groups.initial_members')}</span>
            {#if loadingUsers}
              <div data-testid="groups-hint" class="text-c-label text-[var(--hv-shell-text)] mt-1.5">{$t('users.loading')}</div>
            {:else if (allUsernames ?? []).length === 0}
              <div data-testid="groups-hint" class="text-c-label text-[var(--hv-shell-text)] mt-1.5">{$t('groups.no_users_available')}</div>
            {:else}
              <div data-testid="groups-search" class="sub flex items-center gap-2 h-10 px-3 rounded-lg bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] text-[var(--hv-shell-text)] mt-2.5">
                <IconSearch size={16} stroke={1.9} />
                <input type="search" placeholder={$t('groups.search_user_placeholder')} bind:value={createSearch} autocomplete="off" class="flex-1 min-w-0 bg-transparent border-none outline-none text-[var(--hv-shell-text-strong)] text-sm placeholder:text-[var(--hv-shell-text-3)]" />
              </div>
              <div data-testid="groups-member-count" class="text-xs font-semibold text-[var(--hv-primary)] mt-2">
                {$t('groups.members_selected', { count: String(createForm.members.length) })}
              </div>
              <div class="flex flex-col gap-1.5 mt-2.5 max-h-80 overflow-y-auto">
                {#each filteredCreateUsers as u (u)}
                  <button
                    type="button"
                    data-testid="groups-user"
                    class:on={createForm.members.includes(u)}
                    class={cn(
                      userRow,
                      createForm.members.includes(u)
                        ? 'border-[var(--hv-primary)] bg-[var(--hv-primary-light)]'
                        : 'border-[var(--hv-shell-border)] bg-[var(--hv-shell-fill)] hover:bg-[var(--hv-shell-border-soft)]',
                    )}
                    onclick={() => toggleCreateMember(u)}
                  >
                    <span class={cn(check, createForm.members.includes(u) ? 'bg-[var(--hv-primary)] border-transparent' : 'border-[var(--hv-shell-border-strong)]')}>
                      {#if createForm.members.includes(u)}<IconCheck size={14} stroke={3} />{/if}
                    </span>
                    <span class="flex-1 min-w-0 truncate">@{u}</span>
                  </button>
                {/each}
                {#if filteredCreateUsers.length === 0}
                  <div data-testid="groups-hint" class="pad text-c-label text-[var(--hv-shell-text)] py-3">{$t('groups.no_user_match')}</div>
                {/if}
              </div>
            {/if}
          </div>

          <div class="flex flex-wrap gap-2.5 mt-4">
            <Button variant="primary" onclick={handleCreate} disabled={submitting}>
              <IconCheck size={18} stroke={2.2} /> {$t('groups.create_btn')}
            </Button>
            {#if groups.length > 0}
              <Button variant="ghost" onclick={closeDetail} disabled={submitting} data-testid="groups-cancel">{$t('common.cancel')}</Button>
            {/if}
          </div>
        </SectionCard>

      {:else if selectedGroup}
        {@const prot = isProtected(selectedGroup)}
        <div class="flex items-center gap-3 mb-4">
          {#if narrow}
            <button data-testid="groups-icon-btn" class={cn('only-narrow', tileBtn)} onclick={closeDetail} aria-label={$t('users.back')}>
              <IconArrowLeft size={20} stroke={2} />
            </button>
          {/if}
          <span data-testid="groups-detail-avatar" class="w-[60px] h-[60px] rounded-2xl grid place-items-center text-xl font-bold text-white flex-none" style="background:{colorFor(selectedGroup)}">{initials(selectedGroup)}</span>
          <div class="flex-1 min-w-0">
            <div data-testid="groups-detail-h1" class="text-c-2xl font-semibold tracking-tight text-[var(--hv-shell-text-strong)] truncate">{selectedGroup.display_name || selectedGroup.name}</div>
            <div class="font-[family-name:var(--hv-font-mono)] text-c-base text-[var(--hv-shell-text)] mt-0.5 truncate">@{selectedGroup.display_name} · GID {selectedGroup.gid}</div>
            {#if selectedGroup.description}
              <div data-testid="groups-detail-description" class="text-c-base text-[var(--hv-shell-text)] mt-0.5 truncate">{selectedGroup.description}</div>
            {/if}
          </div>
          {#if prot}
            <span data-testid="groups-state" class="sys lg inline-flex items-center gap-1.5 h-[30px] px-3 rounded-full bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text)] text-xs flex-none">
              <IconShield size={14} stroke={2} /> {$t('groups.system_badge')}
            </span>
          {/if}
        </div>

        {#if errors.action}
          <div data-testid="groups-err" class="px-3 py-2.5 rounded-lg bg-[var(--hv-error-bg)] border border-[var(--hv-error)] mb-3 text-xs text-[var(--hv-error-text)]">{errors.action}</div>
        {/if}

        <SectionCard data-testid="groups-meta-card" class="mb-4">
          <div class="flex items-center gap-2 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)] mb-3">
            <IconUserCog size={16} stroke={2} /> {$t('groups.meta_title')}
          </div>

          {#if prot}
            <div data-testid="groups-meta-readonly" class="px-3 py-3 rounded-lg bg-[var(--hv-warning-light)] border border-[var(--hv-warning)] text-c-label leading-relaxed text-[var(--hv-warning)]">{$t('groups.meta_admin_readonly')}</div>
          {:else}
            <p class="m-0 text-c-base text-[var(--hv-shell-text)]">{$t('groups.meta_hint')}</p>

            {#if errors.meta}
              <div role="alert" data-testid="groups-meta-err" class="mt-3 px-3 py-2 rounded-lg bg-[var(--hv-error-bg)] text-[var(--hv-error-text)] text-sm">{errors.meta}</div>
            {/if}
            {#if metaOk}
              <p role="status" data-testid="groups-meta-ok" class="m-0 mt-3 text-xs text-[var(--hv-success)]">{metaOk}</p>
            {/if}

            <div class="mt-3 flex flex-col gap-3">
              <label class="flex flex-col gap-1 text-c-label font-semibold text-[var(--hv-shell-text-strong)]">
                {$t('groups.rename_label')}
                <input
                  data-testid="groups-rename-input"
                  class={cn(inp, 'font-[family-name:var(--hv-font-mono)]')}
                  bind:value={metaForm.name}
                  maxlength="32"
                  autocomplete="off"
                  aria-describedby="groups-rename-hint"
                />
                <span id="groups-rename-hint" class="text-c-xs font-normal text-[var(--hv-shell-text)]">{$t('groups.rename_hint')}</span>
              </label>

              <label class="flex flex-col gap-1 text-c-label font-semibold text-[var(--hv-shell-text-strong)]">
                {$t('groups.description_label')}
                <input
                  data-testid="groups-description-input"
                  class={inp}
                  bind:value={metaForm.description}
                  maxlength="200"
                  autocomplete="off"
                  aria-describedby="groups-description-hint"
                />
                <span id="groups-description-hint" class="text-c-xs font-normal text-[var(--hv-shell-text)]">{$t('groups.description_hint')}</span>
              </label>
            </div>

            <Button variant="primary" size="sm" class="mt-3" onclick={saveMeta} disabled={metaBusy}>
              <IconCheck size={16} stroke={2.2} /> {$t('common.save')}
            </Button>
          {/if}
        </SectionCard>

        <SectionCard data-testid="groups-card" class="mb-4">
          <div class="flex items-center justify-between gap-2.5 mb-3">
            <div class="flex items-center gap-2 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]"><IconUserCog size={16} stroke={2} /> {$t('groups.members')}</div>
            <span data-testid="groups-member-count" class="text-xs font-semibold text-[var(--hv-primary)]">{$t('groups.members_selected', { count: String(membersSelected.length) })}</span>
          </div>

          {#if prot}
            <div data-testid="groups-warn" class="px-3 py-3 rounded-lg mb-3 bg-[var(--hv-warning-light)] border border-[var(--hv-warning)] text-c-label leading-relaxed text-[var(--hv-warning)]">{$t('groups.admin_members_readonly')}</div>
          {/if}

          {#if loadingMembers}
            <div data-testid="groups-hint" class="text-c-label text-[var(--hv-shell-text)]">{$t('users.loading')}</div>
          {:else}
            <div data-testid="groups-search" class="sub flex items-center gap-2 h-10 px-3 rounded-lg bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] text-[var(--hv-shell-text)]">
              <IconSearch size={16} stroke={1.9} />
              <input type="search" placeholder={$t('groups.search_user_placeholder')} bind:value={memberSearch} autocomplete="off" class="flex-1 min-w-0 bg-transparent border-none outline-none text-[var(--hv-shell-text-strong)] text-sm placeholder:text-[var(--hv-shell-text-3)]" />
            </div>
            <div class="flex flex-col gap-1.5 mt-2.5 max-h-80 overflow-y-auto">
              {#each filteredMemberUsers as u (u)}
                <button
                  type="button"
                  data-testid="groups-user"
                  class:on={membersSelected.includes(u)}
                  class:readonly={prot}
                  disabled={prot}
                  class={cn(
                    userRow,
                    prot && 'cursor-default opacity-65',
                    membersSelected.includes(u)
                      ? 'border-[var(--hv-primary)] bg-[var(--hv-primary-light)]'
                      : 'border-[var(--hv-shell-border)] bg-[var(--hv-shell-fill)] hover:bg-[var(--hv-shell-border-soft)]',
                  )}
                  onclick={() => toggleMember(u)}
                >
                  <span class={cn(check, membersSelected.includes(u) ? 'bg-[var(--hv-primary)] border-transparent' : 'border-[var(--hv-shell-border-strong)]')}>
                    {#if membersSelected.includes(u)}<IconCheck size={14} stroke={3} />{/if}
                  </span>
                  <span class="flex-1 min-w-0 truncate">@{u}</span>
                </button>
              {/each}
              {#if filteredMemberUsers.length === 0}
                <div data-testid="groups-hint" class="pad text-c-label text-[var(--hv-shell-text)] py-3">{$t('groups.no_user_match')}</div>
              {/if}
            </div>
            {#if errors.membersOk}<span data-testid="groups-ok" class="text-xs text-[var(--hv-success)]">{errors.membersOk}</span>{/if}
            {#if !prot}
              <Button variant="primary" size="sm" fullWidth class="mt-3" onclick={saveMembers} disabled={submitting}>
                <IconCheck size={16} stroke={2.2} /> {$t('common.save')}
              </Button>
            {/if}
          {/if}
        </SectionCard>

        <SectionCard data-testid="groups-card" class="mb-4">
          <div class="flex items-center gap-2 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)] mb-3"><IconGauge size={16} stroke={2} /> {$t('groups.sheet_quota')}</div>
          {#if loadingQuota}
            <div data-testid="groups-hint" class="text-c-label text-[var(--hv-shell-text)]">{$t('users.loading')}</div>
          {:else}
            <div data-testid="groups-quota-block" class="py-3 border-t border-[var(--hv-shell-border-soft)] first:border-t-0 first:pt-1">
              <div class="flex items-center gap-3">
                <div class="min-w-0 flex-1">
                  <strong class="block text-sm font-semibold text-[var(--hv-shell-text-strong)]">{$t('groups.quota_upload_title')}</strong>
                  <small class="text-c-xs text-[var(--hv-shell-text)]">{$t('groups.quota_upload_desc')}</small>
                </div>
                <button
                  type="button"
                  data-testid="groups-switch"
                  class:on={quotaForm.uploadEnabled}
                  role="switch"
                  aria-checked={quotaForm.uploadEnabled}
                  aria-label={$t('groups.quota_upload_title')}
                  onclick={() => (quotaForm.uploadEnabled = !quotaForm.uploadEnabled)}
                  class={cn(
                    'w-14 h-8 flex-none p-1 border-none rounded-full flex items-center transition-colors',
                    quotaForm.uploadEnabled ? 'bg-[var(--hv-primary)] justify-end' : 'bg-[var(--hv-shell-border)] justify-start',
                  )}
                >
                  <span class="w-6 h-6 rounded-full bg-white shadow-[var(--hv-shadow-sm)]"></span>
                </button>
              </div>
              {#if quotaForm.uploadEnabled}
                <div class="flex gap-2.5 mt-2.5">
                  <input data-testid="groups-input" class={cn(inp, 'flex-1')} type="number" min="0" step="1" bind:value={quotaForm.uploadValue} aria-label={$t('groups.quota_value_label')} />
                  <select data-testid="groups-input" class={cn(inp, 'flex-[0_0_90px]')} bind:value={quotaForm.uploadUnit} aria-label={$t('groups.quota_unit_label')}>
                    <option value="MB">MB</option><option value="TB">TB</option><option value="PB">PB</option>
                  </select>
                </div>
                <div class="h-2 mt-2.5 rounded-full bg-[var(--hv-shell-border-soft)] overflow-hidden">
                  <span class="block h-full rounded-full bg-[var(--hv-primary)]" style="width:{quotaBarPct(quotaUsage.uploadUsedBytes, quotaForm.uploadValue, quotaForm.uploadUnit)}%"></span>
                </div>
                <p data-testid="groups-hint" class="text-c-label text-[var(--hv-shell-text)] mt-2">{$t('groups.quota_used', { used: formatBytes(quotaUsage.uploadUsedBytes) })}</p>
              {/if}
            </div>

            <div data-testid="groups-quota-block" class="py-3 border-t border-[var(--hv-shell-border-soft)] first:border-t-0 first:pt-1">
              <div class="flex items-center gap-3">
                <div class="min-w-0 flex-1">
                  <strong class="block text-sm font-semibold text-[var(--hv-shell-text-strong)]">{$t('groups.quota_download_title')}</strong>
                  <small class="text-c-xs text-[var(--hv-shell-text)]">{$t('groups.quota_download_desc')}</small>
                </div>
                <button
                  type="button"
                  data-testid="groups-switch"
                  class:on={quotaForm.downloadEnabled}
                  role="switch"
                  aria-checked={quotaForm.downloadEnabled}
                  aria-label={$t('groups.quota_download_title')}
                  onclick={() => (quotaForm.downloadEnabled = !quotaForm.downloadEnabled)}
                  class={cn(
                    'w-14 h-8 flex-none p-1 border-none rounded-full flex items-center transition-colors',
                    quotaForm.downloadEnabled ? 'bg-[var(--hv-primary)] justify-end' : 'bg-[var(--hv-shell-border)] justify-start',
                  )}
                >
                  <span class="w-6 h-6 rounded-full bg-white shadow-[var(--hv-shadow-sm)]"></span>
                </button>
              </div>
              {#if quotaForm.downloadEnabled}
                <div class="flex gap-2.5 mt-2.5">
                  <input data-testid="groups-input" class={cn(inp, 'flex-1')} type="number" min="0" step="1" bind:value={quotaForm.downloadValue} aria-label={$t('groups.quota_value_label')} />
                  <select data-testid="groups-input" class={cn(inp, 'flex-[0_0_90px]')} bind:value={quotaForm.downloadUnit} aria-label={$t('groups.quota_unit_label')}>
                    <option value="MB">MB</option><option value="TB">TB</option><option value="PB">PB</option>
                  </select>
                </div>
                <div class="h-2 mt-2.5 rounded-full bg-[var(--hv-shell-border-soft)] overflow-hidden">
                  <span class="block h-full rounded-full bg-[var(--hv-primary)]" style="width:{quotaBarPct(quotaUsage.downloadUsedBytes, quotaForm.downloadValue, quotaForm.downloadUnit)}%"></span>
                </div>
                <p data-testid="groups-hint" class="text-c-label text-[var(--hv-shell-text)] mt-2">{$t('groups.quota_used', { used: formatBytes(quotaUsage.downloadUsedBytes) })}</p>
              {/if}
            </div>

            <div class="py-3 border-t border-[var(--hv-shell-border-soft)] flex items-center gap-3 flex-wrap">
              <div class="min-w-0 flex-1">
                <label for="g-period" class="block text-sm font-semibold text-[var(--hv-shell-text-strong)]">{$t('quotas.period_label')}</label>
                <small class="text-c-xs text-[var(--hv-shell-text)]">{$t('quotas.period_hint')}</small>
              </div>
              <select id="g-period" data-testid="groups-quota-period" class={cn(inp, 'flex-[0_0_180px]')} bind:value={quotaForm.period}>
                {#each QUOTA_PERIODS as p (p.id)}<option value={p.id}>{$t(p.label)}</option>{/each}
              </select>
            </div>
            {#if quotaForm.period !== 'lifetime' && quotaForm.period === quotaSaved.period && quotaSaved.resetsAt}
              <p data-testid="groups-quota-resets" class="text-c-label text-[var(--hv-shell-text)]">{$t('quotas.resets_on', { date: formatResetDate(quotaSaved.resetsAt) })}</p>
            {:else if quotaForm.period !== quotaSaved.period}
              <p data-testid="groups-quota-resets" class="text-c-label text-[var(--hv-shell-text)]">{$t('quotas.period_change_resets')}</p>
            {/if}

            {#if errors.quotaOk}<span data-testid="groups-ok" class="text-xs text-[var(--hv-success)]">{errors.quotaOk}</span>{/if}
            <Button variant="primary" size="sm" fullWidth class="mt-3" onclick={saveQuota} disabled={submitting}>
              <IconCheck size={16} stroke={2.2} /> {$t('common.save')}
            </Button>
          {/if}
        </SectionCard>

        <SectionCard data-testid="groups-card" class="mb-4 bg-[var(--hv-error-bg)] border-[color:var(--hv-error)]/40">
          <div class="flex items-center gap-3 flex-wrap">
            <span class="grid place-items-center w-11 h-11 rounded-xl bg-[var(--hv-error)]/25 text-[var(--hv-error)] flex-none"><IconTrash size={20} stroke={1.9} /></span>
            <div class="min-w-0 flex-1">
              <strong class="block text-sm font-semibold text-[var(--hv-shell-text-strong)]">{$t('groups.delete')}</strong>
              <small class="text-c-xs text-[var(--hv-shell-text)]">{prot ? $t('groups.protected_desc') : $t('groups.delete_action_desc')}</small>
            </div>
            <Button variant="danger" onclick={handleDelete} disabled={submitting || prot}>{$t('common.delete')}</Button>
          </div>
        </SectionCard>

      {:else}
        <div data-testid="groups-noselect" class="flex-1 min-h-0 flex flex-col items-center justify-center gap-3 py-10 text-center text-[var(--hv-shell-text)]">
          <IconUsers size={54} stroke={1.3} class="text-[var(--hv-shell-border-strong)]" />
          <div data-testid="groups-noselect-title" class="text-c-xl font-semibold text-[var(--hv-shell-text-strong)]">{$t('groups.no_selection_title')}</div>
          <p class="m-0 mb-2 text-c-base max-w-xs">{$t('groups.no_selection_desc')}</p>
          <Button variant="primary" onclick={startCreate}>
            <IconPlus size={18} stroke={2.2} /> {$t('groups.new')}
          </Button>
        </div>
      {/if}
    </div>
  {/if}
  <ConfirmDialog
    {dlg}
    onconfirm={() => dlg.settle(true)}
    oncancel={() => dlg.settle(false)}
  />
</PageLayout>