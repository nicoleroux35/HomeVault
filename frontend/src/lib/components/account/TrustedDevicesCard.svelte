<!-- ========================================================================
     File          : TrustedDevicesCard.svelte
     Author        : Nicolas Le Roux
     Created       : 29/10/2026
     Last modified : 29/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       The account's trusted devices: one row per browser that may skip the
       second factor, when it was last used, when its trust expires, and the
       two revocation actions. The token itself never leaves the cookie.

     Dependencies:
       - svelte (>= 5), $lib/services/api, $lib/stores/i18n, app/SectionCard,
         theme/Button, $lib/utils/confirmDialog.svelte

     Usage:
       Mounted in routes/Account.svelte's grid. See docs/guides/identity.md.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { t, translate } from '$lib/stores/i18n';
  import { Button } from '$lib/components/theme';
  import { SectionCard, ConfirmDialog } from '$lib/components/app';
  import { createConfirmState } from '$lib/utils/confirmDialog.svelte';
  import { cn } from '$lib/utils.js';
  import { IconDeviceDesktop, IconTrash, IconShieldCheckered } from '$lib/icons/lucide';

  interface Device {
    id: string;
    label: string;
    created_at: string;
    last_seen_at: string | null;
    expires_at: string;
  }

  let loading = $state(true);
  let busy = $state(false);
  let error = $state('');
  let notice = $state('');
  let devices = $state<Device[]>([]);
  let lifetimeDays = $state(30);

  const dlg = createConfirmState();

  async function load() {
    loading = true;
    error = '';
    const res = await api.getTrustedDevices();
    if (res.success && res.data) {
      devices = Array.isArray(res.data.devices) ? res.data.devices : [];
      lifetimeDays = Number(res.data.lifetime_days ?? 30);
    } else {
      error = res.message || translate('account.devices_load_error');
    }
    loading = false;
  }

  onMount(load);

  function formatDate(value: string | null): string {
    if (!value) return '—';
    const d = new Date(value);
    return Number.isNaN(d.getTime()) ? '—' : d.toLocaleString();
  }

  async function revoke(device: Device) {
    const ok = await dlg.ask(
      translate('account.devices_revoke_message', { label: device.label }),
      { danger: true },
    );
    if (!ok) return;
    busy = true;
    error = '';
    notice = '';
    const res = await api.revokeTrustedDevice(device.id);
    busy = false;
    if (res.success) {
      devices = devices.filter((d) => d.id !== device.id);
      notice = translate('account.devices_revoked');
    } else {
      error = res.message || translate('account.devices_action_failed');
    }
  }

  async function revokeAll() {
    const ok = await dlg.ask(translate('account.devices_revoke_all_message'), {
      danger: true,
    });
    if (!ok) return;
    busy = true;
    error = '';
    notice = '';
    const res = await api.revokeAllTrustedDevices();
    busy = false;
    if (res.success) {
      devices = [];
      notice = translate('account.devices_revoked');
    } else {
      error = res.message || translate('account.devices_action_failed');
    }
  }
</script>

<SectionCard data-testid="account-card" class="col-span-2 w-full @max-[880px]:col-span-1">
  {#snippet titleSnippet()}
    <div data-testid="account-card-title" class="flex items-center gap-2 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">
      <IconShieldCheckered size={17} strokeWidth={1.8} class="text-[var(--hv-primary)]" /> {$t('account.section_devices')}
    </div>
  {/snippet}

  {#if notice}<div data-testid="devices-ok" class="mb-3 px-3 py-2.5 rounded-lg bg-[var(--hv-success-bg)] border border-[var(--hv-success)]/40 text-[var(--hv-success-text)] text-c-label">{notice}</div>{/if}
  {#if error}<div data-testid="devices-err" role="alert" class="mb-3 px-3 py-2.5 rounded-lg bg-[var(--hv-error-bg)] border border-[var(--hv-error)]/40 text-[var(--hv-error-text)] text-c-label">{error}</div>{/if}

  <p class="m-0 mb-3 text-c-label leading-relaxed text-[var(--hv-shell-text-2)]">
    {$t('account.devices_desc', { days: String(lifetimeDays) })}
  </p>

  {#if loading}
    <p data-testid="devices-loading" class="m-0 text-c-label text-[var(--hv-shell-text-3)]">{$t('common.loading')}</p>
  {:else if devices.length === 0}
    <p data-testid="devices-empty" class="m-0 text-c-label text-[var(--hv-shell-text-3)]">{$t('account.devices_empty')}</p>
  {:else}
    <ul data-testid="devices-list" class="list-none m-0 p-0 flex flex-col gap-2">
      {#each devices as device (device.id)}
        <li data-testid="device-row" class="flex items-center gap-3 px-3 py-2.5 rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]">
          <span class="flex-none grid place-items-center w-11 h-11 rounded-lg bg-[var(--hv-shell-border)] text-[var(--hv-shell-text-2)]">
            <IconDeviceDesktop size={20} strokeWidth={1.8} />
          </span>
          <div class="flex-1 min-w-0">
            <strong class="block text-sm text-[var(--hv-shell-text-strong)] truncate">{device.label}</strong>
            <small class="block text-xs text-[var(--hv-shell-text)] mt-0.5">
              {$t('account.devices_last_seen')}: {formatDate(device.last_seen_at ?? device.created_at)} — {$t('account.devices_expires')}: {formatDate(device.expires_at)}
            </small>
          </div>
          <button
            data-testid="device-revoke"
            class={cn(
              'flex-none grid place-items-center w-[30px] h-[30px] p-0 rounded-lg border-none bg-transparent',
              'text-[var(--hv-shell-text-2)] cursor-pointer transition-colors hover:text-[var(--hv-error-text)]',
              'focus-visible:outline-none focus-visible:shadow-[var(--hv-focus-ring)] disabled:opacity-50 disabled:cursor-default',
            )}
            onclick={() => revoke(device)}
            disabled={busy}
            aria-label={$t('account.devices_revoke_title')}
          >
            <IconTrash size={16} strokeWidth={1.8} />
          </button>
        </li>
      {/each}
    </ul>
  {/if}

  <div class="flex flex-wrap gap-2.5 mt-4">
    <Button variant="ghost" onclick={load} disabled={loading}>{$t('common.refresh')}</Button>
    <Button variant="danger" onclick={revokeAll} disabled={busy || devices.length === 0}>
      {$t('account.devices_revoke_all_btn')}
    </Button>
  </div>
</SectionCard>

<ConfirmDialog {dlg} onconfirm={() => dlg.settle(true)} oncancel={() => dlg.settle(false)} />
