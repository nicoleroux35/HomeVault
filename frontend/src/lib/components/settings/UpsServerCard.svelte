<script lang="ts">
  import { onMount } from 'svelte';
  import { getUpsServerConfig, updateUpsServerConfig, startUpsServer, stopUpsServer } from '$lib/services/api-phase4';
  import type { UpsServerConfig, UpsClient } from '$lib/types/phase4';

  let config: UpsServerConfig | null = null;
  let loading = true;
  let saving = false;
  let error = '';
  let available = false;
  let running = false;
  let showAddClient = false;
  let newClient: Partial<UpsClient> = {
    name: '',
    address: '',
    username: '',
    master: false,
  };

  onMount(async () => {
    try {
      const response = await getUpsServerConfig();
      config = response.config;
      available = response.available;
      running = response.state.running;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load UPS server config';
    } finally {
      loading = false;
    }
  });

  async function handleSave() {
    if (!config) return;

    saving = true;
    error = '';

    try {
      await updateUpsServerConfig(config);
      alert('UPS server configuration saved successfully');
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to save configuration';
    } finally {
      saving = false;
    }
  }

  async function handleStart() {
    try {
      await startUpsServer();
      running = true;
      alert('UPS server started successfully');
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to start UPS server';
    }
  }

  async function handleStop() {
    try {
      await stopUpsServer();
      running = false;
      alert('UPS server stopped successfully');
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to stop UPS server';
    }
  }

  function addClient() {
    if (!config || !newClient.name || !newClient.address || !newClient.username) {
      alert('Please fill all client fields');
      return;
    }

    const client: UpsClient = {
      name: newClient.name,
      address: newClient.address,
      username: newClient.username,
      password_hash: '', // Will be set on backend
      master: newClient.master || false,
    };

    config.allowed_clients = [...config.allowed_clients, client];

    // Reset form
    newClient = { name: '', address: '', username: '', master: false };
    showAddClient = false;
  }

  function removeClient(index: number) {
    if (!config) return;
    config.allowed_clients = config.allowed_clients.filter((_, i) => i !== index);
  }
</script>

<div class="settings-card" data-testid="ups-server-card">
  <h3>UPS Server Mode</h3>

  {#if !available}
    <div class="warning">
      <p>⚠️ UPS server (upsd) is not available on this system.</p>
      <p>Install nut-server package to enable this feature.</p>
    </div>
  {/if}

  {#if loading}
    <p>Loading...</p>
  {:else if config}
    <div class="config-form">
      <!-- Enable Toggle -->
      <label class="toggle-label">
        <input
          type="checkbox"
          bind:checked={config.enabled}
          data-testid="ups-server-enable"
        />
        <span>Enable UPS Server</span>
      </label>

      {#if config.enabled}
        <!-- Network Configuration -->
        <div class="form-section">
          <h4>Network Configuration</h4>

          <div class="form-group">
            <label>
              Interface
              <input
                type="text"
                bind:value={config.bind_interface}
                placeholder="eth0"
              />
            </label>
          </div>

          <div class="form-group">
            <label>
              Port
              <input
                type="number"
                bind:value={config.port}
                min="1024"
                max="65535"
              />
            </label>
          </div>

          <div class="form-group">
            <label>
              Max Clients
              <input
                type="number"
                bind:value={config.max_clients}
                min="1"
                max="100"
              />
            </label>
          </div>
        </div>

        <!-- Allowed Clients -->
        <div class="form-section">
          <h4>Allowed Clients</h4>

          {#if config.allowed_clients.length === 0}
            <p class="hint">No clients configured yet.</p>
          {:else}
            <table class="clients-table">
              <thead>
                <tr>
                  <th>Name</th>
                  <th>Address</th>
                  <th>Username</th>
                  <th>Master</th>
                  <th>Actions</th>
                </tr>
              </thead>
              <tbody>
                {#each config.allowed_clients as client, i}
                  <tr>
                    <td>{client.name}</td>
                    <td>{client.address}</td>
                    <td>{client.username}</td>
                    <td>{client.master ? 'Yes' : 'No'}</td>
                    <td>
                      <button
                        class="btn-small btn-danger"
                        on:click={() => removeClient(i)}
                      >
                        Remove
                      </button>
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          {/if}

          {#if !showAddClient}
            <button
              class="btn-secondary"
              on:click={() => showAddClient = true}
              data-testid="ups-add-client"
            >
              Add Client
            </button>
          {:else}
            <div class="add-client-form">
              <h5>Add New Client</h5>

              <div class="form-group">
                <label>
                  Name
                  <input
                    type="text"
                    bind:value={newClient.name}
                    placeholder="client1"
                  />
                </label>
              </div>

              <div class="form-group">
                <label>
                  Address (IP or CIDR)
                  <input
                    type="text"
                    bind:value={newClient.address}
                    placeholder="192.168.1.100"
                  />
                </label>
              </div>

              <div class="form-group">
                <label>
                  Username
                  <input
                    type="text"
                    bind:value={newClient.username}
                    placeholder="monitor"
                  />
                </label>
              </div>

              <label class="toggle-label">
                <input type="checkbox" bind:checked={newClient.master} />
                <span>Master (can issue shutdown commands)</span>
              </label>

              <div class="form-actions">
                <button class="btn-primary" on:click={addClient}>
                  Add Client
                </button>
                <button class="btn-secondary" on:click={() => showAddClient = false}>
                  Cancel
                </button>
              </div>
            </div>
          {/if}
        </div>

        <!-- Shutdown Policy -->
        <div class="form-section">
          <h4>Shutdown Policy</h4>

          <label class="toggle-label">
            <input type="checkbox" bind:checked={config.shutdown_policy.stop_containers} />
            <span>Stop containers before shutdown</span>
          </label>

          <label class="toggle-label">
            <input type="checkbox" bind:checked={config.shutdown_policy.shutdown_vms} />
            <span>Shutdown VMs before shutdown</span>
          </label>

          <label class="toggle-label">
            <input type="checkbox" bind:checked={config.shutdown_policy.flush_restic} />
            <span>Flush pending Restic operations</span>
          </label>

          <label class="toggle-label">
            <input type="checkbox" bind:checked={config.shutdown_policy.stop_shares} />
            <span>Stop share services</span>
          </label>
        </div>

        <!-- Server Control -->
        <div class="form-section">
          <h4>Server Control</h4>

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
    margin: 16px 0 12px 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--text-secondary);
  }

  .warning {
    background: var(--warning-bg);
    border: 1px solid var(--warning-border);
    border-radius: 4px;
    padding: 12px;
    margin-bottom: 16px;
  }

  .form-section {
    margin-bottom: 24px;
    padding-bottom: 24px;
    border-bottom: 1px solid var(--border-color);
  }

  .form-section:last-child {
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

  input[type="text"],
  input[type="number"] {
    width: 100%;
    padding: 8px 12px;
    border: 1px solid var(--border-color);
    border-radius: 4px;
    font-size: 14px;
  }

  .toggle-label {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 12px;
    cursor: pointer;
  }

  .toggle-label input[type="checkbox"] {
    width: auto;
  }

  .clients-table {
    width: 100%;
    border-collapse: collapse;
    margin-bottom: 16px;
  }

  .clients-table th,
  .clients-table td {
    padding: 8px 12px;
    text-align: left;
    border-bottom: 1px solid var(--border-color);
  }

  .clients-table th {
    font-weight: 600;
    font-size: 13px;
    color: var(--text-secondary);
  }

  .add-client-form {
    background: var(--surface-bg);
    border-radius: 4px;
    padding: 16px;
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

  .btn-small {
    padding: 4px 8px;
    font-size: 12px;
  }

  .error {
    color: var(--error-color);
    font-size: 14px;
    margin-bottom: 12px;
  }

  .hint {
    color: var(--text-secondary);
    font-size: 14px;
    font-style: italic;
  }
</style>
