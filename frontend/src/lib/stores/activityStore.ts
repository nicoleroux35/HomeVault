/* =============================================================================
 * File          : activityStore.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 24/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Desktop background-activity feed: polls GET /api/activity while the tab
 *   is visible and the user is logged in, exposing the task list and the
 *   active count for the dock's activity button and its taskbar panel. A
 *   readable store with start/stop, so the poll only runs while the desktop
 *   shell is mounted; a failing backend is polled with exponential backoff
 *   instead of twice a second.
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
 * Polls `GET /api/activity` while the tab is visible and the user is logged
 * in, and exposes the current task list + active count for the dock's activity
 * button and the taskbar panel above the dock.
 *
 * It is a `readable` with a start/stop notifier, so the poll only runs while a
 * component is actually subscribed (i.e. while the desktop shell is mounted).
 */
import { readable } from 'svelte/store';
import { api, type ActivityTask } from '$lib/services/api';
import { authStore } from './auth';

export interface ActivityState {
  /** Every row to render, terminal failures included (see `ACTIVE_STATES`). */
  tasks: ActivityTask[];
  /** Rows still doing work — what the dock badge counts. */
  count: number;
}

const EMPTY: ActivityState = { tasks: [], count: 0 };

/** Base cadence: two polls a second is plenty for a progress bar. */
export const POLL_MS = 2000;
/**
 * Ceiling for the backoff. A backend that has been down for a minute is not
 * made better by being hit twice a second; one poll a minute still recovers
 * within a minute of it coming back (and any user interaction — the tab
 * becoming visible, a new task starting — resets the delay outright).
 */
export const POLL_MAX_MS = 60_000;
/**
 * A single transient miss is tolerated before the list is cleared: the panel
 * must not flicker because one request lost a race with a restart.
 */
export const FAILURE_TOLERANCE = 2;

/**
 * States that still represent work. A `failed`/`cancelled` row is displayed
 * (so the outcome is visible) but must not keep the dock badge alive, and a
 * `completed` row is not sent by the server at all.
 */
export const ACTIVE_STATES: ReadonlyArray<ActivityTask['state']> = ['pending', 'running'];

/** Number of rows that represent work in progress. */
export function activeCount(tasks: ActivityTask[]): number {
  return tasks.filter((t) => ACTIVE_STATES.includes(t.state)).length;
}

/**
 * Delay before the next poll, given how many polls in a row have failed.
 *
 * 2 s while healthy, then 4, 8, 16, 32, 60, 60 … — exponential, capped, with
 * **no jitter**: a single appliance with a handful of open desktops is not a
 * fleet that needs de-synchronising, and a deterministic series is what the
 * E2E assertion can pin.
 *
 * Before this existed the store used `setInterval(poll, 2000)` unconditionally,
 * so a backend that answered `500` was hit 30 times a minute per open desktop —
 * a self-inflicted load spike exactly when the box is already unwell
 * (ROADMAP 28.1).
 */
export function nextPollDelay(failures: number): number {
  if (failures <= 0) return POLL_MS;
  const backoff = POLL_MS * 2 ** Math.min(failures, 16);
  return Math.min(backoff, POLL_MAX_MS);
}

export const activity = readable<ActivityState>(EMPTY, (set) => {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let stopped = false;
  let consecutiveFailures = 0;
  let loggedIn = false;

  const unsubAuth = authStore.subscribe((s) => {
    loggedIn = !!s.user;
  });

  function schedule(delay: number) {
    if (stopped) return;
    if (timer) clearTimeout(timer);
    timer = setTimeout(poll, delay);
  }

  async function poll() {
    if (stopped) return;
    if (typeof document !== 'undefined' && document.visibilityState !== 'visible') {
      // A hidden tab keeps its timer but does not spend a request; the
      // visibility handler below resumes immediately when it comes back.
      schedule(POLL_MS);
      return;
    }
    if (!loggedIn) {
      set(EMPTY);
      consecutiveFailures = 0;
      schedule(POLL_MS);
      return;
    }
    try {
      const res = await api.getActivity();
      if (res.success && Array.isArray(res.data)) {
        consecutiveFailures = 0;
        set({ tasks: res.data, count: activeCount(res.data) });
      } else {
        // A well-formed refusal (e.g. the session went away) is not a
        // transport failure: clear the list and keep the normal cadence.
        consecutiveFailures = 0;
        set(EMPTY);
      }
    } catch {
      consecutiveFailures += 1;
      // Tolerate a single transient miss (keeps the panel from flickering);
      // clear on the second failure, and back off from there.
      if (consecutiveFailures >= FAILURE_TOLERANCE) set(EMPTY);
    }
    schedule(nextPollDelay(consecutiveFailures));
  }

  function onVisibilityChange() {
    if (document.visibilityState !== 'visible') return;
    // Coming back to the tab is a user action: poll now and forget the
    // backoff, so a panel opened after a long sleep is not stale for a minute.
    consecutiveFailures = 0;
    schedule(0);
  }

  poll();
  if (typeof document !== 'undefined') {
    document.addEventListener('visibilitychange', onVisibilityChange);
  }

  return () => {
    stopped = true;
    if (timer) clearTimeout(timer);
    if (typeof document !== 'undefined') {
      document.removeEventListener('visibilitychange', onVisibilityChange);
    }
    unsubAuth();
  };
});
