<!-- ========================================================================
     File          : RescueEmailCard.svelte
     Author        : Nicolas Le Roux
     Created       : 29/10/2026
     Last modified : 29/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       The account's rescue address: enter an address, receive a six-digit
       code, confirm it — and only then is the address stored. Shows the
       verified address and removes it on request.

     Dependencies:
       - svelte (>= 5), $lib/services/api, $lib/stores/i18n, app/SectionCard,
         theme/Button

     Usage:
       Mounted in routes/Account.svelte's grid. See docs/guides/identity.md.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { t, translate } from '$lib/stores/i18n';
  import { Button } from '$lib/components/theme';
  import { SectionCard } from '$lib/components/app';
  import { cn } from '$lib/utils.js';
  import { IconMail, IconCheck } from '$lib/icons/lucide';

  let loading = $state(true);
  let busy = $state(false);
  let error = $state('');
  let notice = $state('');

  let configured = $state(false);
  let storedEmail = $state<string | null>(null);

  /* The two-step form: an address is only a candidacy until its code comes
     back, so the pending address lives here, not in `storedEmail`. */
  let email = $state('');
  let pending = $state(false);
  let code = $state('');

  const inp =
    'w-full box-border h-[46px] px-3 border border-[var(--hv-shell-border-strong)] rounded-lg ' +
    'bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-strong)] text-c-md outline-none ' +
    'transition-colors placeholder:text-[var(--hv-shell-text-3)] focus:border-[var(--hv-primary)]';

  async function load() {
    loading = true;
    error = '';
    const res = await api.getRescueEmail();
    if (res.success && res.data) {
      configured = !!res.data.configured;
      storedEmail = res.data.email ?? null;
      email = storedEmail ?? '';
    } else {
      error = res.message || translate('account.rescue_load_error');
    }
    loading = false;
  }

  onMount(load);

  async function sendCode() {
    busy = true;
    error = '';
    notice = '';
    pending = false;
    code = '';
    const res = await api.setRescueEmail(email.trim());
    busy = false;
    if (res.success) {
      pending = true;
      notice = translate('account.rescue_code_sent', { email: email.trim() });
    } else {
      // A mail that could not be delivered is a failure, not a pending state:
      // a code nobody received must not sit there waiting to be guessed.
      error = res.message || translate('account.rescue_send_failed');
    }
  }

  async function verifyCode() {
    busy = true;
    error = '';
    const res = await api.verifyRescueEmail(code.trim());
    busy = false;
    if (res.success && res.data) {
      configured = true;
      storedEmail = res.data.email ?? email.trim();
      pending = false;
      code = '';
      notice = translate('account.rescue_verified');
    } else {
      error = res.message || translate('account.rescue_verify_failed');
    }
  }

  async function remove() {
    busy = true;
    error = '';
    notice = '';
    const res = await api.clearRescueEmail();
    busy = false;
    if (res.success) {
      configured = false;
      storedEmail = null;
      pending = false;
      email = '';
      code = '';
      notice = translate('account.rescue_removed');
    } else {
      error = res.message || translate('account.rescue_action_failed');
    }
  }
</script>

<SectionCard data-testid="account-card" class="col-span-2 w-full @max-[880px]:col-span-1">
  {#snippet titleSnippet()}
    <div data-testid="account-card-title" class="flex items-center gap-2 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">
      <IconMail size={17} strokeWidth={1.8} class="text-[var(--hv-primary)]" /> {$t('account.section_rescue')}
    </div>
  {/snippet}

  {#if notice}<div data-testid="rescue-ok" class="mb-3 px-3 py-2.5 rounded-lg bg-[var(--hv-success-bg)] border border-[var(--hv-success)]/40 text-[var(--hv-success-text)] text-c-label">{notice}</div>{/if}
  {#if error}<div data-testid="rescue-err" role="alert" class="mb-3 px-3 py-2.5 rounded-lg bg-[var(--hv-error-bg)] border border-[var(--hv-error)]/40 text-[var(--hv-error-text)] text-c-label">{error}</div>{/if}

  <p class="m-0 mb-3 text-c-label leading-relaxed text-[var(--hv-shell-text-2)]">
    {$t('account.rescue_desc')}
  </p>

  {#if loading}
    <p data-testid="rescue-loading" class="m-0 text-c-label text-[var(--hv-shell-text-3)]">{$t('common.loading')}</p>
  {:else if configured}
    <div data-testid="rescue-verified" class="flex items-center gap-3 px-3 py-2.5 rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]">
      <span class="flex-none grid place-items-center w-11 h-11 rounded-lg bg-[var(--hv-primary-light)] text-[var(--hv-primary)]">
        <IconCheck size={20} strokeWidth={2.2} />
      </span>
      <div class="flex-1 min-w-0">
        <strong class="block text-sm text-[var(--hv-shell-text-strong)] truncate">{storedEmail}</strong>
      </div>
      <Button variant="danger" onclick={remove} disabled={busy}>{$t('account.rescue_remove_btn')}</Button>
    </div>
    <div class="mt-4">
      <label for="rescue-change" class="block mb-1.5 text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('account.rescue_change_label')}</label>
      <div class="flex flex-wrap items-center gap-2.5">
        <input id="rescue-change" class={cn(inp, 'flex-1 min-w-[220px]')} type="email" bind:value={email} placeholder="you@example.com" />
        <Button variant="ghost" onclick={sendCode} disabled={busy || !email.trim()}>{$t('account.rescue_send_btn')}</Button>
      </div>
    </div>
  {:else}
    <div>
      <label for="rescue-email" class="block mb-1.5 text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('account.rescue_email_label')}</label>
      <input
        id="rescue-email"
        data-testid="rescue-email"
        class={inp}
        type="email"
        bind:value={email}
        placeholder="you@example.com"
        autocomplete="email"
        disabled={busy}
      />
      <p class="mt-1.5 mb-0 text-xs text-[var(--hv-shell-text-3)]">{$t('account.rescue_email_hint')}</p>
    </div>
    <div class="flex flex-wrap gap-2.5 mt-3">
      <Button variant="primary" onclick={sendCode} disabled={busy || !email.trim()} data-testid="rescue-send">
        {$t('account.rescue_send_btn')}
      </Button>
    </div>
  {/if}

  {#if pending}
    <div class="mt-4 pt-4 border-t border-[var(--hv-shell-border)]" data-testid="rescue-pending">
      <label for="rescue-code" class="block mb-1.5 text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('account.rescue_code_label')}</label>
      <div class="flex flex-wrap items-center gap-2.5">
        <input
          id="rescue-code"
          data-testid="rescue-code"
          class={cn(inp, 'max-w-[200px] text-center font-[family-name:var(--hv-font-mono)] tracking-[0.32em]')}
          type="text"
          bind:value={code}
          inputmode="numeric"
          maxlength="6"
          autocomplete="one-time-code"
          placeholder="000000"
          disabled={busy}
        />
        <Button variant="primary" onclick={verifyCode} disabled={busy || code.trim().length < 6} data-testid="rescue-verify">
          {$t('account.rescue_verify_btn')}
        </Button>
        <Button variant="ghost" onclick={sendCode} disabled={busy}>{$t('account.rescue_resend_btn')}</Button>
      </div>
    </div>
  {/if}
</SectionCard>
