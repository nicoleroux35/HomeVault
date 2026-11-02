<script lang="ts">
  import { onMount } from 'svelte';
  import {
    listUsbCopyProfiles,
    createUsbCopyProfile,
    updateUsbCopyProfile,
    deleteUsbCopyProfile,
    listUsbDevices,
    executeUsbCopy,
  } from '$lib/services/api-phase4';
  import type { UsbCopyProfile, RemovableDevice } from '$lib/types/phase4';

  let profiles: UsbCopyProfile[] = [];
  let devices: RemovableDevice[] = [];
  let loading = true;
  let error = '';

  let showProfileDialog = false;
  let editingProfile: Partial<UsbCopyProfile> | null = null;
  let isEditing = false;

  onMount(async () => {
    await loadData();
  });

  async function loadData() {
    loading = true;
    error = '';

    try {
      [profiles, devices] = await Promise.all([
        listUsbCopyProfiles(),
        listUsbDevices(),
      ]);
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load data';
    } finally {
      loading = false;
    }
  }

  function openNewProfile() {
    editingProfile = {
      id: `profile_${Date.now()}`,
      name: '',
      device_uuid: '',
      device_label: null,
      source_path: '/',
      destination_path: '/mnt/usb-copy',
      verify_checksums: true,
      auto_eject: true,
      enabled: true,
      last_run: null,
      last_status: null,
    };
    isEditing = false;
    showProfileDialog = true;
  }

  function openEditProfile(profile: UsbCopyProfile) {
    editingProfile = { ...profile };
    isEditing = true;
    showProfileDialog = true;
  }

  async function saveProfile() {
    if (!editingProfile || !editingProfile.name || !editingProfile.device_uuid) {
      alert('Please fill all required fields');
      return;
    }

    try {
      const profile = editingProfile as UsbCopyProfile;

      if (isEditing) {
        await updateUsbCopyProfile(profile.id, profile);
      } else {
        await createUsbCopyProfile(profile);
      }

      await loadData();
      closeDialog();
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to save profile';
    }
  }

  async function deleteProfile(id: string) {
    if (!confirm('Are you sure you want to delete this profile?')) return;

    try {
      await deleteUsbCopyProfile(id);
      await loadData();
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to delete profile';
    }
  }

  async function executeCopy(profileId: string) {
    try {
      const result = await executeUsbCopy(profileId);
      alert(result.message);
      await loadData();
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to execute copy';
    }
  }

  function closeDialog() {
    showProfileDialog = false;
    editingProfile = null;
    isEditing = false;
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

  function formatDate(dateStr: string | null): string {
    if (!dateStr) return 'Never';
    return new Date(dateStr).toLocaleString();
  }
</script>

<div class="page-container">
  <header class="page-header">
    <h1>USB One-Touch Copy</h1>
    <button class="btn-primary" on:click={openNewProfile}>
      Create Profile
    </button>
  </header>

  {#if error}
    <div class="alert alert-error">{error}</div>
  {/if}

  {#if loading}
    <div class="loading">Loading...</div>
  {:else}
    <div class="content-grid">
      <!-- Profiles Section -->
      <section class="card">
        <h2>Copy Profiles</h2>

        {#if profiles.length === 0}
          <p class="empty-state">No profiles configured yet. Create one to get started.</p>
        {:else}
          <div class="profiles-list">
            {#each profiles as profile}
              <div class="profile-item" class:disabled={!profile.enabled}>
                <div class="profile-header">
                  <h3>{profile.name}</h3>
                  <span class="badge" class:enabled={profile.enabled} class:disabled={!profile.enabled}>
                    {profile.enabled ? 'Enabled' : 'Disabled'}
                  </span>
                </div>

                <div class="profile-details">
                  <div class="detail-row">
                    <span class="label">Device UUID:</span>
                    <span class="value">{profile.device_uuid}</span>
                  </div>
                  <div class="detail-row">
                    <span class="label">Source:</span>
                    <span class="value">{profile.source_path}</span>
                  </div>
                  <div class="detail-row">
                    <span class="label">Destination:</span>
                    <span class="value">{profile.destination_path}</span>
                  </div>
                  <div class="detail-row">
                    <span class="label">Last Run:</span>
                    <span class="value">{formatDate(profile.last_run)}</span>
                  </div>
                </div>

                <div class="profile-actions">
                  <button
                    class="btn-secondary btn-sm"
                    on:click={() => executeCopy(profile.id)}
                    disabled={!profile.enabled}
                  >
                    Execute Now
                  </button>
                  <button
                    class="btn-secondary btn-sm"
                    on:click={() => openEditProfile(profile)}
                  >
                    Edit
                  </button>
                  <button
                    class="btn-danger btn-sm"
                    on:click={() => deleteProfile(profile.id)}
                  >
                    Delete
                  </button>
                </div>
              </div>
            {/each}
          </div>
        {/if}
      </section>

      <!-- Devices Section -->
      <section class="card">
        <h2>Detected Devices</h2>

        <button class="btn-secondary btn-sm" on:click={loadData}>
          Refresh
        </button>

        {#if devices.length === 0}
          <p class="empty-state">No removable devices detected.</p>
        {:else}
          <div class="devices-list">
            {#each devices as device}
              <div class="device-item" class:matched={device.matching_profile}>
                <div class="device-icon">💾</div>
                <div class="device-info">
                  <h3>{device.label || device.device}</h3>
                  <p class="device-details">
                    {device.uuid || 'No UUID'} • {formatBytes(device.size)}
                  </p>
                  {#if device.mounted}
                    <p class="device-status mounted">
                      📌 Mounted at {device.mount_point}
                    </p>
                  {:else}
                    <p class="device-status unmounted">Not mounted</p>
                  {/if}

                  {#if device.matching_profile}
                    <p class="matching-profile">
                      ✓ Matches profile: {profiles.find(p => p.id === device.matching_profile)?.name}
                    </p>
                  {/if}
                </div>
              </div>
            {/each}
          </div>
        {/if}
      </section>
    </div>
  {/if}
</div>

<!-- Profile Dialog -->
{#if showProfileDialog && editingProfile}
  <div class="modal-overlay" on:click={closeDialog}>
    <div class="modal-content" on:click|stopPropagation>
      <div class="modal-header">
        <h2>{isEditing ? 'Edit Profile' : 'New Profile'}</h2>
        <button class="btn-close" on:click={closeDialog}>×</button>
      </div>

      <div class="modal-body">
        <div class="form-group">
          <label>
            Profile Name *
            <input
              type="text"
              bind:value={editingProfile.name}
              placeholder="My Camera Photos"
            />
          </label>
        </div>

        <div class="form-group">
          <label>
            Device UUID *
            <input
              type="text"
              bind:value={editingProfile.device_uuid}
              placeholder="1234-5678"
            />
          </label>
          <p class="hint">Find the UUID in the detected devices list above</p>
        </div>

        <div class="form-group">
          <label>
            Source Path
            <input
              type="text"
              bind:value={editingProfile.source_path}
              placeholder="/DCIM"
            />
          </label>
          <p class="hint">Path within the USB device to copy from</p>
        </div>

        <div class="form-group">
          <label>
            Destination Path *
            <input
              type="text"
              bind:value={editingProfile.destination_path}
              placeholder="/mnt/photos"
            />
          </label>
          <p class="hint">Absolute path on HomeVault to copy to</p>
        </div>

        <div class="form-group">
          <label class="checkbox-label">
            <input type="checkbox" bind:checked={editingProfile.verify_checksums} />
            Verify checksums after copy
          </label>
        </div>

        <div class="form-group">
          <label class="checkbox-label">
            <input type="checkbox" bind:checked={editingProfile.auto_eject} />
            Auto-eject device after successful copy
          </label>
        </div>

        <div class="form-group">
          <label class="checkbox-label">
            <input type="checkbox" bind:checked={editingProfile.enabled} />
            Enable this profile
          </label>
        </div>
      </div>

      <div class="modal-footer">
        <button class="btn-secondary" on:click={closeDialog}>Cancel</button>
        <button class="btn-primary" on:click={saveProfile}>
          {isEditing ? 'Save Changes' : 'Create Profile'}
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .page-container {
    padding: 24px;
    max-width: 1400px;
    margin: 0 auto;
  }

  .page-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 24px;
  }

  h1 {
    font-size: 24px;
    font-weight: 600;
    margin: 0;
  }

  h2 {
    font-size: 18px;
    font-weight: 600;
    margin: 0 0 16px 0;
  }

  .content-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 24px;
  }

  @media (max-width: 1024px) {
    .content-grid {
      grid-template-columns: 1fr;
    }
  }

  .card {
    background: var(--card-bg);
    border-radius: 8px;
    padding: 24px;
  }

  .alert {
    padding: 12px 16px;
    border-radius: 4px;
    margin-bottom: 16px;
  }

  .alert-error {
    background: var(--error-bg);
    border: 1px solid var(--error-border);
    color: var(--error-color);
  }

  .loading {
    text-align: center;
    padding: 48px;
    color: var(--text-secondary);
  }

  .empty-state {
    text-align: center;
    padding: 48px 24px;
    color: var(--text-secondary);
    font-style: italic;
  }

  .profiles-list,
  .devices-list {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .profile-item {
    border: 1px solid var(--border-color);
    border-radius: 8px;
    padding: 16px;
    transition: border-color 0.2s;
  }

  .profile-item:hover {
    border-color: var(--primary-color);
  }

  .profile-item.disabled {
    opacity: 0.6;
  }

  .profile-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 12px;
  }

  .profile-header h3 {
    font-size: 16px;
    font-weight: 600;
    margin: 0;
  }

  .badge {
    padding: 4px 12px;
    border-radius: 12px;
    font-size: 12px;
    font-weight: 500;
  }

  .badge.enabled {
    background: var(--success-bg);
    color: var(--success-color);
  }

  .badge.disabled {
    background: var(--secondary-bg);
    color: var(--text-secondary);
  }

  .profile-details {
    margin-bottom: 12px;
  }

  .detail-row {
    display: flex;
    gap: 8px;
    margin-bottom: 4px;
    font-size: 14px;
  }

  .detail-row .label {
    color: var(--text-secondary);
    min-width: 100px;
  }

  .detail-row .value {
    color: var(--text-primary);
    font-family: monospace;
  }

  .profile-actions {
    display: flex;
    gap: 8px;
  }

  .device-item {
    display: flex;
    gap: 16px;
    border: 1px solid var(--border-color);
    border-radius: 8px;
    padding: 16px;
    transition: border-color 0.2s;
  }

  .device-item:hover {
    border-color: var(--primary-color);
  }

  .device-item.matched {
    border-color: var(--success-color);
    background: var(--success-bg-light);
  }

  .device-icon {
    font-size: 32px;
  }

  .device-info {
    flex: 1;
  }

  .device-info h3 {
    font-size: 16px;
    font-weight: 600;
    margin: 0 0 4px 0;
  }

  .device-details {
    font-size: 14px;
    color: var(--text-secondary);
    margin: 0 0 8px 0;
  }

  .device-status {
    font-size: 13px;
    margin: 4px 0;
  }

  .device-status.mounted {
    color: var(--success-color);
  }

  .device-status.unmounted {
    color: var(--text-secondary);
  }

  .matching-profile {
    color: var(--success-color);
    font-size: 13px;
    font-weight: 500;
    margin: 4px 0 0 0;
  }

  /* Modal Styles */
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
    max-width: 600px;
    max-height: 90vh;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }

  .modal-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 24px;
    border-bottom: 1px solid var(--border-color);
  }

  .btn-close {
    background: none;
    border: none;
    font-size: 24px;
    cursor: pointer;
    color: var(--text-secondary);
  }

  .modal-body {
    padding: 24px;
    overflow-y: auto;
  }

  .modal-footer {
    display: flex;
    justify-content: flex-end;
    gap: 12px;
    padding: 24px;
    border-top: 1px solid var(--border-color);
  }

  .form-group {
    margin-bottom: 20px;
  }

  label {
    display: block;
    font-size: 14px;
    font-weight: 500;
    margin-bottom: 6px;
  }

  input[type="text"],
  input[type="number"] {
    width: 100%;
    padding: 10px 12px;
    border: 1px solid var(--border-color);
    border-radius: 4px;
    font-size: 14px;
  }

  .checkbox-label {
    display: flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
  }

  .checkbox-label input[type="checkbox"] {
    width: auto;
  }

  .hint {
    font-size: 12px;
    color: var(--text-secondary);
    margin-top: 4px;
  }

  /* Button Styles */
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
    padding: 6px 12px;
    font-size: 13px;
  }

  button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
