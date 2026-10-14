/* =============================================================================
 * File          : websocket.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Live server-push client (SSE) for real-time notifications. Because a
 *   browser EventSource cannot send the session header, it first exchanges
 *   the session token for a short-lived, single-use ticket, then opens the
 *   stream with that ticket — a long-lived token never travels in a URL.
 *   Reconnects with exponential backoff (2 s to 30 s, 10 attempts) and
 *   exposes connection state through wsStore plus typed listeners.
 *
 * Dependencies:
 *   - svelte (>= 5)
 *
 * Usage:
 *   `import { websocket, wsStore } from '$lib/services/websocket'`; call
 *   websocket.connect() once after login, listen with websocket.on(...).
 * =============================================================================
 */

import { writable } from 'svelte/store';
import { notificationsStore, type ServerNotification } from '$lib/stores/notifications';
import { api } from './api';

/**
 * Tracks the SSE connection health and the most recent server-sent event.
 */
interface LiveState {
  connected: boolean;
  reconnecting: boolean;
  lastMessage: any;
  error: string | null;
}

const initialState: LiveState = {
  connected: false,
  reconnecting: false,
  lastMessage: null,
  error: null,
};

/** Exposed so components can react to connection-state changes. */
export const wsStore = writable<LiveState>(initialState);

/**
 * Server-Sent Events (SSE) client that receives real-time updates from the backend.
 * Automatically reconnects with exponential backoff up to 30 s.
 */
class LiveService {
  private es: EventSource | null = null;
  private reconnectTimer: number | null = null;
  private reconnectAttempts = 0;
  private maxReconnectAttempts = 10;
  /** Starting delay for exponential backoff (doubles + 50% each attempt). */
  private reconnectDelay = 2000;
  private listeners = new Map<string, Set<Function>>();

  /**
   * Open an SSE connection to the backend events endpoint. A browser
   * `EventSource` can't set the `X-Session-Token` header every other
   * request uses, so this first exchanges the real session token for a
   * short-lived, single-use ticket over a normal (headers-based) request,
   * then opens the stream with that ticket instead — the same shape as
   * the Terminal page's WebSocket ticket, deliberately avoiding a
   * long-lived token traveling in a URL query string.
   */
  async connect() {
    this.cleanup();

    let ticket: string | undefined;
    try {
      const result = await api.getNotificationsEventsTicket();
      ticket = result.success ? result.data?.ticket : undefined;
    } catch {
      ticket = undefined;
    }

    if (!ticket) {
      wsStore.update(s => ({ ...s, connected: false, error: 'Failed to obtain events ticket' }));
      this.scheduleReconnect();
      return;
    }

    const url = `/api/events?ticket=${encodeURIComponent(ticket)}`;

    try {
      this.es = new EventSource(url);
    } catch (error) {
      // Handle EventSource creation errors (invalid URL, etc.)
      wsStore.update(s => ({ ...s, connected: false, error: 'Failed to create connection' }));
      this.scheduleReconnect();
      return;
    }

    this.es.onopen = () => {
      this.reconnectAttempts = 0;
      wsStore.update(s => ({ ...s, connected: true, reconnecting: false, error: null }));
    };

    // Real-time notification push: instantly reflects a newly created
    // notification in the bell panel instead of waiting for the next 30s
    // poll of GET /api/notifications (still the source of truth/fallback).
    this.es.addEventListener('notification', (event: MessageEvent) => {
      try {
        const row: ServerNotification = JSON.parse(event.data);
        wsStore.update(s => ({ ...s, lastMessage: row }));
        this.emit('notification', row);
        notificationsStore.upsertFromServer(row);
      } catch {
        // Malformed payload; the next poll will still pick it up.
      }
    });

    this.es.onerror = (error) => {
      // Properly close the EventSource on error before reconnecting
      if (this.es) {
        this.es.close();
        this.es = null;
      }
      wsStore.update(s => ({ ...s, connected: false }));
      this.scheduleReconnect();
    };
  }

  /**
   * Schedule a reconnection attempt with exponential backoff
   * (2 s → 3 s → 4.5 s → … → capped at 30 s, up to 10 attempts).
   */
  private scheduleReconnect() {
    if (this.reconnectTimer) return;
    if (this.reconnectAttempts >= this.maxReconnectAttempts) {
      wsStore.update(s => ({ ...s, error: 'Connection lost' }));
      return;
    }

    this.reconnectAttempts++;
    wsStore.update(s => ({ ...s, reconnecting: true }));

    const delay = Math.min(
      this.reconnectDelay * Math.pow(1.5, this.reconnectAttempts - 1),
      30000,
    );

    this.reconnectTimer = window.setTimeout(() => {
      this.reconnectTimer = null;
      this.connect();
    }, delay);
  }

  /** Close the current SSE connection and clear any pending reconnect timer. */
  private cleanup() {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    if (this.es) {
      this.es.onopen = null;
      this.es.onmessage = null;
      this.es.onerror = null;
      this.es.close();
      this.es = null;
    }
  }

  /** Register a callback for a specific event type. */
  on(event: string, callback: Function) {
    if (!this.listeners.has(event)) {
      this.listeners.set(event, new Set());
    }
    this.listeners.get(event)!.add(callback);
  }

  /** Remove a previously registered callback. */
  off(event: string, callback: Function) {
    this.listeners.get(event)?.delete(callback);
  }

  /** Notify all registered listeners for the given event type. */
  private emit(event: string, data: any) {
    this.listeners.get(event)?.forEach(cb => {
      try {
        cb(data);
      } catch (error) {
        console.error(`Error in listener for ${event}:`, error);
      }
    });
  }

  /** Force-disconnect and prevent any further reconnection attempts. */
  disconnect() {
    this.reconnectAttempts = 999;
    this.cleanup();
    wsStore.update(s => ({ ...s, connected: false }));
  }
}

export const websocket = new LiveService();
