<script lang="ts">
  import { onMount } from 'svelte';
  import {
    getDhcpServerConfig,
    updateDhcpServerConfig,
    startDhcpServer,
    stopDhcpServer,
    getDhcpLeases,
  } from '$lib/services/api-phase4';
  import type { DhcpServerConfig, DhcpLease } from '$lib/types/phase4';

  let config: DhcpServerConfig | null = null;
  let leases: DhcpLease[] = [];
  let loading = true;
  let saving = false;
  let error = '';
  let available = false;
  let running = false;

  let showConfirmDialog = false;
  let newReservationMac = '';
  let newReservationIp = '';

  onMount(async () => {
    await loadData();
  });

  async function loadData() {
    loading = true;
    error = '';

    try {
      const response = await getDhcpServerConfig();
      config = response.config;
      available = response.available;
      running = response.state.running;

      if (running) {
        leases = await getDhcpLeases();
      }
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load DHCP server config';
    } finally {
      loading = false;
    }
  }

  async function handleSave() {
    if (!config) return;

    // Require confirmation for enabling
    if ((config.dhcp_enabled || config.tftp_enabled) && !config.confirmed) {
      showConfirmDialog = true;
      return;
    }

    saving = true;
    error = '';

    try {
      await updateDhcpServerConfig(config);
      await loadData();
      alert('DHCP server configuration saved successfully');
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to save configuration';
    } finally {
      saving = false;
    }
  }

  function confirmAndSave() {
    if (!config) return;
    config.confirmed = true;
    showConfirmDialog = false;
    handleSave();
  }

  async function handleStart() {
    try {
      await startDhcpServer();
      await loadData();
      alert('DHCP server started successfully');
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to start DHCP server';
    }
  }

  async function handleStop() {
    try {
      await stopDhcpServer();
      await loadData();
      alert('DHCP server stopped successfully');
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to stop DHCP server';
    }
  }

  function addReservation() {
    if (!config || !newReservationMac || !newReservationIp) {
      alert('Please enter both MAC address and IP address');
      return;
    }

    config.reservations = {
      ...config.reservations,
      [newReservationMac]: newReservationIp,
    };

    newReservationMac = '';
    newReservationIp = '';
  }

  function removeReservation(mac: string) {
    if (!config) return;
    const { [mac]: removed, ...rest } = config.reservations;
    config.reservations = rest;
  }

  function formatExpiry(timestamp: number): string {
    if (timestamp === 0) return 'Expired';
    const date = new Date(timestamp * 1000);
    return date.toLocaleString();
  }
</script>

<div class="settings-card">
  <h3>DHCP/TFTP Server</h3>

  {#if !available}
    <div class="warning">
      <p>⚠️ dnsmasq is not available on this system.</p>
      <p>Install dnsmasq package to enable this feature.</p>
    </div>
  {/if}

  {#if loading}
    <p>Loading...</p>
  {:else if config}
    <div class="config-form">
      <!-- Enable Toggles -->
      <div class="toggle-section">
        <label class="toggle-label">
          <input type="checkbox" bind:checked={config.dhcp_enabled} />
          <span>Enable DHCP Server</span>
        </label>

        <label class="toggle-label">
          <input type="checkbox" bind:checked={config.tftp_enabled} />
          <span>Enable TFTP Server</span>
        </label>
      </div>

      {#if config.dhcp_enabled || config.tftp_enabled}
        <div class="warning-banner">
          ⚠️ Running a DHCP server can conflict with existing network infrastructure.
          Only enable this on isolated networks or if you know what you're doing.
        </div>

        <!-- Network Configuration -->
        <div class="form-section">
          <h4>Network Configuration</h4>

          <div class="form-row">
            <div class="form-group">
              <label>
                Interface
                <input
                  type="text"
                  bind:value={config.interface}
                  placeholder="eth0"
                />
              </label>
            </div>
          </div>
        </div>

        <!-- DHCP Configuration -->
        {#if config.dhcp_enabled}
          <div class="form-section">
            <h4>DHCP Configuration</h4>

            <div class="form-row">
              <div class="form-group">
                <label>
                  Range Start
                  <input
                    type="text"
                    bind:value={config.dhcp_range_start}
                    placeholder="192.168.1.100"
                  />
                </label>
              </div>

              <div class="form-group">
                <label>
                  Range End
                  <input
                    type="text"
                    bind:value={config.dhcp_range_end}
                    placeholder="192.168.1.200"
                  />
                </label>
              </div>
            </div>

            <div class="form-row">
              <div class="form-group">
                <label>
                  Netmask
                  <input
                    type="text"
                    bind:value={config.netmask}
                    placeholder="255.255.255.0"
                  />
                </label>
              </div>

              <div class="form-group">
                <label>
                  Gateway (optional)
                  <input
                    type="text"
                    bind:value={config.gateway}
                    placeholder="192.168.1.1"
                  />
                </label>
              </div>
            </div>

            <div class="form-group">
              <label>
                Lease Time (seconds)
                <input
                  type="number"
                  bind:value={config.lease_time_secs}
                  min="60"
                  max="2592000"
                />
              </label>
              <p class="hint">Default: 86400 (24 hours)</p>
            </div>

            <!-- Reservations -->
            <div class="reservations-section">
              <h5>MAC Reservations</h5>

              {#if Object.keys(config.reservations).length === 0}
                <p class="hint">No reservations configured.</p>
              {:else}
                <table class="reservations-table">
                  <thead>
                    <tr>
                      <th>MAC Address</th>
                      <th>IP Address</th>
                      <th>Actions</th>
                    </tr>
                  </thead>
                  <tbody>
                    {#each Object.entries(config.reservations) as [mac, ip]}
                      <tr>
                        <td>{mac}</td>
                        <td>{ip}</td>
                        <td>
                          <button
                            class="btn-danger btn-sm"
                            on:click={() => removeReservation(mac)}
                          >
                            Remove
                          </button>
                        </td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              {/if}

              <div class="add-reservation">
                <input
                  type="text"
                  bind:value={newReservationMac}
                  placeholder="aa:bb:cc:dd:ee:ff"
                  class="input-sm"
                />
                <input
                  type="text"
                  bind:value={newReservationIp}
                  placeholder="192.168.1.50"
                  class="input-sm"
                />
                <button class="btn-secondary btn-sm" on:click={addReservation}>
                  Add
                </button>
              </div>
            </div>
          </div>
        {/if}

        <!-- TFTP Configuration -->
        {#if config.tftp_enabled}
          <div class="form-section">
            <h4>TFTP Configuration</h4>

            <div class="form-group">
              <label>
                TFTP Root Directory
                <input
                  type="text"
                  bind:value={config.tftp_root}
                  placeholder="/srv/tftp"
                />
              </label>
              <p class="hint">Absolute path where TFTP files are stored</p>
            </div>

            <div class="form-group">
              <label>
                PXE Boot File (optional)
                <input
                  type="text"
                  bind:value={config.pxe_boot_file}
                  placeholder="pxelinux.0"
                />
              </label>
              <p class="hint">Relative path within TFTP root</p>
            </div>
          </div>
        {/if}

        <!-- Server Status -->
        <div class="form-section">
          <h4>Server Status</h4>

          <div class="server-status">
            <span>Status:</span>
            <span class:running class:stopped={!running}>
              {running ? 'Running' : 'Stopped'}
            </span>
          </div>

          <div class="form-actions">
            {#if running}
              <button class="btn-danger" on:click={handleStop}>
                Stop Server
              </button>
            {:else}
              <button class="btn-primary" on:click={handleStart}>
                Start Server
              </button>
            {/if}
          </div>
        </div>

        <!-- Active Leases -->
        {#if running && config.dhcp_enabled && leases.length > 0}
          <div class="form-section">
            <h4>Active Leases</h4>

            <table class="leases-table">
              <thead>
                <tr>
                  <th>IP Address</th>
                  <th>MAC Address</th>
                  <th>Hostname</th>
                  <th>Expires</th>
                </tr>
              </thead>
              <tbody>
                {#each leases as lease}
                  <tr>
                    <td>{lease.ip}</td>
                    <td>{lease.mac}</td>
                    <td>{lease.hostname || '-'}</td>
                    <td>{formatExpiry(lease.expires_at)}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      {/if}

      <!-- Save Button -->
      <div class="card-actions">
        {#if error}
          <p class="error">{error}</p>
        {/if}

        <button
          class="btn-primary"
          on:click={handleSave}
          disabled={saving}
        >
          {saving ? 'Saving...' : 'Save Configuration'}
        </button>
      </div>
    </div>
  {/if}
</div>

<!-- Confirmation Dialog -->
{#if showConfirmDialog}
  <div class="modal-overlay" on:click={() => showConfirmDialog = false}>
    <div class="modal-content" on:click|stopPropagation>
      <div class="modal-header">
        <h3>⚠️ Confirm DHCP Server Enable</h3>
      </div>

      <div class="modal-body">
        <p>
          You are about to enable a DHCP server on your network. This can cause conflicts
          if another DHCP server is already running (such as your router).
        </p>
        <p>
          <strong>Only proceed if:</strong>
        </p>
        <ul>
          <li>You are on an isolated network</li>
          <li>You are the network administrator</li>
          <li>You have disabled other DHCP servers</li>
        </ul>
        <p>
          <strong>Do you understand and want to proceed?</strong>
        </p>
      </div>

      <div class="modal-footer">
        <button class="btn-secondary" on:click={() => showConfirmDialog = false}>
          Cancel
        </button>
        <button class="btn-danger" on:click={confirmAndSave}>
          Yes, Enable DHCP Server
        </button>
      </div>
    </div>
  </div>
{/if}

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
    margin: 16px 0 8px 0;
    font-size: 13px;
    font-weight: 600;
  }

  .warning {
    background: var(--warning-bg);
    border: 1px solid var(--warning-border);
    border-radius: 4px;
    padding: 12px;
    margin-bottom: 16px;
  }

  .warning-banner {
    background: var(--warning-bg);
    border-left: 4px solid var(--warning-color);
    padding: 12px 16px;
    margin: 16px 0;
    border-radius: 4px;
  }

  .toggle-section {
    display: flex;
    flex-direction: column;
    gap: 12px;
    margin-bottom: 24px;
    padding-bottom: 24px;
    border-bottom: 1px solid var(--border-color);
  }

  .form-section {
    margin-bottom: 24px;
    padding-bottom: 24px;
    border-bottom: 1px solid var(--border-color);
  }

  .form-section:last-child {
    border-bottom: none;
  }

  .form-row {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 16px;
  }

  @media (max-width: 768px) {
    .form-row {
      grid-template-columns: 1fr;
    }
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

  input[type="text"],
  input[type="number"] {
    width: 100%;
    padding: 8px 12px;
    border: 1px solid var(--border-color);
    border-radius: 4px;
    font-size: 14px;
  }

  .input-sm {
    padding: 6px 10px;
    font-size: 13px;
  }

  .toggle-label {
    display: flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
  }

  .toggle-label input[type="checkbox"] {
    width: auto;
  }

  .hint {
    font-size: 12px;
    color: var(--text-secondary);
    margin-top: 4px;
  }

  .reservations-table,
  .leases-table {
    width: 100%;
    border-collapse: collapse;
    margin-bottom: 16px;
  }

  .reservations-table th,
  .reservations-table td,
  .leases-table th,
  .leases-table td {
    padding: 8px 12px;
    text-align: left;
    border-bottom: 1px solid var(--border-color);
  }

  .reservations-table th,
  .leases-table th {
    font-weight: 600;
    font-size: 13px;
    color: var(--text-secondary);
  }

  .add-reservation {
    display: flex;
    gap: 8px;
    margin-top: 12px;
  }

  .server-status {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 12px;
  }

  .server-status .running {
    color: var(--success-color);
    font-weight: 600;
  }

  .server-status .stopped {
    color: var(--error-color);
    font-weight: 600;
  }

  .form-actions {
    display: flex;
    gap: 8px;
    margin-top: 16px;
  }

  .card-actions {
    margin-top: 24px;
    padding-top: 24px;
    border-top: 1px solid var(--border-color);
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

  .btn-danger {
    background: var(--error-color);
    color: white;
  }

  .btn-danger:hover {
    background: var(--error-hover);
  }

  .btn-sm {
    padding: 4px 8px;
    font-size: 12px;
  }

  .error {
    color: var(--error-color);
    font-size: 14px;
    margin-bottom: 12px;
  }

  /* Modal */
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
  }

  .modal-content {
    background: var(--card-bg);
    border-radius: 8px;
    width: 90%;
    max-width: 500px;
  }

  .modal-header {
    padding: 20px 24px;
    border-bottom: 1px solid var(--border-color);
  }

  .modal-header h3 {
    margin: 0;
    font-size: 16px;
  }

  .modal-body {
    padding: 24px;
  }

  .modal-body p {
    margin: 0 0 12px 0;
  }

  .modal-body ul {
    margin: 8px 0 12px 20px;
  }

  .modal-footer {
    display: flex;
    justify-content: flex-end;
    gap: 12px;
    padding: 20px 24px;
    border-top: 1px solid var(--border-color);
  }
</style>
