<!-- ========================================================================
     File          : NotificationCenter.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       The bell panel: lists system notifications with relative timestamps
       (locale-aware), read/unread state, per-item dismiss and mark-all, and
       a live badge fed by the notifications store (30 s poll + SSE push).
       The canonical surface for system events; toasts are only ephemeral
       action feedback.

     Dependencies:
       - svelte (>= 5), date-fns, @tabler/icons-svelte,
         stores/notifications, stores/i18n

     Usage:
       Mounted by the desktop shell's topbar bell.
     ======================================================================== -->
<script lang="ts">
  import { notificationsStore, type Notification } from '$lib/stores/notifications';
  import { api } from '$lib/services/api';
  import { formatDistanceToNow } from 'date-fns';
  import { fr, de, es, enUS } from 'date-fns/locale';
  import { IconCheck, IconX } from '@tabler/icons-svelte';
  import { Icon } from '$lib/components/theme';
  import NotificationBellIcon from '$lib/components/shared/NotificationBellIcon.svelte';
  import { t, languageStore } from '$lib/stores/i18n';
  import { get } from 'svelte/store';
  import { fade } from 'svelte/transition';

  /** date-fns locale matching the active UI language (English fallback). */
  const DATE_LOCALES: Record<string, typeof enUS> = { fr, de, es, en: enUS };
  $: dateLocale = DATE_LOCALES[$languageStore] ?? enUS;

  /** Whether the notification panel is currently visible */
  export let isOpen = false;

  /**
   * Persistent-centre filters. The panel is not just a toast list — it keeps
   * the full server-side history, so it needs the same affordances as any
   * inbox: severity, unread-only and a text search.
   */
  let severityFilter: 'all' | 'info' | 'warning' | 'error' = 'all';
  let unreadOnly = false;
  let query = '';

  const SEVERITY_FILTERS: Array<{ id: 'all' | 'info' | 'warning' | 'error'; label: string }> = [
    { id: 'all', label: 'notifications.filter_all' },
    { id: 'info', label: 'notifications.filter_info' },
    { id: 'warning', label: 'notifications.filter_warning' },
    { id: 'error', label: 'notifications.filter_error' },
  ];

  /** The list after applying severity / unread / search filters. */
  $: filteredNotifications = $notificationsStore.notifications.filter((n) => {
    if (unreadOnly && n.read) return false;
    if (severityFilter !== 'all' && n.type !== severityFilter) return false;
    if (query.trim()) {
      const q = query.trim().toLowerCase();
      if (!n.title.toLowerCase().includes(q) && !n.message.toLowerCase().includes(q)) {
        return false;
      }
    }
    return true;
  });

  function getTypeColor(type: Notification['type']): string {
    switch (type) {
      case 'success': return 'var(--hv-success)';
      case 'error': return 'var(--hv-error)';
      case 'warning': return 'var(--hv-warning)';
      default: return 'var(--hv-primary)';
    }
  }

  /** Read highlight on the light panel — severity is carried by the left
     border and the icon colour, so the unread fill is a uniform, slightly
     raised white tint (matching the light solid shells). */
  function getTypeBg(_type: Notification['type']): string {
    return 'var(--hv-primary-selected-bg)';
  }

  function getTypeIcon(type: Notification['type']): string {
    switch (type) {
      case 'success': return 'check_circle';
      case 'error': return 'error';
      case 'warning': return 'warning';
      default: return 'info';
    }
  }

  function handleMarkAsRead(id: string) {
    notificationsStore.markAsRead(id);
    api.markNotificationRead(id);
  }

  function handleRemove(id: string) {
    notificationsStore.remove(id);
    api.deleteNotification(id);
  }

  function handleMarkAllAsRead() {
    notificationsStore.markAllAsRead();
    api.markAllNotificationsRead();
  }

  function handleClearAll() {
    const ids = get(notificationsStore).notifications.map(n => n.id);
    notificationsStore.clear();
    for (const id of ids) api.deleteNotification(id);
  }
</script>

{#if isOpen}
  <div class="notifications-panel hv-topbar-panel">
    <div class="panel-header">
      <h3>{$t('notifications.title')}</h3>
      <div class="header-actions">
        {#if $notificationsStore.unreadCount > 0}
          <button class="action-link" onclick={handleMarkAllAsRead}>
            {$t('notifications.mark_all_read')}
          </button>
        {/if}
        {#if $notificationsStore.notifications.length > 0}
          <button class="action-link" onclick={handleClearAll}>
            {$t('notifications.clear_all')}
          </button>
        {/if}
      </div>
    </div>

    <div class="panel-content">
      <div class="center-filters">
        <div class="severity-filters" role="group" aria-label={$t('notifications.filter_severity')}>
          {#each SEVERITY_FILTERS as f (f.id)}
            <button
              type="button"
              class="severity-chip"
              class:active={severityFilter === f.id}
              aria-pressed={severityFilter === f.id}
              onclick={() => (severityFilter = f.id)}
            >
              {$t(f.label)}
            </button>
          {/each}
        </div>
        <div class="center-filters-row">
          <input
            type="search"
            class="center-search"
            bind:value={query}
            placeholder={$t('notifications.search_placeholder')}
            aria-label={$t('notifications.search_placeholder')}
          />
          <label class="unread-toggle">
            <input type="checkbox" bind:checked={unreadOnly} />
            <span>{$t('notifications.unread_only')}</span>
          </label>
        </div>
      </div>

      {#if $notificationsStore.notifications.length === 0}
        <div class="empty-state">
          <NotificationBellIcon size={48} stroke={2} />
          <p>{$t('notifications.empty')}</p>
        </div>
      {:else if filteredNotifications.length === 0}
        <div class="empty-state" data-testid="notifications-no-match">
          <p>{$t('notifications.no_match')}</p>
        </div>
      {:else}
        {#each filteredNotifications as notification (notification.id)}
          <div
            class="notification-item"
            class:unread={!notification.read}
            style="border-left-color: {getTypeColor(notification.type)}; background: {notification.read ? 'var(--hv-shell-card)' : getTypeBg(notification.type)};"
            transition:fade={{ duration: 150 }}
          >
            <div class="notification-icon" style="color: {getTypeColor(notification.type)};">
              <Icon name={getTypeIcon(notification.type)} size={24} />
            </div>
            <div class="notification-content">
              <div class="notification-title">{notification.title}</div>
              <div class="notification-message">{notification.message}</div>
              <div class="notification-time">
                {formatDistanceToNow(notification.timestamp, { addSuffix: true, locale: dateLocale })}
              </div>
            </div>
            <div class="notification-actions">
              {#if !notification.read}
                <button
                  class="icon-btn hv-ic"
                  onclick={() => handleMarkAsRead(notification.id)}
                  title={$t('notifications.mark_read')}
                  aria-label={$t('notifications.mark_read')}
                >
                  <IconCheck size={18} stroke={2} />
                </button>
              {/if}
              <button
                class="icon-btn hv-ic"
                onclick={() => handleRemove(notification.id)}
                title={$t('common.delete')}
                aria-label={$t('common.delete')}
              >
                <IconX size={18} stroke={2} />
              </button>
            </div>
          </div>
        {/each}
      {/if}
    </div>
  </div>
{/if}

<style>
  .notifications-panel {
    /* Placement, width clamp, and the whole solid-white card chrome
       (background, border, blur, shadow, radius, entrance animation) come
       from the shared `.hv-topbar-panel` class (theme/components.css) — this
       rule only owns what is specific to a scrollable list panel. */
    max-height: 560px;
    display: flex;
    flex-direction: column;
  }

  .panel-header {
    padding: var(--hv-space-3) var(--hv-space-3) var(--hv-space-3) var(--hv-space-6);
    background: var(--hv-shell-fill);
    border-bottom: 1px solid var(--hv-shell-border-soft);
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: var(--hv-space-3);
  }

  .panel-header h3 {
    font-size: var(--hv-font-size-lg);
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-shell-text-strong);
    margin: 0;
    flex: 1;
  }

  .header-actions {
    display: flex;
    gap: var(--hv-space-2);
    flex-shrink: 0;
  }

  .action-link {
    min-height: var(--hv-touch-target-min);
    padding: 0 16px;
    background: var(--hv-shell-fill);
    border: 1px solid var(--hv-shell-border);
    border-radius: var(--hv-radius-control);
    color: var(--hv-shell-text-strong);
    font-size: var(--hv-font-size-sm);
    font-weight: var(--hv-font-weight-semibold);
    cursor: pointer;
    transition: background 150ms;
  }

  .action-link:hover {
    background: var(--hv-shell-border-soft);
  }

  .panel-content {
    flex: 1;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: var(--hv-space-2-5);
    padding: 16px;
  }

  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: var(--hv-space-14) var(--hv-space-5);
    color: var(--hv-shell-text-2);
  }

  .center-filters {
    display: flex;
    flex-direction: column;
    gap: var(--hv-space-2);
    padding-bottom: var(--hv-space-2);
    border-bottom: 1px solid var(--hv-shell-border-soft);
  }

  .severity-filters {
    display: flex;
    flex-wrap: wrap;
    gap: var(--hv-space-1-5);
  }

  /* Chip buttons are real buttons at the 44px shared hit-target minimum so
     they stay reachable with touch/pen/keyboard, not just a mouse. */
  .severity-chip {
    min-height: var(--hv-touch-target-min);
    padding: 0 var(--hv-space-3);
    background: transparent;
    border: 1px solid var(--hv-shell-border);
    border-radius: var(--hv-radius-full);
    color: var(--hv-shell-text-2);
    font-size: var(--hv-font-size-sm);
    font-weight: var(--hv-font-weight-semibold);
    cursor: pointer;
  }

  .severity-chip:hover {
    background: var(--hv-shell-border-soft);
    color: var(--hv-shell-text-strong);
  }

  .severity-chip.active {
    background: var(--hv-primary);
    border-color: var(--hv-primary);
    color: var(--hv-on-primary, #fff);
  }

  .severity-chip:focus-visible {
    outline: none;
    box-shadow: var(--hv-focus-ring);
  }

  .center-filters-row {
    display: flex;
    align-items: center;
    gap: var(--hv-space-2);
    flex-wrap: wrap;
  }

  .center-search {
    flex: 1;
    min-width: 8rem;
    min-height: var(--hv-touch-target-min);
    padding: 0 var(--hv-space-3);
    background: var(--hv-shell-fill);
    border: 1px solid var(--hv-shell-border);
    border-radius: var(--hv-radius-control);
    color: var(--hv-shell-text-strong);
    font-size: var(--hv-font-size-sm);
  }

  .center-search:focus-visible {
    outline: none;
    box-shadow: var(--hv-focus-ring);
  }

  .unread-toggle {
    display: inline-flex;
    align-items: center;
    gap: var(--hv-space-1-5);
    min-height: var(--hv-touch-target-min);
    color: var(--hv-shell-text-2);
    font-size: var(--hv-font-size-sm);
    cursor: pointer;
  }

  .empty-state :global(svg) {
    color: var(--hv-shell-border-strong);
    margin-bottom: var(--hv-space-3);
  }

  .notification-item {
    display: flex;
    gap: var(--hv-space-3);
    padding: 16px;
    border-radius: var(--hv-radius-2xl);
    border-left: 4px solid transparent;
    transition: background 150ms, border-left-color 150ms;
  }

  .notification-item:not(.unread):hover {
    background: var(--hv-shell-border-soft) !important;
  }

  .notification-item.unread .notification-title {
    font-weight: var(--hv-font-weight-bold);
  }

  .notification-icon {
    flex-shrink: 0;
  }

  .notification-content {
    flex: 1;
    min-width: 0;
  }

  .notification-title {
    font-size: var(--hv-font-size-base);
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-shell-text-strong);
    margin-bottom: var(--hv-space-1);
  }

  .notification-message {
    font-size: var(--hv-font-size-sm);
    color: var(--hv-shell-text-2);
    margin-bottom: var(--hv-space-1);
    word-wrap: break-word;
  }

  .notification-time {
    font-size: var(--hv-font-size-xs);
    color: var(--hv-shell-text-3);
  }

  .notification-actions {
    display: flex;
    gap: var(--hv-space-1);
    flex-shrink: 0;
  }

  .icon-btn {
    width: 28px;
    height: 28px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    border-radius: var(--hv-radius-control);
    color: var(--hv-shell-text-2);
    cursor: pointer;
    transition:
      color var(--hv-transition-fast) var(--hv-ease-default),
      background-color var(--hv-transition-fast) var(--hv-ease-default);
  }

  .icon-btn:hover {
    background: var(--hv-shell-border);
    color: var(--hv-shell-text-strong);
  }
</style>
