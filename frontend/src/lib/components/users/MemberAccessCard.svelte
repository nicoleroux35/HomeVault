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
  import { IconKey, IconPlus, IconTrash, IconUsers } from '@tabler/icons-svelte';
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

  /** A grant held by a group (migration 051). Every member of that group
   *  inherits it — the card only edits it, the backend resolves the union. */
  interface GroupGrant {
    id: number;
    group_name: string;
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

  // Group-inherited grants: the member's own groups, the grants those groups
  // hold, and a small editor so the admin can manage them from the same card.
  let memberGroups: string[] = [];
  let groupGrants: GroupGrant[] = [];
  let groupsLoading = true;
  let groupError = '';
  let groupBusy = false;
  let newGroup = '';
  let newGroupKind: Kind = 'app';
  let newGroupResource = '';
  let newGroupPermission: Permission = 'read';

  async function load() {
    loading = true;
    error = '';
    const res = await api.listResourceAccess(username);
    if (res.success) grants = (res.data as Grant[]) ?? [];
    else error = res.message || res.error || translate('users.access_load_failed');
    loading = false;
  }

  $: if (username) load();

  async function loadGroups() {
    groupsLoading = true;
    groupError = '';
    const [mine, all] = await Promise.all([
      api.getUserGroups(username),
      api.listGroupResourceAccess(),
    ]);
    groupsLoading = false;
    if (!mine.success) {
      groupError = mine.message || mine.error || translate('users.access_group_load_failed');
      return;
    }
    memberGroups = (mine.data as string[]) ?? [];
    if (!newGroup && memberGroups.length > 0) newGroup = memberGroups[0];
    if (all.success) {
      const rows = (all.data as GroupGrant[]) ?? [];
      // Only the grants this member actually inherits — a group they are not
      // in is none of their business, even on the admin screen.
      groupGrants = rows.filter((g) => memberGroups.includes(g.group_name));
    } else {
      groupError = all.message || all.error || translate('users.access_group_load_failed');
    }
  }

  // Nothing to inherit for an admin target (admins bypass the filter), and the
  // section is not rendered — do not pay for two API calls.
  $: if (username && !isAdmin) loadGroups();

  async function addGroupGrant() {
    if (!newGroup || !newGroupResource.trim() || groupBusy) return;
    groupBusy = true;
    const res = await api.grantGroupResourceAccess(
      newGroup,
      newGroupKind,
      newGroupResource.trim(),
      newGroupPermission,
    );
    if (res.success) {
      newGroupResource = '';
      await loadGroups();
    } else {
      groupError = res.message || res.error || translate('users.access_group_grant_failed');
    }
    groupBusy = false;
  }

  async function removeGroupGrant(g: GroupGrant) {
    groupBusy = true;
    const res = await api.revokeGroupResourceAccess(g.group_name, g.kind, g.resource);
    if (res.success) await loadGroups();
    else groupError = res.message || res.error || translate('users.access_group_revoke_failed');
    groupBusy = false;
  }

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

  {#if !isAdmin}
    <div class="mt-5 border-t border-[var(--hv-shell-border-soft)] pt-4" data-testid="users-access-groups">
      <h3 class="m-0 flex items-center gap-2 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">
        <IconUsers size={16} stroke={1.8} aria-hidden="true" /> {$t('users.access_group_title')}
      </h3>
      <p class="m-0 mt-1 text-c-xs text-[var(--hv-shell-text)]">{$t('users.access_group_hint')}</p>

      {#if groupError}
        <div role="alert" data-testid="users-access-group-err" class="mt-2 px-3 py-2 rounded-md bg-[var(--hv-error-bg)] text-[var(--hv-error-text)] text-sm">{groupError}</div>
      {/if}

      {#if groupsLoading}
        <div class="py-3 flex justify-center"><Spinner /></div>
      {:else if memberGroups.length === 0}
        <p data-testid="users-access-group-empty" class="m-0 mt-2 text-c-base text-[var(--hv-shell-text)]">
          {$t('users.access_group_no_groups')}
        </p>
      {:else}
        {#if groupGrants.length > 0}
          <ul data-testid="users-access-group-list" class="m-0 mt-2 p-0 list-none flex flex-col gap-1.5">
            {#each groupGrants as g (g.id)}
              <li data-testid="users-access-group-row" class="flex items-center gap-3 min-h-[44px] px-2 rounded-lg bg-[var(--hv-shell-fill)]">
                <span class="inline-flex items-center h-6 px-2 rounded-full bg-[var(--hv-shell-border)] text-[var(--hv-shell-text-2)] text-c-xs font-bold">{g.group_name}</span>
                <span class="inline-flex items-center h-6 px-2 rounded-full bg-[var(--hv-primary-light)] text-[var(--hv-primary)] text-c-xs font-bold">{kindLabel(g.kind)}</span>
                <span class="flex-1 min-w-0 truncate font-[family-name:var(--hv-font-mono)] text-c-base text-[var(--hv-shell-text-strong)]">{g.resource}</span>
                <span class="text-c-base text-[var(--hv-shell-text-2)]">{permLabel(g.permission)}</span>
                <button
                  type="button"
                  class="hv-ic text-[var(--hv-error-text)]"
                  aria-label={$t('users.access_group_revoke_for', { resource: g.resource, group: g.group_name })}
                  disabled={groupBusy}
                  onclick={() => removeGroupGrant(g)}
                >
                  <IconTrash size={17} stroke={1.8} />
                </button>
              </li>
            {/each}
          </ul>
        {:else}
          <p data-testid="users-access-group-empty" class="m-0 mt-2 text-c-base text-[var(--hv-shell-text)]">
            {$t('users.access_group_empty')}
          </p>
        {/if}

        <form class="flex flex-wrap items-end gap-2 mt-3" onsubmit={(e) => { e.preventDefault(); addGroupGrant(); }}>
          <label class="flex flex-col gap-1 text-c-xs text-[var(--hv-shell-text-2)]">
            {$t('users.access_group_label')}
            <select data-testid="users-access-group-select" bind:value={newGroup} class="h-9 px-2 rounded-md border border-[var(--hv-shell-border)] bg-[var(--hv-shell-card)] text-c-base text-[var(--hv-shell-text-strong)]">
              {#each memberGroups as g}<option value={g}>{g}</option>{/each}
            </select>
          </label>
          <label class="flex flex-col gap-1 text-c-xs text-[var(--hv-shell-text-2)]">
            {$t('users.access_kind')}
            <select data-testid="users-access-group-kind" bind:value={newGroupKind} class="h-9 px-2 rounded-md border border-[var(--hv-shell-border)] bg-[var(--hv-shell-card)] text-c-base text-[var(--hv-shell-text-strong)]">
              {#each KINDS as k}<option value={k}>{kindLabel(k)}</option>{/each}
            </select>
          </label>
          <label class="flex flex-col gap-1 text-c-xs text-[var(--hv-shell-text-2)] flex-1 min-w-[160px]">
            {$t('users.access_resource')}
            <input
              data-testid="users-access-group-resource"
              bind:value={newGroupResource}
              placeholder={$t('users.access_resource_placeholder')}
              class="h-9 px-2 rounded-md border border-[var(--hv-shell-border)] bg-[var(--hv-shell-card)] text-c-base text-[var(--hv-shell-text-strong)]"
            />
          </label>
          <label class="flex flex-col gap-1 text-c-xs text-[var(--hv-shell-text-2)]">
            {$t('users.access_permission')}
            <select data-testid="users-access-group-permission" bind:value={newGroupPermission} class="h-9 px-2 rounded-md border border-[var(--hv-shell-border)] bg-[var(--hv-shell-card)] text-c-base text-[var(--hv-shell-text-strong)]">
              {#each PERMISSIONS as p}<option value={p}>{permLabel(p)}</option>{/each}
            </select>
          </label>
          <button
            type="submit"
            data-testid="users-access-group-add"
            class={cn('inline-flex items-center gap-1.5 h-9 px-3 rounded-md bg-[var(--hv-primary)] text-white text-c-base font-semibold', groupBusy && 'opacity-60')}
            disabled={groupBusy || !newGroupResource.trim() || !newGroup}
          >
            <IconPlus size={16} stroke={2} /> {$t('users.access_group_add')}
          </button>
        </form>
      {/if}
    </div>
  {/if}
</SectionCard>
