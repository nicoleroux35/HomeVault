/* =============================================================================
 * File          : notifications.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 23/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Client-side notification list for the bell panel: replaces its contents
 *   from the 30 s poll, accepts live SSE pushes keyed by the real server id
 *   (so the next poll recognizes them instead of duplicating), mints ids
 *   for client-local entries, and tracks the server-side pagination state
 *   (`total`/`offset`/`hasMore`) so the panel can load older rows
 *   (ROADMAP 21.3). Mutations are confirmed by the caller before they reach
 *   the store — the store itself stays a pure state holder.
 *
 * Dependencies:
 *   - svelte (>= 5)
 *
 * Usage:
 *   notificationsStore in NotificationCenter.svelte; fed by api.ts and
 *   services/websocket.ts.
 * =============================================================================
 */

import { writable } from 'svelte/store';

/** A single notification: shown in the notification center to inform the user. */
export interface Notification {
  id: string;
  title: string;
  message: string;
  type: 'info' | 'success' | 'warning' | 'error';
  timestamp: Date;
  read: boolean;
}

/** Shape of a notification row as returned by `GET /api/notifications`. */
export interface ServerNotification {
  id: string;
  title: string;
  message: string;
  severity: string;
  category: string;
  source: string;
  created_at: string;
  read_at: string | null;
}

/** The paginated envelope `GET /api/notifications` answers with (ROADMAP 21.3). */
export interface NotificationsPage {
  notifications: ServerNotification[];
  total: number;
  limit: number;
  offset: number;
}

interface NotificationsState {
  notifications: Notification[];
  unreadCount: number;
  /** Server-side total feed size, for the "load older" affordance. */
  total: number;
  /** Offset the currently-loaded window starts at (0 = newest page loaded). */
  offset: number;
  hasMore: boolean;
  loadingOlder: boolean;
}

const initialState: NotificationsState = {
  notifications: [],
  unreadCount: 0,
  total: 0,
  offset: 0,
  hasMore: false,
  loadingOlder: false,
};

function severityToType(severity: string): Notification['type'] {
  switch (severity) {
    case 'critical':
    case 'error': return 'error';
    case 'warning': return 'warning';
    default: return 'info';
  }
}

function rowsToNotifications(rows: ServerNotification[]): Notification[] {
  return rows.map(r => ({
    id: r.id,
    title: r.title,
    message: r.message,
    type: severityToType(r.severity),
    timestamp: new Date(r.created_at),
    read: r.read_at !== null
  }));
}

/** Safety ceiling for client-local rows only (SSE pushes, `add`): the
   server feed is unbounded and paginated, so loaded pages must never evict
   each other, but a long-lived session should not accumulate forever. */
const CLIENT_CAP = 500;

function capped(items: Notification[]): Notification[] {
  return items.slice(0, CLIENT_CAP);
}

function createNotificationsStore() {
  const { subscribe, update, set } = writable<NotificationsState>(initialState);

  return {
    subscribe,

    /**
     * Replace the store contents with the first page fetched from the
     * backend, preserving server ids. Resets the pagination window.
     */
    setFromServer(page: NotificationsPage) {
      const notifications = rowsToNotifications(page.notifications);
      set({
        notifications,
        unreadCount: notifications.filter(n => !n.read).length,
        total: page.total,
        offset: page.offset,
        hasMore: page.offset + page.notifications.length < page.total,
        loadingOlder: false,
      });
    },

    /**
     * Append an older page fetched through the "load older" button.
     * Dedupes by id (a row may have been pushed live since the first page)
     * and advances the window.
     */
    appendFromServer(page: NotificationsPage) {
      update(state => {
        const merged = rowsToNotifications(page.notifications);
        const existing = new Set(state.notifications.map(n => n.id));
        const fresh = merged.filter(n => !existing.has(n.id));
        const notifications = capped([...state.notifications, ...fresh]);
        return {
          notifications,
          unreadCount: notifications.filter(n => !n.read).length,
          total: page.total,
          offset: page.offset,
          hasMore: page.offset + page.notifications.length < page.total,
          loadingOlder: false,
        };
      });
    },

    /** Push a new notification (client-local event feedback, Settings etc.). */
    add(notification: Omit<Notification, 'id' | 'timestamp' | 'read'>) {
      update(state => {
        const newNotif: Notification = {
          ...notification,
          id: crypto.randomUUID(),
          timestamp: new Date(),
          read: false
        };

        return {
          ...state,
          notifications: capped([newNotif, ...state.notifications]),
          unreadCount: state.unreadCount + 1
        };
      });
    },

    /**
     * Inserts a single real server-backed row pushed live over the
     * `/api/events` SSE stream (see websocket.ts). Keyed by the row's real
     * id (unlike `add`, which mints a client-side id) so the next 30s poll
     * of `GET /api/notifications` recognizes it as the same notification
     * instead of duplicating it.
     */
    upsertFromServer(row: ServerNotification) {
      update(state => {
        if (state.notifications.some(n => n.id === row.id)) return state;
        const notif: Notification = {
          id: row.id,
          title: row.title,
          message: row.message,
          type: severityToType(row.severity),
          timestamp: new Date(row.created_at),
          read: row.read_at !== null
        };
        return {
          ...state,
          notifications: capped([notif, ...state.notifications]),
          unreadCount: state.unreadCount + (notif.read ? 0 : 1)
        };
      });
    },

    markAsRead(id: string) {
      update(state => ({
        ...state,
        notifications: state.notifications.map(n =>
          n.id === id ? { ...n, read: true } : n
        ),
        unreadCount: Math.max(0, state.unreadCount - 1)
      }));
    },

    markAllAsRead() {
      update(state => ({
        ...state,
        notifications: state.notifications.map(n => ({ ...n, read: true })),
        unreadCount: 0
      }));
    },

    /** Remove a single notification; adjusts the unread counter if it was unread. */
    remove(id: string) {
      update(state => {
        const notification = state.notifications.find(n => n.id === id);
        const unreadDelta = notification && !notification.read ? 1 : 0;

        return {
          ...state,
          notifications: state.notifications.filter(n => n.id !== id),
          unreadCount: Math.max(0, state.unreadCount - unreadDelta)
        };
      });
    },

    /** Dismiss every notification at once. */
    clear() {
      set({ ...initialState });
    },

    /** Marks an older-page fetch as in flight (disables the button). */
    setLoadingOlder(loading: boolean) {
      update(state => ({ ...state, loadingOlder: loading }));
    },
  };
}

export const notificationsStore = createNotificationsStore();
