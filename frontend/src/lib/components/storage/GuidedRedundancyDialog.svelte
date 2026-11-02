<!-- ========================================================================
     File          : GuidedRedundancyDialog.svelte
     Author        : Nicolas Le Roux
     Created       : 02/11/2026
     Last modified : 02/11/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       The RAID layout suggestion dialog opened from the Storage screen: one
       card per layout the attached disks could become, with usable capacity,
       efficiency, tolerated failures and the risk assessment.

       Rewritten on the shared `Modal` (which traps focus and restores it) and
       the design tokens. The previous version was a hand-rolled overlay with
       hardcoded English strings, a native `alert()` for the "not implemented"
       answer, CSS variables that do not exist in the token set, and a
       click-only card selection (no keyboard path).

       The Apply button is honest: creating the array from a recommendation is
       not implemented, and `POST …/apply/{id}` answers `501`. The dialog says
       so up front instead of letting the click fail.

     Dependencies:
       - svelte (>= 5), $lib/services/api-phase4, $lib/stores/i18n,
         $lib/components/theme/{Modal,Button}

     Usage:
       Opened from routes/Storage.svelte. See docs/guides/guided-redundancy.md.
     ======================================================================== -->
<script lang="ts">
  import { t } from '$lib/stores/i18n';
  import { getRedundancyRecommendations } from '$lib/services/api-phase4';
  import type { LayoutRecommendation, RedundancyLevel } from '$lib/types/phase4';
  import Modal from '$lib/components/theme/Modal.svelte';
  import { Button } from '$lib/components/theme';

  let {
    open = false,
    minRedundancy = 'single' as RedundancyLevel,
    onclose
  }: {
    open?: boolean;
    minRedundancy?: RedundancyLevel;
    onclose?: () => void;
  } = $props();

  let recommendations = $state<LayoutRecommendation[]>([]);
  let loading = $state(true);
  let error = $state('');
  let selectedId = $state<string | null>(null);
  let totalDisks = $state(0);
  let availableDisks = $state(0);

  // Load whenever the dialog is opened, not once at mount: the disk inventory
  // can change between two openings.
  $effect(() => {
    if (open) {
      void loadRecommendations();
    }
  });

  async function loadRecommendations() {
    loading = true;
    error = '';
    selectedId = null;
    try {
      const response = await getRedundancyRecommendations(minRedundancy);
      recommendations = response.recommendations ?? [];
      totalDisks = response.total_disks ?? 0;
      availableDisks = response.available_disks ?? 0;
    } catch (e) {
      error = e instanceof Error ? e.message : $t('settings.guided_redundancy_load_failed');
    } finally {
      loading = false;
    }
  }

  function close() {
    onclose?.();
  }

  function formatBytes(bytes: number): string {
    const units = ['B', 'KB', 'MB', 'GB', 'TB'];
    let size = bytes;
    let unitIndex = 0;
    while (size >= 1024 && unitIndex < units.length - 1) {
      size /= 1024;
      unitIndex++;
    }
    return `${size.toFixed(1)} ${units[unitIndex]}`;
  }

  function riskTone(risk: string): string {
    switch (risk) {
      case 'low':
        return 'gr-badge-ok';
      case 'medium':
        return 'gr-badge-warn';
      case 'high':
        return 'gr-badge-error';
      default:
        return 'gr-badge';
    }
  }

  function redundancyLabel(redundancy: string): string {
    switch (redundancy) {
      case 'none':
        return $t('settings.guided_redundancy_failures') + ' : 0';
      case 'single':
        return $t('settings.guided_redundancy_failures') + ' : 1';
      case 'double':
        return $t('settings.guided_redundancy_failures') + ' : 2';
      default:
        return redundancy;
    }
  }
</script>

<Modal
  open={open}
  title={$t('settings.guided_redundancy_title')}
  size="lg"
  onclose={close}
>
  <div class="gr-body">
    {#if loading}
      <p class="gr-hint">{$t('settings.guided_redundancy_analyzing')}</p>
    {:else if error}
      <div class="gr-alert" role="alert">{error}</div>
    {:else if recommendations.length === 0}
      <p class="gr-hint" data-testid="gr-empty">
        {$t('settings.guided_redundancy_empty')}
      </p>
      {#if availableDisks === 0}
        <p class="gr-hint">{$t('settings.guided_redundancy_all_in_use')}</p>
      {/if}
    {:else}
      <dl class="gr-summary">
        <div>
          <dt>{$t('settings.guided_redundancy_total_disks')}</dt>
          <dd>{totalDisks}</dd>
        </div>
        <div>
          <dt>{$t('settings.guided_redundancy_available')}</dt>
          <dd>{availableDisks}</dd>
        </div>
      </dl>

      <ul class="gr-list" role="radiogroup" aria-label={$t('settings.guided_redundancy_title')}>
        {#each recommendations as layout (layout.id)}
          <li>
            <!-- A real radio inside the card, so the selection is reachable
                 by keyboard. The previous version only had an `on:click` on
                 the container, which mouse and touch could use but a keyboard
                 could not. -->
            <label class="gr-card" class:is-selected={selectedId === layout.id}>
              <input
                type="radio"
                class="gr-radio"
                name="gr-layout"
                value={layout.id}
                bind:group={selectedId}
                data-testid={`gr-select-${layout.id}`}
              />
              <span class="min-w-0">
                <span class="gr-card-head">
                  <span class="gr-name">{layout.name}</span>
                  <span class="gr-level">RAID {layout.level}</span>
                </span>
                <span class="gr-desc">{layout.description}</span>

                <span class="gr-stats">
                  <span>
                    <span class="gr-stat-label">{$t('settings.guided_redundancy_usable')}</span>
                    <span class="gr-stat-value">{formatBytes(layout.usable_bytes)}</span>
                  </span>
                  <span>
                    <span class="gr-stat-label">{$t('settings.guided_redundancy_efficiency')}</span>
                    <span class="gr-stat-value">{layout.efficiency_pct.toFixed(1)}%</span>
                  </span>
                  <span>
                    <span class="gr-stat-label">{$t('settings.guided_redundancy_failures')}</span>
                    <span class="gr-stat-value">{layout.risk.failure_tolerance}</span>
                  </span>
                </span>

                <span class="gr-badges">
                  <span class="gr-badge">{redundancyLabel(layout.redundancy)}</span>
                  <span class={riskTone(layout.risk.data_loss_risk)}>
                    {$t('settings.guided_redundancy_risk')} : {layout.risk.data_loss_risk}
                  </span>
                  <span class="gr-badge">{layout.technology}</span>
                </span>

                {#if layout.warnings.length > 0}
                  <span class="gr-warnings">
                    {#each layout.warnings as warning}
                      <span class="gr-warning">{warning}</span>
                    {/each}
                  </span>
                {/if}
              </span>
            </label>

            {#if selectedId === layout.id}
              <div class="gr-details">
                <h4 class="gr-detail-title">{$t('settings.guided_redundancy_risk')}</h4>
                <p class="gr-desc">{layout.risk.explanation}</p>
                <p class="gr-desc">
                  {$t('settings.guided_redundancy_rebuild')}: {layout.risk.rebuild_time.replace(
                    '_',
                    ' '
                  )}
                </p>

                <h4 class="gr-detail-title">{$t('settings.guided_redundancy_layout')}</h4>
                {#each layout.layout.groups as group, i (group.role + i)}
                  <p class="gr-detail-line">
                    <strong>{group.role}</strong> — {group.disks.join(', ')}
                  </p>
                {/each}
                {#if layout.layout.metadata_bytes > 0}
                  <p class="gr-detail-line">
                    <strong>{$t('settings.guided_redundancy_parity')}</strong>
                    — {formatBytes(layout.layout.metadata_bytes)}
                  </p>
                {/if}
              </div>
            {/if}
          </li>
        {/each}
      </ul>

      <p class="gr-hint" data-testid="gr-apply-unavailable">
        {$t('settings.guided_redundancy.apply_unavailable')}
      </p>
    {/if}

    <div class="gr-actions">
      <Button variant="ghost" onclick={close}>{$t('common.close')}</Button>
      <Button
        variant="primary"
        onclick={close}
        disabled={!selectedId || recommendations.length === 0}
        data-testid="gr-apply"
      >
        {$t('settings.guided_redundancy_apply')}
      </Button>
    </div>
  </div>
</Modal>

<style>
  .gr-body {
    display: flex;
    flex-direction: column;
    gap: 12px;
    color: var(--hv-shell-text);
  }

  .gr-hint {
    margin: 0;
    font-size: 0.75rem;
    color: var(--hv-shell-text-2);
  }

  .gr-alert {
    border: 1px solid var(--hv-error);
    background: var(--hv-error-bg);
    color: var(--hv-error);
    border-radius: 8px;
    padding: 10px 12px;
    font-size: 0.8125rem;
  }

  .gr-summary {
    display: flex;
    gap: 20px;
    margin: 0;
    font-size: 0.8125rem;
  }

  .gr-summary dt {
    color: var(--hv-shell-text-3);
  }

  .gr-summary dd {
    margin: 0;
    font-weight: 600;
    font-family: var(--hv-font-mono);
  }

  .gr-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 10px;
    max-height: 55vh;
    overflow-y: auto;
  }

  .gr-card {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    border: 1px solid var(--hv-shell-border);
    border-radius: 10px;
    padding: 12px;
    cursor: pointer;
  }

  /* Revealed on keyboard focus too, not only on hover (WCAG 2.2 AA). */
  .gr-card:hover,
  .gr-card:focus-within {
    border-color: var(--hv-primary);
    background: var(--hv-shell-border-soft);
  }

  .gr-card.is-selected {
    border-color: var(--hv-primary);
    box-shadow: inset 0 0 0 1px var(--hv-primary);
  }

  /* A bare radio inherits `width: 100%` from base.css (design-system
     pitfall 6). */
  .gr-radio {
    height: 1rem;
    width: 1rem;
    flex: none;
    margin-top: 2px;
    accent-color: var(--hv-primary);
  }

  .gr-card-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
  }

  .gr-name {
    font-size: 0.875rem;
    font-weight: 600;
    color: var(--hv-shell-text);
  }

  .gr-level {
    font-size: 0.6875rem;
    color: var(--hv-shell-text-3);
    font-family: var(--hv-font-mono);
  }

  .gr-desc {
    display: block;
    margin: 4px 0 0;
    font-size: 0.75rem;
    color: var(--hv-shell-text-2);
  }

  .gr-stats {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 8px;
    margin-top: 8px;
  }

  .gr-stat-label {
    display: block;
    font-size: 0.6875rem;
    color: var(--hv-shell-text-3);
  }

  .gr-stat-value {
    display: block;
    font-size: 0.75rem;
    font-weight: 600;
    font-family: var(--hv-font-mono);
    color: var(--hv-shell-text);
  }

  .gr-badges {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 8px;
  }

  .gr-badge,
  .gr-badge-ok,
  .gr-badge-warn,
  .gr-badge-error {
    font-size: 0.6875rem;
    padding: 2px 8px;
    border-radius: 999px;
    background: var(--hv-shell-border-soft);
    color: var(--hv-shell-text-2);
  }

  .gr-badge-ok {
    background: var(--hv-success-bg);
    color: var(--hv-success);
  }

  .gr-badge-warn {
    background: var(--hv-shell-border-soft);
    color: var(--hv-shell-text);
  }

  .gr-badge-error {
    background: var(--hv-error-bg);
    color: var(--hv-error);
  }

  .gr-warnings {
    display: block;
    margin-top: 8px;
  }

  .gr-warning {
    display: block;
    font-size: 0.75rem;
    color: var(--hv-error);
  }

  .gr-details {
    border: 1px solid var(--hv-shell-border);
    border-top: 0;
    border-radius: 0 0 10px 10px;
    padding: 10px 12px;
    background: var(--hv-shell-border-soft);
  }

  .gr-detail-title {
    margin: 8px 0 4px;
    font-size: 0.75rem;
    font-weight: 600;
    color: var(--hv-settings-title);
  }

  .gr-detail-line {
    margin: 2px 0;
    font-size: 0.75rem;
    color: var(--hv-shell-text-2);
    overflow-wrap: anywhere;
  }

  .gr-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 4px;
  }
</style>
