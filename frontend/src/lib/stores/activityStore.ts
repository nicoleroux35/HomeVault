/* =============================================================================
 * File          : activityStore.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Desktop background-activity feed: polls GET /api/activity (~2 s) while
 *   the tab is visible and the user is logged in, exposing the task list
 *   and count for the dock's activity button and its taskbar panel. A
 *   readable store with start/stop, so the poll only runs while the desktop
 *   shell is mounted; tolerates one transient failure to avoid flicker.
 *
 * Dependencies:
 *   - svelte (>= 5)
 *
 * Usage:
 *   `import { activity } from '$lib/stores/activityStore'` in the desktop
 *   shell / taskbar.
 * =============================================================================
 */

/**
 * Desktop background-activity feed.
 *
 * Polls `GET /api/activity` (~2 s) while the tab is visible and the user is
 * logged in, and exposes the current task list + count for the dock's activity
 * button and the taskbar panel above the dock.
 *
 * It is a `readable` with a start/stop notifier, so the poll only runs while a
 * component is actually subscribed (i.e. while the desktop shell is mounted).
 */
import { readable } from 'svelte/store';
import { api, type ActivityTask } from '$lib/services/api';
import { authStore } from './auth';

export interface ActivityState {
  tasks: ActivityTask[];
  count: number;
}

const EMPTY: ActivityState = { tasks: [], count: 0 };
const POLL_MS = 2000;

export const activity = readable<ActivityState>(EMPTY, (set) => {
  let timer: ReturnType<typeof setInterval> | undefined;
  let stopped = false;
  let consecutiveFailures = 0;
  let loggedIn = false;

  const unsubAuth = authStore.subscribe((s) => {
    loggedIn = !!s.user;
  });

  async function poll() {
    if (stopped) return;
    if (typeof document !== 'undefined' && document.visibilityState !== 'visible') return;
    if (!loggedIn) {
      set(EMPTY);
      return;
    }
    try {
      const res = await api.getActivity();
      if (res.success && Array.isArray(res.data)) {
        consecutiveFailures = 0;
        set({ tasks: res.data, count: res.data.length });
      } else {
        set(EMPTY);
      }
    } catch {
      // Tolerate a single transient miss (keeps the panel from flickering);
      // clear on a second failure.
      if (++consecutiveFailures >= 2) set(EMPTY);
    }
  }

  function onVisibilityChange() {
    if (document.visibilityState === 'visible') poll();
  }

  poll();
  timer = setInterval(poll, POLL_MS);
  if (typeof document !== 'undefined') {
    document.addEventListener('visibilitychange', onVisibilityChange);
  }

  return () => {
    stopped = true;
    if (timer) clearInterval(timer);
    if (typeof document !== 'undefined') {
      document.removeEventListener('visibilitychange', onVisibilityChange);
    }
    unsubAuth();
  };
});
