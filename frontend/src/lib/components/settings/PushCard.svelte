<!-- ========================================================================
     File          : PushCard.svelte
     Author        : Nicolas Le Roux
     Created       : 06/11/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Web Push notifications (phase D2): turn them on for this browser, list
       the devices this account has subscribed, and revoke one. The browser
       asks its own permission; nothing is subscribed before that explicit
       click, and the server's VAPID public key is the only key that reaches
       the page.

       Design rules kept here (CLAUDE.md, AGENTS.md):
       - never claim support the browser does not have: HTTP on a LAN is not a
         secure context, so the card explains HTTPS instead of showing a dead
         button;
       - permission denied is a state with its own sentence, not a silent
         no-op;
       - removing a device names it and is reversible by enabling again;
       - every control is a 44px target, labelled and keyboard reachable.

     Dependencies:
       - svelte (>= 5), $lib/services/api-phase4, $lib/stores/i18n,
         $lib/utils/notify, ./SettingsCard, ./vocabulary, theme/Modal

     Usage:
       Embedded in routes/Settings.svelte's notifications category. See
       docs/guides/web-push.md.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { translate } from '$lib/stores/i18n';
  import { notifyError, notifySuccess } from '$lib/utils/notify';
  import { getPushState, subscribePush, unsubscribePush } from '$lib/services/api-phase4';
  import type { PushDevice } from '$lib/types/phase4';
  import SettingsCard from './SettingsCard.svelte';
  import Modal from '$lib/components/theme/Modal.svelte';
  import { cn } from '$lib/utils.js';
  import {
    settingsRowLabel,
    settingsRowHint,
    settingsHint,
    settingsBtn,
    settingsBtnPrimary,
    settingsBtnDanger
  } from './vocabulary';

  let { class: klass = '' }: { class?: string } = $props();

  const DEVICE_KEY = 'hv-push-device';

  let loading = $state(true);
  let busy = $state(false);
  let loadError = $state('');
  let actionError = $state('');
  let available = $state(false);
  let publicKey = $state<string | null>(null);
  let devices = $state<PushDevice[]>([]);
  let maxDevices = $state(10);
  let serverError = $state('');
  let permission = $state<string>('default');
  let thisDeviceId = $state<string | null>(null);
  let removeTarget = $state<PushDevice | null>(null);

  let supported = $state(false);

  onMount(async () => {
    supported = supportsPush();
    if (typeof Notification !== 'undefined') {
      permission = Notification.permission;
    }
    try {
      thisDeviceId = localStorage.getItem(DEVICE_KEY);
    } catch {
      thisDeviceId = null;
    }
    await load();
  });

  function supportsPush(): boolean {
    if (typeof window === 'undefined' || typeof navigator === 'undefined') return false;
    return (
      window.isSecureContext === true &&
      'serviceWorker' in navigator &&
      'PushManager' in window &&
      'Notification' in window
    );
  }

  async function load() {
    loading = true;
    loadError = '';
    try {
      const res = await getPushState();
      available = !!res.available;
      publicKey = res.public_key ?? null;
      devices = res.subscriptions ?? [];
      maxDevices = res.max_devices ?? 10;
      serverError = res.error ?? '';
    } catch (e) {
      loadError = e instanceof Error ? e.message : translate('settings.push_load_failed');
    } finally {
      loading = false;
    }
  }

  function isThisDevice(device: PushDevice): boolean {
    return thisDeviceId !== null && device.id === thisDeviceId;
  }

  async function enableHere() {
    if (busy || !publicKey) return;
    busy = true;
    actionError = '';
    try {
      const permissionResult = await Notification.requestPermission();
      permission = permissionResult;
      if (permissionResult !== 'granted') {
        actionError = translate('settings.push_permission_denied');
        return;
      }

      const registration = await (navigator as any).serviceWorker.register('/sw.js');
      await (navigator as any).serviceWorker.ready;

      let subscription = await registration.pushManager.getSubscription();
      if (!subscription) {
        subscription = await registration.pushManager.subscribe({
          userVisibleOnly: true,
          applicationServerKey: urlBase64ToUint8Array(publicKey)
        });
      }

      const p256dh = subscription.getKey('p256dh');
      const auth = subscription.getKey('auth');
      if (!p256dh || !auth) {
        throw new Error(translate('settings.push_key_missing'));
      }

      const res = await subscribePush({
        endpoint: subscription.endpoint,
        keys: { p256dh: toBase64Url(p256dh), auth: toBase64Url(auth) }
      });
      thisDeviceId = res.subscription.id;
      try {
        localStorage.setItem(DEVICE_KEY, res.subscription.id);
      } catch {
        /* storage unavailable — the list still works, this device is just not tagged */
      }
      notifySuccess('settings.push_enabled');
      await load();
    } catch (e) {
      actionError = e instanceof Error ? e.message : translate('settings.push_enable_failed');
    } finally {
      busy = false;
    }
  }

  async function confirmRemove() {
    const target = removeTarget;
    if (!target || busy) return;
    busy = true;
    try {
      if (isThisDevice(target)) {
        const registration = await (navigator as any).serviceWorker?.getRegistration?.();
        const subscription = await registration?.pushManager?.getSubscription?.();
        await subscription?.unsubscribe?.().catch(() => false);
        try {
          localStorage.removeItem(DEVICE_KEY);
        } catch {
          /* ignore */
        }
        thisDeviceId = null;
      }
      await unsubscribePush(target.id);
      removeTarget = null;
      notifySuccess('settings.push_removed');
      await load();
    } catch (e) {
      notifyError(
        e instanceof Error ? { success: false, message: e.message } : null,
        'settings.push_remove_failed'
      );
    } finally {
      busy = false;
    }
  }

  function urlBase64ToUint8Array(base64: string): Uint8Array {
    const padded = base64.replace(/-/g, '+').replace(/_/g, '/');
    const raw = atob(padded + '='.repeat((4 - (padded.length % 4)) % 4));
    const bytes = new Uint8Array(raw.length);
    for (let i = 0; i < raw.length; i += 1) {
      bytes[i] = raw.charCodeAt(i);
    }
    return bytes;
  }

  function toBase64Url(buffer: ArrayBuffer): string {
    const bytes = new Uint8Array(buffer);
    let binary = '';
    for (const byte of bytes) {
      binary += String.fromCharCode(byte);
    }
    return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  }

  function addedOn(value: string): string {
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
  }

  function lastDelivery(device: PushDevice): string {
    if (device.last_success_at) {
      return translate('settings.push_last_success', { date: addedOn(device.last_success_at) });
    }
    if (device.failure_count > 0) {
      return translate('settings.push_failures', { count: String(device.failure_count) });
    }
    return translate('settings.push_never_delivered');
  }
</script>

<SettingsCard
  title={translate('settings.push_title')}
  subtitle={translate('settings.push_subtitle')}
  class={cn('push-card', klass)}
  data-testid="push-card"
>
  {#if loading}
    <p class={settingsHint}>{translate('settings.loading')}</p>
  {:else}
    {#if loadError}
      <div class="push-alert push-alert-error" role="alert" data-testid="push-error">
        {loadError}
      </div>
    {:else if !available}
      <div class="push-alert" role="status" data-testid="push-unavailable">
        {translate('settings.push_unavailable')}
        {#if serverError}
          <span class="push-reason">{serverError}</span>
        {/if}
      </div>
    {:else if !supported}
      <div class="push-alert" role="status" data-testid="push-unsupported">
        {translate('settings.push_unsupported')}
      </div>
    {:else}
      {#if permission === 'denied'}
        <div class="push-alert push-alert-error" role="alert" data-testid="push-permission">
          {translate('settings.push_permission_denied')}
        </div>
      {/if}
      {#if actionError}
        <div class="push-alert push-alert-error" role="alert" data-testid="push-action-error">
          {actionError}
        </div>
      {/if}

      {#if devices.length === 0}
        <p class={settingsHint} data-testid="push-empty">{translate('settings.push_empty')}</p>
      {:else}
        <ul class="push-list" data-testid="push-devices">
          {#each devices as device (device.id)}
            <li class="push-item" data-testid="push-device">
              <div class="push-item-main">
                <span class={settingsRowLabel} data-testid="push-device-label">
                  {device.device_label}
                  {#if isThisDevice(device)}
                    <span class="push-this" data-testid="push-this-device">
                      {translate('settings.push_this_device')}
                    </span>
                  {/if}
                </span>
                <span class={settingsRowHint} data-testid="push-device-host">
                  {device.endpoint_host} · {addedOn(device.created_at)} · {lastDelivery(device)}
                </span>
              </div>
              <button
                type="button"
                class={settingsBtnDanger}
                onclick={() => (removeTarget = device)}
                aria-label={translate('settings.push_remove_aria', { device: device.device_label })}
                data-testid="push-remove"
              >
                {translate('settings.push_remove_short')}
              </button>
            </li>
          {/each}
        </ul>
      {/if}

      <div class="push-actions">
        {#if thisDeviceId && devices.some((device) => device.id === thisDeviceId)}
          <span class="push-ok" role="status" data-testid="push-this-device-status">
            {translate('settings.push_this_device_enabled')}
          </span>
        {:else}
          <button
            type="button"
            class={settingsBtnPrimary}
            onclick={enableHere}
            disabled={busy}
            data-testid="push-enable"
          >
            {translate('settings.push_enable')}
          </button>
        {/if}
      </div>
      <p class={settingsHint}>
        {translate('settings.push_count', {
          count: String(devices.length),
          max: String(maxDevices)
        })}
      </p>
      <p class={settingsHint}>{translate('settings.push_privacy')}</p>
    {/if}
  {/if}
</SettingsCard>

<Modal
  open={removeTarget !== null}
  title={translate('settings.push_remove_title')}
  size="sm"
  onclose={() => (removeTarget = null)}
>
  <div class="push-form">
    <p>
      {translate('settings.push_remove_confirm', {
        device: removeTarget?.device_label ?? ''
      })}
    </p>
    <div class="push-form-actions">
      <button type="button" class={settingsBtn} onclick={() => (removeTarget = null)}>
        {translate('common.cancel')}
      </button>
      <button
        type="button"
        class={settingsBtnDanger}
        onclick={confirmRemove}
        disabled={busy}
        data-testid="push-remove-confirm"
      >
        {translate('settings.push_remove_short')}
      </button>
    </div>
  </div>
</Modal>

<style>
  .push-alert {
    border: 1px solid var(--hv-shell-border);
    border-radius: 8px;
    padding: 10px 12px;
    margin-bottom: 12px;
    font-size: 0.8125rem;
    line-height: 1.25rem;
    color: var(--hv-shell-text);
    background: var(--hv-shell-border-soft);
  }

  .push-alert-error {
    border-color: var(--hv-danger, #b42318);
  }

  .push-reason {
    display: block;
    margin-top: 4px;
    overflow-wrap: anywhere;
  }

  .push-list {
    list-style: none;
    margin: 10px 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .push-item {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 8px 10px;
    border: 1px solid var(--hv-shell-border);
    border-radius: 8px;
  }

  .push-item-main {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .push-this {
    display: inline-block;
    margin-left: 6px;
    border: 1px solid var(--hv-shell-border);
    border-radius: 999px;
    padding: 0 8px;
    font-size: 0.6875rem;
    font-weight: 500;
    color: var(--hv-settings-label);
  }

  .push-actions {
    display: flex;
    justify-content: flex-end;
    margin-top: 8px;
  }

  .push-ok {
    font-size: 0.8125rem;
    color: var(--hv-success, #067647);
    border: 1px solid currentColor;
    border-radius: 999px;
    padding: 2px 10px;
  }

  .push-form {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .push-form-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
</style>
