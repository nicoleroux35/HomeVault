import { writable, derived } from 'svelte/store';
import type { User } from '$lib/types/api';
import { api } from '$lib/services/api';

/** Tracks the current user session and authentication lifecycle. */
interface AuthState {
  user: User | null;
  isAuthenticated: boolean;
  isLoading: boolean;
}

function createAuthStore() {
  const { subscribe, set, update } = writable<AuthState>({
    user: null,
    isAuthenticated: false,
    isLoading: true,
  });

  return {
    subscribe,
    set,

    /**
     * Authenticate with username/password (and optional 2FA code).
     * On success the API tokens are stored and the UI transitions to authenticated.
     * When the server responds with `requires_2fa` the caller should prompt for a TOTP.
     */
    async login(username: string, password: string, totpCode?: string, rememberMe = false) {
      update(state => ({ ...state, isLoading: true }));

      // Accounts with 2FA enabled must complete login through the dedicated
      // verification endpoint once a TOTP code is available; `/auth/login`
      // has no way to accept one and would just report `requires_2fa` again.
      const response = totpCode
        ? await api.loginWith2FA(username, password, totpCode, rememberMe)
        : await api.login({ username, password, remember_me: rememberMe });

      if (response.success && response.data) {
        // This login page is administrators-only (see the "Access Denied" /
        // login.error.not_admin handling in Login.svelte) — but the backend
        // has no separate concept of a "web-eligible" account, so a
        // non-admin username/password (or 2FA) pair still authenticates a
        // real, valid session server-side. That session must never be
        // persisted or left dangling: invalidate it immediately via the
        // real /auth/logout call (which needs the token to know which
        // session to kill) rather than just skipping the client-side
        // redirect, otherwise a page reload would pick the still-valid
        // localStorage-free-but-server-real session back up via
        // checkAuth() and let the non-admin user into the desktop anyway.
        if (!response.data.is_admin) {
          if (response.data.token) {
            api.setTokens(response.data.token, response.data.csrf_token || '');
            await api.logout();
          }

          set({
            user: null,
            isAuthenticated: false,
            isLoading: false,
          });

          return { success: true, isAdmin: false };
        }

        const user: User = {
          username: response.data.username,
          is_admin: response.data.is_admin,
        };

        // Persist tokens BEFORE updating the auth store. Stores subscribed to
        // authStore (e.g. monitorStore/weatherLocation) trigger server fetches
        // as soon as `user` becomes non-null; if tokens aren't set yet those
        // requests go out without a session token and fail with 400. Setting
        // tokens first guarantees the session is available for those fetches.
        if (response.data.token) {
          api.setTokens(response.data.token, response.data.csrf_token || '');
        }

        // Fetch full user profile (includes avatar_seed, avatar_url, etc.)
        // so the store reflects the complete user object immediately instead
        // of only having username/is_admin until the next page reload.
        const profile = await api.getCurrentUser();
        const fullUser = profile.success && profile.data ? profile.data : user;

        set({
          user: fullUser,
          isAuthenticated: user.is_admin,
          isLoading: false,
        });

        return { success: true, isAdmin: response.data.is_admin };
      } else if (response.data?.requires_2fa) {
        // Keep the store in a non-authenticated state; the UI will show the 2FA form
        update(state => ({ ...state, isLoading: false }));
        return { success: false, requires2fa: true };
      } else {
        set({
          user: null,
          isAuthenticated: false,
          isLoading: false,
        });

        return { success: false, error: response.message || response.error || 'Login failed' };
      }
    },

    /**
     * End the session: notify the server and wipe all local auth state.
     */
    async logout() {
      await api.logout();
      set({
        user: null,
        isAuthenticated: false,
        isLoading: false,
      });
    },

    /**
     * Verify the stored session token is still valid, usually called on app startup.
     * If no token is found or it is rejected by the server we silently reset to logged-out state.
     */
    async checkAuth() {
      const sessionToken = localStorage.getItem('session-token');

      if (!sessionToken) {
        set({
          user: null,
          isAuthenticated: false,
          isLoading: false,
        });
        return;
      }

      const response = await api.getCurrentUser();

      if (response.success && response.data) {
        set({
          user: response.data,
          isAuthenticated: true,
          isLoading: false,
        });
      } else {
        // Token is invalid or expired – wipe it so the user has to log in
        // again. Must go through api.clearTokens(), not a manual
        // localStorage.removeItem of just these two keys: ApiClient also
        // caches the session/CSRF token pair in memory (hydrated once from
        // localStorage in its constructor), and only clearTokens() resets
        // that in-memory copy too. Removing just the localStorage keys left
        // the stale pair alive in ApiClient for the rest of the page's
        // lifetime, so any later request that runs with includeAuth (the
        // default) — most visibly a first-run Setup Wizard submission
        // reusing a browser that previously had a real session — still
        // attached the dead token, which the CSRF middleware then rejects
        // with a bare 403 instead of the request ever reaching a handler.
        api.clearTokens();
        set({
          user: null,
          isAuthenticated: false,
          isLoading: false,
        });
      }
    },
  };
}

export const authStore = createAuthStore();

/** True when the logged-in user has admin privileges. */
export const isAdmin = derived(
  authStore,
  $auth => $auth.user?.is_admin || false
);
