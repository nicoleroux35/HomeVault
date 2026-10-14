/* =============================================================================
 * File          : notifications.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Client-side notification list for the bell panel: replaces its contents
 *   from the 30 s poll, accepts live SSE pushes keyed by the real server id
 *   (so the next poll recognizes them instead of duplicating), mints ids
 *   for client-local entries, caps the list at 50 and keeps the unread
 *   counter consistent through every mutation.
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

interface NotificationsState {
  notifications: Notification[];
  unreadCount: number;
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

const initialState: NotificationsState = {
  notifications: [],
  unreadCount: 0
};

function severityToType(severity: string): Notification['type'] {
  switch (severity) {
    case 'critical':
    case 'error': return 'error';
    case 'warning': return 'warning';
    default: return 'info';
  }
}

function createNotificationsStore() {
  const { subscribe, update, set } = writable<NotificationsState>(initialState);

  return {
    subscribe,

    /** Replace the store contents with rows fetched from the backend, preserving server ids. */
    setFromServer(rows: ServerNotification[]) {
      const notifications: Notification[] = rows.map(r => ({
        id: r.id,
        title: r.title,
        message: r.message,
        type: severityToType(r.severity),
        timestamp: new Date(r.created_at),
        read: r.read_at !== null
      }));
      set({
        notifications,
        unreadCount: notifications.filter(n => !n.read).length
      });
    },

    /** Push a new notification. The list is capped at 50 items to avoid memory bloat. */
    add(notification: Omit<Notification, 'id' | 'timestamp' | 'read'>) {
      update(state => {
        const newNotif: Notification = {
          ...notification,
          id: crypto.randomUUID(),
          timestamp: new Date(),
          read: false
        };

        return {
          notifications: [newNotif, ...state.notifications].slice(0, 50),
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
          notifications: [notif, ...state.notifications].slice(0, 50),
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
          notifications: state.notifications.filter(n => n.id !== id),
          unreadCount: Math.max(0, state.unreadCount - unreadDelta)
        };
      });
    },

    /** Dismiss every notification at once. */
    clear() {
      update(() => initialState);
    }
  };
}

export const notificationsStore = createNotificationsStore();
