<!-- ========================================================================
     File          : Destinations.svelte
     Author        : Nicolas Le Roux
     Created       : 31/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
     Backup destination (Backup → Destinations): HomeVault can receive restic
     backups from another machine. This screen turns the receive role on,
     creates one quota'd tenant per source and shows the repository URL the
     source must be pointed at. The tenant token and repository password are
     displayed exactly once, at creation or rotation.

     Dependencies:
       - svelte (>= 5), theme + app composites, $lib/services/api

     Usage:
       Registered as the "Backup destination" desktop app.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { t, translate } from '$lib/stores/i18n';
  import { Spinner, Button, Toggle, Modal, Alert } from '$lib/components/theme';
  import { PageLayout, ConfirmDialog, EmptyState } from '$lib/components/app';
  import { createConfirmState } from '$lib/utils/confirmDialog.svelte';
  import { notifyError, notifySuccess } from '$lib/utils/notify';
  import { cn } from '$lib/utils.js';
  import { IconPlus, IconRefresh, IconTrash, IconShield, IconDatabase } from '@tabler/icons-svelte';

  interface Tenant {
    id: number;
    name: string;
    repo_path: string;
    repository_url: string;
    quota_bytes: number;
    used_bytes: number;
    quota_percent: number | null;
    quota_verdict: 'unlimited' | 'under' | 'exceeded';
    over_quota: boolean;
    has_token: boolean;
    verify_enabled: boolean;
    last_seen_at: string | null;
    last_verified_at: string | null;
    last_verify_status: string | null;
    last_verify_error: string | null;
    created_at: string;
  }

  interface TargetState {
    settings: { enabled: boolean; port: number; listen: string };
    tenants: Tenant[];
    rest_server_available: boolean;
    restic_available: boolean;
    service_active: boolean;
    default_port: number;
    repos_root: string;
  }

  let target = $state<TargetState | null>(null);
  let loading = $state(true);
  let busy = $state(false);
  let error = $state('');
  let stateError = $state('');

  let showCreate = $state(false);
  let createForm = $state({ name: '', quota_gb: '' });
  let createError = $state('');
  let secretReveal = $state<{ token: string; password: string; url: string; warning?: string } | null>(null);

  let quotaDrafts = $state<Record<number, string>>({});
  let quotaBusy = $state<number | null>(null);
  let verifyBusy = $state<number | null>(null);
  let rotateBusy = $state<number | null>(null);
  const dlg = createConfirmState();

  const field = 'flex flex-col gap-1.5';
  const fieldLabel = 'text-c-label font-semibold text-[var(--hv-shell-text-strong)]';
  const inp = 'w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-c-base text-[var(--hv-shell-text-strong)] outline-none focus:border-[var(--hv-primary)]';
  const card = 'flex flex-col gap-4 p-5 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)]';

  onMount(load);

  async function load() {
    loading = true;
    error = '';
    try {
      const resp = await api.get('/backup-target');
      if (resp.success && resp.data) {
        target = resp.data as TargetState;
        const drafts: Record<number, string> = {};
        for (const tenant of target.tenants) {
          drafts[tenant.id] = tenant.quota_bytes > 0 ? String(Math.round(tenant.quota_bytes / 1073741824)) : '';
        }
        quotaDrafts = drafts;
      } else {
        error = resp.message || translate('destinations.load_failed');
      }
    } catch {
      error = translate('destinations.load_failed');
    } finally {
      loading = false;
    }
  }

  function bytes(value: number): string {
    if (!value) return '0 B';
    const units = ['B', 'KB', 'MB', 'GB', 'TB'];
    const index = Math.min(units.length - 1, Math.floor(Math.log(value) / Math.log(1024)));
    return `${(value / Math.pow(1024, index)).toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
  }

  async function saveSettings() {
    if (!target) return;
    busy = true;
    stateError = '';
    try {
      const resp = await api.put('/backup-target/config', target.settings);
      if (resp.success && resp.data) {
        target = resp.data as TargetState;
        notifySuccess('common.saved');
      } else {
        stateError = resp.message || translate('destinations.save_failed');
      }
    } catch {
      stateError = translate('destinations.save_failed');
    } finally {
      busy = false;
    }
  }

  function openCreate() {
    createForm = { name: '', quota_gb: '' };
    createError = '';
    showCreate = true;
  }

  async function createTenant() {
    const name = createForm.name.trim();
    if (!name) {
      createError = translate('destinations.name_required');
      return;
    }
    const quotaGb = Number(createForm.quota_gb || '0');
    if (Number.isNaN(quotaGb) || quotaGb < 0) {
      createError = translate('destinations.quota_invalid');
      return;
    }
    busy = true;
    createError = '';
    try {
      const resp = await api.post('/backup-target/tenants', {
        name,
        quota_bytes: Math.round(quotaGb * 1073741824),
      });
      if (resp.success && resp.data) {
        showCreate = false;
        secretReveal = {
          token: resp.data.token,
          password: resp.data.repository_password,
          url: resp.data.repository_url,
        };
        await load();
      } else {
        createError = resp.message || translate('destinations.save_failed');
      }
    } catch {
      createError = translate('destinations.save_failed');
    } finally {
      busy = false;
    }
  }

  async function saveQuota(tenant: Tenant) {
    const quotaGb = Number(quotaDrafts[tenant.id] || '0');
    if (Number.isNaN(quotaGb) || quotaGb < 0) {
      notifyError({ message: translate('destinations.quota_invalid') });
      return;
    }
    quotaBusy = tenant.id;
    try {
      const resp = await api.put(`/backup-target/tenants/${tenant.id}`, {
        quota_bytes: Math.round(quotaGb * 1073741824),
      });
      if (resp.success && resp.data) {
        target = resp.data as TargetState;
        notifySuccess('common.saved');
      } else {
        notifyError(resp);
      }
    } finally {
      quotaBusy = null;
    }
  }

  async function verify(tenant: Tenant) {
    verifyBusy = tenant.id;
    try {
      const resp = await api.post(`/backup-target/tenants/${tenant.id}/verify`, {});
      if (resp.success) {
        notifySuccess('destinations.verify_ok');
      } else {
        notifyError(resp);
      }
      await load();
    } finally {
      verifyBusy = null;
    }
  }

  async function rotate(tenant: Tenant) {
    if (!(await dlg.ask(translate('destinations.rotate_confirm', { name: tenant.name })))) return;
    rotateBusy = tenant.id;
    try {
      const resp = await api.post(`/backup-target/tenants/${tenant.id}/rotate`, {});
      if (resp.success && resp.data) {
        secretReveal = {
          token: resp.data.token,
          password: resp.data.repository_password,
          url: resp.data.repository_url,
          warning: resp.data.warning,
        };
        await load();
      } else {
        notifyError(resp);
      }
    } finally {
      rotateBusy = null;
    }
  }

  async function removeTenant(tenant: Tenant) {
    if (!(await dlg.ask(translate('destinations.delete_confirm', { name: tenant.name })))) return;
    const resp = await api.delete(`/backup-target/tenants/${tenant.id}`);
    if (resp.success && resp.data) {
      target = resp.data as TargetState;
      notifySuccess('common.saved');
    } else {
      notifyError(resp);
    }
  }

  async function copy(value: string, label: string) {
    try {
      await navigator.clipboard.writeText(value);
      notifySuccess(label);
    } catch {
      // Clipboard access can be denied; the value stays selectable on screen.
    }
  }
</script>

<PageLayout title={$t('destinations.title')} data-testid="destinations-page">
  {#snippet icon()}
    <IconDatabase stroke={1.8} />
  {/snippet}

  {#if loading}
    <div data-testid="destinations-loading" class="flex items-center justify-center gap-3 py-16 text-[var(--hv-shell-text)]">
      <Spinner />
      <span>{$t('common.loading')}</span>
    </div>
  {:else if target}
    <div data-testid="destinations-content" class="flex flex-col gap-4 max-w-[820px]">
      {#if error}
        <Alert variant="error">{error}</Alert>
      {/if}
      {#if !target.rest_server_available}
        <Alert variant="warning">{$t('destinations.missing_rest_server')}</Alert>
      {/if}
      {#if !target.restic_available}
        <Alert variant="warning">{$t('destinations.missing_restic')}</Alert>
      {/if}

      <div data-testid="destinations-settings" class={card}>
        <div>
          <h2 class="m-0 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">{$t('destinations.settings_title')}</h2>
          <p class="m-0 mt-1 text-c-base text-[var(--hv-shell-text)] leading-relaxed">{$t('destinations.settings_desc')}</p>
        </div>
        <div class="flex items-center justify-between gap-4 flex-wrap">
          <span class="text-c-base font-semibold text-[var(--hv-shell-text-strong)]">{$t('destinations.enable')}</span>
          <Toggle bind:checked={target.settings.enabled} label={$t('destinations.enable')} />
        </div>
        <div class="grid gap-3 sm:grid-cols-2">
          <label class={field}>
            <span class={fieldLabel}>{$t('destinations.port')}</span>
            <input class={inp} type="number" min="1" max="65535" bind:value={target.settings.port} data-testid="destinations-port" />
          </label>
          <label class={field}>
            <span class={fieldLabel}>{$t('destinations.listen')}</span>
            <select class={inp} bind:value={target.settings.listen}>
              <option value="127.0.0.1">{$t('destinations.listen_local')}</option>
              <option value="0.0.0.0">{$t('destinations.listen_lan')}</option>
            </select>
          </label>
        </div>
        <p class="m-0 text-c-label text-[var(--hv-shell-text-2)]">
          {$t('destinations.service_state', { state: target.service_active ? $t('destinations.running') : $t('destinations.stopped') })}
        </p>
        {#if stateError}
          <Alert variant="error">{stateError}</Alert>
        {/if}
        <Button variant="primary" size="lg" onclick={saveSettings} disabled={busy}>
          {busy ? $t('common.loading') : $t('common.save')}
        </Button>
      </div>

      <div data-testid="destinations-tenants" class={card}>
        <div class="flex items-center justify-between gap-3 flex-wrap">
          <div>
            <h2 class="m-0 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">{$t('destinations.tenants_title')}</h2>
            <p class="m-0 mt-1 text-c-base text-[var(--hv-shell-text)] leading-relaxed">{$t('destinations.tenants_desc')}</p>
          </div>
          <Button variant="primary" onclick={openCreate}>
            <IconPlus size={16} stroke={2} /> {$t('destinations.add_tenant')}
          </Button>
        </div>

        {#if target.tenants.length === 0}
          <EmptyState title={$t('destinations.empty_title')} description={$t('destinations.empty_desc')} />
        {:else}
          <div class="flex flex-col gap-3">
            {#each target.tenants as tenant (tenant.id)}
              <div data-testid="destinations-tenant" class="flex flex-col gap-2 p-4 rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]">
                <div class="flex items-start justify-between gap-3 flex-wrap">
                  <div class="flex flex-col min-w-0">
                    <span id={`destinations-tenant-${tenant.id}`} class="font-semibold text-c-card-title text-[var(--hv-shell-text-strong)]">{tenant.name}</span>
                    <code class="font-[family-name:var(--hv-font-mono)] text-c-label text-[var(--hv-shell-text-2)] break-all">{tenant.repository_url}</code>
                  </div>
                  <div class="flex items-center gap-1">
                    <button
                      type="button"
                      class="grid place-items-center w-9 h-9 flex-none rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:bg-[var(--hv-subtle-hover)] hover:text-[var(--hv-shell-text-strong)] hv-ic"
                      aria-label={$t('destinations.copy_url')}
                      data-testid="destinations-copy"
                      onclick={() => copy(tenant.repository_url, 'destinations.copy_url')}
                    ><IconRefresh size={17} stroke={1.8} /></button>
                    <button
                      type="button"
                      class="grid place-items-center w-9 h-9 flex-none rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:bg-[var(--hv-subtle-hover)] hover:text-[var(--hv-shell-text-strong)] hv-ic"
                      aria-label={$t('destinations.rotate')}
                      data-testid="destinations-rotate"
                      disabled={rotateBusy === tenant.id}
                      onclick={() => rotate(tenant)}
                    ><IconShield size={17} stroke={1.8} /></button>
                    <button
                      type="button"
                      class="grid place-items-center w-9 h-9 flex-none rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:bg-[var(--hv-error-bg)] hover:text-[var(--hv-error)] hv-ic"
                      aria-label={$t('tasks.delete', { name: tenant.name })}
                      data-testid="destinations-delete"
                      onclick={() => removeTenant(tenant)}
                    ><IconTrash size={17} stroke={1.8} /></button>
                  </div>
                </div>

                <div class="flex items-end gap-2 flex-wrap">
                  <label class={cn(field, 'flex-1 min-w-[160px]')}>
                    <span class={fieldLabel}>{$t('destinations.quota_gb')}</span>
                    <input
                      class={inp}
                      type="number"
                      min="0"
                      step="1"
                      placeholder={$t('destinations.quota_unlimited')}
                      bind:value={quotaDrafts[tenant.id]}
                      data-testid="destinations-quota"
                    />
                  </label>
                  <Button variant="ghost" onclick={() => saveQuota(tenant)} disabled={quotaBusy === tenant.id}>
                    {$t('common.save')}
                  </Button>
                  <Button variant="ghost" onclick={() => verify(tenant)} disabled={verifyBusy === tenant.id}>
                    {$t('destinations.verify')}
                  </Button>
                </div>

                <div class="flex items-center gap-3 flex-wrap text-c-label text-[var(--hv-shell-text-2)]">
                  <span>{$t('destinations.used', { used: bytes(tenant.used_bytes), quota: tenant.quota_bytes > 0 ? bytes(tenant.quota_bytes) : $t('destinations.quota_unlimited') })}</span>
                  {#if tenant.quota_percent !== null}
                    <span class="inline-flex items-center gap-1">
                      <span class="w-24 h-1.5 rounded-full bg-[var(--hv-shell-border)] overflow-hidden">
                        <span class="block h-full bg-[var(--hv-primary)]" style={`width: ${tenant.quota_percent}%`}></span>
                      </span>
                      {tenant.quota_percent}%
                    </span>
                  {/if}
                  {#if tenant.over_quota}
                    <span class="text-[var(--hv-danger)] font-semibold">{$t('destinations.over_quota')}</span>
                  {/if}
                  {#if tenant.last_verify_status}
                    <span class={tenant.last_verify_status === 'success' ? 'text-[var(--hv-success)]' : 'text-[var(--hv-danger)]'}>
                      {$t('destinations.last_verify', { status: tenant.last_verify_status })}
                    </span>
                  {/if}
                </div>
                {#if tenant.last_verify_error}
                  <p class="m-0 text-c-label text-[var(--hv-danger)]" role="alert">{tenant.last_verify_error}</p>
                {/if}
              </div>
            {/each}
          </div>
        {/if}

        <p class="m-0 text-c-label text-[var(--hv-shell-text-2)]">
          {$t('destinations.repos_root', { path: target.repos_root })}
        </p>
      </div>
    </div>
  {:else}
    <Alert variant="error">{error || $t('destinations.load_failed')}</Alert>
  {/if}

  {#snippet footer()}
    {#if target}
      <span data-testid="destinations-footer" class="flex items-center gap-2 flex-wrap">
        <span>{target.tenants.length} {$t('destinations.footer_tenants')}</span>
        <span class="text-[var(--hv-shell-text-3)]">&middot;</span>
        <span>{target.settings.enabled ? $t('destinations.running') : $t('destinations.stopped')}</span>
      </span>
    {/if}
  {/snippet}
</PageLayout>

<!-- New tenant -->
<Modal open={showCreate} title={$t('destinations.add_tenant')} size="md" onclose={() => (showCreate = false)}>
  {#snippet icon()}
    <IconPlus size={22} stroke={2} />
  {/snippet}
  {#snippet children()}
    <div class="flex flex-col gap-4">
      <p class="m-0 text-c-base text-[var(--hv-shell-text)] leading-relaxed">{$t('destinations.add_tenant_desc')}</p>
      <label class={field}>
        <span class={fieldLabel}>{$t('destinations.tenant_name')}</span>
        <input class={inp} bind:value={createForm.name} placeholder="family-nas" data-testid="destinations-name" />
        <span class="text-c-label text-[var(--hv-shell-text-2)]">{$t('destinations.tenant_name_hint')}</span>
      </label>
      <label class={field}>
        <span class={fieldLabel}>{$t('destinations.quota_gb')}</span>
        <input class={inp} type="number" min="0" step="1" bind:value={createForm.quota_gb} placeholder={$t('destinations.quota_unlimited')} data-testid="destinations-create-quota" />
        <span class="text-c-label text-[var(--hv-shell-text-2)]">{$t('destinations.quota_hint')}</span>
      </label>
      {#if createError}
        <Alert variant="error">{createError}</Alert>
      {/if}
    </div>
  {/snippet}
  {#snippet footer()}
    <Button variant="ghost" size="lg" onclick={() => (showCreate = false)}>{$t('common.cancel')}</Button>
    <Button variant="primary" size="lg" onclick={createTenant} disabled={busy}>{$t('common.create')}</Button>
  {/snippet}
</Modal>

<!-- Secrets are shown once -->
<Modal open={!!secretReveal} title={$t('destinations.secrets_title')} size="md" onclose={() => (secretReveal = null)}>
  {#snippet icon()}
    <IconShield size={22} stroke={1.9} />
  {/snippet}
  {#snippet children()}
    {#if secretReveal}
      <div class="flex flex-col gap-3">
        <Alert variant="warning">{$t('destinations.secrets_once')}</Alert>
        {#if secretReveal.warning}
          <Alert variant="warning">{secretReveal.warning}</Alert>
        {/if}
        <label class={field}>
          <span class={fieldLabel}>{$t('destinations.repository_url')}</span>
          <input class={inp} readonly value={secretReveal.url} />
        </label>
        <label class={field}>
          <span class={fieldLabel}>{$t('destinations.token')}</span>
          <input class={cn(inp, 'font-[family-name:var(--hv-font-mono)]')} readonly value={secretReveal.token} data-testid="destinations-token" />
        </label>
        <label class={field}>
          <span class={fieldLabel}>{$t('destinations.repo_password')}</span>
          <input class={cn(inp, 'font-[family-name:var(--hv-font-mono)]')} readonly value={secretReveal.password} data-testid="destinations-password" />
        </label>
        <pre class="m-0 p-3 rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] overflow-x-auto text-c-label font-[family-name:var(--hv-font-mono)] text-[var(--hv-shell-text-strong)]">{`export RESTIC_REPOSITORY=${secretReveal.url}
export RESTIC_PASSWORD=${secretReveal.password}
restic init`}</pre>
      </div>
    {/if}
  {/snippet}
  {#snippet footer()}
    <Button variant="primary" size="lg" onclick={() => (secretReveal = null)}>{$t('common.close')}</Button>
  {/snippet}
</Modal>

<ConfirmDialog {dlg} onconfirm={() => dlg.settle(true)} oncancel={() => dlg.settle(false)} />
