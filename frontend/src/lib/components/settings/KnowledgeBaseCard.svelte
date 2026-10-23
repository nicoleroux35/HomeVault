<!-- ========================================================================
     File          : KnowledgeBaseCard.svelte
     Author        : Nicolas Le Roux
     Created       : 22/10/2026 by Nicolas Le Roux
     Last modified : 22/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Settings card for the AI Assistant's knowledge base (RAG): shows what is
       indexed, whether the embedding model the index needs is installed, and
       lets an admin install that model and trigger a reindex. The reindex runs
       in the background (desktop activity taskbar) and this card polls the
       status until it finishes.

     Dependencies:
       - svelte (>= 5), $lib/services/api, $lib/stores/i18n,
         $lib/stores/auth, ./SettingsCard, ./vocabulary

     Usage:
       Embedded in routes/Settings.svelte's `ai` (AI Assistant) category,
       under the model card and above the MCP gateway card.
     ======================================================================== -->
<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { t, translate } from '$lib/stores/i18n';
  import { isAdmin } from '$lib/stores/auth';
  import { IconCheck, IconRefresh } from '@tabler/icons-svelte';
  import SettingsCard from './SettingsCard.svelte';
  import { cn } from '$lib/utils.js';
  import {
    settingsRow,
    settingsRowStacked,
    settingsRowLabel,
    settingsRowHint,
    settingsHint,
    settingsBtn,
    settingsBtnGhost,
    settingsBtnPrimary
  } from './vocabulary';

  /* `SettingsCard` forwards a `class` for stacking spacing (same as McpCard). */
  let { class: klass = '' }: { class?: string } = $props();

  interface RagStatus {
    last_indexed_at: string | null;
    total_documents: number;
    total_chunks: number;
    embedding_model: string;
    status: string;
  }

  let loading = $state(true);
  let status = $state<RagStatus | null>(null);
  let installedModels = $state<string[]>([]);
  let busy = $state(false);
  let notice = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);

  /** The index cannot be built without its embedding model. */
  let embeddingInstalled = $derived(
    !!status && installedModels.some((m) => m === status!.embedding_model)
  );
  let indexing = $derived(status?.status === 'indexing');

  let pollTimer: ReturnType<typeof setInterval> | null = null;

  function stopPolling() {
    if (pollTimer) {
      clearInterval(pollTimer);
      pollTimer = null;
    }
  }

  /** Poll the status while a reindex is running, so progress is visible. */
  function syncPolling() {
    if (indexing && !pollTimer) {
      pollTimer = setInterval(loadStatus, 2000);
    } else if (!indexing) {
      stopPolling();
    }
  }

  async function loadStatus() {
    try {
      const res: any = await api.ragStatus();
      if (res?.success) {
        status = res.data as RagStatus;
      }
    } catch {
      // Status is best-effort; the card keeps its last known value.
    }
    syncPolling();
  }

  async function loadModels() {
    try {
      const res: any = await api.chatListModels();
      installedModels = (res?.data?.models ?? []).map((m: any) => m.name);
    } catch {
      installedModels = [];
    }
  }

  async function load() {
    loading = true;
    await Promise.all([loadStatus(), loadModels()]);
    loading = false;
  }

  onMount(load);
  onDestroy(stopPolling);

  async function installEmbedding() {
    if (!status) return;
    const model = status.embedding_model;
    busy = true;
    notice = null;
    const res: any = await api.chatPullModel(model);
    busy = false;
    if (res?.success) {
      await loadModels();
      notice = { kind: 'ok', text: translate('chat.rag_embedding_installed', { model }) };
    } else {
      notice = { kind: 'error', text: res?.error || res?.message || $t('chat.rag_action_failed') };
    }
  }

  async function reindex() {
    busy = true;
    notice = null;
    const res: any = await api.ragIndex();
    busy = false;
    if (res?.success) {
      // Optimistically show the running state; the poll confirms it.
      if (status) status = { ...status, status: 'indexing' };
      syncPolling();
      notice = { kind: 'ok', text: $t('chat.rag_reindex_started') };
    } else {
      notice = { kind: 'error', text: res?.error || res?.message || $t('chat.rag_action_failed') };
    }
  }

  function stateLabel(state: string): string {
    if (state === 'indexing') return $t('chat.rag_indexing');
    if (state === 'error') return $t('chat.rag_index_error');
    return $t('chat.rag_state_ready');
  }
</script>

<!--
  KnowledgeBaseCard — the only user-facing surface of the RAG knowledge base.
  The `/api/rag/*` endpoints existed and answered since the base was built, but
  nothing in the product called them, so the index stayed empty and the
  assistant answered "I don't know" for shipped features (ROADMAP 4.1). The
  card makes the two required steps reachable: install the embedding model,
  then reindex. It reuses the `chat.rag_*` translation keys the retired Chat
  RAG panel left behind rather than inventing a second namespace.

  Read is session-level (the status endpoint allows any session, so a
  non-admin can see whether the assistant is grounded); both actions are
  admin-only and simply absent for a non-admin rather than failing on click.
-->
<SettingsCard
  title={$t('chat.rag_title')}
  subtitle={$t('chat.rag_hint')}
  class={klass}
>
  <div class="flex items-center gap-2">
    {#if loading}
      <p class={cn(settingsHint, 'm-0!')} role="status">{$t('common.loading')}</p>
    {:else}
      <p class={cn(settingsHint, 'm-0!')} data-testid="kb-summary">
        {translate('chat.rag_documents', { count: String(status?.total_documents ?? 0) })}
        ·
        {translate('chat.rag_chunks', { count: String(status?.total_chunks ?? 0) })}
      </p>
    {/if}
    <button
      class={cn(settingsBtn, settingsBtnGhost, 'ml-auto hv-ic px-3')}
      onclick={load}
      disabled={busy || loading}
      aria-label={$t('chat.rag_refresh')}
      title={$t('chat.rag_refresh')}
    >
      <IconRefresh size={18} stroke={2} />
    </button>
  </div>

  {#if !loading}
    <div class={settingsRow}>
      <span class={settingsRowLabel}>{$t('chat.rag_status')}</span>
      <span
        data-testid="kb-state"
        data-state={status?.status ?? 'idle'}
        class={cn(
          'inline-flex items-center gap-1.5 px-2.5 py-1 rounded-full text-c-xs font-semibold',
          status?.status === 'error'
            ? 'bg-[var(--hv-error-bg)] text-[var(--hv-error)]'
            : indexing
              ? 'bg-[var(--hv-warning-bg)] text-[var(--hv-warning)]'
              : 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]'
        )}
      >
        {stateLabel(status?.status ?? 'idle')}
      </span>
    </div>

    <div class={settingsRowStacked}>
      <span class={settingsRowLabel}>{$t('chat.rag_embedding_model', { model: status?.embedding_model ?? '' })}</span>
      <div class="flex flex-wrap items-center gap-2">
        {#if embeddingInstalled}
          <span data-testid="kb-embedding-installed" class="inline-flex items-center gap-1 text-xs font-semibold text-[var(--hv-success)]">
            <IconCheck size={16} stroke={2.5} /> {$t('chat.rag_embedding_ready')}
          </span>
        {:else if $isAdmin}
          <button
            class={cn(settingsBtn, settingsBtnGhost)}
            data-testid="kb-install-embedding"
            onclick={installEmbedding}
            disabled={busy}
          >
            {$t('chat.rag_embedding_install')}
          </button>
        {/if}
      </div>
      <p class={settingsRowHint}>{$t('chat.rag_embedding_hint')}</p>
    </div>

    {#if status?.last_indexed_at}
      <div class={settingsRow}>
        <span class={settingsRowLabel}>{$t('chat.rag_last_indexed', { date: status.last_indexed_at })}</span>
      </div>
    {/if}

    {#if $isAdmin}
      <div class={settingsRowStacked}>
        <button
          class={cn(settingsBtn, settingsBtnPrimary, 'self-start')}
          data-testid="kb-reindex"
          onclick={reindex}
          disabled={busy || indexing || !embeddingInstalled}
        >
          {indexing ? $t('chat.rag_indexing') : $t('chat.rag_index_start')}
        </button>
        {#if !embeddingInstalled}
          <p class={settingsRowHint}>{$t('chat.rag_reindex_needs_embedding')}</p>
        {/if}
      </div>
    {/if}

    {#if notice}
      <p
        data-testid="kb-notice"
        class={cn('m-0! py-2 text-xs font-semibold', notice.kind === 'ok' ? 'text-[var(--hv-success)]' : 'text-[var(--hv-error)]')}
        role="status"
      >{notice.text}</p>
    {/if}
  {/if}
</SettingsCard>
