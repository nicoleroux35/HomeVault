<script lang="ts">
  import { onMount } from 'svelte';
  import {
    listFanChannels,
    getFanConfig,
    updateFanConfig,
    restoreFanAuto,
  } from '$lib/services/api-phase4';
  import type { FanConfig, FanChannel, FanProfile } from '$lib/types/phase4';

  let config: FanConfig | null = null;
  let channels: FanChannel[] = [];
  let isLaptop = false;
  let loading = true;
  let saving = false;
  let error = '';
  let message = '';

  const profileOptions: { value: FanProfile; label: string; description: string }[] = [
    { value: 'auto', label: 'Auto (Firmware)', description: 'Let firmware control fans' },
    { value: 'quiet', label: 'Quiet', description: 'Prioritize silence' },
    { value: 'balanced', label: 'Balanced', description: 'Balance noise and cooling' },
    { value: 'performance', label: 'Performance', description: 'Prioritize cooling' },
    { value: 'manual', label: 'Manual', description: 'Fixed speed' },
  ];

  onMount(async () => {
    await loadData();
  });

  async function loadData() {
    loading = true;
    error = '';
    message = '';

    try {
      const [channelsResponse, configResponse] = await Promise.all([
        listFanChannels(),
        getFanConfig(),
      ]);

      channels = channelsResponse.channels;
      isLaptop = channelsResponse.is_laptop;
      message = channelsResponse.message || '';
      config = configResponse;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load fan control data';
    } finally {
      loading = false;
    }
  }

  async function handleSave() {
    if (!config) return;

    saving = true;
    error = '';

    try {
      await updateFanConfig(config);
      await loadData();
      alert('Fan control configuration saved successfully');
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to save configuration';
    } finally {
      saving = false;
    }
  }

  async function handleRestoreAuto() {
    if (!confirm('Restore all fans to automatic (firmware) control?')) return;

    try {
      await restoreFanAuto();
      await loadData();
      alert('Fans restored to automatic control');
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to restore automatic control';
    }
  }

  function formatTemp(tempMillidegrees: number | null): string {
    if (tempMillidegrees === null) return 'N/A';
    return `${(tempMillidegrees / 1000).toFixed(1)}°C`;
  }

  function formatDuty(pwm: number): string {
    const percent = Math.round((pwm / 255) * 100);
    return `${percent}%`;
  }

  function getModeLabel(mode: number): string {
    switch (mode) {
      case 0: return 'Full Speed';
      case 1: return 'Manual';
      case 2: return 'Auto';
      default: return 'Unknown';
    }
  }
</script>

<div class="settings-card">
  <h3>Fan Control</h3>

  {#if isLaptop}
    <div class="warning">
      <p>⚠️ {message || 'Fan control is disabled on laptop systems.'}</p>
      <p>Laptop fans are managed by the embedded controller firmware.</p>
    </div>
  {:else if loading}
    <p>Loading...</p>
  {:else if config}
    <div class="config-form">
      {#if channels.length === 0}
        <div class="info">
          <p>ℹ️ No controllable fan channels detected.</p>
          <p>This system may not have software-controllable fans, or you may need additional permissions.</p>
        </div>
      {:else}
        <!-- Profile Selection -->
        <div class="form-section">
          <h4>Fan Profile</h4>

          <div class="profile-selector">
            {#each profileOptions as option}
              <label class="profile-option" class:selected={config.profile === option.value}>
                <input
                  type="radio"
                  bind:group={config.profile}
                  value={option.value}
                />
                <div class="profile-info">
                  <span class="profile-label">{option.label}</span>
                  <span class="profile-description">{option.description}</span>
                </div>
              </label>
            {/each}
          </div>

          {#if config.profile === 'manual'}
            <div class="form-group">
              <label>
                Manual Fan Speed: {config.manual_duty}%
                <input
                  type="range"
                  bind:value={config.manual_duty}
                  min="20"
                  max="100"
                  class="slider"
                />
              </label>
              <p class="hint">Minimum: 20% (safety limit)</p>
            </div>
          {/if}
        </div>

        <!-- Fan Channels -->
        <div class="form-section">
          <h4>Detected Fan Channels</h4>

          <div class="channels-list">
            {#each channels as channel}
              <div class="channel-item">
                <div class="channel-header">
                  <div class="channel-name">
                    <strong>{channel.name}</strong>
                    <span class="channel-id">{channel.id}</span>
                  </div>
                  <div class="channel-mode">
                    {getModeLabel(channel.current_mode)}
                  </div>
                </div>

                <div class="channel-stats">
                  <div class="stat">
                    <span class="stat-label">Speed:</span>
                    <span class="stat-value">{formatDuty(channel.current_pwm)}</span>
                  </div>
                  <div class="stat">
                    <span class="stat-label">Temperature:</span>
                    <span class="stat-value">{formatTemp(channel.current_temp)}</span>
                  </div>
                  <div class="stat">
                    <span class="stat-label">Auto Support:</span>
                    <span class="stat-value">{channel.supports_auto ? 'Yes' : 'No'}</span>
                  </div>
                </div>

                <!-- Per-channel override (optional feature) -->
                <!-- Uncomment if you want per-channel control
                <details class="channel-override">
                  <summary>Advanced: Per-channel settings</summary>
                  <p class="hint">Override global profile for this channel</p>
                </details>
                -->
              </div>
            {/each}
          </div>
        </div>

        <!-- Control Interval -->
        <div class="form-section">
          <h4>Control Settings</h4>

          <div class="form-group">
            <label>
              Update Interval (seconds)
              <input
                type="number"
                bind:value={config.interval_secs}
                min="5"
                max="60"
              />
            </label>
            <p class="hint">How often to check temperatures and adjust fan speeds</p>
          </div>
        </div>

        <!-- Safety Information -->
        <div class="info-box">
          <h5>🛡️ Safety Features</h5>
          <ul>
            <li>Minimum fan speed: 20% (hardware safety limit)</li>
            <li>Automatic failsafe: fans restore to auto mode on errors</li>
            <li>Temperature monitoring: continuous monitoring</li>
            <li>Hysteresis: prevents rapid speed changes</li>
          </ul>
        </div>
      {/if}

      <!-- Actions -->
      <div class="card-actions">
        {#if error}
          <p class="error">{error}</p>
        {/if}

        <div class="action-buttons">
          <button
            class="btn-primary"
            on:click={handleSave}
            disabled={saving || channels.length === 0}
          >
            {saving ? 'Saving...' : 'Apply Configuration'}
          </button>

          {#if channels.length > 0}
            <button
              class="btn-secondary"
              on:click={handleRestoreAuto}
            >
              Restore Auto Control
            </button>
          {/if}

          <button
            class="btn-secondary"
            on:click={loadData}
          >
            Refresh
          </button>
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  .settings-card {
    background: var(--card-bg);
    border-radius: 8px;
    padding: 24px;
    margin-bottom: 16px;
  }

  h3 {
    margin: 0 0 16px 0;
    font-size: 18px;
    font-weight: 600;
  }

  h4 {
    margin: 0 0 12px 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--text-secondary);
  }

  h5 {
    margin: 0 0 8px 0;
    font-size: 13px;
    font-weight: 600;
  }

  .warning,
  .info {
    background: var(--warning-bg);
    border: 1px solid var(--warning-border);
    border-radius: 4px;
    padding: 12px;
    margin-bottom: 16px;
  }

  .info {
    background: var(--info-bg);
    border-color: var(--info-border);
  }

  .form-section {
    margin-bottom: 24px;
    padding-bottom: 24px;
    border-bottom: 1px solid var(--border-color);
  }

  .form-section:last-of-type {
    border-bottom: none;
  }

  .form-group {
    margin-bottom: 16px;
  }

  label {
    display: block;
    margin-bottom: 4px;
    font-size: 14px;
    font-weight: 500;
  }

  input[type="number"] {
    width: 100%;
    padding: 8px 12px;
    border: 1px solid var(--border-color);
    border-radius: 4px;
    font-size: 14px;
  }

  .slider {
    width: 100%;
    margin-top: 8px;
  }

  .hint {
    font-size: 12px;
    color: var(--text-secondary);
    margin-top: 4px;
  }

  .profile-selector {
    display: grid;
    gap: 8px;
  }

  .profile-option {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px;
    border: 2px solid var(--border-color);
    border-radius: 8px;
    cursor: pointer;
    transition: all 0.2s;
  }

  .profile-option:hover {
    border-color: var(--primary-color);
    background: var(--surface-bg);
  }

  .profile-option.selected {
    border-color: var(--primary-color);
    background: var(--primary-bg-light);
  }

  .profile-option input[type="radio"] {
    flex-shrink: 0;
  }

  .profile-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .profile-label {
    font-weight: 600;
    font-size: 14px;
  }

  .profile-description {
    font-size: 12px;
    color: var(--text-secondary);
  }

  .channels-list {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .channel-item {
    border: 1px solid var(--border-color);
    border-radius: 8px;
    padding: 16px;
  }

  .channel-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 12px;
  }

  .channel-name {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .channel-name strong {
    font-size: 14px;
  }

  .channel-id {
    font-size: 12px;
    color: var(--text-secondary);
    font-family: monospace;
  }

  .channel-mode {
    padding: 4px 12px;
    background: var(--secondary-bg);
    border-radius: 12px;
    font-size: 12px;
    font-weight: 500;
  }

  .channel-stats {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 16px;
  }

  @media (max-width: 768px) {
    .channel-stats {
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
    font-family: monospace;
  }

  .info-box {
    background: var(--surface-bg);
    border-radius: 8px;
    padding: 16px;
    margin-top: 24px;
  }

  .info-box ul {
    margin: 8px 0 0 20px;
    padding: 0;
  }

  .info-box li {
    font-size: 13px;
    margin-bottom: 4px;
    color: var(--text-secondary);
  }

  .card-actions {
    margin-top: 24px;
    padding-top: 24px;
    border-top: 1px solid var(--border-color);
  }

  .action-buttons {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }

  button {
    padding: 8px 16px;
    border: none;
    border-radius: 4px;
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

  .error {
    color: var(--error-color);
    font-size: 14px;
    margin-bottom: 12px;
  }
</style>
