<!-- ========================================================================
     File          : IsoMountPanel.svelte
     Author        : Nicolas Le Roux
     Created       : 02/11/2026
     Last modified : 02/11/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       The read-only ISO/IMG mounting panel of the Explorer sidebar: what is
       mounted, an action to mount the selected image, and an action to unmount
       each entry.

       The panel is honest about what it cannot do: when `losetup`/`mount` are
       absent the list is replaced by a sentence naming the missing tools, and
       the mount action is withheld — never a button that always fails
       (`docs/API.md`, capability contract).

     Dependencies:
       - svelte (>= 5), $lib/services/api, $lib/stores/i18n,
         $lib/components/theme/{Button, Modal}

     Usage:
       Rendered by routes/Explorer.svelte inside its sidebar. See
       docs/guides/iso-mount.md.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { t } from '$lib/stores/i18n';
  import { Button } from '$lib/components/theme';
  import { IconDisc, IconPlayerEject } from '@tabler/icons-svelte';

  let {
    selectedPath = null,
    selectedKind = null,
    onmounted,
    onunmounted
  }: {
    /** The Explorer's current selection, so "mount" can act on it. */
    selectedPath?: string | null;
    /** `file` shows the mount action; anything else does not. */
    selectedKind?: string | null;
    onmounted?: (mountPoint: string) => void;
    onunmounted?: (mountPoint: string) => void;
  } = $props();

  interface IsoMount {
    id: string;
    image_path: string;
    label: string;
    mount_point: string;
    loop_device: string | null;
    size_bytes: number;
  }

  let loading = $state(true);
  let busy = $state<string | null>(null);
  let notice = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);
  let available = $state(false);
  let mounts = $state<IsoMount[]>([]);

  /** A `.iso`/`.img` file is the only thing the mount action accepts. */
  const canMount = $derived(
    !!selectedPath &&
      selectedKind === 'file' &&
      /\.(iso|img)$/i.test(selectedPath)
  );

  onMount(load);

  async function load() {
    loading = true;
    const res = await api.listIsoMounts();
    if (res.success && res.data) {
      available = !!res.data.available;
      mounts = res.data.mounts ?? [];
    } else {
      notice = {
        kind: 'error',
        text: res.message || res.error || $t('explorer.iso_mount_load_failed')
      };
    }
    loading = false;
  }

  async function mountSelected() {
    if (!selectedPath) return;
    busy = selectedPath;
    notice = null;
    const res = await api.mountIso(selectedPath);
    if (res.success) {
      const point = (res.data as any)?.mount_point;
      notice = { kind: 'ok', text: $t('explorer.iso_mount_mounted') };
      await load();
      if (point) onmounted?.(point);
    } else {
      // The server names the rule it refused on ("outside the folders you may
      // read", "a '.zip' file"); surfacing it verbatim is what makes the
      // refusal actionable.
      notice = {
        kind: 'error',
        text: res.message || res.error || $t('explorer.iso_mount_mount_failed')
      };
    }
    busy = null;
  }

  async function unmount(mount: IsoMount) {
    busy = mount.id;
    notice = null;
    const res = await api.unmountIso(mount.id);
    if (res.success) {
      notice = { kind: 'ok', text: $t('explorer.iso_mount_unmounted') };
      // Leave the folder before it disappears, so the Explorer does not sit
      // on a path that no longer exists.
      onunmounted?.(mount.mount_point);
      await load();
    } else {
      notice = {
        kind: 'error',
        text: res.message || res.error || $t('explorer.iso_mount_unmount_failed')
      };
    }
    busy = null;
  }

  function formatBytes(bytes: number): string {
    const units = ['B', 'KB', 'MB', 'GB', 'TB'];
    let size = bytes;
    let unit = 0;
    while (size >= 1024 && unit < units.length - 1) {
      size /= 1024;
      unit += 1;
    }
    return `${size.toFixed(1)} ${units[unit]}`;
  }
</script>

<div class="iso-panel" data-testid="iso-panel">
  <div class="iso-head">
    <span class="iso-title">{$t('explorer.iso_mount_title')}</span>
    <Button variant="ghost" size="sm" onclick={load} data-testid="iso-refresh">
      {$t('common.refresh')}
    </Button>
  </div>

  {#if notice}
    <div
      class={notice.kind === 'ok' ? 'iso-notice iso-notice-ok' : 'iso-notice iso-notice-error'}
      role={notice.kind === 'ok' ? 'status' : 'alert'}
      data-testid="iso-notice"
    >
      {notice.text}
    </div>
  {/if}

  {#if loading}
    <p class="iso-hint">{$t('common.loading')}</p>
  {:else if !available}
    <!-- `503` semantics made visible: name the missing tools, offer nothing. -->
    <p class="iso-hint" data-testid="iso-unavailable">
      {$t('explorer.iso_mount_unavailable')}
    </p>
  {:else}
    {#if canMount}
      <Button
        variant="primary"
        size="sm"
        onclick={mountSelected}
        disabled={busy !== null}
        data-testid="iso-mount"
      >
        <IconDisc size={14} />
        <span>{$t('explorer.iso_mount_mount')}</span>
      </Button>
    {/if}

    {#if mounts.length === 0}
      <p class="iso-hint" data-testid="iso-none">{$t('explorer.iso_mount_none')}</p>
    {:else}
      <ul class="iso-list" data-testid="iso-list">
        {#each mounts as mount (mount.id)}
          <li class="iso-item">
            <div class="iso-item-main">
              <span class="iso-label">{mount.label}</span>
              <span class="iso-size">{formatBytes(mount.size_bytes)}</span>
            </div>
            <span class="iso-path">{mount.mount_point}</span>
            <Button
              variant="ghost"
              size="sm"
              onclick={() => unmount(mount)}
              disabled={busy === mount.id}
              data-testid={`iso-unmount-${mount.id}`}
              aria-label={$t('explorer.iso_mount_unmount_named', { name: mount.label })}
            >
              <IconPlayerEject size={14} />
            </Button>
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
</div>

<style>
  .iso-panel {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 8px;
  }

  .iso-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }

  .iso-title {
    font-size: 0.75rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--hv-shell-text-3);
  }

  .iso-hint {
    margin: 0;
    font-size: 0.75rem;
    color: var(--hv-shell-text-2);
  }

  .iso-notice {
    border: 1px solid var(--hv-shell-border);
    border-radius: 6px;
    padding: 6px 8px;
    font-size: 0.75rem;
    line-height: 1rem;
  }

  .iso-notice-error {
    background: var(--hv-error-bg);
    border-color: var(--hv-error);
    color: var(--hv-error);
  }

  .iso-notice-ok {
    background: var(--hv-success-bg);
    border-color: var(--hv-success);
    color: var(--hv-success);
  }

  .iso-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 6px;
  }

  .iso-item {
    display: flex;
    flex-direction: column;
    gap: 2px;
    border: 1px solid var(--hv-shell-border);
    border-radius: 6px;
    padding: 6px 8px;
  }

  .iso-item-main {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
  }

  .iso-label {
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--hv-shell-text);
    overflow-wrap: anywhere;
  }

  .iso-size {
    font-size: 0.6875rem;
    color: var(--hv-shell-text-3);
    font-family: var(--hv-font-mono);
    flex: none;
  }

  .iso-path {
    font-size: 0.6875rem;
    color: var(--hv-shell-text-2);
    font-family: var(--hv-font-mono);
    overflow-wrap: anywhere;
  }
</style>
