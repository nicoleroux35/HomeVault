/* =============================================================================
 * File          : appGate.ts
 * Author        : Nicolas Le Roux
 * Created       : 12/10/2026
 * Last modified : 12/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Application-level route guard: the single place deciding whether the
 *   shell shows the Setup Wizard, the hardware-change recovery screen, the
 *   login form, the desktop or a connection-error state. The server is the
 *   source of truth (first run = no admin account in the database), and the
 *   hardware-fingerprint mismatch gets a dedicated recovery route instead
 *   of the wizard, which the server would refuse.
 *
 * Dependencies:
 *   - svelte (>= 5)
 *
 * Usage:
 *   appGate.refresh() at boot and after logout/enterApp on success;
 *   App.svelte renders the matching route.
 * =============================================================================
 */

import { get, writable } from 'svelte/store';
import type { SetupStatus } from '$lib/types/api';
import { api } from '$lib/services/api';
import { authStore } from '$lib/stores/auth';
import { languageStore } from '$lib/stores/i18n';

/**
 * Application-level route guard.
 *
 * Route gate: a single place decides whether the shell shows the
 * Setup Wizard, the hardware-change recovery screen, the login form, the
 * desktop, or a connection-error state. It replaces the previous design where
 * `Login.svelte` fetched `/auth/setup-status` itself and rendered
 * `<SetupWizard>` inline — which dead-ended on `hardware_changed` (the wizard
 * was shown, but `/auth/setup` correctly refused with "Setup already
 * complete").
 *
 * The server is the source of truth: `is_first_run` is derived from the
 * existence of an admin account in the database, never from a marker file.
 */
export type GateRoute =
  | 'loading'
  | 'setup'
  | 'recovery'
  | 'login'
  | 'app'
  | 'password-change'
  | 'error';

export interface GateState {
  route: GateRoute;
  status: SetupStatus | null;
}

function createAppGate() {
  const { subscribe, set } = writable<GateState>({ route: 'loading', status: null });

  /** Re-evaluate the gate from the server's setup status and the local session. */
  async function refresh() {
    set({ route: 'loading', status: null });

    let status: SetupStatus;
    try {
      status = await api.getSetupStatus();
    } catch {
      set({ route: 'error', status: null });
      return;
    }

    // The host's own POSIX locale is the last language hint (see
    // `languageStore.applySystemLanguage`): it only applies when the user made
    // no explicit choice and the browser locale is not one we ship. The wizard
    // no longer asks for a language — Settings is where a choice is stored.
    languageStore.applySystemLanguage(status.system_language);

    // First run: nobody has claimed the appliance yet — the wizard owns it.
    if (status.is_first_run) {
      set({ route: 'setup', status });
      return;
    }

    // An admin exists but the recorded hardware fingerprint no longer matches.
    // The wizard cannot run (the server refuses a second admin), so this goes
    // to the dedicated recovery screen instead of a dead-end.
    if (status.needs_recovery) {
      set({ route: 'recovery', status });
      return;
    }

    await authStore.checkAuth();
    // A session windowed down to the password-change flow (admin-chosen
    // password, migration 049) may not reach the desktop: before anything
    // else the owner sets their own password on the forced-change screen.
    if (get(authStore).mustChangePassword) {
      set({ route: 'password-change', status });
      return;
    }
    set({ route: get(authStore).isAuthenticated ? 'app' : 'login', status });
  }

  /** Enter the desktop after a confirmed login / setup / recovery. */
  function enterApp(status: SetupStatus | null = null) {
    set({ route: 'app', status });
  }

  /** Re-run the guard after a session ends (logout). */
  async function signOut() {
    set({ route: 'login', status: null });
  }

  /** The login flow found a windowed (`must-change-password`) session: the
   *  gate switches to the forced password-change screen, which reuses
   *  Login's gateway shell (`Login.svelte` `passwordChangeMode`). */
  function requirePasswordChange() {
    set({ route: 'password-change', status: null });
  }

  return { subscribe, refresh, enterApp, signOut, requirePasswordChange };
}

export const appGate = createAppGate();
