<!-- ========================================================================
     File          : MemberAccessCard.svelte
     Author        : Nicolas Le Roux
     Created       : 13/10/2026
     Last modified : 13/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Per-member resource-access editor embedded in the Users detail view:
       the admin grants a member access to specific apps, folders and shares
       (stored server-side as resource_shares, enforced server-side too).

     Dependencies:
       - svelte (>= 5), $lib/services/api

     Usage:
       Embedded in the Users app detail view.
     ======================================================================== -->
<script lang="ts">
  /**
   * Per-member resource access editor, embedded in the Users app's detail view.
   *
   * The admin (HomeVault's "owner") grants a member access to specific apps,
   * folders and shares. The grants are stored server-side in `resource_shares`
   * and enforced by the backend — this card is only the editor. A member sees
   * exactly the rows they hold (via `GET /shares/access/mine`); an admin sees
   * everything implicitly and needs no rows here.
   */
  import { api } from '$lib/services/api';
  import { t, translate } from '$lib/stores/i18n';
  import { SectionCard } from '$lib/components/app';
  import { Spinner } from '$lib/components/theme';
  import { IconKey, IconPlus, IconTrash } from '@tabler/icons-svelte';
  import { cn } from '$lib/utils.js';

  export let username: string;
  export let isAdmin = false;

  type Kind = 'app' | 'folder' | 'share';
  type Permission = 'read' | 'write' | 'full';

  interface Grant {
    id: number;
    member: string;
    kind: Kind;
    resource: string;
    permission: Permission;
  }

  const KINDS: Kind[] = ['app', 'folder', 'share'];
  const PERMISSIONS: Permission[] = ['read', 'write', 'full'];

  let grants: Grant[] = [];
  let loading = true;
  let busy = false;
  let error = '';

  let newKind: Kind = 'app';
  let newResource = '';
  let newPermission: Permission = 'read';

  async function load() {
    loading = true;
    error = '';
    const res = await api.listResourceAccess(username);
    if (res.success) grants = (res.data as Grant[]) ?? [];
    else error = res.message || res.error || translate('users.access_load_failed');
    loading = false;
  }

  $: if (username) load();

  async function add() {
    if (!newResource.trim() || busy) return;
    busy = true;
    const res = await api.grantResourceAccess(username, newKind, newResource.trim(), newPermission);
    if (res.success) {
      newResource = '';
      await load();
    } else {
      error = res.message || res.error || translate('users.access_grant_failed');
    }
    busy = false;
  }

  async function remove(g: Grant) {
    busy = true;
    const res = await api.revokeResourceAccess(username, g.kind, g.resource);
    if (res.success) await load();
    else error = res.message || res.error || translate('users.access_revoke_failed');
    busy = false;
  }

  const kindLabel = (k: Kind) => translate(`users.access_kind_${k}`);
  const permLabel = (p: Permission) => translate(`users.access_perm_${p}`);
</script>

<SectionCard data-testid="users-access-card" divider>
  {#snippet titleSnippet()}
    <h2 class="text-c-card-title font-semibold text-[var(--hv-shell-text-strong)] flex items-center gap-2">
      <IconKey size={17} stroke={1.8} aria-hidden="true" /> {$t('users.access_title')}
    </h2>
  {/snippet}

  {#if isAdmin}
    <p data-testid="users-access-admin-note" class="m-0 text-c-base text-[var(--hv-shell-text)]">
      {$t('users.access_admin_note')}
    </p>
  {:else if loading}
    <div class="py-4 flex justify-center"><Spinner /></div>
  {:else}
    {#if error}
      <div role="alert" class="mb-3 px-3 py-2 rounded-md bg-[var(--hv-error-bg)] text-[var(--hv-error-text)] text-sm">{error}</div>
    {/if}

    {#if grants.length === 0}
      <p data-testid="users-access-empty" class="m-0 text-c-base text-[var(--hv-shell-text)]">
        {$t('users.access_empty')}
      </p>
    {:else}
      <ul data-testid="users-access-list" class="m-0 p-0 list-none flex flex-col gap-1.5">
        {#each grants as g (g.id)}
          <li data-testid="users-access-row" class="flex items-center gap-3 min-h-[44px] px-2 rounded-lg bg-[var(--hv-shell-fill)]">
            <span class="inline-flex items-center h-6 px-2 rounded-full bg-[var(--hv-primary-light)] text-[var(--hv-primary)] text-c-xs font-bold">{kindLabel(g.kind)}</span>
            <span class="flex-1 min-w-0 truncate font-[family-name:var(--hv-font-mono)] text-c-base text-[var(--hv-shell-text-strong)]">{g.resource}</span>
            <span class="text-c-base text-[var(--hv-shell-text-2)]">{permLabel(g.permission)}</span>
            <button
              type="button"
              class="hv-ic text-[var(--hv-error-text)]"
              aria-label={$t('users.access_revoke_for', { resource: g.resource })}
              disabled={busy}
              onclick={() => remove(g)}
            >
              <IconTrash size={17} stroke={1.8} />
            </button>
          </li>
        {/each}
      </ul>
    {/if}

    <form class="flex flex-wrap items-end gap-2 mt-4" onsubmit={(e) => { e.preventDefault(); add(); }}>
      <label class="flex flex-col gap-1 text-c-xs text-[var(--hv-shell-text-2)]">
        {$t('users.access_kind')}
        <select data-testid="users-access-kind" bind:value={newKind} class="h-9 px-2 rounded-md border border-[var(--hv-shell-border)] bg-[var(--hv-shell-card)] text-c-base text-[var(--hv-shell-text-strong)]">
          {#each KINDS as k}<option value={k}>{kindLabel(k)}</option>{/each}
        </select>
      </label>
      <label class="flex flex-col gap-1 text-c-xs text-[var(--hv-shell-text-2)] flex-1 min-w-[160px]">
        {$t('users.access_resource')}
        <input
          data-testid="users-access-resource"
          bind:value={newResource}
          placeholder={$t('users.access_resource_placeholder')}
          class="h-9 px-2 rounded-md border border-[var(--hv-shell-border)] bg-[var(--hv-shell-card)] text-c-base text-[var(--hv-shell-text-strong)]"
        />
      </label>
      <label class="flex flex-col gap-1 text-c-xs text-[var(--hv-shell-text-2)]">
        {$t('users.access_permission')}
        <select data-testid="users-access-permission" bind:value={newPermission} class="h-9 px-2 rounded-md border border-[var(--hv-shell-border)] bg-[var(--hv-shell-card)] text-c-base text-[var(--hv-shell-text-strong)]">
          {#each PERMISSIONS as p}<option value={p}>{permLabel(p)}</option>{/each}
        </select>
      </label>
      <button
        type="submit"
        data-testid="users-access-add"
        class={cn('inline-flex items-center gap-1.5 h-9 px-3 rounded-md bg-[var(--hv-primary)] text-white text-c-base font-semibold', busy && 'opacity-60')}
        disabled={busy || !newResource.trim()}
      >
        <IconPlus size={16} stroke={2} /> {$t('users.access_add')}
      </button>
    </form>
  {/if}
</SectionCard>
