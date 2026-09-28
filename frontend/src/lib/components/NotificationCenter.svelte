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
      {#if $notificationsStore.notifications.length === 0}
        <div class="empty-state">
          <NotificationBellIcon size={48} stroke={2} />
          <p>{$t('notifications.empty')}</p>
        </div>
      {:else}
        {#each $notificationsStore.notifications as notification (notification.id)}
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
    border-radius: 20px;
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

  .empty-state :global(svg) {
    color: var(--hv-shell-border-strong);
    margin-bottom: var(--hv-space-3);
  }

  .notification-item {
    display: flex;
    gap: var(--hv-space-3);
    padding: 16px;
    border-radius: 20px;
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
    border-radius: var(--hv-radius-sm);
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
