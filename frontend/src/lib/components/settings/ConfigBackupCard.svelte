<!-- ========================================================================
     File          : ConfigBackupCard.svelte
     Author        : Nicolas Le Roux
     Created       : 31/10/2026
     Last modified : 31/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Settings card for the full configuration backup: export an archive (with
       an optional passphrase and an optional secrets switch), download a
       previous one, and restore an archive by validating then staging it for
       the next start.

     Dependencies:
       - svelte (>= 5), $lib/services/api, $lib/stores/i18n,
         ./SettingsCard, ./vocabulary, theme/Switch

     Usage:
       Embedded in routes/Settings.svelte's `system` category. See
       docs/guides/settings.md.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { t } from '$lib/stores/i18n';
  import { Switch } from '$lib/components/theme';
  import SettingsCard from './SettingsCard.svelte';
  import { cn } from '$lib/utils.js';
  import {
    settingsRow,
    settingsRowLabel,
    settingsRowHint,
    settingsHint,
    settingsInput,
    settingsBtn,
    settingsBtnGhost,
    settingsBtnPrimary
  } from './vocabulary';

  interface Archive {
    id: string;
    bytes: number;
    created_at: string | null;
  }

  interface PendingRestore {
    created_at: string;
    app_version: string;
    includes_secrets: boolean;
    entries: number;
  }

  let { class: klass = '' }: { class?: string } = $props();

  let loading = $state(true);
  let loadError = $state('');
  let notice = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);
  let busy = $state(false);

  let archives = $state<Archive[]>([]);
  let pending = $state<PendingRestore | null>(null);
  let appVersion = $state('');
  let formatVersion = $state(1);
  let schemaVersion = $state(0);
  let maxBytes = $state(0);
  let configFiles = $state<{ archive: string; host: string }[]>([]);

  // Export form
  let includeSecrets = $state(false);
  let passphrase = $state('');

  // Restore form
  let restoreFile = $state<File | null>(null);
  let restorePassphrase = $state('');
  let restoreConfirm = $state(false);

  async function load() {
    loading = true;
    loadError = '';
    const res = await api.getConfigBackupState();
    if (res.success && res.data) {
      const d = res.data as any;
      archives = Array.isArray(d.archives) ? d.archives : [];
      pending = d.pending_restore ?? null;
      appVersion = d.app_version ?? '';
      formatVersion = d.format_version ?? 1;
      schemaVersion = d.settings_schema ?? 0;
      maxBytes = d.max_bytes ?? d.max_archive_bytes ?? 0;
      configFiles = Array.isArray(d.config_files) ? d.config_files : [];
    } else {
      loadError = res.message || $t('settings.config_backup_load_error');
    }
    loading = false;
  }

  onMount(load);

  function human(bytes: number): string {
    if (bytes >= 1_000_000_000) return `${(bytes / 1_000_000_000).toFixed(1)} GB`;
    if (bytes >= 1_000_000) return `${(bytes / 1_000_000).toFixed(1)} MB`;
    if (bytes >= 1_000) return `${(bytes / 1_000).toFixed(0)} KB`;
    return `${bytes} B`;
  }

  async function exportArchive() {
    busy = true;
    notice = null;
    const res = await api.exportConfigBackup(
      includeSecrets,
      passphrase.trim() || undefined,
    );
    if (res.success && res.data) {
      const d = res.data as any;
      notice = {
        kind: 'ok',
        text: $t('settings.config_backup_exported', { size: human(d.bytes ?? 0) }),
      };
      includeSecrets = false;
      passphrase = '';
      await load();
    } else {
      // The server names the problem (a passphrase is required for secrets, or
      // too short); repeating it is the only way the admin can fix it.
      notice = { kind: 'error', text: res.message || $t('settings.config_backup_export_error') };
    }
    busy = false;
  }

  async function download(archive: Archive) {
    // A plain navigation: the browser saves the file, the token travels in the
    // query-less header path the API client already sets for downloads.
    const url = api.getConfigBackupArchiveUrl(archive.id);
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.rel = 'noopener';
    anchor.download = `homevault-config-${archive.id}.hvbak`;
    document.body.appendChild(anchor);
    anchor.click();
    anchor.remove();
  }

  function pickFile(event: Event) {
    const input = event.target as HTMLInputElement;
    restoreFile = input.files?.[0] ?? null;
    restoreConfirm = false;
    notice = null;
  }

  async function inspect() {
    if (!restoreFile) return;
    busy = true;
    notice = null;
    const res = await api.inspectConfigBackup(restoreFile, restorePassphrase.trim() || undefined);
    busy = false;
    if (res.success && res.data) {
      const manifest = (res.data as any).manifest;
      notice = {
        kind: 'ok',
        text: $t('settings.config_backup_inspected', {
          date: manifest?.created_at ?? '',
          version: manifest?.app_version ?? '',
          entries: String(manifest?.entries?.length ?? 0),
        }),
      };
    } else {
      notice = { kind: 'error', text: res.message || $t('settings.config_backup_inspect_error') };
    }
  }

  async function restore() {
    if (!restoreFile) return;
    if (!restoreConfirm) {
      restoreConfirm = true;
      notice = {
        kind: 'error',
        text: $t('settings.config_backup_restore_confirm'),
      };
      return;
    }
    busy = true;
    notice = null;
    const res = await api.restoreConfigBackup(restoreFile, restorePassphrase.trim() || undefined);
    busy = false;
    if (res.success && res.data) {
      const d = res.data as any;
      notice = { kind: 'ok', text: d.message || $t('settings.config_backup_staged') };
      restoreFile = null;
      restoreConfirm = false;
      restorePassphrase = '';
      await load();
    } else {
      notice = { kind: 'error', text: res.message || $t('settings.config_backup_restore_error') };
    }
  }
</script>

<SettingsCard
  title={$t('settings.config_backup_title')}
  subtitle={$t('settings.config_backup_subtitle')}
  class={cn('config-backup-card', klass)}
>
  {#if loading}
    <p class={settingsHint}>{$t('common.loading')}</p>
  {:else}
    {#if loadError}
      <div class="cb-alert cb-alert-error" role="alert">{loadError}</div>
    {/if}
    {#if notice}
      <div
        class={cn('cb-alert', notice.kind === 'ok' ? 'cb-alert-ok' : 'cb-alert-error')}
        role={notice.kind === 'ok' ? 'status' : 'alert'}
        data-testid="config-backup-notice"
      >
        {notice.text}
      </div>
    {/if}

    {#if pending}
      <div class="cb-alert cb-alert-error" role="alert" data-testid="config-backup-pending">
        {$t('settings.config_backup_pending', {
          date: pending.created_at,
          version: pending.app_version
        })}
      </div>
    {/if}

    <div class={settingsRow}>
      <div class="min-w-0">
        <span class={settingsRowLabel}>{$t('settings.config_backup_contents')}</span>
        <p class={settingsRowHint}>
          {$t('settings.config_backup_contents_hint', {
            version: String(formatVersion),
            schema: String(schemaVersion),
            max: human(maxBytes)
          })}
        </p>
      </div>
      <span class="cb-version" data-testid="config-backup-version">{appVersion}</span>
    </div>

    <div class={settingsRow}>
      <div class="min-w-0">
        <span id="cb-secrets-label" class={settingsRowLabel}>
          {$t('settings.config_backup_include_secrets')}
        </span>
        <p class={settingsRowHint}>{$t('settings.config_backup_include_secrets_hint')}</p>
      </div>
      <Switch bind:checked={includeSecrets} ariaLabelledby="cb-secrets-label" />
    </div>

    {#if includeSecrets}
      <div class={settingsRow}>
        <label class={settingsRowLabel} for="cb-passphrase">
          {$t('settings.config_backup_passphrase')}
        </label>
        <input
          id="cb-passphrase"
          class={cn(settingsInput, 'cb-input')}
          type="password"
          autocomplete="new-password"
          minlength="12"
          bind:value={passphrase}
          data-testid="config-backup-passphrase"
        />
      </div>
    {/if}

    <div class="flex flex-wrap gap-2 py-3">
      <button
        class={cn(settingsBtn, settingsBtnPrimary)}
        onclick={exportArchive}
        disabled={busy}
        data-testid="config-backup-export"
      >
        {$t('settings.config_backup_export')}
      </button>
      <button
        class={cn(settingsBtn, settingsBtnGhost)}
        onclick={load}
        disabled={busy}
        data-testid="config-backup-refresh"
      >
        {$t('common.refresh')}
      </button>
    </div>

    {#if archives.length > 0}
      <ul class="m-0 p-0 list-none" data-testid="config-backup-archives">
        {#each archives as archive (archive.id)}
          <li class="cb-row" data-testid="config-backup-archive" data-archive-id={archive.id}>
            <span class="min-w-0">
              <span class="block text-sm font-medium text-[var(--hv-settings-title)] cb-mono">
                {archive.id}
              </span>
              <span class="block text-xs text-[var(--hv-settings-label)]">
                {archive.created_at ? new Date(archive.created_at).toLocaleString() : ''} ·
                {human(archive.bytes)}
              </span>
            </span>
            <button
              class={cn(settingsBtn, settingsBtnGhost)}
              onclick={() => download(archive)}
              aria-label={$t('settings.config_backup_download_id', { id: archive.id })}
              data-testid="config-backup-download"
            >
              {$t('settings.config_backup_download')}
            </button>
          </li>
        {/each}
      </ul>
    {:else}
      <p class={settingsHint} data-testid="config-backup-empty">
        {$t('settings.config_backup_empty')}
      </p>
    {/if}

    <div class={settingsRow} data-testid="config-backup-restore-row">
      <div class="min-w-0">
        <label class={settingsRowLabel} for="cb-restore-file">
          {$t('settings.config_backup_restore')}
        </label>
        <p class={settingsRowHint}>{$t('settings.config_backup_restore_hint')}</p>
      </div>
      <input
        id="cb-restore-file"
        class="text-sm"
        type="file"
        accept=".hvbak,application/octet-stream"
        onchange={pickFile}
        data-testid="config-backup-file"
      />
    </div>

    {#if restoreFile}
      <div class={settingsRow}>
        <label class={settingsRowLabel} for="cb-restore-passphrase">
          {$t('settings.config_backup_passphrase')}
        </label>
        <input
          id="cb-restore-passphrase"
          class={cn(settingsInput, 'cb-input')}
          type="password"
          autocomplete="off"
          bind:value={restorePassphrase}
          data-testid="config-backup-restore-passphrase"
        />
      </div>
      <div class="flex flex-wrap gap-2 py-3">
        <button
          class={cn(settingsBtn, settingsBtnGhost)}
          onclick={inspect}
          disabled={busy}
          data-testid="config-backup-inspect"
        >
          {$t('settings.config_backup_inspect')}
        </button>
        <button
          class={cn(settingsBtn, settingsBtnGhost, restoreConfirm && 'cb-danger')}
          onclick={restore}
          disabled={busy}
          data-testid="config-backup-restore-btn"
        >
          {restoreConfirm
            ? $t('settings.config_backup_restore_confirm_btn')
            : $t('settings.config_backup_restore_btn')}
        </button>
      </div>
    {/if}

    {#if configFiles.length > 0}
      <p class={settingsHint}>
        {$t('settings.config_backup_files', {
          files: configFiles.map((f) => f.archive.replace('configs/', '')).join(', ')
        })}
      </p>
    {/if}
    <p class={settingsHint}>{$t('settings.config_backup_hint')}</p>
  {/if}
</SettingsCard>

<style>
  .cb-input {
    max-width: 320px;
  }

  .cb-mono {
    font-family: var(--hv-font-mono);
  }

  .cb-version {
    font-family: var(--hv-font-mono);
    font-size: var(--hv-font-size-sm);
    color: var(--hv-settings-label);
  }

  .cb-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 16px;
    padding: 8px 0;
    border-bottom: 1.45px solid var(--hv-settings-hairline);
  }

  .cb-row:last-child {
    border-bottom: none;
  }

  /* The second click of a restore: text *and* outline, never colour alone. */
  .cb-danger {
    outline: 1.45px solid var(--hv-error);
    color: var(--hv-error);
  }

  .cb-alert {
    margin: 0 0 12px;
    padding: var(--hv-space-2-5) var(--hv-space-3);
    border-radius: var(--hv-radius-lg);
    font-size: var(--hv-font-size-sm);
  }

  .cb-alert-ok {
    background: var(--hv-success-bg);
    color: var(--hv-success-text);
  }

  .cb-alert-error {
    background: var(--hv-error-light);
    color: var(--hv-error);
  }
</style>
