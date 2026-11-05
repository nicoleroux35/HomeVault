<!-- ========================================================================
     File          : Iscsi.svelte
     Author        : Nicolas Le Roux
     Created       : 01/11/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
     iSCSI exports (ROADMAP P3.3). Two things to create, then one button to
     activate: a disk (a file the NAS owns, or a raw block device) and a
     target (the IQN an initiator logs in to, with its portals, LUNs,
     allowed initiators and optional CHAP). Saving any of it only writes
     HomeVault's metadata — `Apply` is what actually runs `targetcli`, so an
     admin can prepare a whole export before it is reachable.

     The editors are hidden on a host that cannot export (no `targetcli`,
     no LIO modules, no configfs) and the banner says exactly what is
     missing, instead of letting every save fail later.

     Dependencies:
       - svelte (>= 5), theme + app composites, $lib/services/api

     Usage:
       Registered as the "iSCSI" desktop app (admin only).
     ======================================================================== -->
<script lang="ts">
  /**
   * Iscsi — disks + targets, applied on demand.
   *
   * Accessibility: the banner is a `role="status"` live region, every
   * compact icon button carries an `aria-label` and the shared 44px `.hv-ic`
   * hit square, every input has a label (visible or `sr-only`), and both
   * editors live in `theme/Modal` so focus is trapped and restored.
   */
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { t, translate } from '$lib/stores/i18n';
  import { Spinner, Button, Alert, Badge, Checkbox, Modal } from '$lib/components/theme';
  import { PageLayout, SectionCard, EmptyState, ConfirmDialog } from '$lib/components/app';
  import { notifyError, notifySuccess } from '$lib/utils/notify';
  import { createConfirmState } from '$lib/utils/confirmDialog.svelte';
  import { cn } from '$lib/utils.js';
  import {
    IconServer,
    IconDatabase,
    IconCheck,
    IconPlus,
    IconPencil,
    IconTrash,
    IconRefresh,
    IconInfoCircle,
    IconAlertTriangle,
  } from '@tabler/icons-svelte';

  interface Capabilities {
    kernel_available: boolean;
    configfs_available: boolean;
    targetcli_available: boolean;
    modules_loaded: string[];
    modules_missing: string[];
    usable: boolean;
  }

  interface Backstore {
    id: number;
    name: string;
    kind: string;
    path: string;
    size_bytes: number;
    readonly: boolean;
    write_policy: string;
    emulate_fua: boolean;
    unmap: boolean;
    created_at: string;
    updated_at: string;
    lun_count: number;
  }

  interface Portal {
    id: number;
    target_id: number;
    address: string;
    port: number;
  }

  interface Lun {
    id: number;
    target_id: number;
    lun: number;
    backstore_id: number;
    backstore_name: string;
  }

  interface Acl {
    id: number;
    target_id: number;
    initiator_iqn: string;
    userid: string | null;
    has_secret: boolean;
  }

  interface Target {
    id: number;
    iqn: string;
    enabled: boolean;
    created_at: string;
    updated_at: string;
    portals: Portal[];
    luns: Lun[];
    acls: Acl[];
  }

  interface Settings {
    enabled: boolean;
    applied_at: string | null;
    updated_at: string;
  }

  interface Defaults {
    portal_address: string;
    portal_port: number;
    iqn: string;
  }

  interface IscsiState {
    usable: boolean;
    enabled: boolean;
    pending: boolean;
    applied_at: string | null;
    exported_targets: number;
  }

  interface ApplyReport {
    applied: boolean;
    reason: string | null;
    reason_code: string | null;
    commands: string[];
    failed: string | null;
  }

  interface BackstoreDraft {
    id: number | null;
    name: string;
    kind: 'fileio' | 'block';
    path: string;
    size_gib: number;
    readonly: boolean;
    write_policy: 'buffered' | 'direct';
    emulate_fua: boolean;
    unmap: boolean;
  }

  interface PortalDraft {
    address: string;
    port: number;
  }

  interface LunDraft {
    lun: number;
    backstore_id: number;
  }

  interface AclDraft {
    initiator_iqn: string;
    userid: string;
    secret: string;
    has_secret: boolean;
  }

  interface TargetDraft {
    id: number | null;
    iqn: string;
    enabled: boolean;
    portals: PortalDraft[];
    luns: LunDraft[];
    acls: AclDraft[];
  }

  let settings = $state<Settings>({ enabled: false, applied_at: null, updated_at: '' });
  let capabilities = $state<Capabilities | null>(null);
  let backstores = $state<Backstore[]>([]);
  let targets = $state<Target[]>([]);
  let defaults = $state<Defaults>({ portal_address: '0.0.0.0', portal_port: 3260, iqn: '' });
  let liveState = $state<IscsiState>({
    usable: false,
    enabled: false,
    pending: false,
    applied_at: null,
    exported_targets: 0,
  });
  let lastApply = $state<ApplyReport | null>(null);

  let loading = $state(true);
  let busy = $state<'settings' | 'apply' | 'backstore' | 'target' | 'delete-store' | null>(null);
  let error = $state('');

  let storeOpen = $state(false);
  let storeDraft = $state<BackstoreDraft>(emptyStore());
  let storeError = $state('');

  let deleteStore = $state<Backstore | null>(null);
  let deleteFile = $state(false);
  let confirmName = $state('');

  let targetOpen = $state(false);
  let targetDraft = $state<TargetDraft>(emptyTarget());
  let targetError = $state('');

  const dlg = createConfirmState();

  const usable = $derived(capabilities?.usable === true);

  const fieldLabel = 'text-c-label font-semibold text-[var(--hv-shell-text-strong)]';
  const hint = 'm-0 text-c-label text-[var(--hv-shell-text-2)]';
  const inp =
    'w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-c-base text-[var(--hv-shell-text-strong)] outline-none focus-visible:border-[var(--hv-primary)] focus-visible:shadow-[var(--hv-focus-ring)]';
  const iconBtn =
    'grid place-items-center w-9 h-9 flex-none rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:bg-[var(--hv-shell-border-soft)] hover:text-[var(--hv-shell-text-strong)] hv-ic';

  onMount(load);

  async function load() {
    loading = true;
    error = '';
    try {
      const resp = await api.get('/storage/iscsi');
      if (resp.success && resp.data) {
        applyPayload(resp.data);
      } else {
        error = resp.message || translate('iscsi.load_failed');
      }
    } catch {
      error = translate('iscsi.load_failed');
    } finally {
      loading = false;
    }
  }

  function applyPayload(data: any) {
    settings = data.settings as Settings;
    capabilities = data.capabilities as Capabilities;
    backstores = (data.backstores ?? []) as Backstore[];
    targets = (data.targets ?? []) as Target[];
    if (data.defaults) defaults = data.defaults as Defaults;
    liveState = data.state as IscsiState;
    if (data.apply) lastApply = data.apply as ApplyReport;
  }

  function emptyStore(): BackstoreDraft {
    return { id: null, name: '', kind: 'fileio', path: '', size_gib: 10, readonly: false, write_policy: 'buffered', emulate_fua: true, unmap: true };
  }

  function emptyTarget(): TargetDraft {
    return { id: null, iqn: '', enabled: true, portals: [], luns: [], acls: [] };
  }

  function formatSize(bytes: number): string {
    if (!bytes) return translate('iscsi.backstore_device');
    const gib = bytes / 1024 ** 3;
    if (gib >= 1024) {
      const tib = gib / 1024;
      return `${tib % 1 === 0 ? tib.toFixed(0) : tib.toFixed(1)} TiB`;
    }
    return `${gib % 1 === 0 ? gib.toFixed(0) : gib.toFixed(1)} GiB`;
  }

  function reasonText(code: string | null | undefined): string {
    if (!code) return translate('iscsi.reason_none');
    const key = `iscsi.reason_${code}`;
    const text = translate(key);
    return text === key ? translate('iscsi.reason_none') : text;
  }

  // ------------------------------------------------------------------
  // Feature switch
  // ------------------------------------------------------------------

  async function setEnabled(enabled: boolean) {
    busy = 'settings';
    error = '';
    try {
      const resp = await api.put('/storage/iscsi/settings', { enabled });
      if (resp.success && resp.data) {
        applyPayload(resp.data);
        notifySuccess(enabled ? 'iscsi.enabled_done' : 'iscsi.disabled_done');
      } else {
        error = resp.message || translate('iscsi.save_failed');
        notifyError(resp);
      }
    } catch {
      error = translate('iscsi.save_failed');
    } finally {
      busy = null;
    }
  }

  async function applyNow() {
    busy = 'apply';
    error = '';
    try {
      const resp = await api.post('/storage/iscsi/apply');
      if (resp.success && resp.data) {
        applyPayload(resp.data);
        const report = resp.data.apply as ApplyReport | null;
        if (report?.failed) {
          notifyError({ message: report.failed });
        } else if (report?.applied) {
          notifySuccess('iscsi.applied');
        } else {
          notifySuccess(`iscsi.reason_${report?.reason_code ?? 'empty'}`);
        }
      } else {
        notifyError(resp);
      }
    } catch {
      notifyError({ message: translate('iscsi.apply_failed') });
    } finally {
      busy = null;
    }
  }

  // ------------------------------------------------------------------
  // Backstores
  // ------------------------------------------------------------------

  function openNewStore() {
    storeDraft = emptyStore();
    storeError = '';
    storeOpen = true;
  }

  function openEditStore(store: Backstore) {
    storeDraft = {
      id: store.id,
      name: store.name,
      kind: store.kind === 'block' ? 'block' : 'fileio',
      path: store.path,
      size_gib: Math.max(1, Math.round(store.size_bytes / 1024 ** 3)),
      readonly: store.readonly,
      write_policy: store.write_policy === 'direct' ? 'direct' : 'buffered',
      emulate_fua: store.emulate_fua !== false,
      unmap: store.unmap !== false,
    };
    storeError = '';
    storeOpen = true;
  }

  function closeStore() {
    storeOpen = false;
    storeError = '';
  }

  async function saveStore() {
    storeError = '';
    busy = 'backstore';
    const id = storeDraft.id;
    const body = {
      name: storeDraft.name.trim(),
      kind: storeDraft.kind,
      path: storeDraft.path.trim(),
      size_bytes: storeDraft.kind === 'fileio' ? Math.round(storeDraft.size_gib * 1024 ** 3) : 0,
      readonly: storeDraft.readonly,
      write_policy: storeDraft.write_policy,
      emulate_fua: storeDraft.emulate_fua,
      unmap: storeDraft.unmap,
    };
    try {
      const resp =
        id === null
          ? await api.post('/storage/iscsi/backstores', body)
          : await api.put(`/storage/iscsi/backstores/${id}`, body);
      if (resp.success && resp.data) {
        applyPayload(resp.data);
        closeStore();
        notifySuccess('iscsi.backstore_saved');
      } else {
        storeError = resp.message || translate('iscsi.save_failed');
      }
    } catch {
      storeError = translate('iscsi.save_failed');
    } finally {
      busy = null;
    }
  }

  function askDeleteStore(store: Backstore) {
    if (store.lun_count > 0) {
      notifyError({ message: translate('iscsi.backstore_in_use_error', { count: String(store.lun_count) }) });
      return;
    }
    deleteStore = store;
    deleteFile = false;
    confirmName = '';
    storeError = '';
  }

  function closeDeleteStore() {
    deleteStore = null;
    storeError = '';
  }

  async function confirmDeleteStore() {
    if (!deleteStore) return;
    storeError = '';
    busy = 'delete-store';
    try {
      const params = new URLSearchParams();
      if (deleteFile) {
        params.set('delete_file', 'true');
        params.set('confirm', confirmName.trim());
      }
      const query = params.toString();
      const resp = await api.delete(
        `/storage/iscsi/backstores/${deleteStore.id}${query ? `?${query}` : ''}`,
      );
      if (resp.success && resp.data) {
        applyPayload(resp.data);
        closeDeleteStore();
        notifySuccess('iscsi.backstore_deleted');
      } else {
        storeError = resp.message || translate('iscsi.save_failed');
      }
    } catch {
      storeError = translate('iscsi.save_failed');
    } finally {
      busy = null;
    }
  }

  // ------------------------------------------------------------------
  // Targets
  // ------------------------------------------------------------------

  function openNewTarget() {
    targetDraft = {
      ...emptyTarget(),
      iqn: defaults.iqn,
      portals: [{ address: defaults.portal_address, port: defaults.portal_port }],
    };
    targetError = '';
    targetOpen = true;
  }

  function openEditTarget(target: Target) {
    targetDraft = {
      id: target.id,
      iqn: target.iqn,
      enabled: target.enabled,
      portals: target.portals.map((p) => ({ address: p.address, port: p.port })),
      luns: target.luns.map((l) => ({ lun: l.lun, backstore_id: l.backstore_id })),
      acls: target.acls.map((a) => ({
        initiator_iqn: a.initiator_iqn,
        userid: a.userid ?? '',
        secret: '',
        has_secret: a.has_secret,
      })),
    };
    targetError = '';
    targetOpen = true;
  }

  function closeTarget() {
    targetOpen = false;
    targetError = '';
  }

  function addPortal() {
    targetDraft.portals = [
      ...targetDraft.portals,
      { address: defaults.portal_address, port: defaults.portal_port },
    ];
  }

  function removePortal(index: number) {
    targetDraft.portals = targetDraft.portals.filter((_, i) => i !== index);
  }

  function addLun() {
    if (backstores.length === 0) return;
    const used = new Set(targetDraft.luns.map((l) => l.lun));
    let next = 0;
    while (used.has(next)) next += 1;
    const free = backstores.find(
      (b) => !targetDraft.luns.some((l) => l.backstore_id === b.id),
    );
    targetDraft.luns = [
      ...targetDraft.luns,
      { lun: next, backstore_id: (free ?? backstores[0]).id },
    ];
  }

  function removeLun(index: number) {
    targetDraft.luns = targetDraft.luns.filter((_, i) => i !== index);
  }

  function addAcl() {
    targetDraft.acls = [
      ...targetDraft.acls,
      { initiator_iqn: '', userid: '', secret: '', has_secret: false },
    ];
  }

  function removeAcl(index: number) {
    targetDraft.acls = targetDraft.acls.filter((_, i) => i !== index);
  }

  async function saveTarget() {
    targetError = '';
    busy = 'target';
    const id = targetDraft.id;
    const body = {
      iqn: targetDraft.iqn.trim(),
      enabled: targetDraft.enabled,
      portals: targetDraft.portals.map((p) => ({
        address: p.address.trim(),
        port: Number(p.port),
      })),
      luns: targetDraft.luns.map((l) => ({ lun: Number(l.lun), backstore_id: Number(l.backstore_id) })),
      acls: targetDraft.acls
        .filter((a) => a.initiator_iqn.trim() !== '')
        .map((a) => ({
          initiator_iqn: a.initiator_iqn.trim(),
          userid: a.userid.trim() || null,
          secret: a.secret ? a.secret : null,
        })),
    };
    try {
      const resp =
        id === null
          ? await api.post('/storage/iscsi/targets', body)
          : await api.put(`/storage/iscsi/targets/${id}`, body);
      if (resp.success && resp.data) {
        applyPayload(resp.data);
        closeTarget();
        notifySuccess('iscsi.target_saved');
      } else {
        targetError = resp.message || translate('iscsi.save_failed');
      }
    } catch {
      targetError = translate('iscsi.save_failed');
    } finally {
      busy = null;
    }
  }

  async function askDeleteTarget(target: Target) {
    const ok = await dlg.ask(translate('iscsi.target_delete_confirm', { iqn: target.iqn }), {
      danger: true,
    });
    if (!ok) return;
    try {
      const resp = await api.delete(`/storage/iscsi/targets/${target.id}`);
      if (resp.success && resp.data) {
        applyPayload(resp.data);
        notifySuccess('iscsi.target_deleted');
      } else {
        notifyError(resp);
      }
    } catch {
      notifyError({ message: translate('iscsi.save_failed') });
    }
  }

  function targetSummary(target: Target): string {
    return translate('iscsi.target_summary', {
      portals: String(target.portals.length),
      luns: String(target.luns.length),
      initiators: String(target.acls.length),
    });
  }

  function backstoreName(id: number): string {
    return backstores.find((b) => b.id === id)?.name ?? `#${id}`;
  }
</script>

<PageLayout title={$t('iscsi.title')} data-testid="iscsi-page">
  {#snippet icon()}
    <IconServer stroke={1.8} />
  {/snippet}

  {#snippet toolbarRight()}
    <div class="flex items-center gap-2">
      <Button
        variant="secondary"
        data-testid="iscsi-apply"
        disabled={busy !== null || !usable}
        onclick={applyNow}
      >
        <IconRefresh size={16} stroke={2} /> {$t('iscsi.apply_now')}
      </Button>
    </div>
  {/snippet}

  {#if loading}
    <div data-testid="iscsi-loading" class="flex items-center justify-center gap-3 py-16 text-[var(--hv-shell-text)]">
      <Spinner />
      <span>{$t('common.loading')}</span>
    </div>
  {:else}
    <div data-testid="iscsi-content" class="flex flex-col gap-4 max-w-[880px]">
      {#if error}
        <Alert variant="error">{error}</Alert>
      {/if}

      <!-- Live status: what the host can do, whether exports are reachable,
           and whether the saved metadata has been applied yet. -->
      <div
        data-testid="iscsi-status"
        role="status"
        class={cn(
          'flex items-start gap-3 p-4 rounded-xl border',
          !usable || lastApply?.failed
            ? 'bg-[var(--hv-warning-bg)] border-[var(--hv-warning)]'
            : settings.enabled && !liveState.pending
              ? 'bg-[var(--hv-success-bg)] border-[var(--hv-success)]'
              : 'bg-[var(--hv-shell-border-soft)] border-[var(--hv-shell-border)]',
        )}
      >
        {#if !usable || lastApply?.failed}
          <IconAlertTriangle size={20} stroke={2} class="flex-none text-[var(--hv-warning)]" aria-hidden="true" />
        {:else if settings.enabled && !liveState.pending}
          <IconCheck size={20} stroke={2} class="flex-none text-[var(--hv-success)]" aria-hidden="true" />
        {:else}
          <IconInfoCircle size={20} stroke={2} class="flex-none text-[var(--hv-shell-text-2)]" aria-hidden="true" />
        {/if}
        <div class="flex flex-col gap-1 min-w-0">
          {#if !usable}
            <span class="font-semibold text-[var(--hv-shell-text-strong)]" data-testid="iscsi-unusable">
              {$t('iscsi.status_unusable')}
            </span>
            <span class={hint}>{$t('iscsi.unusable_hint')}</span>
            <ul class="m-0 pl-5 flex flex-col gap-0.5 text-c-label text-[var(--hv-shell-text-2)]">
              {#if !capabilities?.targetcli_available}
                <li data-testid="iscsi-missing-targetcli">{$t('iscsi.missing_targetcli')}</li>
              {/if}
              {#if (capabilities?.modules_missing.length ?? 0) > 0}
                <li data-testid="iscsi-missing-modules">
                  {$t('iscsi.missing_modules', { modules: (capabilities?.modules_missing ?? []).join(', ') })}
                </li>
              {/if}
              {#if !capabilities?.configfs_available}
                <li data-testid="iscsi-missing-configfs">{$t('iscsi.missing_configfs')}</li>
              {/if}
            </ul>
          {:else if !settings.enabled}
            <span class="font-semibold text-[var(--hv-shell-text-strong)]">{$t('iscsi.status_off')}</span>
            <span class={hint}>{$t('iscsi.status_off_hint')}</span>
          {:else if liveState.pending}
            <span class="font-semibold text-[var(--hv-shell-text-strong)]" data-testid="iscsi-pending">
              {$t('iscsi.status_pending')}
            </span>
            <span class={hint}>{$t('iscsi.status_pending_hint')}</span>
          {:else}
            <span class="font-semibold text-[var(--hv-shell-text-strong)]" data-testid="iscsi-active">
              {$t('iscsi.status_on', { count: String(liveState.exported_targets) })}
            </span>
            <span class={hint}>{$t('iscsi.status_on_hint')}</span>
          {/if}
          {#if lastApply?.failed}
            <span class="text-c-label text-[var(--hv-danger)]" role="alert" data-testid="iscsi-apply-error">
              {lastApply.failed}
            </span>
          {:else if lastApply && !lastApply.applied && lastApply.reason_code}
            <span class="text-c-label text-[var(--hv-shell-text-2)]" data-testid="iscsi-apply-reason">
              {reasonText(lastApply.reason_code)}
            </span>
          {/if}
        </div>
      </div>

      <SectionCard
        title={$t('iscsi.enable_title')}
        description={$t('iscsi.enable_desc')}
        data-testid="iscsi-enable"
      >
        <div class="flex items-center justify-between gap-3 flex-wrap">
          <div class="flex flex-col gap-0.5 min-w-0">
            <span class="flex items-center gap-2">
              <span class={fieldLabel}>{$t('iscsi.enabled')}</span>
              <Badge variant={settings.enabled ? 'success' : 'secondary'} size="sm">
                {settings.enabled ? $t('iscsi.state_on') : $t('iscsi.state_off')}
              </Badge>
            </span>
            <span class={hint}>{$t('iscsi.enabled_hint')}</span>
          </div>
          <Button
            variant={settings.enabled ? 'secondary' : 'primary'}
            data-testid="iscsi-enable-toggle"
            loading={busy === 'settings'}
            disabled={busy !== null || (!usable && !settings.enabled)}
            onclick={() => setEnabled(!settings.enabled)}
          >
            {settings.enabled ? $t('iscsi.disable') : $t('iscsi.enable')}
          </Button>
        </div>
      </SectionCard>

      {#if usable}
        <SectionCard
          title={$t('iscsi.backstores_title')}
          description={$t('iscsi.backstores_desc')}
          data-testid="iscsi-backstores"
        >
          {#snippet actions()}
            <Button variant="secondary" size="sm" data-testid="iscsi-add-backstore" onclick={openNewStore}>
              <IconPlus size={15} stroke={2} /> {$t('iscsi.add_disk')}
            </Button>
          {/snippet}
          <div class="flex flex-col gap-2">
            {#if backstores.length === 0}
              <EmptyState
                icon="database"
                title={$t('iscsi.backstores_empty')}
                description={$t('iscsi.backstores_empty_hint')}
                data-testid="iscsi-backstores-empty"
              />
            {:else}
              {#each backstores as store (store.id)}
                <div
                  class="flex items-start justify-between gap-3 flex-wrap p-3 rounded-lg bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]"
                  data-testid="iscsi-backstore-{store.id}"
                >
                  <div class="flex flex-col gap-0.5 min-w-0">
                    <span class="flex items-center gap-2 flex-wrap">
                      <span class="font-semibold text-[var(--hv-shell-text-strong)]">{store.name}</span>
                      <Badge variant="secondary" size="sm">
                        {store.kind === 'block' ? $t('iscsi.kind_block') : $t('iscsi.kind_fileio')}
                      </Badge>
                      {#if store.readonly}
                        <Badge variant="info" size="sm">{$t('iscsi.backstore_readonly')}</Badge>
                      {/if}
                    </span>
                    <span class="text-c-label text-[var(--hv-shell-text-2)] font-mono break-all">{store.path}</span>
                    <span class="text-c-label text-[var(--hv-shell-text-2)]">
                      {store.kind === 'block' ? $t('iscsi.kind_block') : formatSize(store.size_bytes)}
                      <span class="px-1" aria-hidden="true">&middot;</span>
                      {store.lun_count > 0
                        ? $t('iscsi.backstore_in_use', { count: String(store.lun_count) })
                        : $t('iscsi.backstore_unused')}
                    </span>
                  </div>
                  <div class="flex items-center gap-1 flex-none">
                    <button
                      type="button"
                      class={iconBtn}
                      aria-label={$t('iscsi.edit_disk_named', { name: store.name })}
                      data-testid="iscsi-backstore-edit-{store.id}"
                      onclick={() => openEditStore(store)}
                    ><IconPencil size={16} stroke={1.8} /></button>
                    <button
                      type="button"
                      class={cn(iconBtn, 'hover:bg-[var(--hv-error-bg)] hover:text-[var(--hv-error)]')}
                      aria-label={$t('iscsi.delete_disk_named', { name: store.name })}
                      data-testid="iscsi-backstore-delete-{store.id}"
                      onclick={() => askDeleteStore(store)}
                    ><IconTrash size={16} stroke={1.8} /></button>
                  </div>
                </div>
              {/each}
            {/if}
          </div>
        </SectionCard>

        <SectionCard
          title={$t('iscsi.targets_title')}
          description={$t('iscsi.targets_desc')}
          data-testid="iscsi-targets"
        >
          {#snippet actions()}
            <Button variant="secondary" size="sm" data-testid="iscsi-add-target" onclick={openNewTarget}>
              <IconPlus size={15} stroke={2} /> {$t('iscsi.add_target')}
            </Button>
          {/snippet}
          <div class="flex flex-col gap-2">
            {#if targets.length === 0}
              <EmptyState
                icon="server"
                title={$t('iscsi.targets_empty')}
                description={$t('iscsi.targets_empty_hint')}
                data-testid="iscsi-targets-empty"
              />
            {:else}
              {#each targets as target (target.id)}
                <div
                  class="flex items-start justify-between gap-3 flex-wrap p-3 rounded-lg bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]"
                  data-testid="iscsi-target-{target.id}"
                >
                  <div class="flex flex-col gap-0.5 min-w-0">
                    <span class="flex items-center gap-2 flex-wrap">
                      <span class="font-mono text-c-base font-semibold text-[var(--hv-shell-text-strong)] break-all">
                        {target.iqn}
                      </span>
                      {#if target.enabled}
                        <Badge variant="success" size="sm" dot>{$t('iscsi.target_on')}</Badge>
                      {:else}
                        <Badge variant="secondary" size="sm">{$t('iscsi.target_off')}</Badge>
                      {/if}
                    </span>
                    <span class="text-c-label text-[var(--hv-shell-text-2)]">{targetSummary(target)}</span>
                  </div>
                  <div class="flex items-center gap-1 flex-none">
                    <button
                      type="button"
                      class={iconBtn}
                      aria-label={$t('iscsi.edit_target_named', { iqn: target.iqn })}
                      data-testid="iscsi-target-edit-{target.id}"
                      onclick={() => openEditTarget(target)}
                    ><IconPencil size={16} stroke={1.8} /></button>
                    <button
                      type="button"
                      class={cn(iconBtn, 'hover:bg-[var(--hv-error-bg)] hover:text-[var(--hv-error)]')}
                      aria-label={$t('iscsi.delete_target_named', { iqn: target.iqn })}
                      data-testid="iscsi-target-delete-{target.id}"
                      onclick={() => askDeleteTarget(target)}
                    ><IconTrash size={16} stroke={1.8} /></button>
                  </div>
                </div>
              {/each}
            {/if}
          </div>
        </SectionCard>
      {/if}
    </div>
  {/if}

  {#snippet footer()}
    {#if !loading}
      <span data-testid="iscsi-footer" class="flex items-center gap-2 flex-wrap">
        <span>
          {settings.enabled && !liveState.pending
            ? $t('iscsi.footer_on', { count: String(liveState.exported_targets) })
            : $t('iscsi.footer_off')}
        </span>
        <span class="text-[var(--hv-shell-text-3)]">&middot;</span>
        <span>{$t('iscsi.footer_counts', { disks: String(backstores.length), targets: String(targets.length) })}</span>
      </span>
    {/if}
  {/snippet}
</PageLayout>

<!-- ================= Backstore editor ================= -->
<Modal
  open={storeOpen}
  title={storeDraft.id === null ? $t('iscsi.backstore_new') : $t('iscsi.backstore_edit')}
  size="md"
  onclose={closeStore}
>
  {#snippet icon()}
    <IconDatabase size={20} stroke={1.9} />
  {/snippet}
  <div class="flex flex-col gap-4 p-1" data-testid="iscsi-backstore-modal">
    {#if storeError}
      <p role="alert" class="m-0 text-c-label text-[var(--hv-danger)]" data-testid="iscsi-backstore-error">
        {storeError}
      </p>
    {/if}

    <label class="flex flex-col gap-1.5">
      <span class={fieldLabel}>{$t('iscsi.backstore_name')}</span>
      <input class={inp} type="text" bind:value={storeDraft.name} data-testid="iscsi-backstore-name" />
      <span class={hint}>{$t('iscsi.backstore_name_hint')}</span>
    </label>

    <label class="flex flex-col gap-1.5">
      <span class={fieldLabel}>{$t('iscsi.backstore_kind')}</span>
      <select class={inp} bind:value={storeDraft.kind} data-testid="iscsi-backstore-kind">
        <option value="fileio">{$t('iscsi.kind_fileio')}</option>
        <option value="block">{$t('iscsi.kind_block')}</option>
      </select>
      <span class={hint}>
        {storeDraft.kind === 'block' ? $t('iscsi.backstore_kind_block_hint') : $t('iscsi.backstore_kind_fileio_hint')}
      </span>
    </label>

    <label class="flex flex-col gap-1.5">
      <span class={fieldLabel}>{$t('iscsi.backstore_path')}</span>
      <input
        class={cn(inp, 'font-mono')}
        type="text"
        bind:value={storeDraft.path}
        data-testid="iscsi-backstore-path"
        placeholder={storeDraft.kind === 'block' ? '/dev/disk/by-id/…' : '/path/to/disk.img'}
      />
      <span class={hint}>
        {storeDraft.kind === 'block' ? $t('iscsi.backstore_path_block_hint') : $t('iscsi.backstore_path_hint')}
      </span>
    </label>

    {#if storeDraft.kind === 'fileio'}
      <label class="flex flex-col gap-1.5">
        <span class={fieldLabel}>{$t('iscsi.backstore_size')}</span>
        <input
          class={inp}
          type="number"
          min="1"
          step="1"
          bind:value={storeDraft.size_gib}
          data-testid="iscsi-backstore-size"
        />
        <span class={hint}>{$t('iscsi.backstore_size_hint')}</span>
      </label>
    {/if}

    <label class="flex flex-col gap-1.5">
      <span class={fieldLabel}>{$t('iscsi.write_policy')}</span>
      <select class={inp} bind:value={storeDraft.write_policy} data-testid="iscsi-backstore-write-policy">
        <option value="buffered">{$t('iscsi.write_buffered')}</option>
        <option value="direct">{$t('iscsi.write_direct')}</option>
      </select>
      <span class={hint}>{$t('iscsi.write_policy_hint')}</span>
    </label>

    <Checkbox
      bind:checked={storeDraft.emulate_fua}
      label={$t('iscsi.emulate_fua')}
      description={$t('iscsi.emulate_fua_hint')}
    />
    <Checkbox
      bind:checked={storeDraft.unmap}
      label={$t('iscsi.unmap')}
      description={$t('iscsi.unmap_hint')}
    />

    <Checkbox
      bind:checked={storeDraft.readonly}
      label={$t('iscsi.backstore_readonly')}
      description={$t('iscsi.backstore_readonly_hint')}
    />
  </div>
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={closeStore}>{$t('common.cancel')}</Button>
      <Button
        variant="primary"
        data-testid="iscsi-backstore-save"
        loading={busy === 'backstore'}
        disabled={busy !== null}
        onclick={saveStore}
      >
        <IconCheck size={16} stroke={2} /> {$t('common.save')}
      </Button>
    </div>
  {/snippet}
</Modal>

<!-- ================= Forget a backstore ================= -->
<Modal
  open={deleteStore !== null}
  title={$t('iscsi.backstore_delete_title')}
  size="md"
  onclose={closeDeleteStore}
>
  {#snippet icon()}
    <IconTrash size={20} stroke={1.9} />
  {/snippet}
  <div class="flex flex-col gap-3 p-1" data-testid="iscsi-delete-store-modal">
    {#if deleteStore}
      {#if storeError}
        <p role="alert" class="m-0 text-c-label text-[var(--hv-danger)]" data-testid="iscsi-delete-store-error">
          {storeError}
        </p>
      {/if}
      <p class="m-0 text-c-label text-[var(--hv-shell-text)] leading-relaxed">
        {$t('iscsi.backstore_delete_desc', { name: deleteStore.name })}
      </p>
      {#if deleteStore.kind !== 'block'}
        <Checkbox
          bind:checked={deleteFile}
          label={$t('iscsi.backstore_delete_file', { path: deleteStore.path })}
          description={$t('iscsi.backstore_delete_file_hint')}
        />
      {/if}
      {#if deleteFile}
        <label class="flex flex-col gap-1.5">
          <span class={fieldLabel}>{$t('iscsi.backstore_confirm_name', { name: deleteStore.name })}</span>
          <input
            class={inp}
            type="text"
            bind:value={confirmName}
            autocomplete="off"
            data-testid="iscsi-delete-confirm-name"
          />
        </label>
      {/if}
    {/if}
  </div>
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={closeDeleteStore}>{$t('common.cancel')}</Button>
      <Button
        variant="danger"
        data-testid="iscsi-delete-store-confirm"
        loading={busy === 'delete-store'}
        disabled={
          busy !== null ||
          (deleteFile && deleteStore !== null && confirmName.trim() !== deleteStore.name)
        }
        onclick={confirmDeleteStore}
      >
        <IconTrash size={16} stroke={2} /> {$t('common.delete')}
      </Button>
    </div>
  {/snippet}
</Modal>

<!-- ================= Target editor ================= -->
<Modal
  open={targetOpen}
  title={targetDraft.id === null ? $t('iscsi.target_new') : $t('iscsi.target_edit')}
  size="lg"
  onclose={closeTarget}
>
  {#snippet icon()}
    <IconServer size={20} stroke={1.9} />
  {/snippet}
  <div class="flex flex-col gap-5 p-1" data-testid="iscsi-target-modal">
    {#if targetError}
      <p role="alert" class="m-0 text-c-label text-[var(--hv-danger)]" data-testid="iscsi-target-error">
        {targetError}
      </p>
    {/if}

    <label class="flex flex-col gap-1.5">
      <span class={fieldLabel}>{$t('iscsi.target_iqn')}</span>
      <input
        class={cn(inp, 'font-mono')}
        type="text"
        bind:value={targetDraft.iqn}
        disabled={targetDraft.id !== null}
        data-testid="iscsi-target-iqn"
      />
      <span class={hint}>
        {targetDraft.id === null ? $t('iscsi.target_iqn_hint') : $t('iscsi.target_iqn_locked')}
      </span>
    </label>

    <Checkbox
      bind:checked={targetDraft.enabled}
      label={$t('iscsi.target_enabled')}
      description={$t('iscsi.target_enabled_hint')}
    />

    <!-- Portals -->
    <div class="flex flex-col gap-2">
      <div class="flex items-center justify-between gap-2">
        <span class={fieldLabel}>{$t('iscsi.portals_title')}</span>
        <Button variant="secondary" size="sm" data-testid="iscsi-portal-add" onclick={addPortal}>
          <IconPlus size={15} stroke={2} /> {$t('iscsi.add')}
        </Button>
      </div>
      <p class={hint}>{$t('iscsi.portals_hint')}</p>
      {#each targetDraft.portals as portal, index (index)}
        <div class="flex items-end gap-2">
          <label class="flex flex-col gap-1 flex-1 min-w-0">
            <span class="sr-only">{$t('iscsi.portal_address')}</span>
            <input
              class={cn(inp, 'font-mono')}
              type="text"
              bind:value={portal.address}
              placeholder="0.0.0.0"
              data-testid="iscsi-portal-address-{index}"
            />
          </label>
          <label class="flex flex-col gap-1 w-28">
            <span class="sr-only">{$t('iscsi.portal_port')}</span>
            <input
              class={inp}
              type="number"
              min="1"
              max="65535"
              bind:value={portal.port}
              data-testid="iscsi-portal-port-{index}"
            />
          </label>
          <button
            type="button"
            class={cn(iconBtn, 'hover:bg-[var(--hv-error-bg)] hover:text-[var(--hv-error)]')}
            aria-label={$t('iscsi.portal_remove')}
            data-testid="iscsi-portal-remove-{index}"
            onclick={() => removePortal(index)}
          ><IconTrash size={16} stroke={1.8} /></button>
        </div>
      {/each}
    </div>

    <!-- LUNs -->
    <div class="flex flex-col gap-2">
      <div class="flex items-center justify-between gap-2">
        <span class={fieldLabel}>{$t('iscsi.luns_title')}</span>
        <Button
          variant="secondary"
          size="sm"
          data-testid="iscsi-lun-add"
          disabled={backstores.length === 0}
          onclick={addLun}
        >
          <IconPlus size={15} stroke={2} /> {$t('iscsi.add')}
        </Button>
      </div>
      <p class={hint}>{$t('iscsi.luns_hint')}</p>
      {#if backstores.length === 0}
        <p class={hint} data-testid="iscsi-lun-no-backstores">{$t('iscsi.lun_no_backstores')}</p>
      {/if}
      {#each targetDraft.luns as lun, index (index)}
        <div class="flex items-end gap-2">
          <label class="flex flex-col gap-1 w-24">
            <span class="sr-only">{$t('iscsi.lun_number')}</span>
            <input class={inp} type="number" min="0" max="16383" bind:value={lun.lun} data-testid="iscsi-lun-{index}" />
          </label>
          <label class="flex flex-col gap-1 flex-1 min-w-0">
            <span class="sr-only">{$t('iscsi.lun_backstore')}</span>
            <select class={inp} bind:value={lun.backstore_id} data-testid="iscsi-lun-backstore-{index}">
              {#each backstores as store (store.id)}
                <option value={store.id}>{store.name}</option>
              {/each}
            </select>
          </label>
          <button
            type="button"
            class={cn(iconBtn, 'hover:bg-[var(--hv-error-bg)] hover:text-[var(--hv-error)]')}
            aria-label={$t('iscsi.lun_remove')}
            data-testid="iscsi-lun-remove-{index}"
            onclick={() => removeLun(index)}
          ><IconTrash size={16} stroke={1.8} /></button>
        </div>
      {/each}
    </div>

    <!-- Initiators -->
    <div class="flex flex-col gap-2">
      <div class="flex items-center justify-between gap-2">
        <span class={fieldLabel}>{$t('iscsi.acls_title')}</span>
        <Button variant="secondary" size="sm" data-testid="iscsi-acl-add" onclick={addAcl}>
          <IconPlus size={15} stroke={2} /> {$t('iscsi.add')}
        </Button>
      </div>
      <p class={hint}>{$t('iscsi.acls_hint')}</p>
      {#each targetDraft.acls as acl, index (index)}
        <div
          class="flex flex-col gap-3 p-3 rounded-lg bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]"
          data-testid="iscsi-acl-{index}"
        >
          <div class="flex items-end gap-2">
            <label class="flex flex-col gap-1 flex-1 min-w-0">
              <span class={fieldLabel}>{$t('iscsi.acl_initiator')}</span>
              <input
                class={cn(inp, 'font-mono')}
                type="text"
                bind:value={acl.initiator_iqn}
                placeholder="iqn.2026-11.local.client:laptop"
                data-testid="iscsi-acl-initiator-{index}"
              />
            </label>
            <button
              type="button"
              class={cn(iconBtn, 'hover:bg-[var(--hv-error-bg)] hover:text-[var(--hv-error)]')}
              aria-label={$t('iscsi.acl_remove')}
              data-testid="iscsi-acl-remove-{index}"
              onclick={() => removeAcl(index)}
            ><IconTrash size={16} stroke={1.8} /></button>
          </div>
          <div class="grid gap-3 sm:grid-cols-2">
            <label class="flex flex-col gap-1.5">
              <span class={fieldLabel}>{$t('iscsi.acl_userid')}</span>
              <input
                class={inp}
                type="text"
                autocomplete="off"
                bind:value={acl.userid}
                data-testid="iscsi-acl-userid-{index}"
              />
            </label>
            <label class="flex flex-col gap-1.5">
              <span class={fieldLabel}>{$t('iscsi.acl_secret')}</span>
              <input
                class={inp}
                type="password"
                autocomplete="new-password"
                bind:value={acl.secret}
                placeholder={acl.has_secret ? $t('iscsi.acl_secret_keep') : ''}
                data-testid="iscsi-acl-secret-{index}"
              />
            </label>
          </div>
          <p class={hint}>{$t('iscsi.acl_chap_note')}</p>
        </div>
      {/each}
    </div>
  </div>
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={closeTarget}>{$t('common.cancel')}</Button>
      <Button
        variant="primary"
        data-testid="iscsi-target-save"
        loading={busy === 'target'}
        disabled={busy !== null}
        onclick={saveTarget}
      >
        <IconCheck size={16} stroke={2} /> {$t('common.save')}
      </Button>
    </div>
  {/snippet}
</Modal>

<ConfirmDialog {dlg} onconfirm={() => dlg.settle(true)} oncancel={() => dlg.settle(false)} />
