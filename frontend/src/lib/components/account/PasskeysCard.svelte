<!-- ========================================================================
     File          : PasskeysCard.svelte
     Author        : Nicolas Le Roux
     Created       : 31/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       The account's passkeys (WebAuthn credentials): one row per registered
       authenticator with a rename and a revoke action, plus the "Add a
       passkey" button that runs the registration ceremony. A passkey is a
       separate credential — this card never touches the password or the two-
       factor setting.

     Dependencies:
       - svelte (>= 5), $lib/services/api, $lib/stores/i18n,
         $lib/stores/passkey, app/SectionCard, theme/Button,
         $lib/utils/confirmDialog.svelte

     Usage:
       Mounted in routes/Account.svelte's grid. See docs/guides/identity.md.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { t, translate } from '$lib/stores/i18n';
  import { registerPasskey, passkeySupported } from '$lib/stores/passkey';
  import { Button } from '$lib/components/theme';
  import { SectionCard, ConfirmDialog } from '$lib/components/app';
  import { createConfirmState } from '$lib/utils/confirmDialog.svelte';
  import { cn } from '$lib/utils.js';
  import { IconFingerprint, IconTrash, IconPencil } from '$lib/icons/lucide';

  interface Passkey {
    credential_id: string;
    label: string;
    algorithm: string;
    attestation_fmt: string;
    user_verified: boolean;
    created_at: string;
    last_used_at: string | null;
  }

  let loading = $state(true);
  let busy = $state(false);
  let error = $state('');
  let notice = $state('');
  let passkeys = $state<Passkey[]>([]);
  let supported = $state(true);
  let relyingParty = $state<{ id: string; origin: string } | null>(null);
  let max = $state(20);

  // New-passkey label, kept in a small inline form rather than a modal: the
  // ceremony already interrupts with the browser's own sheet.
  let newLabel = $state('');
  let renaming = $state<string | null>(null);
  let renameLabel = $state('');

  const dlg = createConfirmState();
  const canRunCeremony = $derived(supported && passkeySupported());

  async function load() {
    loading = true;
    error = '';
    const res = await api.getPasskeys();
    if (res.success && res.data) {
      const data = res.data as any;
      passkeys = Array.isArray(data.passkeys) ? data.passkeys : [];
      supported = data.supported !== false;
      relyingParty = data.relying_party ?? null;
      max = Number(data.max ?? 20);
    } else {
      error = res.message || translate('account.passkeys_load_error');
    }
    loading = false;
  }

  onMount(load);

  function formatDate(value: string | null): string {
    if (!value) return '—';
    const d = new Date(value);
    return Number.isNaN(d.getTime()) ? '—' : d.toLocaleString();
  }

  async function add() {
    busy = true;
    error = '';
    notice = '';
    const result = await registerPasskey(newLabel.trim());
    busy = false;
    if (result.ok) {
      newLabel = '';
      notice = translate('account.passkeys_added');
      await load();
      return;
    }
    // Three failure shapes, three sentences: unsupported browser, user
    // cancelled, and everything else (already registered, server refusal).
    if (result.error === 'unsupported') {
      error = translate('account.passkeys_unsupported');
    } else if (result.error === 'cancelled') {
      error = translate('account.passkeys_cancelled');
    } else {
      error = result.error;
    }
  }

  async function rename(passkey: Passkey) {
    if (renaming === passkey.credential_id) {
      busy = true;
      error = '';
      notice = '';
      const res = await api.renamePasskey(passkey.credential_id, renameLabel.trim());
      busy = false;
      if (res.success) {
        renaming = null;
        notice = translate('account.passkeys_renamed');
        await load();
      } else {
        error = res.message || translate('account.passkeys_action_failed');
      }
    } else {
      renaming = passkey.credential_id;
      renameLabel = passkey.label;
    }
  }

  async function revoke(passkey: Passkey) {
    const ok = await dlg.ask(
      translate('account.passkeys_revoke_message', { label: passkey.label }),
      { danger: true },
    );
    if (!ok) return;
    busy = true;
    error = '';
    notice = '';
    const res = await api.deletePasskey(passkey.credential_id);
    busy = false;
    if (res.success) {
      passkeys = passkeys.filter((p) => p.credential_id !== passkey.credential_id);
      notice = translate('account.passkeys_revoked');
    } else {
      error = res.message || translate('account.passkeys_action_failed');
    }
  }
</script>

<SectionCard data-testid="account-card" class="col-span-2 w-full @max-[880px]:col-span-1">
  {#snippet titleSnippet()}
    <div data-testid="account-card-title" class="flex items-center gap-2 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">
      <IconFingerprint size={17} strokeWidth={1.8} class="text-[var(--hv-primary)]" /> {$t('account.section_passkeys')}
    </div>
  {/snippet}

  {#if notice}<div data-testid="passkeys-ok" class="mb-3 px-3 py-2.5 rounded-lg bg-[var(--hv-success-bg)] border border-[var(--hv-success)]/40 text-[var(--hv-success-text)] text-c-label">{notice}</div>{/if}
  {#if error}<div data-testid="passkeys-err" role="alert" class="mb-3 px-3 py-2.5 rounded-lg bg-[var(--hv-error-bg)] border border-[var(--hv-error)]/40 text-[var(--hv-error-text)] text-c-label">{error}</div>{/if}

  <p class="m-0 mb-3 text-c-label leading-relaxed text-[var(--hv-shell-text-2)]">
    {$t('account.passkeys_desc')}
  </p>

  {#if !canRunCeremony}
    <p data-testid="passkeys-unsupported" class="m-0 text-c-label text-[var(--hv-shell-text-3)]">
      {$t('account.passkeys_unsupported')}
    </p>
  {:else if loading}
    <p data-testid="passkeys-loading" class="m-0 text-c-label text-[var(--hv-shell-text-3)]">{$t('common.loading')}</p>
  {:else if passkeys.length === 0 && !error}
    <p data-testid="passkeys-empty" class="m-0 text-c-label text-[var(--hv-shell-text-3)]">{$t('account.passkeys_empty')}</p>
  {:else}
    <ul data-testid="passkeys-list" class="list-none m-0 p-0 flex flex-col gap-2">
      {#each passkeys as passkey (passkey.credential_id)}
        <li data-testid="passkey-row" class="flex items-center gap-3 px-3 py-2.5 rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]">
          <span class="flex-none grid place-items-center w-11 h-11 rounded-lg bg-[var(--hv-shell-border)] text-[var(--hv-shell-text-2)]">
            <IconFingerprint size={20} strokeWidth={1.8} />
          </span>
          <div class="flex-1 min-w-0">
            {#if renaming === passkey.credential_id}
              <form class="flex items-center gap-2" onsubmit={(e) => { e.preventDefault(); rename(passkey); }}>
                <input
                  class="flex-1 min-w-0 px-2 py-1 rounded-md border border-[var(--hv-shell-border)] bg-[var(--hv-shell-glass)] text-sm text-[var(--hv-shell-text-strong)]"
                  bind:value={renameLabel}
                  aria-label={$t('account.passkeys_rename')}
                  maxlength="64"
                  data-testid="passkey-rename-input"
                />
                <Button variant="primary" size="sm" type="submit" disabled={busy}>{$t('common.save')}</Button>
              </form>
            {:else}
              <strong class="block text-sm text-[var(--hv-shell-text-strong)] truncate" data-testid="passkey-label">{passkey.label}</strong>
            {/if}
            <small class="block text-xs text-[var(--hv-shell-text)] mt-0.5">
              {$t('account.passkeys_created')}: {formatDate(passkey.created_at)}
              · {$t('account.passkeys_last_used')}: {formatDate(passkey.last_used_at)}
              {#if passkey.user_verified}· <span data-testid="passkey-uv">{$t('account.passkeys_user_verified')}</span>{/if}
            </small>
          </div>
          <button
            data-testid="passkey-rename"
            class={cn(
              'flex-none grid place-items-center w-[30px] h-[30px] p-0 rounded-lg border-none bg-transparent',
              'text-[var(--hv-shell-text-2)] cursor-pointer transition-colors hover:text-[var(--hv-primary)]',
              'focus-visible:outline-none focus-visible:shadow-[var(--hv-focus-ring)] disabled:opacity-50 disabled:cursor-default',
            )}
            onclick={() => rename(passkey)}
            disabled={busy}
            aria-label={$t('account.passkeys_rename')}
          >
            <IconPencil size={16} strokeWidth={1.8} />
          </button>
          <button
            data-testid="passkey-revoke"
            class={cn(
              'flex-none grid place-items-center w-[30px] h-[30px] p-0 rounded-lg border-none bg-transparent',
              'text-[var(--hv-shell-text-2)] cursor-pointer transition-colors hover:text-[var(--hv-error-text)]',
              'focus-visible:outline-none focus-visible:shadow-[var(--hv-focus-ring)] disabled:opacity-50 disabled:cursor-default',
            )}
            onclick={() => revoke(passkey)}
            disabled={busy}
            aria-label={$t('account.passkeys_revoke_title')}
          >
            <IconTrash size={16} strokeWidth={1.8} />
          </button>
        </li>
      {/each}
    </ul>
  {/if}

  {#if canRunCeremony}
    <div class="flex flex-wrap items-center gap-2.5 mt-4">
      <input
        class="flex-1 min-w-40 px-3 py-2 rounded-lg border border-[var(--hv-shell-border)] bg-[var(--hv-shell-glass)] text-sm text-[var(--hv-shell-text-strong)]"
        placeholder={$t('account.passkeys_label_placeholder')}
        aria-label={$t('account.passkeys_label_placeholder')}
        bind:value={newLabel}
        maxlength="64"
        data-testid="passkey-new-label"
      />
      <Button
        variant="primary"
        data-testid="passkeys-add"
        onclick={add}
        disabled={busy || passkeys.length >= max}
        loading={busy}
      >
        {$t('account.passkeys_add')}
      </Button>
    </div>
    {#if passkeys.length >= max}
      <p class="m-0 mt-2 text-c-label text-[var(--hv-shell-text-3)]">
        {$t('account.passkeys_max', { max: String(max) })}
      </p>
    {/if}
    {#if relyingParty}
      <!-- The origin a passkey is bound to: showing it makes a misconfigured
           hostname visible instead of puzzling. -->
      <p data-testid="passkeys-origin" class="m-0 mt-2 text-c-label text-[var(--hv-shell-text-3)]">
        {$t('account.passkeys_origin', { origin: relyingParty.origin })}
      </p>
    {/if}
  {/if}
</SectionCard>

<ConfirmDialog {dlg} onconfirm={() => dlg.settle(true)} oncancel={() => dlg.settle(false)} />
