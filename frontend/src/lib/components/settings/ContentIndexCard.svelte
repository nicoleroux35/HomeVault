<!-- ========================================================================
     File          : ContentIndexCard.svelte
     Author        : Nicolas Le Roux
     Created       : 06/11/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Content index settings (phase D1): the opt-in switch, the roots chosen
       from the appliance's own Explorer roots, explicit exclusions, retention,
       the size/status line, a rebuild button and "delete my index".

       Design rules kept here (CLAUDE.md, AGENTS.md):
       - off by default; turning it on is a named confirmation because the
         appliance will read file contents;
       - roots are ticked from a real list, never typed;
       - status is honest: documents, stored size, last run, last error, and
         a truncation flag;
       - deleting the index is a named confirmation with the document count;
       - every control is a 44px target, labelled and keyboard reachable.

     Dependencies:
       - svelte (>= 5), $lib/services/api-phase4, $lib/stores/i18n,
         $lib/utils/notify, ./SettingsCard, ./vocabulary, theme/Modal

     Usage:
       Embedded in routes/Settings.svelte's system category. See
       docs/guides/content-index.md.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { translate } from '$lib/stores/i18n';
  import { notifyError, notifySuccess } from '$lib/utils/notify';
  import {
    deleteContentIndex,
    getContentIndex,
    runContentIndex,
    updateContentIndex
  } from '$lib/services/api-phase4';
  import type {
    ContentIndexConfig,
    ContentIndexLimits,
    ContentIndexRunState
  } from '$lib/types/phase4';
  import SettingsCard from './SettingsCard.svelte';
  import Modal from '$lib/components/theme/Modal.svelte';
  import { cn } from '$lib/utils.js';
  import {
    settingsRow,
    settingsRowLabel,
    settingsRowHint,
    settingsHint,
    settingsInput,
    settingsBtn,
    settingsBtnPrimary,
    settingsBtnDanger
  } from './vocabulary';

  let { class: klass = '' }: { class?: string } = $props();

  let loading = $state(true);
  let saving = $state(false);
  let busy = $state(false);
  let loadError = $state('');
  let actionError = $state('');
  let config = $state<ContentIndexConfig>(emptyConfig());
  let candidates = $state<string[]>([]);
  let runState = $state<ContentIndexRunState>({
    running: false,
    last_run: null,
    last_error: null,
    truncated: false
  });
  let docs = $state(0);
  let bytes = $state(0);
  let limits = $state<ContentIndexLimits>({
    max_roots: 16,
    max_docs: 50000,
    max_total_bytes: 268435456,
    max_file_bytes: 1048576
  });
  let excludeNamesText = $state('');
  let excludeExtensionsText = $state('');

  let confirmEnableOpen = $state(false);
  let deleteOpen = $state(false);
  let pollTimer: ReturnType<typeof setInterval> | null = null;

  function emptyConfig(): ContentIndexConfig {
    return {
      enabled: false,
      roots: [],
      exclude_names: [],
      exclude_extensions: [],
      retention_days: 30,
      confirmed: false
    };
  }

  onMount(load);

  async function load() {
    loading = true;
    loadError = '';
    try {
      const res = await getContentIndex();
      config = {
        ...res.config,
        roots: [...(res.config.roots ?? [])],
        exclude_names: [...(res.config.exclude_names ?? [])],
        exclude_extensions: [...(res.config.exclude_extensions ?? [])]
      };
      excludeNamesText = config.exclude_names.join(', ');
      excludeExtensionsText = config.exclude_extensions.join(', ');
      candidates = res.candidates ?? [];
      runState = res.state;
      docs = res.docs;
      bytes = res.bytes;
      limits = res.limits;
      if (runState.running) startPolling();
    } catch (e) {
      loadError = e instanceof Error ? e.message : translate('settings.content_index_load_failed');
    } finally {
      loading = false;
    }
  }

  function startPolling() {
    if (pollTimer) return;
    pollTimer = setInterval(async () => {
      await load();
      if (!runState.running && pollTimer) {
        clearInterval(pollTimer);
        pollTimer = null;
      }
    }, 2000);
  }

  function toggleRoot(path: string, checked: boolean) {
    config.roots = checked
      ? [...config.roots.filter((root) => root !== path), path]
      : config.roots.filter((root) => root !== path);
  }

  function onEnabledChange(checked: boolean) {
    config.enabled = checked;
    if (checked && !config.confirmed) {
      confirmEnableOpen = true;
    }
  }

  function cancelEnable() {
    config.enabled = false;
    confirmEnableOpen = false;
  }

  function confirmEnable() {
    config.confirmed = true;
    confirmEnableOpen = false;
  }

  async function save() {
    if (saving) return;
    saving = true;
    actionError = '';
    config.exclude_names = splitList(excludeNamesText);
    config.exclude_extensions = splitList(excludeExtensionsText);
    try {
      const res = await updateContentIndex(config);
      config = { ...res.config };
      excludeNamesText = config.exclude_names.join(', ');
      excludeExtensionsText = config.exclude_extensions.join(', ');
      notifySuccess('settings.content_index_saved');
      await load();
      if (config.enabled) startPolling();
    } catch (e) {
      actionError = e instanceof Error ? e.message : translate('settings.content_index_save_failed');
    } finally {
      saving = false;
    }
  }

  async function rebuild() {
    if (busy) return;
    busy = true;
    actionError = '';
    try {
      const res = await runContentIndex();
      if (res.started) {
        notifySuccess('settings.content_index_started');
      }
      // A run may already have started right after a save; the status line
      // says so, and no error is raised for asking twice.
      startPolling();
    } catch (e) {
      actionError = e instanceof Error ? e.message : translate('settings.content_index_run_failed');
    } finally {
      busy = false;
    }
  }

  async function confirmDelete() {
    if (busy) return;
    busy = true;
    try {
      const res = await deleteContentIndex();
      deleteOpen = false;
      notifySuccess(
        res.deleted === 1
          ? 'settings.content_index_deleted_one'
          : 'settings.content_index_deleted'
      );
      await load();
    } catch (e) {
      notifyError(
        e instanceof Error ? { success: false, message: e.message } : null,
        'settings.content_index_delete_failed'
      );
    } finally {
      busy = false;
    }
  }

  function splitList(value: string): string[] {
    return value
      .split(',')
      .map((item) => item.trim())
      .filter(Boolean);
  }

  function humanBytes(value: number): string {
    if (value < 1024) return `${value} B`;
    if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`;
    if (value < 1024 * 1024 * 1024) return `${(value / (1024 * 1024)).toFixed(1)} MB`;
    return `${(value / (1024 * 1024 * 1024)).toFixed(2)} GB`;
  }

  function lastRunLabel(): string {
    if (runState.running) return translate('settings.content_index_running');
    if (runState.last_run) {
      const date = new Date(runState.last_run);
      return translate('settings.content_index_last_run', {
        date: Number.isNaN(date.getTime()) ? runState.last_run : date.toLocaleString()
      });
    }
    return translate('settings.content_index_never_run');
  }
</script>

<SettingsCard
  title={translate('settings.content_index_title')}
  subtitle={translate('settings.content_index_subtitle')}
  class={cn('content-index-card', klass)}
  data-testid="content-index-card"
>
  {#if loading}
    <p class={settingsHint}>{translate('settings.loading')}</p>
  {:else}
    {#if loadError}
      <div class="ci-alert ci-alert-error" role="alert" data-testid="content-index-error">
        {loadError}
      </div>
    {/if}
    {#if actionError}
      <div class="ci-alert ci-alert-error" role="alert" data-testid="content-index-action-error">
        {actionError}
      </div>
    {/if}

    <div class={settingsRow}>
      <div class="min-w-0">
        <span class={settingsRowLabel} id="content-index-enable-label">
          {translate('settings.content_index_enable')}
        </span>
        <p class={settingsRowHint}>{translate('settings.content_index_enable_hint')}</p>
      </div>
      <input
        id="content-index-enabled"
        type="checkbox"
        class="ci-check"
        checked={config.enabled}
        onchange={(event) => onEnabledChange(event.currentTarget.checked)}
        aria-labelledby="content-index-enable-label"
        data-testid="content-index-enabled"
      />
    </div>

    {#if config.enabled}
      <div class={settingsRow}>
        <div class="min-w-0">
          <span class={settingsRowLabel}>{translate('settings.content_index_roots')}</span>
          <p class={settingsRowHint}>{translate('settings.content_index_roots_hint')}</p>
        </div>
      </div>
      {#if candidates.length === 0}
        <p class={settingsHint} data-testid="content-index-no-roots">
          {translate('settings.content_index_no_roots')}
        </p>
      {:else}
        <ul class="ci-roots" data-testid="content-index-roots">
          {#each candidates as candidate (candidate)}
            <li>
              <label class="ci-root-row" for={`content-index-root-${candidate}`}>
                <input
                  id={`content-index-root-${candidate}`}
                  type="checkbox"
                  checked={config.roots.includes(candidate)}
                  onchange={(event) => toggleRoot(candidate, event.currentTarget.checked)}
                  data-testid="content-index-root"
                />
                <span class="ci-root-path">{candidate}</span>
              </label>
            </li>
          {/each}
        </ul>
      {/if}

      <div class={settingsRow}>
        <div class="min-w-0">
          <label class={settingsRowLabel} for="content-index-exclude-names">
            {translate('settings.content_index_exclude_names')}
          </label>
          <p class={settingsRowHint}>{translate('settings.content_index_exclude_names_hint')}</p>
        </div>
        <input
          id="content-index-exclude-names"
          class={cn(settingsInput, 'max-w-[280px]')}
          bind:value={excludeNamesText}
          placeholder="node_modules, *.bak"
          data-testid="content-index-exclude-names"
        />
      </div>

      <div class={settingsRow}>
        <div class="min-w-0">
          <label class={settingsRowLabel} for="content-index-exclude-extensions">
            {translate('settings.content_index_exclude_extensions')}
          </label>
          <p class={settingsRowHint}>
            {translate('settings.content_index_exclude_extensions_hint')}
          </p>
        </div>
        <input
          id="content-index-exclude-extensions"
          class={cn(settingsInput, 'max-w-[280px]')}
          bind:value={excludeExtensionsText}
          placeholder="iso, mp4, pdf"
          data-testid="content-index-exclude-extensions"
        />
      </div>

      <div class={settingsRow}>
        <div class="min-w-0">
          <label class={settingsRowLabel} for="content-index-retention">
            {translate('settings.content_index_retention')}
          </label>
          <p class={settingsRowHint}>{translate('settings.content_index_retention_hint')}</p>
        </div>
        <input
          id="content-index-retention"
          class={cn(settingsInput, 'max-w-[120px]')}
          type="number"
          min="1"
          max="3650"
          bind:value={config.retention_days}
          data-testid="content-index-retention"
        />
      </div>
    {/if}

    <div class="ci-status" role="status" data-testid="content-index-status">
      <span>
        {translate('settings.content_index_docs', {
          count: String(docs),
          size: humanBytes(bytes)
        })}
      </span>
      <span>{lastRunLabel()}</span>
      {#if runState.truncated}
        <span class="ci-warn" data-testid="content-index-truncated">
          {translate('settings.content_index_truncated')}
        </span>
      {/if}
      {#if runState.last_error}
        <span class="ci-warn" data-testid="content-index-run-error">{runState.last_error}</span>
      {/if}
    </div>

    <div class="ci-actions">
      <button
        type="button"
        class={settingsBtnPrimary}
        onclick={save}
        disabled={saving}
        data-testid="content-index-save"
      >
        {translate('settings.content_index_save')}
      </button>
      <button
        type="button"
        class={settingsBtn}
        onclick={rebuild}
        disabled={busy || !config.enabled || runState.running}
        data-testid="content-index-run"
      >
        {translate('settings.content_index_run')}
      </button>
      <button
        type="button"
        class={settingsBtnDanger}
        onclick={() => (deleteOpen = true)}
        disabled={busy || docs === 0}
        data-testid="content-index-delete"
      >
        {translate('settings.content_index_delete')}
      </button>
    </div>
    <p class={settingsHint}>{translate('settings.content_index_privacy')}</p>
  {/if}
</SettingsCard>

<Modal
  open={confirmEnableOpen}
  title={translate('settings.content_index_confirm_title')}
  size="sm"
  onclose={cancelEnable}
>
  <div class="ci-form">
    <p>{translate('settings.content_index_confirm_intro')}</p>
    <ul class="ci-confirm-list">
      <li>{translate('settings.content_index_confirm_roots')}</li>
      <li>{translate('settings.content_index_confirm_text_only')}</li>
      <li>{translate('settings.content_index_confirm_acl')}</li>
      <li>{translate('settings.content_index_confirm_delete')}</li>
    </ul>
    <div class="ci-form-actions">
      <button type="button" class={settingsBtn} onclick={cancelEnable}>
        {translate('common.cancel')}
      </button>
      <button
        type="button"
        class={settingsBtnPrimary}
        onclick={confirmEnable}
        data-testid="content-index-confirm-enable"
      >
        {translate('settings.content_index_confirm_understand')}
      </button>
    </div>
  </div>
</Modal>

<Modal
  open={deleteOpen}
  title={translate('settings.content_index_delete_title')}
  size="sm"
  onclose={() => (deleteOpen = false)}
>
  <div class="ci-form">
    <p>
      {translate('settings.content_index_delete_confirm', { count: String(docs) })}
    </p>
    <div class="ci-form-actions">
      <button type="button" class={settingsBtn} onclick={() => (deleteOpen = false)}>
        {translate('common.cancel')}
      </button>
      <button
        type="button"
        class={settingsBtnDanger}
        onclick={confirmDelete}
        disabled={busy}
        data-testid="content-index-delete-confirm"
      >
        {translate('settings.content_index_delete')}
      </button>
    </div>
  </div>
</Modal>

<style>
  .ci-alert {
    border: 1px solid var(--hv-shell-border);
    border-radius: 8px;
    padding: 10px 12px;
    margin-bottom: 12px;
    font-size: 0.8125rem;
    line-height: 1.25rem;
    color: var(--hv-shell-text);
    background: var(--hv-shell-border-soft);
  }

  .ci-alert-error {
    border-color: var(--hv-danger, #b42318);
  }

  .ci-check {
    width: 20px;
    height: 20px;
    flex: none;
  }

  .ci-roots {
    list-style: none;
    margin: 8px 0 12px;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .ci-root-row {
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: var(--hv-touch-target-min, 44px);
    cursor: pointer;
  }

  .ci-root-row input {
    width: 20px;
    height: 20px;
    flex: none;
  }

  .ci-root-path {
    font-family: var(--hv-font-mono, ui-monospace, monospace);
    font-size: 0.8125rem;
    overflow-wrap: anywhere;
  }

  .ci-status {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 14px;
    margin: 10px 0;
    padding: 8px 10px;
    border: 1px solid var(--hv-shell-border);
    border-radius: 8px;
    font-size: 0.8125rem;
    color: var(--hv-settings-label);
  }

  .ci-warn {
    color: var(--hv-danger, #b42318);
    overflow-wrap: anywhere;
  }

  .ci-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    justify-content: flex-end;
  }

  .ci-form {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .ci-confirm-list {
    margin: 0;
    padding-left: 20px;
    font-size: 0.8125rem;
    line-height: 1.5;
  }

  .ci-form-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
</style>
