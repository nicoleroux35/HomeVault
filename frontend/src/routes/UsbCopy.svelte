<!-- ========================================================================
     File          : UsbCopy.svelte
     Author        : Nicolas Le Roux
     Created       : 02/11/2026
     Last modified : 02/11/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       The USB one-touch copy app: the copy rules, the removable devices that
       are currently attached, and the rule editor.

       Rewritten on the shared design tokens, the shared `Modal` (which traps
       focus) and the i18n bundle. The previous version carried hardcoded
       English strings, native `alert()` / `confirm()`, a hand-rolled overlay
       with no focus trap, and CSS variables that do not exist in the token
       set, so it rendered unstyled.

     Dependencies:
       - svelte (>= 5), $lib/services/api-phase4, $lib/stores/i18n,
         $lib/components/theme/Modal, $lib/components/theme/Button

     Usage:
       Launched from the desktop's USB Copy window. See
       docs/guides/usb-copy.md.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from '$lib/stores/i18n';
  import {
    listUsbCopyProfiles,
    createUsbCopyProfile,
    updateUsbCopyProfile,
    deleteUsbCopyProfile,
    listUsbDevices,
    executeUsbCopy
  } from '$lib/services/api-phase4';
  import type { UsbCopyProfile, RemovableDevice } from '$lib/types/phase4';
  import Modal from '$lib/components/theme/Modal.svelte';
  import { Button } from '$lib/components/theme';
  import { PageLayout, SectionCard } from '$lib/components/app';
  import { IconUsb } from '$lib/icons/lucide';

  let profiles = $state<UsbCopyProfile[]>([]);
  let devices = $state<RemovableDevice[]>([]);
  let loading = $state(true);
  let busy = $state(false);
  let error = $state('');
  let notice = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);

  let showProfileDialog = $state(false);
  let editingProfile = $state<Partial<UsbCopyProfile> | null>(null);
  let isEditing = $state(false);

  onMount(loadData);

  async function loadData() {
    loading = true;
    error = '';
    try {
      const [nextProfiles, nextDevices] = await Promise.all([
        listUsbCopyProfiles(),
        listUsbDevices()
      ]);
      profiles = nextProfiles;
      devices = nextDevices;
    } catch (e) {
      error = e instanceof Error ? e.message : $t('settings.usb_copy_load_failed');
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
      strategy: 'incremental',
      conflict: 'overwrite',
      versions_to_keep: 10,
      auto_eject: true,
      enabled: true,
      last_run: null,
      last_status: null
    };
    isEditing = false;
    notice = null;
    showProfileDialog = true;
  }

  function openEditProfile(profile: UsbCopyProfile) {
    editingProfile = { ...profile };
    isEditing = true;
    notice = null;
    showProfileDialog = true;
  }

  async function saveProfile() {
    if (!editingProfile || !editingProfile.name || !editingProfile.device_uuid) {
      notice = { kind: 'error', text: $t('settings.usb_copy_save_failed') };
      return;
    }

    busy = true;
    try {
      const profile = editingProfile as UsbCopyProfile;
      if (isEditing) {
        await updateUsbCopyProfile(profile.id, profile);
      } else {
        await createUsbCopyProfile(profile);
      }
      await loadData();
      closeDialog();
      notice = { kind: 'ok', text: $t('settings.usb_copy_saved') };
    } catch (e) {
      notice = {
        kind: 'error',
        text: e instanceof Error ? e.message : $t('settings.usb_copy_save_failed')
      };
    } finally {
      busy = false;
    }
  }

  async function deleteProfile(id: string) {
    busy = true;
    try {
      await deleteUsbCopyProfile(id);
      await loadData();
      notice = { kind: 'ok', text: $t('settings.usb_copy_saved') };
    } catch (e) {
      notice = {
        kind: 'error',
        text: e instanceof Error ? e.message : $t('settings.usb_copy_save_failed')
      };
    } finally {
      busy = false;
    }
  }

  async function executeCopy(profileId: string) {
    busy = true;
    notice = null;
    try {
      const result = await executeUsbCopy(profileId);
      notice = { kind: 'ok', text: result?.message || $t('settings.usb_copy_started') };
      await loadData();
    } catch (e) {
      notice = {
        kind: 'error',
        text: e instanceof Error ? e.message : $t('settings.usb_copy_failed')
      };
    } finally {
      busy = false;
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
    if (!dateStr) return $t('settings.usb_copy_never');
    return new Date(dateStr).toLocaleString();
  }

  function profileName(id: string | null): string {
    return profiles.find((p) => p.id === id)?.name ?? id ?? '';
  }
</script>

<PageLayout title={$t('settings.usb_copy_title')} data-testid="usb-page">
  {#snippet icon()}
    <IconUsb strokeWidth={1.8} />
  {/snippet}

  {#snippet toolbarRight()}
    <Button variant="ghost" onclick={loadData} data-testid="usb-refresh">
      {$t('common.refresh')}
    </Button>
    <Button variant="primary" onclick={openNewProfile} data-testid="usb-new-profile">
      {$t('settings.usb_copy_new_profile')}
    </Button>
  {/snippet}

  <div class="flex flex-col gap-4">
    <p class="m-0 text-c-sm text-[var(--hv-shell-text-2)]">{$t('settings.usb_copy_subtitle')}</p>

  {#if error}
    <div class="usb-alert usb-alert-error" role="alert">{error}</div>
  {/if}
  {#if notice}
    <div
      class={notice.kind === 'ok' ? 'usb-alert usb-alert-ok' : 'usb-alert usb-alert-error'}
      role={notice.kind === 'ok' ? 'status' : 'alert'}
      data-testid="usb-notice"
    >
      {notice.text}
    </div>
  {/if}

  {#if loading}
    <p class="usb-hint">{$t('common.loading')}</p>
  {:else}
    <div class="grid gap-4 @min-[900px]:grid-cols-[3fr_2fr]">
      <SectionCard title={$t('settings.usb_copy_profiles_title')} class="min-w-0">
        {#if profiles.length === 0}
          <p class="usb-hint" data-testid="usb-no-profiles">
            {$t('settings.usb_copy_profiles_empty')}
          </p>
        {:else}
          <ul class="usb-profiles" data-testid="usb-profiles">
            {#each profiles as profile (profile.id)}
              <li class="usb-profile" class:is-disabled={!profile.enabled}>
                <div class="usb-profile-head">
                  <h3 class="usb-profile-name">{profile.name}</h3>
                  <span class={profile.enabled ? 'usb-badge usb-badge-on' : 'usb-badge'}>
                    {profile.enabled ? $t('settings.usb_copy_enabled') : $t('common.no')}
                  </span>
                </div>

                <dl class="usb-details">
                  <div>
                    <dt>{$t('settings.usb_copy_device_uuid')}</dt>
                    <dd>{profile.device_uuid}</dd>
                  </div>
                  <div>
                    <dt>{$t('settings.usb_copy_source')}</dt>
                    <dd>{profile.source_path}</dd>
                  </div>
                  <div>
                    <dt>{$t('settings.usb_copy_destination')}</dt>
                    <dd>{profile.destination_path}</dd>
                  </div>
                  <div>
                    <dt>{$t('settings.usb_copy_strategy')}</dt>
                    <dd>{$t(`settings.usb_copy_strategy_${profile.strategy}`)}</dd>
                    {#if profile.strategy === 'versioned'}
                      <dt>{$t('settings.usb_copy_versions_to_keep')}</dt>
                      <dd>
                        {profile.versions_to_keep === 0
                          ? $t('settings.usb_copy_versions_unlimited')
                          : profile.versions_to_keep}
                      </dd>
                    {/if}
                  </div>
                  <div>
                    <dt>{$t('settings.usb_copy_last_run')}</dt>
                    <dd>{formatDate(profile.last_run)}</dd>
                  </div>
                </dl>

                <div class="usb-actions">
                  <Button
                    variant="secondary"
                    size="sm"
                    onclick={() => executeCopy(profile.id)}
                    disabled={busy || !profile.enabled}
                    data-testid={`usb-run-${profile.id}`}
                  >
                    {$t('settings.usb_copy_run_now')}
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    onclick={() => openEditProfile(profile)}
                    data-testid={`usb-edit-${profile.id}`}
                  >
                    {$t('settings.usb_copy_edit_profile')}
                  </Button>
                  <Button
                    variant="danger"
                    size="sm"
                    onclick={() => deleteProfile(profile.id)}
                    disabled={busy}
                    data-testid={`usb-delete-${profile.id}`}
                  >
                    {$t('settings.usb_copy_delete_profile')}
                  </Button>
                </div>
              </li>
            {/each}
          </ul>
        {/if}
      </SectionCard>

      <SectionCard title={$t('settings.usb_copy_devices_title')} class="min-w-0">
        {#if devices.length === 0}
          <p class="usb-hint" data-testid="usb-no-devices">
            {$t('settings.usb_copy_devices_empty')}
          </p>
        {:else}
          <ul class="usb-devices" data-testid="usb-devices">
            {#each devices as device (device.device)}
              <li class="usb-device" class:is-matched={device.matching_profile}>
                <div class="usb-device-head">
                  <span class="usb-device-name">{device.label ?? device.device}</span>
                  <span class="usb-device-size">{formatBytes(device.size)}</span>
                </div>
                <p class="usb-hint">{device.uuid ?? '—'}</p>
                <p class="usb-device-state">
                  {device.mounted
                    ? `${device.mount_point ?? ''}`
                    : $t('settings.usb_copy_not_mounted')}
                </p>
                {#if device.matching_profile}
                  <p class="usb-device-match">
                    {$t('settings.usb_copy_matching_profile')}: {profileName(device.matching_profile)}
                  </p>
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
      </SectionCard>
    </div>
  {/if}
  </div>
</PageLayout>

<Modal
  open={showProfileDialog}
  title={isEditing ? $t('settings.usb_copy_edit_profile') : $t('settings.usb_copy_new_profile')}
  size="md"
  onclose={closeDialog}
>
  {#if editingProfile}
    <div class="usb-form">
      <div class="usb-field">
        <label for="usb-name">{$t('settings.usb_copy_profile_name')}</label>
        <input id="usb-name" type="text" bind:value={editingProfile.name} data-testid="usb-name" />
      </div>

      <div class="usb-field">
        <label for="usb-uuid">{$t('settings.usb_copy_device_uuid')}</label>
        <select id="usb-uuid" bind:value={editingProfile.device_uuid} data-testid="usb-uuid">
          <option value="">—</option>
          {#each devices as device (device.device)}
            <option value={device.uuid ?? ''}>
              {device.label ?? device.device} ({device.uuid ?? '—'})
            </option>
          {/each}
        </select>
        <p class="usb-hint">{$t('settings.usb_copy_device_uuid_hint')}</p>
      </div>

      <div class="usb-field">
        <label for="usb-source">{$t('settings.usb_copy_source')}</label>
        <input
          id="usb-source"
          type="text"
          bind:value={editingProfile.source_path}
          placeholder="/DCIM"
          data-testid="usb-source"
        />
        <p class="usb-hint">{$t('settings.usb_copy_source_hint')}</p>
      </div>

      <div class="usb-field">
        <label for="usb-destination">{$t('settings.usb_copy_destination')}</label>
        <input
          id="usb-destination"
          type="text"
          bind:value={editingProfile.destination_path}
          placeholder="/mnt/photos"
          data-testid="usb-destination"
        />
        <p class="usb-hint">{$t('settings.usb_copy_destination_hint')}</p>
      </div>

      <div class="usb-field">
        <label for="usb-strategy">{$t('settings.usb_copy_strategy')}</label>
        <select id="usb-strategy" bind:value={editingProfile.strategy} data-testid="usb-strategy">
          <option value="incremental">{$t('settings.usb_copy_strategy_incremental')}</option>
          <option value="mirror">{$t('settings.usb_copy_strategy_mirror')}</option>
          <option value="versioned">{$t('settings.usb_copy_strategy_versioned')}</option>
          <option value="photo_import">{$t('settings.usb_copy_strategy_photo_import')}</option>
        </select>
        <p class="usb-hint">{$t('settings.usb_copy_strategy_hint')}</p>
      </div>

      {#if editingProfile.strategy === 'versioned'}
        <div class="usb-field">
          <label for="usb-versions">{$t('settings.usb_copy_versions_to_keep')}</label>
          <input
            id="usb-versions"
            type="number"
            min="0"
            max="1000"
            step="1"
            bind:value={editingProfile.versions_to_keep}
            data-testid="usb-versions-to-keep"
          />
          <p class="usb-hint">{$t('settings.usb_copy_versions_to_keep_hint')}</p>
        </div>
      {/if}

      <div class="usb-field">
        <label for="usb-conflict">{$t('settings.usb_copy_conflict')}</label>
        <select id="usb-conflict" bind:value={editingProfile.conflict} data-testid="usb-conflict">
          <option value="overwrite">{$t('settings.usb_copy_conflict_overwrite')}</option>
          <option value="skip">{$t('settings.usb_copy_conflict_skip')}</option>
          <option value="rename">{$t('settings.usb_copy_conflict_rename')}</option>
        </select>
        <p class="usb-hint">{$t('settings.usb_copy_conflict_hint')}</p>
      </div>

      <div class="usb-field usb-field-check">
        <input
          id="usb-verify"
          type="checkbox"
          class="usb-check"
          bind:checked={editingProfile.verify_checksums}
          data-testid="usb-verify"
        />
        <div>
          <label for="usb-verify">{$t('settings.usb_copy_verify_checksums')}</label>
          <p class="usb-hint">{$t('settings.usb_copy_verify_checksums_hint')}</p>
        </div>
      </div>

      <div class="usb-field usb-field-check">
        <input
          id="usb-eject"
          type="checkbox"
          class="usb-check"
          bind:checked={editingProfile.auto_eject}
          data-testid="usb-eject"
        />
        <div>
          <label for="usb-eject">{$t('settings.usb_copy_auto_eject')}</label>
          <p class="usb-hint">{$t('settings.usb_copy_auto_eject_hint')}</p>
        </div>
      </div>

      <div class="usb-field usb-field-check">
        <input
          id="usb-enabled"
          type="checkbox"
          class="usb-check"
          bind:checked={editingProfile.enabled}
          data-testid="usb-enabled"
        />
        <label for="usb-enabled">{$t('settings.usb_copy_enabled')}</label>
      </div>

      <div class="usb-actions">
        <Button variant="primary" onclick={saveProfile} disabled={busy} data-testid="usb-save">
          {$t('common.save')}
        </Button>
        <Button variant="ghost" onclick={closeDialog}>{$t('common.cancel')}</Button>
      </div>
    </div>
  {/if}
</Modal>

<style>
  .usb-alert {
    border: 1px solid var(--hv-shell-border);
    border-radius: var(--hv-radius-control);
    padding: 10px 12px;
    font-size: var(--hv-text-base);
  }

  .usb-alert-error {
    background: var(--hv-error-bg);
    border-color: var(--hv-error);
    color: var(--hv-error);
  }

  .usb-alert-ok {
    background: var(--hv-success-bg);
    border-color: var(--hv-success);
    color: var(--hv-success);
  }

  .usb-hint {
    margin: 4px 0 0;
    font-size: 0.75rem;
    color: var(--hv-shell-text-2);
  }

  .usb-profiles,
  .usb-devices {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 10px;
  }

  .usb-profile,
  .usb-device {
    border: 1px solid var(--hv-shell-border);
    border-radius: var(--hv-radius-control);
    background: var(--hv-shell-card);
    padding: 12px;
  }

  .usb-profile.is-disabled {
    opacity: 0.65;
  }

  .usb-profile-head,
  .usb-device-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin-bottom: 6px;
  }

  .usb-profile-name,
  .usb-device-name {
    margin: 0;
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--hv-shell-text);
  }

  .usb-badge {
    font-size: 0.6875rem;
    padding: 2px 8px;
    border-radius: 999px;
    background: var(--hv-shell-border-soft);
    color: var(--hv-shell-text-2);
  }

  .usb-badge-on {
    background: var(--hv-success-bg);
    color: var(--hv-success);
  }

  .usb-details {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 4px 12px;
    margin: 0 0 8px;
    font-size: 0.75rem;
  }

  .usb-details dt {
    color: var(--hv-shell-text-3);
  }

  .usb-details dd {
    margin: 0;
    font-family: var(--hv-font-mono);
    color: var(--hv-shell-text);
    overflow-wrap: anywhere;
  }

  .usb-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 8px;
  }

  .usb-device.is-matched {
    border-color: var(--hv-primary);
  }

  .usb-device-size {
    font-size: 0.75rem;
    color: var(--hv-shell-text-2);
    font-family: var(--hv-font-mono);
  }

  .usb-device-state {
    margin: 4px 0 0;
    font-size: 0.75rem;
    color: var(--hv-shell-text-2);
  }

  .usb-device-match {
    margin: 4px 0 0;
    font-size: 0.75rem;
    color: var(--hv-success);
  }

  .usb-form {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .usb-field label {
    display: block;
    font-size: var(--hv-text-label);
    font-weight: var(--hv-font-weight-medium);
    color: var(--hv-shell-text-strong);
  }

  .usb-field input[type='text'],
  .usb-field input[type='number'],
  .usb-field select {
    width: 100%;
    min-height: var(--hv-touch-target-min);
    margin-top: 4px;
    border: 1px solid var(--hv-shell-border-strong);
    border-radius: var(--hv-radius-control);
    padding: 0 12px;
    font-size: var(--hv-text-base);
    background: var(--hv-shell-fill);
    color: var(--hv-shell-text-strong);
  }

  .usb-field input:focus-visible,
  .usb-field select:focus-visible {
    outline: none;
    border-color: var(--hv-primary);
    box-shadow: var(--hv-focus-ring);
  }

  .usb-field-check {
    display: flex;
    align-items: flex-start;
    gap: 10px;
  }

  /* A bare checkbox inherits `width: 100%` from base.css (design-system
     pitfall 6). */
  .usb-check {
    height: 1rem;
    width: 1rem;
    flex: none;
    margin-top: 2px;
    accent-color: var(--hv-primary);
  }
</style>
