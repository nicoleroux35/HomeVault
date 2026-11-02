<!-- ========================================================================
     File          : FanControlCard.svelte
     Author        : Nicolas Le Roux
     Created       : 02/11/2026
     Last modified : 02/11/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Settings card for thermal management: the detected hwmon fan channels,
       the active profile (Auto/Quiet/Balanced/Performance/Manual), the manual
       duty slider and the "hand control back to the firmware" action.

       Rewritten on the shared settings vocabulary and design tokens. The
       previous version carried hardcoded English strings, native `alert()` /
       `confirm()` dialogs and CSS variables that do not exist in the token
       set (`--card-bg`, `--primary-color`), so it rendered unstyled.

     Dependencies:
       - svelte (>= 5), $lib/services/api-phase4, $lib/stores/i18n,
         ./SettingsCard, ./vocabulary

     Usage:
       Embedded in routes/Settings.svelte's hardware category. See
       docs/guides/fan-control.md.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from '$lib/stores/i18n';
  import {
    listFanChannels,
    getFanConfig,
    updateFanConfig,
    restoreFanAuto
  } from '$lib/services/api-phase4';
  import type { FanConfig, FanChannel, FanProfile } from '$lib/types/phase4';
  import SettingsCard from './SettingsCard.svelte';
  import { cn } from '$lib/utils.js';
  import {
    settingsHint,
    settingsRow,
    settingsRowLabel,
    settingsRowHint,
    settingsBtn,
    settingsBtnPrimary
  } from './vocabulary';

  let { class: klass = '' }: { class?: string } = $props();

  let loading = $state(true);
  let saving = $state(false);
  let restoring = $state(false);
  let loadError = $state('');
  let notice = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);

  let config = $state<FanConfig | null>(null);
  let channels = $state<FanChannel[]>([]);
  let isLaptop = $state(false);
  let serverMessage = $state('');

  /**
   * The profile catalogue is built inside a `$derived` so every label goes
   * through `$t` and re-renders when the language changes. A module-level
   * `const` would freeze the labels at import time.
   */
  const profileOptions = $derived<
    { value: FanProfile; label: string; description: string }[]
  >([
    {
      value: 'auto',
      label: $t('settings.fan_profile_auto'),
      description: $t('settings.fan_profile_auto_desc')
    },
    {
      value: 'quiet',
      label: $t('settings.fan_profile_quiet'),
      description: $t('settings.fan_profile_quiet_desc')
    },
    {
      value: 'balanced',
      label: $t('settings.fan_profile_balanced'),
      description: $t('settings.fan_profile_balanced_desc')
    },
    {
      value: 'performance',
      label: $t('settings.fan_profile_performance'),
      description: $t('settings.fan_profile_performance_desc')
    },
    {
      value: 'manual',
      label: $t('settings.fan_profile_manual'),
      description: $t('settings.fan_profile_manual_desc')
    }
  ]);

  onMount(load);

  async function load() {
    loading = true;
    loadError = '';
    try {
      const [channelsResponse, configResponse] = await Promise.all([
        listFanChannels(),
        getFanConfig()
      ]);
      channels = channelsResponse.channels ?? [];
      isLaptop = !!channelsResponse.is_laptop;
      serverMessage = channelsResponse.message ?? '';
      config = configResponse;
    } catch (e) {
      loadError = e instanceof Error ? e.message : $t('settings.fan_load_failed');
    } finally {
      loading = false;
    }
  }

  async function handleSave() {
    if (!config) return;
    saving = true;
    notice = null;
    try {
      await updateFanConfig(config);
      await load();
      notice = { kind: 'ok', text: $t('settings.fan_saved') };
    } catch (e) {
      notice = {
        kind: 'error',
        text: e instanceof Error ? e.message : $t('settings.fan_save_failed')
      };
    } finally {
      saving = false;
    }
  }

  async function handleRestoreAuto() {
    restoring = true;
    notice = null;
    try {
      await restoreFanAuto();
      await load();
      notice = { kind: 'ok', text: $t('settings.fan_restored_auto') };
    } catch (e) {
      notice = {
        kind: 'error',
        text: e instanceof Error ? e.message : $t('settings.fan_restore_failed')
      };
    } finally {
      restoring = false;
    }
  }

  function formatTemp(millidegrees: number | null): string {
    if (millidegrees === null || millidegrees === undefined) {
      return $t('settings.fan_value_unknown');
    }
    return `${(millidegrees / 1000).toFixed(1)}°C`;
  }

  function formatDuty(pwm: number): string {
    return `${Math.round((pwm / 255) * 100)}%`;
  }

  function modeLabel(mode: number): string {
    switch (mode) {
      case 0:
        return $t('settings.fan_mode_full');
      case 1:
        return $t('settings.fan_mode_manual');
      case 2:
        return $t('settings.fan_mode_auto');
      default:
        return $t('settings.fan_mode_unknown');
    }
  }
</script>

<SettingsCard
  title={$t('settings.fan_title')}
  subtitle={$t('settings.fan_subtitle')}
  class={cn('fan-card', klass)}
  data-testid="fan-card"
>
  {#if loading}
    <p class={settingsHint}>{$t('common.loading')}</p>
  {:else}
    {#if loadError}
      <div class="fan-alert fan-alert-error" role="alert">{loadError}</div>
    {/if}
    {#if notice}
      <div
        class={cn('fan-alert', notice.kind === 'ok' ? 'fan-alert-ok' : 'fan-alert-error')}
        role={notice.kind === 'ok' ? 'status' : 'alert'}
        data-testid="fan-notice"
      >
        {notice.text}
      </div>
    {/if}

    {#if isLaptop}
      <div class="fan-alert fan-alert-info" data-testid="fan-laptop">
        {serverMessage || $t('settings.fan_laptop_disabled')}
      </div>
      <p class={settingsRowHint}>{$t('settings.fan_laptop_hint')}</p>
    {:else if config}
      {#if channels.length === 0}
        <div class="fan-alert fan-alert-info" data-testid="fan-no-channels">
          {$t('settings.fan_no_channels')}
        </div>
        <p class={settingsRowHint}>{$t('settings.fan_no_channels_hint')}</p>
      {:else}
        <fieldset class="fan-fieldset">
          <legend class={settingsRowLabel}>{$t('settings.fan_profile_legend')}</legend>
          <div class="fan-profiles">
            {#each profileOptions as option (option.value)}
              <label
                class="fan-profile"
                class:is-selected={config.profile === option.value}
              >
                <input
                  type="radio"
                  class="fan-radio"
                  bind:group={config.profile}
                  value={option.value}
                />
                <span class="min-w-0">
                  <span class="fan-profile-label">{option.label}</span>
                  <span class="fan-profile-desc">{option.description}</span>
                </span>
              </label>
            {/each}
          </div>
        </fieldset>

        {#if config.profile === 'manual'}
          <div class={settingsRow}>
            <div class="min-w-0">
              <label class={settingsRowLabel} for="fan-manual-duty">
                {$t('settings.fan_manual_speed', { percent: String(config.manual_duty) })}
              </label>
              <p class={settingsRowHint}>{$t('settings.fan_manual_hint')}</p>
            </div>
            <input
              id="fan-manual-duty"
              type="range"
              class="fan-slider"
              min="20"
              max="100"
              step="1"
              bind:value={config.manual_duty}
              data-testid="fan-manual-duty"
            />
          </div>
        {/if}

        <div class={settingsRow}>
          <div class="min-w-0">
            <label class={settingsRowLabel} for="fan-interval">
              {$t('settings.fan_interval')}
            </label>
            <p class={settingsRowHint}>{$t('settings.fan_interval_hint')}</p>
          </div>
          <input
            id="fan-interval"
            type="number"
            class="fan-number"
            min="5"
            max="60"
            bind:value={config.interval_secs}
            data-testid="fan-interval"
          />
        </div>

        <h4 class="fan-subhead">{$t('settings.fan_channels_title')}</h4>
        <ul class="fan-channels" data-testid="fan-channels">
          {#each channels as channel (channel.id)}
            <li class="fan-channel">
              <div class="fan-channel-head">
                <span class="fan-channel-name">{channel.name}</span>
                <span class="fan-channel-mode">{modeLabel(channel.current_mode)}</span>
              </div>
              <dl class="fan-stats">
                <div>
                  <dt>{$t('settings.fan_stat_speed')}</dt>
                  <dd>{formatDuty(channel.current_pwm)}</dd>
                </div>
                <div>
                  <dt>{$t('settings.fan_stat_temp')}</dt>
                  <dd>{formatTemp(channel.current_temp)}</dd>
                </div>
                <div>
                  <dt>{$t('settings.fan_stat_auto')}</dt>
                  <dd>{channel.supports_auto ? $t('common.yes') : $t('common.no')}</dd>
                </div>
              </dl>
            </li>
          {/each}
        </ul>

        <div class="fan-safety">
          <h4 class="fan-subhead">{$t('settings.fan_safety_title')}</h4>
          <ul>
            <li>{$t('settings.fan_safety_floor')}</li>
            <li>{$t('settings.fan_safety_failsafe')}</li>
            <li>{$t('settings.fan_safety_monitoring')}</li>
            <li>{$t('settings.fan_safety_hysteresis')}</li>
          </ul>
        </div>
      {/if}

      <div class="fan-actions">
        <button
          type="button"
          class={settingsBtnPrimary}
          onclick={handleSave}
          disabled={saving || channels.length === 0}
          data-testid="fan-save"
        >
          {saving ? $t('common.saving') : $t('settings.fan_apply')}
        </button>
        {#if channels.length > 0}
          <button
            type="button"
            class={settingsBtn}
            onclick={handleRestoreAuto}
            disabled={restoring}
            data-testid="fan-restore-auto"
          >
            {$t('settings.fan_restore_auto')}
          </button>
        {/if}
        <button type="button" class={settingsBtn} onclick={load} data-testid="fan-refresh">
          {$t('common.refresh')}
        </button>
      </div>
    {/if}
  {/if}
</SettingsCard>

<style>
  .fan-alert {
    border: 1px solid var(--hv-shell-border);
    border-radius: 8px;
    padding: 10px 12px;
    margin-bottom: 12px;
    font-size: 0.8125rem;
    line-height: 1.25rem;
  }

  .fan-alert-error {
    background: var(--hv-error-bg);
    border-color: var(--hv-error);
    color: var(--hv-error);
  }

  .fan-alert-ok {
    background: var(--hv-success-bg);
    border-color: var(--hv-success);
    color: var(--hv-success);
  }

  .fan-alert-info {
    background: var(--hv-shell-border-soft);
    color: var(--hv-shell-text-2);
  }

  .fan-fieldset {
    border: 0;
    margin: 0 0 12px;
    padding: 0;
    display: grid;
    gap: 8px;
  }

  .fan-profiles {
    display: grid;
    gap: 8px;
    margin-top: 6px;
  }

  .fan-profile {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 10px 12px;
    border: 1px solid var(--hv-shell-border);
    border-radius: 8px;
    cursor: pointer;
  }

  /* Revealed on keyboard focus too, not only on hover (WCAG 2.2 AA). */
  .fan-profile:hover,
  .fan-profile:focus-within {
    border-color: var(--hv-primary);
    background: var(--hv-shell-border-soft);
  }

  .fan-profile.is-selected {
    border-color: var(--hv-primary);
    box-shadow: inset 0 0 0 1px var(--hv-primary);
  }

  /* A bare `<input type=radio>` inherits `width: 100%` from base.css, which
     stretched it across the label (design-system pitfall 6). Pin the box. */
  .fan-radio {
    height: 1rem;
    width: 1rem;
    flex: none;
    margin-top: 2px;
    accent-color: var(--hv-primary);
  }

  .fan-profile-label {
    display: block;
    font-size: 0.875rem;
    font-weight: 600;
    color: var(--hv-shell-text);
  }

  .fan-profile-desc {
    display: block;
    font-size: 0.75rem;
    color: var(--hv-shell-text-2);
  }

  .fan-slider {
    width: 220px;
    accent-color: var(--hv-primary);
  }

  .fan-number {
    width: 84px;
    border: 1px solid var(--hv-shell-border-strong);
    border-radius: 6px;
    padding: 4px 8px;
    font-size: 0.8125rem;
    background: var(--hv-shell-surface);
    color: var(--hv-shell-text);
  }

  .fan-subhead {
    margin: 16px 0 8px;
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--hv-settings-title);
  }

  .fan-channels {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 8px;
  }

  .fan-channel {
    border: 1px solid var(--hv-shell-border);
    border-radius: 8px;
    padding: 10px 12px;
  }

  .fan-channel-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin-bottom: 6px;
  }

  .fan-channel-name {
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--hv-shell-text);
  }

  .fan-channel-mode {
    font-size: 0.6875rem;
    padding: 2px 8px;
    border-radius: 999px;
    background: var(--hv-shell-border-soft);
    color: var(--hv-shell-text-2);
  }

  .fan-stats {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 8px;
    margin: 0;
    font-size: 0.75rem;
  }

  @media (max-width: 640px) {
    .fan-stats {
      grid-template-columns: 1fr;
    }
  }

  .fan-stats dt {
    color: var(--hv-shell-text-3);
  }

  .fan-stats dd {
    margin: 0;
    font-weight: 600;
    font-family: var(--hv-font-mono);
    color: var(--hv-shell-text);
  }

  .fan-safety {
    margin-top: 12px;
    padding: 10px 12px;
    border-radius: 8px;
    background: var(--hv-shell-border-soft);
  }

  .fan-safety ul {
    margin: 0;
    padding-left: 18px;
    font-size: 0.75rem;
    color: var(--hv-shell-text-2);
  }

  .fan-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 16px;
  }
</style>
