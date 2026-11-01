<!-- ========================================================================
     File          : ReachabilityCard.svelte
     Author        : Nicolas Le Roux
     Created       : 29/10/2026
     Last modified : 29/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Read-only Settings card for the reachability advisor: the ordered list
       of conditions a browser outside the house depends on, each with its
       state, what is wrong, and the fix. Nothing here writes.

     Dependencies:
       - svelte (>= 5), $lib/services/api, $lib/stores/i18n,
         ./SettingsCard, ./vocabulary

     Usage:
       Embedded in routes/Settings.svelte's `security` category. See
       docs/guides/reachability.md.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { t } from '$lib/stores/i18n';
  import SettingsCard from './SettingsCard.svelte';
  import { cn } from '$lib/utils.js';
  import {
    settingsRowLabel,
    settingsHint,
    settingsBtn,
    settingsBtnGhost
  } from './vocabulary';

  let { class: klass = '' }: { class?: string } = $props();

  interface Check {
    id: string;
    state: 'ok' | 'warn' | 'fail';
    detail: string;
    fix: string | null;
  }

  let loading = $state(true);
  let error = $state('');
  let checks = $state<Check[]>([]);
  let overall = $state<'ok' | 'warn' | 'fail'>('ok');
  let counts = $state({ ok: 0, warn: 0, fail: 0 });

  async function load() {
    loading = true;
    error = '';
    const res = await api.getReachability();
    if (res.success && res.data) {
      const d = res.data as any;
      checks = Array.isArray(d.checks) ? d.checks : [];
      overall = d.state ?? 'ok';
      counts = d.counts ?? { ok: 0, warn: 0, fail: 0 };
    } else {
      error = res.message || $t('settings.reachability_load_error');
    }
    loading = false;
  }

  onMount(load);

  function itemClass(state: string): string {
    if (state === 'ok') return 'reach-item reach-item-ok';
    if (state === 'warn') return 'reach-item reach-item-warn';
    return 'reach-item reach-item-fail';
  }

  function summaryClass(state: string): string {
    if (state === 'ok') return 'reach-summary reach-summary-ok';
    if (state === 'warn') return 'reach-summary reach-summary-warn';
    return 'reach-summary reach-summary-fail';
  }
</script>

<SettingsCard
  title={$t('settings.reachability_title')}
  subtitle={$t('settings.reachability_subtitle')}
  class={klass}
>
  {#if loading}
    <p class={settingsHint}>{$t('common.loading')}</p>
  {:else}
    {#if error}
      <div class="reach-error" role="alert">{error}</div>
    {:else}
      <div class={summaryClass(overall)} data-testid="reachability-summary">
        <strong>{$t('settings.reachability_state')}</strong>
        <span>{overall}</span>
        <span class="reach-counts" data-testid="reachability-counts">
          {counts.ok} ok · {counts.warn} warn · {counts.fail} fail
        </span>
      </div>

      <ul class="reach-list" data-testid="reachability-list">
        {#each checks as check (check.id)}
          <li class={itemClass(check.state)} data-testid="reachability-check" data-check-id={check.id} data-state={check.state}>
            <span class="reach-dot" aria-hidden="true"></span>
            <div class="min-w-0">
              <span class={settingsRowLabel}>{$t('settings.reachability_check_' + check.id.replace(/\./g, '_'))}</span>
              <p class={settingsHint}>{check.detail}</p>
              {#if check.fix}
                <p class="reach-fix">{$t('settings.reachability_fix')} {check.fix}</p>
              {/if}
            </div>
          </li>
        {/each}
      </ul>
    {/if}

    <div class="reach-actions">
      <button class={cn(settingsBtn, settingsBtnGhost)} onclick={load} disabled={loading} data-testid="reachability-refresh">
        {$t('common.refresh')}
      </button>
    </div>
  {/if}
</SettingsCard>

<style>
  .reach-summary {
    display: flex;
    align-items: baseline;
    flex-wrap: wrap;
    gap: 8px;
    margin: 0 0 12px;
    padding: var(--hv-space-2-5) var(--hv-space-3);
    border-radius: var(--hv-radius-lg);
    font-size: var(--hv-font-size-sm);
    background: var(--hv-settings-tile);
    border: 1.45px solid var(--hv-settings-border);
    color: var(--hv-settings-row);
  }

  .reach-summary-ok {
    background: var(--hv-success-bg);
    border-color: var(--hv-success);
    color: var(--hv-success-text);
  }

  .reach-summary-warn {
    background: var(--hv-warning-light);
    border-color: var(--hv-warning);
    color: var(--hv-warning);
  }

  .reach-summary-fail {
    background: var(--hv-error-light);
    border-color: var(--hv-error);
    color: var(--hv-error);
  }

  .reach-counts {
    margin-left: auto;
    font-family: var(--hv-font-mono);
    font-size: var(--hv-font-size-xs);
  }

  .reach-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
  }

  .reach-item {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 10px 0;
    border-bottom: 1.45px solid var(--hv-settings-hairline);
  }

  .reach-item:last-child {
    border-bottom: none;
  }

  .reach-dot {
    flex: none;
    width: 10px;
    height: 10px;
    margin-top: 7px;
    border-radius: 50%;
    background: var(--hv-settings-label);
  }

  .reach-item-ok .reach-dot {
    background: var(--hv-success);
  }

  .reach-item-warn .reach-dot {
    background: var(--hv-warning);
  }

  .reach-item-fail .reach-dot {
    background: var(--hv-error);
  }

  .reach-fix {
    margin: 4px 0 0;
    font-size: var(--hv-font-size-xs);
    color: var(--hv-settings-row);
  }

  .reach-error {
    margin: 0 0 12px;
    padding: var(--hv-space-2-5) var(--hv-space-3);
    border-radius: var(--hv-radius-lg);
    font-size: var(--hv-font-size-sm);
    background: var(--hv-error-light);
    color: var(--hv-error);
  }

  .reach-actions {
    display: flex;
    gap: 10px;
    padding: 12px 0;
  }
</style>
