<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { getRedundancyRecommendations } from '$lib/services/api-phase4';
  import type { LayoutRecommendation, RedundancyLevel } from '$lib/types/phase4';

  export let show = false;
  export let minRedundancy: RedundancyLevel = 'single';

  const dispatch = createEventDispatcher();

  let recommendations: LayoutRecommendation[] = [];
  let loading = true;
  let error = '';
  let selectedLayout: LayoutRecommendation | null = null;
  let totalDisks = 0;
  let availableDisks = 0;

  $: if (show) {
    loadRecommendations();
  }

  async function loadRecommendations() {
    loading = true;
    error = '';

    try {
      const response = await getRedundancyRecommendations(minRedundancy);
      recommendations = response.recommendations;
      totalDisks = response.total_disks;
      availableDisks = response.available_disks;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load recommendations';
    } finally {
      loading = false;
    }
  }

  function close() {
    show = false;
    dispatch('close');
  }

  function selectLayout(layout: LayoutRecommendation) {
    selectedLayout = layout;
  }

  function applyLayout() {
    if (!selectedLayout) return;

    alert(
      'Layout application is not yet implemented.\n\n' +
      'This feature requires integration with the existing pool creation system.\n\n' +
      'Use the details shown to manually create the recommended layout via the Storage screen.'
    );

    close();
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

  function getRiskColor(risk: string): string {
    switch (risk) {
      case 'low': return 'success';
      case 'medium': return 'warning';
      case 'high': return 'error';
      default: return 'secondary';
    }
  }

  function getRedundancyBadge(redundancy: string): string {
    switch (redundancy) {
      case 'none': return '⚠️ No Redundancy';
      case 'single': return '✓ Single Fault Tolerance';
      case 'double': return '✓✓ Double Fault Tolerance';
      default: return redundancy;
    }
  }
</script>

{#if show}
  <div class="modal-overlay" on:click={close}>
    <div class="modal-dialog" on:click|stopPropagation>
      <div class="modal-header">
        <h2>RAID Layout Recommendations</h2>
        <button class="btn-close" on:click={close}>×</button>
      </div>

      <div class="modal-body">
        {#if loading}
          <div class="loading">
            <p>Analyzing disk configuration...</p>
          </div>
        {:else if error}
          <div class="alert alert-error">{error}</div>
        {:else if recommendations.length === 0}
          <div class="empty-state">
            <p>No recommendations available.</p>
            {#if availableDisks === 0}
              <p>All disks are currently in use.</p>
            {:else}
              <p>Not enough available disks to create a layout with {minRedundancy} redundancy.</p>
            {/if}
          </div>
        {:else}
          <div class="disk-summary">
            <div class="summary-item">
              <span class="summary-label">Total Disks:</span>
              <span class="summary-value">{totalDisks}</span>
            </div>
            <div class="summary-item">
              <span class="summary-label">Available:</span>
              <span class="summary-value">{availableDisks}</span>
            </div>
          </div>

          <div class="recommendations-list">
            {#each recommendations as layout}
              <div
                class="recommendation-card"
                class:selected={selectedLayout?.id === layout.id}
                on:click={() => selectLayout(layout)}
              >
                <div class="card-header">
                  <div class="layout-name">
                    <h3>{layout.name}</h3>
                    <span class="layout-level">RAID {layout.level}</span>
                  </div>
                  <div class="score-badge">
                    Score: {layout.score}
                  </div>
                </div>

                <p class="layout-description">{layout.description}</p>

                <div class="layout-stats">
                  <div class="stat">
                    <span class="stat-label">Usable:</span>
                    <span class="stat-value">{formatBytes(layout.usable_bytes)}</span>
                  </div>
                  <div class="stat">
                    <span class="stat-label">Efficiency:</span>
                    <span class="stat-value">{layout.efficiency_pct.toFixed(1)}%</span>
                  </div>
                  <div class="stat">
                    <span class="stat-label">Failures Tolerated:</span>
                    <span class="stat-value">{layout.risk.failure_tolerance}</span>
                  </div>
                </div>

                <div class="badges">
                  <span class="badge badge-redundancy">
                    {getRedundancyBadge(layout.redundancy)}
                  </span>
                  <span class="badge badge-{getRiskColor(layout.risk.data_loss_risk)}">
                    {layout.risk.data_loss_risk.toUpperCase()} Risk
                  </span>
                  <span class="badge badge-secondary">
                    {layout.technology.toUpperCase()}
                  </span>
                </div>

                {#if layout.warnings.length > 0}
                  <div class="warnings">
                    {#each layout.warnings as warning}
                      <p class="warning">⚠️ {warning}</p>
                    {/each}
                  </div>
                {/if}

                {#if selectedLayout?.id === layout.id}
                  <details class="layout-details">
                    <summary>View Details</summary>

                    <div class="detail-section">
                      <h4>Risk Assessment</h4>
                      <p>{layout.risk.explanation}</p>
                      <p class="detail-item">
                        <strong>Rebuild Time:</strong>
                        {layout.risk.rebuild_time.replace('_', ' ')}
                      </p>
                    </div>

                    <div class="detail-section">
                      <h4>Disk Layout</h4>
                      {#each layout.layout.groups as group, i}
                        <div class="disk-group">
                          <strong>Group {i + 1} ({group.role}):</strong>
                          <ul>
                            {#each group.disks as disk}
                              <li>{disk}</li>
                            {/each}
                          </ul>
                        </div>
                      {/each}
                      {#if layout.layout.metadata_bytes > 0}
                        <p class="detail-item">
                          <strong>Metadata/Parity:</strong>
                          {formatBytes(layout.layout.metadata_bytes)}
                        </p>
                      {/if}
                    </div>
                  </details>
                {/if}
              </div>
            {/each}
          </div>
        {/if}
      </div>

      <div class="modal-footer">
        <button class="btn-secondary" on:click={close}>Cancel</button>
        <button
          class="btn-primary"
          on:click={applyLayout}
          disabled={!selectedLayout}
        >
          Apply Selected Layout
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .modal-overlay {
    position: fixed;
    top: 0;
    left: 0;
    right: 0;
    bottom: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
    padding: 20px;
  }

  .modal-dialog {
    background: var(--card-bg);
    border-radius: 12px;
    width: 100%;
    max-width: 900px;
    max-height: 90vh;
    display: flex;
    flex-direction: column;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.3);
  }

  .modal-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 24px;
    border-bottom: 1px solid var(--border-color);
  }

  .modal-header h2 {
    margin: 0;
    font-size: 20px;
    font-weight: 600;
  }

  .btn-close {
    background: none;
    border: none;
    font-size: 28px;
    cursor: pointer;
    color: var(--text-secondary);
    padding: 0;
    width: 32px;
    height: 32px;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .btn-close:hover {
    color: var(--text-primary);
  }

  .modal-body {
    padding: 24px;
    overflow-y: auto;
    flex: 1;
  }

  .modal-footer {
    display: flex;
    justify-content: flex-end;
    gap: 12px;
    padding: 20px 24px;
    border-top: 1px solid var(--border-color);
  }

  .loading {
    text-align: center;
    padding: 48px 24px;
    color: var(--text-secondary);
  }

  .alert {
    padding: 12px 16px;
    border-radius: 8px;
    margin-bottom: 16px;
  }

  .alert-error {
    background: var(--error-bg);
    border: 1px solid var(--error-border);
    color: var(--error-color);
  }

  .empty-state {
    text-align: center;
    padding: 48px 24px;
    color: var(--text-secondary);
  }

  .disk-summary {
    display: flex;
    gap: 24px;
    padding: 16px;
    background: var(--surface-bg);
    border-radius: 8px;
    margin-bottom: 24px;
  }

  .summary-item {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .summary-label {
    font-size: 12px;
    color: var(--text-secondary);
  }

  .summary-value {
    font-size: 20px;
    font-weight: 600;
  }

  .recommendations-list {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .recommendation-card {
    border: 2px solid var(--border-color);
    border-radius: 12px;
    padding: 20px;
    cursor: pointer;
    transition: all 0.2s;
  }

  .recommendation-card:hover {
    border-color: var(--primary-color);
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.1);
  }

  .recommendation-card.selected {
    border-color: var(--primary-color);
    background: var(--primary-bg-light);
  }

  .card-header {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    margin-bottom: 12px;
  }

  .layout-name {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .layout-name h3 {
    margin: 0;
    font-size: 16px;
    font-weight: 600;
  }

  .layout-level {
    font-size: 12px;
    color: var(--text-secondary);
    font-weight: 500;
  }

  .score-badge {
    padding: 6px 12px;
    background: var(--primary-color);
    color: white;
    border-radius: 12px;
    font-size: 13px;
    font-weight: 600;
  }

  .layout-description {
    margin: 0 0 16px 0;
    font-size: 14px;
    color: var(--text-secondary);
  }

  .layout-stats {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 16px;
    margin-bottom: 16px;
  }

  @media (max-width: 640px) {
    .layout-stats {
      grid-template-columns: 1fr;
      gap: 8px;
    }
  }

  .stat {
    display: flex;
    justify-content: space-between;
    font-size: 13px;
  }

  .stat-label {
    color: var(--text-secondary);
  }

  .stat-value {
    font-weight: 600;
  }

  .badges {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-bottom: 12px;
  }

  .badge {
    padding: 4px 10px;
    border-radius: 12px;
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
  }

  .badge-redundancy {
    background: var(--info-bg);
    color: var(--info-color);
  }

  .badge-success {
    background: var(--success-bg);
    color: var(--success-color);
  }

  .badge-warning {
    background: var(--warning-bg);
    color: var(--warning-color);
  }

  .badge-error {
    background: var(--error-bg);
    color: var(--error-color);
  }

  .badge-secondary {
    background: var(--secondary-bg);
    color: var(--text-secondary);
  }

  .warnings {
    margin-top: 12px;
  }

  .warning {
    font-size: 13px;
    color: var(--warning-color);
    margin: 4px 0;
  }

  .layout-details {
    margin-top: 16px;
    padding-top: 16px;
    border-top: 1px solid var(--border-color);
  }

  .layout-details summary {
    cursor: pointer;
    font-weight: 600;
    font-size: 14px;
    margin-bottom: 12px;
  }

  .detail-section {
    margin-bottom: 16px;
  }

  .detail-section h4 {
    margin: 0 0 8px 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-secondary);
  }

  .detail-section p {
    margin: 4px 0;
    font-size: 13px;
  }

  .detail-item {
    margin: 8px 0;
  }

  .disk-group {
    margin: 12px 0;
    padding: 12px;
    background: var(--surface-bg);
    border-radius: 4px;
  }

  .disk-group ul {
    margin: 8px 0 0 20px;
    padding: 0;
  }

  .disk-group li {
    font-size: 12px;
    font-family: monospace;
    margin: 4px 0;
  }

  button {
    padding: 10px 20px;
    border: none;
    border-radius: 6px;
    font-size: 14px;
    font-weight: 500;
    cursor: pointer;
    transition: background-color 0.2s;
  }

  .btn-primary {
    background: var(--primary-color);
    color: white;
  }

  .btn-primary:hover {
    background: var(--primary-hover);
  }

  .btn-primary:disabled {
    background: var(--disabled-color);
    cursor: not-allowed;
  }

  .btn-secondary {
    background: var(--secondary-color);
    color: var(--text-primary);
  }

  .btn-secondary:hover {
    background: var(--secondary-hover);
  }
</style>
