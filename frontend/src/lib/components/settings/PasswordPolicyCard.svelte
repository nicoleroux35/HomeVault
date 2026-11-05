<!-- ========================================================================
     File          : PasswordPolicyCard.svelte
     Author        : Nicolas Le Roux
     Created       : 05/11/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Settings card for the global password-expiry policy. `0` keeps the
       historical “never expire”; a positive number flags an account at
       sign-in once its password is older than that, and the existing
       password-change guard limits the session to the change flow.

     Dependencies:
       - svelte (>= 5), $lib/services/api, $lib/stores/i18n, ./SettingsCard,
         ./vocabulary

     Usage:
       Embedded in routes/Settings.svelte's `security` category. See
       docs/guides/security.md.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { t } from '$lib/stores/i18n';
  import SettingsCard from './SettingsCard.svelte';
  import { cn } from '$lib/utils.js';
  import {
    settingsRow,
    settingsRowLabel,
    settingsRowHint,
    settingsHint,
    settingsInput,
    settingsBtnPrimary
  } from './vocabulary';

  let { class: klass = '' }: { class?: string } = $props();

  let loading = $state(true);
  let expiryDays = $state(0);
  let maxDays = $state(3650);
  let busy = $state(false);
  let notice = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);

  async function load() {
    loading = true;
    const res = await api.get('/settings/password-policy');
    if (res.success && res.data) {
      expiryDays = Number(res.data.expiry_days ?? 0);
      maxDays = Number(res.data.max_expiry_days ?? 3650);
    } else {
      notice = { kind: 'error', text: res.message || $t('settings.password_policy_load_error') };
    }
    loading = false;
  }

  onMount(load);

  async function save() {
    busy = true;
    notice = null;
    const res = await api.put('/settings/password-policy', {
      expiry_days: Number(expiryDays)
    });
    if (res.success) {
      expiryDays = Number(res.data?.expiry_days ?? expiryDays);
      notice = { kind: 'ok', text: $t('settings.password_policy_saved') };
    } else {
      notice = { kind: 'error', text: res.message || $t('settings.password_policy_save_error') };
    }
    busy = false;
  }
</script>

<SettingsCard
  title={$t('settings.password_policy_title')}
  subtitle={$t('settings.password_policy_subtitle')}
  class={cn('password-policy-card', klass)}
>
  {#if loading}
    <p class={settingsHint}>{$t('common.loading')}</p>
  {:else}
    <div class={settingsRow}>
      <div>
        <span class={settingsRowLabel}>{$t('settings.password_policy_days')}</span>
        <p class={settingsRowHint}>
          {$t('settings.password_policy_hint', { max: String(maxDays) })}
        </p>
      </div>
      <input
        class={cn(settingsInput, 'w-28')}
        type="number"
        min="0"
        max={maxDays}
        step="1"
        bind:value={expiryDays}
        aria-label={$t('settings.password_policy_days')}
      />
    </div>

    {#if notice}
      <p
        class={cn(
          'm-0 mt-2 text-xs',
          notice.kind === 'ok' ? 'text-[var(--hv-success)]' : 'text-[var(--hv-error)]'
        )}
        role="status"
      >
        {notice.text}
      </p>
    {/if}

    <div class="mt-3 flex justify-end">
      <button class={settingsBtnPrimary} onclick={save} disabled={busy}>
        {$t('common.save')}
      </button>
    </div>
  {/if}
</SettingsCard>
