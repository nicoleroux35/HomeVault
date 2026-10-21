/* =============================================================================
 * File          : api.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The central API client for the HomeVault backend: one class through
 *   which every screen talks to the server. It injects the session and CSRF
 *   tokens (hydrated from localStorage so a reload keeps the session), sends
 *   the X-HomeVault-Contract header and surfaces a contract-mismatch reload
 *   prompt, and caches GET responses for 5 s so rapid re-renders cost
 *   nothing. Also hosts the shared client-side types for the activity feed
 *   and progress events.
 *
 * Dependencies:
 *   - none (browser fetch + internal $lib modules)
 *
 * Usage:
 *   `import { api } from '$lib/services/api'` — used by every service call
 *   site; tokens are set once after login via api.setTokens(...).
 * =============================================================================
 */

import type { ApiResponse, DbBackupEntry, DbBackupStatus, LoginRequest, LoginResponse, SetupAdminRequest, SetupStatus } from '$lib/types/api';
import { notifyContractMismatch } from '$lib/stores/contract';

/** One row of the desktop background-activity taskbar (`GET /api/activity`). */
export interface ActivityTask {
  /** `PROGRESS_MANAGER` job uuid, or `"aria2:<gid>"`. */
  id: string;
  /** Stable slug — localised on the client via `desktop.activity_kind_<kind>`. */
  kind: string;
  /** Dynamic name (torrent / file); empty for a generic job. */
  label: string;
  /** `0..100`, or `null` for an indeterminate job (sliding bar, no `%`). */
  progress: number | null;
  /** Latest message / "12/40 files" / "8.4 MB/s". */
  detail: string | null;
}

/** Progress event received from the server via SSE for long-running jobs. */
export interface ProgressEvent {
  job_id: string;
  /**
   * Serde-tagged category: `{ type: 'backup' }` for the built-in kinds,
   * `{ type: 'custom', name: 'explorer_copy' }` for a feature job. Never a
   * bare string — it is an object on the wire (the backend used to fail
   * serializing the `custom` case outright, which emptied `data` and made
   * `JSON.parse` throw for every explorer copy/move job).
   */
  job_type: { type: string; name?: string };
  state: string;
  progress: number;
  message?: string;
  error?: string;
}

interface CacheEntry {
  data: any;
  timestamp: number;
}

/**
 * The client half of the versioned client/server contract.
 *
 * Must match `CONTRACT_VERSION` in `src/api_contract.rs`. Bump **both** only on
 * a breaking API change (renamed field, removed endpoint, changed shape). The
 * client sends it as `X-HomeVault-Contract` on every request, and `GET /api/meta`
 * is compared against it so a stale cached bundle can surface a reload prompt
 * instead of failing silently.
 */
export const CONTRACT_VERSION = 1;

/**
 * API client for the HomeVault backend.
 * Includes a request-level cache (keyed by method + endpoint) and automatic
 * token injection for authenticated endpoints.
 */
class ApiClient {
  private baseUrl = '/api';
  private sessionToken: string | null = null;
  private csrfToken: string | null = null;
  private cache = new Map<string, CacheEntry>();
  /** Default cache TTL for GET requests (milliseconds). */
  private defaultTTL = 5000;

  constructor() {
    // Hydrate tokens from localStorage so they survive page reloads
    this.sessionToken = localStorage.getItem('session-token');
    this.csrfToken = localStorage.getItem('csrf-token');
  }

  /**
   * Persist session + CSRF tokens both in-memory and to localStorage
   * so they survive page reloads.
   */
  setTokens(sessionToken: string, csrfToken: string) {
    this.sessionToken = sessionToken;
    this.csrfToken = csrfToken;
    localStorage.setItem('session-token', sessionToken);
    localStorage.setItem('csrf-token', csrfToken);
  }

  /**
   * Remove all auth tokens and clear the response cache.
   * Called on logout or when tokens are detected as invalid.
   */
  clearTokens() {
    this.sessionToken = null;
    this.csrfToken = null;
    localStorage.removeItem('session-token');
    localStorage.removeItem('csrf-token');
    // The `hv_session` cookie is read only by the per-app reverse proxy's
    // auth gate; the server clears it on /logout, this is belt-and-braces
    // for client-side session wipes that don't hit the server.
    document.cookie = 'hv_session=; Path=/; Max-Age=0';
    this.clearCache();
  }

  /**
   * Wipe every cached API response.
   */
  clearCache() {
    this.cache.clear();
  }

  /**
   * Remove cache entries whose key contains the given pattern.
   * Used by write-methods (POST/PUT/DELETE) to evict stale data.
   */
  invalidateCache(pattern: string) {
    const keys = Array.from(this.cache.keys());
    keys.forEach(key => {
      if (key.includes(pattern)) {
        this.cache.delete(key);
      }
    });
  }

  /**
   * Evict every cached GET a mutation on `endpoint` can make stale.
   * A `/settings/<group>` write also changes the aggregate `GET:/settings`
   * payload that seeds the Settings window (5 s TTL), whose key does not
   * contain the group path — without this second eviction, closing and
   * reopening the window right after a save re-seeds it from pre-save data.
   */
  private invalidateForMutation(endpoint: string) {
    this.invalidateCache(endpoint);
    if (endpoint.startsWith('/settings/')) {
      this.invalidateCache('/settings');
    }
  }

  /**
   * Return a cached response if it exists and has not exceeded the TTL.
   */
  private getFromCache<T>(key: string, ttl: number): T | null {
    const entry = this.cache.get(key);
    if (entry && Date.now() - entry.timestamp < ttl) {
      return entry.data as T;
    }
    return null;
  }

  /**
   * Store a response in the cache.
   * Drops the oldest entry when the cache exceeds 100 items to prevent memory leaks.
   * Also drops entries older than 5 minutes (300000ms) on every save.
   */
  private saveToCache(key: string, data: any) {
    // Clean up expired entries first (TTL-based eviction)
    const now = Date.now();
    const maxAge = 300000; // 5 minutes

    for (const [k, entry] of this.cache.entries()) {
      if (now - entry.timestamp > maxAge) {
        this.cache.delete(k);
      }
    }

    // Then apply size limit (keep most recent 100 items)
    if (this.cache.size >= 100) {
      const firstKey = this.cache.keys().next().value;
      if (firstKey !== undefined) {
        this.cache.delete(firstKey);
      }
    }

    this.cache.set(key, { data, timestamp: Date.now() });
  }

  /**
   * Build headers for API requests
   */
  private getHeaders(includeAuth = true): HeadersInit {
    const headers: HeadersInit = {
      'Content-Type': 'application/json',
      'X-HomeVault-Contract': String(CONTRACT_VERSION),
    };

    // Always re-read tokens from localStorage so requests made before
    // `setTokens` has populated the in-memory field (e.g. store fetches
    // fired during the initial shell mount, prior to login completing)
    // still carry a valid session token once one is present.
    const sessionToken = this.sessionToken ?? localStorage.getItem('session-token');
    const csrfToken = this.csrfToken ?? localStorage.getItem('csrf-token');

    if (includeAuth && sessionToken) {
      headers['X-Session-Token'] = sessionToken;
    }

    if (csrfToken) {
      headers['X-CSRF-Token'] = csrfToken;
    }

    return headers;
  }

  /**
   * Parse a JSON response, flagging a client/server contract mismatch
   * (HTTP 426, see `src/middleware/contract.rs`) so the shell can prompt a
   * reload instead of the request silently failing.
   */
  private async parseResponse<T>(response: Response): Promise<ApiResponse<T>> {
    if (response.status === 426) {
      notifyContractMismatch();
    }
    return await response.json();
  }

  /**
   * Generic GET request with optional in-memory caching.
   * Returns cached data for the configured TTL, then fetches fresh data.
   */
  async get<T = any>(endpoint: string, options?: { cache?: boolean; ttl?: number }): Promise<ApiResponse<T>> {
    const useCache = options?.cache !== false;
    const ttl = options?.ttl || this.defaultTTL;
    const cacheKey = `GET:${endpoint}`;

    if (useCache) {
      const cached = this.getFromCache<ApiResponse<T>>(cacheKey, ttl);
      if (cached) {
        return cached;
      }
    }

    try {
      const response = await fetch(`${this.baseUrl}${endpoint}`, {
        method: 'GET',
        headers: this.getHeaders(),
      });

      const data = await this.parseResponse<T>(response);

      // Only cache responses that indicate success
      if (useCache && data.success) {
        this.saveToCache(cacheKey, data);
      }

      return data;
    } catch (error) {
      return {
        success: false,
        error: error instanceof Error ? error.message : 'Network error',
      };
    }
  }

  /**
   * Generic POST request
   */
  async post<T = any>(endpoint: string, data?: any, includeAuth = true): Promise<ApiResponse<T>> {
    // Invalidate cache for mutations
    this.invalidateForMutation(endpoint);

    try {
      const response = await fetch(`${this.baseUrl}${endpoint}`, {
        method: 'POST',
        headers: this.getHeaders(includeAuth),
        body: JSON.stringify(data),
      });

      return await this.parseResponse<T>(response);
    } catch (error) {
      return {
        success: false,
        error: error instanceof Error ? error.message : 'Network error',
      };
    }
  }

  /**
   * Generic PUT request
   */
  async put<T = any>(endpoint: string, data?: any, includeAuth = true): Promise<ApiResponse<T>> {
    // Invalidate cache for mutations
    this.invalidateForMutation(endpoint);

    try {
      const response = await fetch(`${this.baseUrl}${endpoint}`, {
        method: 'PUT',
        headers: this.getHeaders(includeAuth),
        body: JSON.stringify(data),
      });

      return await this.parseResponse<T>(response);
    } catch (error) {
      return {
        success: false,
        error: error instanceof Error ? error.message : 'Network error',
      };
    }
  }

  /**
   * Generic DELETE request. `data`, when provided, is sent as a JSON body
   * (used by endpoints like share-permission revoke that identify the
   * resource to delete via the request body rather than the URL).
   */
  async delete<T = any>(endpoint: string, data?: any): Promise<ApiResponse<T>> {
    // Invalidate cache for mutations
    this.invalidateForMutation(endpoint);

    try {
      const response = await fetch(`${this.baseUrl}${endpoint}`, {
        method: 'DELETE',
        headers: this.getHeaders(),
        body: data !== undefined ? JSON.stringify(data) : undefined,
      });

      return await this.parseResponse<T>(response);
    } catch (error) {
      return {
        success: false,
        error: error instanceof Error ? error.message : 'Network error',
      };
    }
  }

  // ==============================================
  // Authentication
  // ==============================================

  async login(credentials: LoginRequest) {
    return this.post<LoginResponse>('/auth/login', credentials, false);
  }

  /**
   * Completes login for an account with 2FA enabled. `/auth/login` only ever
   * signals that a TOTP code is required (`requires_2fa`); it does not accept
   * one. The code must be submitted here instead, against the dedicated
   * verification endpoint.
   */
  async loginWith2FA(username: string, password: string, totpCode: string, rememberMe = false) {
    return this.post<LoginResponse>(
      '/auth/2fa/verify',
      { username, password, totp_code: totpCode, remember_me: rememberMe },
      false
    );
  }

  async logout() {
    const response = await this.post('/auth/logout');
    this.clearTokens();
    return response;
  }

  async getCurrentUser() {
    return this.get('/auth/me', { cache: false }); // Never cache user data
  }

  // Numeric recovery code — the "forgot password" flow.
  // Generation is authenticated (used by the setup wizard right after the
  // account is created, and later by the account page).
  async generateRecoveryCode(regenerate = false): Promise<ApiResponse<{ recovery_code: string }>> {
    const qs = regenerate ? '?regenerate=true' : '';
    return this.post<{ recovery_code: string }>(`/auth/recovery-code${qs}`, {});
  }

  async verifyRecoveryCode(username: string, recovery_code: string): Promise<ApiResponse<{ valid: boolean }>> {
    return this.post<{ valid: boolean }>('/auth/verify-recovery-code', { username, recovery_code }, false);
  }

  async resetPasswordWithRecovery(username: string, recovery_code: string, new_password: string) {
    return this.post('/auth/reset-password-recovery', { username, recovery_code, new_password }, false);
  }

  async getSetupStatus(): Promise<SetupStatus> {
    // Never cached: the app gate re-reads this after setup / login / a
    // hardware re-baseline and a stale answer would route the app wrongly.
    const response = await this.get<SetupStatus>('/auth/setup-status', { cache: false });
    if (!response.success || !response.data) {
      // Throwing (rather than falling back to a fake first-run status) is what
      // lets the gate show its connection-error screen instead of the Setup
      // Wizard when the server is unreachable.
      throw new Error(response.error || response.message || 'Failed to load setup status');
    }
    return response.data;
  }

  // Confirms a hardware change and re-baselines the stored fingerprint.
  // Admin-only; used by the recovery screen the app gate routes to when
  // `/auth/setup-status` reports `needs_recovery`.
  async rebaselineSetup(): Promise<ApiResponse<{ system_hash: string; machine_id: string }>> {
    return this.post<{ system_hash: string; machine_id: string }>('/auth/setup/rebaseline', {});
  }

  async setupAdmin(data: SetupAdminRequest): Promise<ApiResponse<LoginResponse>> {
    return this.post<LoginResponse>('/auth/setup', data, false);
  }

  async checkUsername(username: string): Promise<ApiResponse<{ available: boolean; reason: string }>> {
    return this.get<{ available: boolean; reason: string }>(
      `/auth/check-username?username=${encodeURIComponent(username)}`
    );
  }

  // Pre-creation TOTP validation for the Setup Wizard's password step: the
  // secret is client-generated, so this stateless, first-run-only endpoint
  // just answers "is this code valid for this secret right now?".
  async verifySetupTotp(secret: string, code: string): Promise<ApiResponse<{ valid: boolean }>> {
    return this.post<{ valid: boolean }>('/auth/setup/verify-totp', { secret, code }, false);
  }

  /** Reboot-safe wizard progress: current step + operator choices. */
  async getSetupProgress(): Promise<ApiResponse<{ step: number; data: Record<string, unknown> }>> {
    return this.get<{ step: number; data: Record<string, unknown> }>('/auth/setup/progress', { cache: false });
  }

  async saveSetupProgress(step: number, data: Record<string, unknown>): Promise<ApiResponse<{ step: number; saved: boolean }>> {
    // includeAuth: false — same reason as setupAdmin(): a stale session pair
    // from a previous install must not trip the CSRF middleware on this
    // public pre-session endpoint.
    return this.put<{ step: number; saved: boolean }>('/auth/setup/progress', { step, data }, false);
  }

  // 2FA enrollment (reuses the authenticated 2FA setup endpoints)
  async setup2FASecret(): Promise<ApiResponse<{ secret: string; qrcode_url: string }>> {
    return this.post<{ secret: string; qrcode_url: string }>('/auth/2fa/setup', { action: 'generate' });
  }

  async verify2FASecret(code: string, password: string): Promise<ApiResponse<{ enabled: boolean; backup_codes?: string[] }>> {
    return this.post<{ enabled: boolean; backup_codes?: string[] }>('/auth/2fa/setup', { action: 'verify', totp_code: code, password });
  }

  // Self-service 2FA management (My Account page)
  async disableMy2FA(password: string): Promise<ApiResponse<{ enabled: boolean }>> {
    return this.post<{ enabled: boolean }>('/auth/2fa/setup', { action: 'disable', password });
  }

  async getMy2FABackupCodes(): Promise<ApiResponse<{ codes: string[] }>> {
    return this.post<{ codes: string[] }>('/auth/2fa/setup', { action: 'backup_codes' });
  }

  // ==============================================
  // Users (no cache for mutations)
  // ==============================================

  async updateUser(username: string, userData: any) {
    const res = await this.put(`/users/${username}`, userData);
    this.invalidateCache('/users');
    return res;
  }

  async updateUserFull(
    username: string,
    userData: {
      fullname: string;
      email: string;
      home_enabled: boolean;
      home_disk: string | null;
      home_path?: string | null;
      home_size_bytes?: number | null;
    }
  ) {
    // Same list-cache staleness as updateUserRole/setUserLocked below: the
    // generic put() only invalidates cache keys containing the exact
    // endpoint (`/users/{username}/full`), which never matches the list's
    // own cache key (`GET:/users`) — confirmed live this left the list row
    // *and* the just-saved detail pane (hydrated from that same stale list
    // on the next refreshAll()) showing the pre-save name/e-mail for up to
    // the cache's 5s TTL.
    const res = await this.put(`/users/${username}/full`, userData);
    this.invalidateCache('/users');
    return res;
  }

  async deleteUser(username: string, deleteHomeFolder = false) {
    const qs = deleteHomeFolder ? '?delete_home_folder=true' : '';
    const res = await this.delete(`/users/${username}${qs}`);
    this.invalidateCache('/users');
    return res;
  }

  async updateUserGroups(username: string, groups: string[]) {
    const res = await this.put(`/users/${username}/groups`, { groups });
    // Also invalidate '/groups': the Groups page's list caches each group's
    // member_count and its own member sheet caches the member list, both of
    // which just changed here — without this, editing a user's groups from
    // the Users page can leave the Groups page showing a stale count/member
    // list for the affected group(s) for up to its own TTL.
    this.invalidateCache('/users');
    this.invalidateCache('/groups');
    return res;
  }

  async setUserHome(
    username: string,
    enabled: boolean,
    disk?: string | null,
    sizeBytes?: number | null,
    path?: string | null
  ) {
    return this.put(`/users/${username}/home`, {
      enabled,
      disk,
      path: path ?? null,
      size_bytes: sizeBytes ?? null,
    });
  }

  async deleteUserHome(username: string) {
    return this.delete(`/users/${username}/home`);
  }

  async updateUserPassword(username: string, newPassword: string, currentPassword?: string) {
    return this.put(`/users/${username}/password`, {
      current_password: currentPassword || null,
      new_password: newPassword,
    });
  }

  async updateUserRole(username: string, roleId: number) {
    // The generic `put()` only invalidates cache keys containing this exact
    // endpoint (`/users/{username}/role`), which never matches the list's
    // own cache key (`GET:/users`) — confirmed live this left the list
    // showing a stale role badge for several seconds after a real change.
    const res = await this.put(`/users/${username}/role`, { role_id: roleId });
    this.invalidateCache('/users');
    // Promoting to admin syncs this user into the real homevault_admin OS
    // group server-side (see sync_admin_user_to_group), which changes that
    // group's member list/count on the Groups page too.
    this.invalidateCache('/groups');
    return res;
  }

  async setUserLocked(username: string, locked: boolean) {
    // Same list-cache staleness as updateUserRole above — confirmed live via
    // a real click that the Locked/Active badge did not update until a full
    // page reload.
    const res = await this.put(`/users/${username}/locked`, { locked });
    this.invalidateCache('/users');
    return res;
  }

  async adminDisable2FA(username: string) {
    const res = await this.post(`/users/${username}/2fa/disable`);
    this.invalidateCache('/users');
    return res;
  }

  async getAvailableHomeDisks() {
    return this.get('/storage/available-home-disks', { cache: false });
  }

  // ==============================================
  // Docker (with caching)
  // ==============================================

  async getDockerContainers() {
    return this.get('/docker/containers', { ttl: 3000 }); // Cache 3s
  }

  /**
   * Containers WITH live `docker stats` sampling (CPU %, memory, net/block
   * IO). `stats=true` makes the backend run `docker stats --no-stream`, which
   * blocks ~1s, so this is deliberately uncached and only used by the
   * Dashboard's live-usage card — never by the Docker app's first paint.
   */
  async getDockerContainersWithStats() {
    return this.get('/docker/containers?stats=true', { cache: false });
  }

  /** Remove (uninstall) a container. `force` also stops a running one. */
  async removeDockerContainer(name: string, force = false) {
    return this.delete(`/docker/containers/${encodeURIComponent(name)}?force=${force ? 'true' : 'false'}`);
  }

  async getDockerCatalog() {
    return this.get('/docker/catalog', { ttl: 5000 });
  }

  async startContainer(name: string) {
    return this.put(`/docker/containers/${name}/start`);
  }

  async stopContainer(name: string) {
    return this.put(`/docker/containers/${name}/stop`);
  }

  async restartContainer(name: string) {
    return this.put(`/docker/containers/${name}/restart`);
  }

  async isDockerAvailable() {
    return this.get('/docker/available', { ttl: 10000 });
  }

  // ==============================================
  // Per-member resource access (apps / folders / shares)
  // ==============================================

  /** Admin: every grant, optionally narrowed to one member. */
  async listResourceAccess(member?: string) {
    const qs = member ? `?member=${encodeURIComponent(member)}` : '';
    return this.get(`/shares/access${qs}`, { cache: false });
  }

  /** The calling account's own effective access (server-filtered). */
  async getMyResourceAccess() {
    return this.get('/shares/access/mine', { cache: false });
  }

  /** Admin: grant (or update) a member's access to a resource. */
  async grantResourceAccess(member: string, kind: 'app' | 'folder' | 'share', resource: string, permission: 'read' | 'write' | 'full' = 'read') {
    return this.post('/shares/access', { member, kind, resource, permission });
  }

  /** Admin: revoke a member's access to a resource. */
  async revokeResourceAccess(member: string, kind: 'app' | 'folder' | 'share', resource: string) {
    return this.delete(`/shares/access?member=${encodeURIComponent(member)}&kind=${kind}&resource=${encodeURIComponent(resource)}`);
  }

  /** Admin: every group-level grant, optionally narrowed to one group. */
  async listGroupResourceAccess(group?: string) {
    const qs = group ? `?group=${encodeURIComponent(group)}` : '';
    return this.get(`/shares/access/groups${qs}`, { cache: false });
  }

  /** Admin: grant (or update) a group's access to a resource. Every member of
   *  the group inherits it, unioned with their own direct grants. */
  async grantGroupResourceAccess(group: string, kind: 'app' | 'folder' | 'share', resource: string, permission: 'read' | 'write' | 'full' = 'read') {
    return this.post('/shares/access/groups', { group, kind, resource, permission });
  }

  /** Admin: revoke a group's access to a resource. */
  async revokeGroupResourceAccess(group: string, kind: 'app' | 'folder' | 'share', resource: string) {
    return this.delete(`/shares/access/groups?group=${encodeURIComponent(group)}&kind=${kind}&resource=${encodeURIComponent(resource)}`);
  }

  /** The group names a given account belongs to (OS membership). */
  async getUserGroups(username: string) {
    return this.get(`/users/${encodeURIComponent(username)}/groups`, { cache: false });
  }

  // ==============================================
  // Web shortcuts
  // ==============================================

  /** Fetch a page's title / description / favicon (server-side, SSRF-guarded). */
  async fetchShortcutMetadata(url: string) {
    return this.post('/shortcuts/fetch-metadata', { url });
  }

  // ==============================================
  // Virtual Machines
  // ==============================================

  /** Catalogue install: { name, kind, os, size, cpu_cores?, memory_mb?, disk_gb?, username, password }
   *  ISO install:       { name, iso_path, iso_os: "linux"|"windows", size, drivers_iso_path?, secure_boot?, cpu_cores?, memory_mb?, disk_gb? } */

  /** { ticket } for the graphical screen of an ISO-install VM. Connect the WS
   *  to /api/vm/screen/ws?ticket=… (noVNC). */

  // Managed ISO store ({DATA_DIRECTORY}/vm/isos)

  /** Body: { name, cols?, rows? } → { ticket }. Connect the WS to
   *  /api/vm/console/ws?ticket=… for an interactive shell. */

  // ==============================================
  // Dashboard Endpoints (with caching)
  // ==============================================

  // `addWidget`/`removeWidget` were removed 2026-09-07: they had no caller
  // and pointed at `/dashboard/widgets` (plural) with a `{type}` body, while
  // the real routes are `POST /api/dashboard/widget` (singular, an
  // `AddWidgetRequest`) and `DELETE /api/dashboard/widget/{id}` — so they
  // would have 404'd. The dashboard-widget grid is not wired to any UI (the
  // Widgets app uses `monitorStore`).

  // ==============================================
  // Groups
  // ==============================================

  async getGroups() {
    return this.get('/groups', { ttl: 10000 });
  }

  async getGroup(name: string) {
    return this.get(`/groups/${encodeURIComponent(name)}`);
  }

  async createGroup(name: string, members: string[] = [], description?: string) {
    return this.post('/groups', { name, members, description: description || null });
  }

  /** Rename a group and/or set its description. The server migrates every
   *  display-name reference (mirror, resource grants, quotas, ACLs) on a
   *  rename, so callers never patch those by hand. */
  async updateGroup(name: string, patch: { name?: string; description?: string | null }) {
    const res = await this.put(`/groups/${encodeURIComponent(name)}`, patch);
    this.invalidateCache('/groups');
    return res;
  }

  async deleteGroup(name: string) {
    const res = await this.delete(`/groups/${encodeURIComponent(name)}`);
    this.invalidateCache('/groups');
    return res;
  }

  async getAvailableUsers() {
    return this.get('/groups/users', { cache: false });
  }

  async addMemberToGroup(groupName: string, username: string) {
    const res = await this.post(`/groups/${encodeURIComponent(groupName)}/members`, { username });
    // Also invalidate '/users': the Users page's list and per-user views
    // embed each user's live groups, which just changed here — without
    // this, adding/removing a member from the Groups page can leave the
    // Users page showing a stale group list for that user for up to its
    // own TTL.
    this.invalidateCache('/groups');
    this.invalidateCache('/users');
    return res;
  }

  async removeMemberFromGroup(groupName: string, username: string) {
    const res = await this.delete(`/groups/${encodeURIComponent(groupName)}/members/${encodeURIComponent(username)}`);
    this.invalidateCache('/groups');
    this.invalidateCache('/users');
    return res;
  }

  // ==============================================
  // Terminal
  // ==============================================

  async getTerminalTicket(tabId?: string): Promise<ApiResponse<{ ticket: string }>> {
    const q = tabId ? `?tab=${encodeURIComponent(tabId)}` : '';
    return this.post(`/terminal/ticket${q}`);
  }

  async closeTerminalSession(tabId: string): Promise<ApiResponse<unknown>> {
    return this.post(`/terminal/close?tab=${encodeURIComponent(tabId)}`);
  }

  async getTerminalCwd(tabId: string): Promise<ApiResponse<{ cwd: string | null }>> {
    return this.get(`/terminal/cwd?tab=${encodeURIComponent(tabId)}`, { cache: false });
  }

  async listTerminalDir(tabId: string, path: string): Promise<ApiResponse<{ entries: { name: string; is_dir: boolean }[] }>> {
    return this.get(
      `/terminal/ls?tab=${encodeURIComponent(tabId)}&path=${encodeURIComponent(path)}`,
      { cache: false }
    );
  }

  // ==============================================
  // Health Check
  // ==============================================

  async healthCheck() {
    return this.get('/health', { cache: false }); // Never cache health
  }

  // ==============================================
  // Settings (all categories)
  // ==============================================

  /** Aggregate: fetch all settings in one call */
  async getAllSettings() {
    return this.get('/settings', { ttl: 5000 });
  }

  /** Public settings (no auth required) */
  async getPublicSettings() {
    return this.get('/settings/public', { ttl: 10000, cache: false });
  }

  // General

  async updateGeneralSettings(data: { server_name?: string; timezone?: string }) {
    return this.put('/settings/general', data);
  }

  async getAutostart() {
    return this.get('/settings/autostart', { ttl: 5000 });
  }

  async setAutostart(enabled: boolean) {
    return this.put('/settings/autostart', { enabled });
  }

  // Email / SMTP

  async updateEmailSettings(data: {
    smtp_server?: string;
    smtp_port?: number;
    smtp_username?: string;
    smtp_password?: string;
    from_address?: string;
    use_tls?: boolean;
  }) {
    return this.put('/settings/email', data);
  }

  async testEmailSettings() {
    return this.post('/settings/email/test');
  }

  // Notifications

  async updateNotificationSettings(data: {
    email?: string;
    enable_alerts?: boolean;
    alert_threshold?: number;
    smtp_enabled?: boolean;
    webhook_url?: string;
    webhook_enabled?: boolean;
    webhook_format?: string;
  }) {
    return this.put('/settings/notifications', data);
  }

  // Appearance

  async updateAppearanceSettings(data: {
    theme?: string;
    wallpaper?: string;
    accent_color?: string;
    density?: string;
  }) {
    return this.put('/settings/appearance', data);
  }

  // Updates

  async updateUpdateSettings(data: { check_frequency?: string }) {
    return this.put('/settings/update', data);
  }

  async checkForUpdates() {
    return this.get('/settings/version', { cache: false });
  }

  // Language

  async setLanguage(language: string) {
    return this.post('/settings/language', { language });
  }

  // Settings Export/Import/Reset
  async exportSettings() {
    return this.get('/settings/export', { cache: false });
  }

  async importSettings(data: any) {
    return this.post('/settings/import', { data });
  }

  async resetSettings() {
    return this.post('/settings/reset');
  }

  // Snapshots of HomeVault's own database. The database runs in WAL mode, so
  // the `.db` file alone is not restorable — these endpoints produce and list
  // `VACUUM INTO` snapshots (see docs/guides/backup-restic.md).
  async getDbBackupStatus(): Promise<ApiResponse<DbBackupStatus>> {
    return this.get<DbBackupStatus>('/settings/backup/db', { cache: false });
  }

  async backupDatabaseNow(): Promise<ApiResponse<{ backup: DbBackupEntry; status: DbBackupStatus }>> {
    return this.post<{ backup: DbBackupEntry; status: DbBackupStatus }>('/settings/backup/db/now', {});
  }

  async saveDbBackupSettings(intervalHours: number, retain: number): Promise<ApiResponse<DbBackupStatus>> {
    return this.put<DbBackupStatus>('/settings/backup/db', { interval_hours: intervalHours, retain: retain });
  }

  // SNMP
  async getSnmpSettings() {
    return this.get('/snmp/settings', { ttl: 5000 });
  }

  async updateSnmpSettings(data: { community?: string; version?: string }) {
    return this.put('/snmp/settings', data);
  }

  async getSnmpAvailability() {
    return this.get('/snmp/availability', { ttl: 10000 });
  }

  async getSnmpInfo(params?: { community?: string }) {
    const qs = params?.community ? `?community=${encodeURIComponent(params.community)}` : '';
    return this.get(`/snmp/info${qs}`, { ttl: 5000 });
  }

  async getSnmpInterfaces(params?: { community?: string; iface?: string }) {
    const parts: string[] = [];
    if (params?.community) parts.push(`community=${encodeURIComponent(params.community)}`);
    if (params?.iface) parts.push(`iface=${encodeURIComponent(params.iface)}`);
    const qs = parts.length ? `?${parts.join('&')}` : '';
    return this.get(`/snmp/interfaces${qs}`, { ttl: 5000 });
  }

  async getSnmpStorage(params?: { community?: string }) {
    const qs = params?.community ? `?community=${encodeURIComponent(params.community)}` : '';
    return this.get(`/snmp/storage${qs}`, { ttl: 5000 });
  }

  // UPS
  async getUpsList() {
    return this.get('/ups/list', { ttl: 5000 });
  }

  async getUpsAvailability() {
    return this.get('/ups/availability', { ttl: 10000 });
  }

  // UPS device configuration (`/etc/nut/ups.conf`) — serial / network / USB.
  async getUpsConfig() {
    return this.get('/ups/config', { cache: false });
  }

  async saveUpsConfig(data: { name: string; driver: string; port: string; desc?: string }) {
    return this.post('/ups/config', data);
  }

  async deleteUpsConfig(name: string) {
    return this.delete(`/ups/config/${encodeURIComponent(name)}`);
  }

  // 2FA

  // Sessions management
  async getMySessions() {
    return this.get('/security/sessions', { cache: false });
  }

  async revokeSessions(tokens: string[]) {
    return this.post('/security/sessions/revoke', { tokens });
  }

  async revokeAllSessions() {
    return this.post('/security/sessions/revoke-all');
  }

  /** Admin Users screen: another user's active sessions — token values are
   *  masked to audit references (`id`), revocation is bulk only. */
  async getAdminUserSessions(username: string) {
    return this.get(`/users/${username}/sessions`, { cache: false });
  }

  async revokeAdminUserSessions(username: string) {
    return this.delete(`/users/${username}/sessions`);
  }

  /** Admin Users screen: set (`expiresAt` = RFC 3339) or clear (`null`)
   *  the account's expiry horizon (migration 050). */
  async setUserExpiry(username: string, expiresAt: string | null) {
    return this.put(`/users/${username}/expiry`, { expires_at: expiresAt });
  }

  // Login history
  async getLoginHistory() {
    return this.get('/security/login-history', { cache: false });
  }

  async getUserLoginHistory(username: string) {
    return this.get(`/security/login-history/${encodeURIComponent(username)}`, { cache: false });
  }

  async getActiveSessionsCount() {
    return this.get('/security/active-sessions-count', { cache: false });
  }

  // Monitor layout (desktop widget positions/visibility, synced per-account
  // across browser origins - see monitorStore.ts)
  async getMonitorLayout() {
    return this.get<{ layout_json: string | null }>('/monitor-layout', { cache: false });
  }

  async setMonitorLayout(layoutJson: string) {
    return this.post('/monitor-layout', { layout_json: layoutJson });
  }

  // Weather widget location (synced per-account across browser origins -
  // see weatherLocation.ts)
  async getWeatherLocation() {
    return this.get<{ location_json: string | null }>('/weather-location', { cache: false });
  }

  async setWeatherLocation(locationJson: string) {
    return this.post('/weather-location', { location_json: locationJson });
  }

  // Server
  async restartServer() {
    return this.post('/server/restart');
  }

  async shutdownServer() {
    return this.post('/server/shutdown');
  }

  async getServerInfo() {
    return this.get('/server/info', { ttl: 5000 });
  }

  // ==============================================
  // Widget Data (real-time desktop widgets)
  // ==============================================

  async getWidgetCpu() {
    return this.get('/widgets/cpu', { ttl: 5000 });
  }

  async getWidgetRam() {
    return this.get('/widgets/ram', { ttl: 5000 });
  }

  async getWidgetNetworkTraffic() {
    return this.get('/widgets/network-traffic', { ttl: 5000 });
  }

  async getWidgetCpuTemp() {
    return this.get('/widgets/cpu-temp', { ttl: 5000 });
  }

  async getWidgetUpdateInfo() {
    return this.get('/widgets/update-info', { ttl: 30000 });
  }

  async getHealthData() {
    return this.get('/dashboard/health', { ttl: 5000 });
  }

  async getServerDetails() {
    return this.get('/dashboard/server-details', { ttl: 5000 });
  }

  // ==============================================
  // Network
  // ==============================================

  async getNetworkInterfaces() {
    return this.get('/network/interfaces', { ttl: 5000 });
  }

  async getNetworkInterfaceConfig(name: string) {
    return this.get(`/network/interfaces/${encodeURIComponent(name)}/config`, { ttl: 5000 });
  }

  async updateNetworkInterfaceConfig(name: string, data: {
    method: string;
    ip?: string;
    netmask?: string;
    gateway?: string;
    dns_servers?: string[];
    ipv6_method?: string;
    ipv6?: string;
    ipv6_prefix?: number;
    ipv6_gateway?: string;
    ipv6_dns_servers?: string[];
  }) {
    return this.put(`/network/interfaces/${encodeURIComponent(name)}/config`, data);
  }

  async setNetworkInterfaceState(name: string, up: boolean) {
    return this.put(`/network/interfaces/${encodeURIComponent(name)}/state`, { up });
  }

  async scanWifiNetworks() {
    return this.post('/network/wifi/scan');
  }

  async getWifiStatus() {
    return this.get('/network/wifi/status', { ttl: 5000 });
  }

  async connectWifi(ssid: string, password: string, security: string = 'wpa2') {
    return this.post('/network/wifi/connect', { ssid, password, security });
  }

  async disconnectWifi(ssid: string) {
    return this.post('/network/wifi/disconnect', { ssid });
  }

  async getNetworkDiagnostics() {
    return this.get('/network/diagnostics', { ttl: 5000 });
  }

  async getNetworkSummary() {
    return this.get('/network/summary', { ttl: 5000 });
  }

  async getPublicIp() {
    return this.get('/network/public-ip', { ttl: 30000 });
  }

  // ==============================================
  // Notifications
  // ==============================================

  async getNotifications() {
    return this.get('/notifications', { cache: false });
  }

  async markNotificationRead(id: string) {
    return this.post(`/notifications/${encodeURIComponent(id)}/read`, {});
  }

  async markAllNotificationsRead() {
    return this.post('/notifications/read-all', {});
  }

  async deleteNotification(id: string) {
    return this.delete(`/notifications/${encodeURIComponent(id)}`);
  }

  /** One-time ticket for authenticating the `/api/events` SSE connection (see websocket.ts). */
  async getNotificationsEventsTicket() {
    return this.post('/notifications/events/ticket', {});
  }

  // ==============================================
  // Scheduled tasks (generic cron engine)
  // ==============================================

  async getScheduledTasks() {
    return this.get<{ tasks: ScheduledTask[] }>('/scheduled-tasks', { cache: false });
  }

  async createScheduledTask(task: {
    name: string; task_type: string; cron: string; payload: Record<string, unknown>; enabled: boolean;
  }) {
    return this.post<ScheduledTask>('/scheduled-tasks', task);
  }

  async updateScheduledTask(id: string, task: {
    name: string; task_type: string; cron: string; payload: Record<string, unknown>; enabled: boolean;
  }) {
    return this.put<ScheduledTask>(`/scheduled-tasks/${encodeURIComponent(id)}`, task);
  }

  async deleteScheduledTask(id: string) {
    return this.delete(`/scheduled-tasks/${encodeURIComponent(id)}`);
  }

  async runScheduledTaskNow(id: string) {
    return this.post<ScheduledTask>(`/scheduled-tasks/${encodeURIComponent(id)}/run`, {});
  }

  async getScheduledTaskRuns(id: string) {
    return this.get<{ runs: ScheduledTaskRun[] }>(
      `/scheduled-tasks/${encodeURIComponent(id)}/runs`,
      { cache: false },
    );
  }

  // ==============================================
  // Metrics history
  // ==============================================

  async getMetricsHistory(range: '1h' | '24h' | '7d' = '1h') {
    return this.get<{ range: string; bucket_seconds: number; points: MetricsPoint[] }>(
      `/metrics/history?range=${encodeURIComponent(range)}`,
      { ttl: 30000 },
    );
  }

  async getMetricsLatest() {
    return this.get('/metrics/latest', { cache: false });
  }

  // ==============================================
  // Notification events (per-event toggles + webhook sink)
  // ==============================================

  async getNotificationEventsConfig() {
    return this.get<{ events: { id: string; group: string; label_key: string; enabled: boolean }[]; webhook_format: string | null }>(
      '/notifications/events/config',
      { cache: false },
    );
  }

  async updateNotificationEventsConfig(body: {
    events?: Record<string, boolean>;
    webhook_format?: string;
  }) {
    return this.put('/notifications/events/config', body);
  }

  // ==============================================
  // Host power management + disk spindown
  // ==============================================

  async getPowerStatus() {
    return this.get<{
      governor: string | null;
      available_governors: string[];
      governor_supported: boolean;
      spindown_supported: boolean;
    }>('/power/status', { ttl: 10000 });
  }

  async setPowerGovernor(governor: string) {
    return this.put('/power/governor', { governor });
  }

  async hostPowerAction(action: 'reboot' | 'shutdown' | 'suspend') {
    return this.post(`/power/${encodeURIComponent(action)}`, {});
  }

  async getPowerSpindown() {
    return this.get<{ settings: DiskSpindownSetting[] }>('/power/spindown', { cache: false });
  }

  async setPowerSpindown(setting: {
    device: string; apm_level?: number | null; spindown_minutes?: number | null;
  }) {
    return this.put<{ setting: DiskSpindownSetting; applied: boolean }>('/power/spindown', setting);
  }

  async applyPowerSpindown() {
    return this.post('/power/spindown/apply', {});
  }

  // ==============================================
  // Snapshot schedules (GFS retention)
  // ==============================================

  async getSnapshotSchedules() {
    return this.get<{ schedules: SnapshotSchedule[] }>('/snapshot-schedules', { cache: false });
  }

  async createSnapshotSchedule(schedule: {
    pool: string; cron: string; prefix: string; recursive: boolean;
    gfs_hourly: number; gfs_daily: number; gfs_weekly: number;
    gfs_monthly: number; gfs_yearly: number; enabled: boolean;
  }) {
    return this.post<SnapshotSchedule>('/snapshot-schedules', schedule);
  }

  async updateSnapshotSchedule(id: string, schedule: {
    pool: string; cron: string; prefix: string; recursive: boolean;
    gfs_hourly: number; gfs_daily: number; gfs_weekly: number;
    gfs_monthly: number; gfs_yearly: number; enabled: boolean;
  }) {
    return this.put<SnapshotSchedule>(`/snapshot-schedules/${encodeURIComponent(id)}`, schedule);
  }

  async deleteSnapshotSchedule(id: string) {
    return this.delete(`/snapshot-schedules/${encodeURIComponent(id)}`);
  }

  async runSnapshotScheduleNow(id: string) {
    return this.post<{ summary: string }>(`/snapshot-schedules/${encodeURIComponent(id)}/run`, {});
  }

  async getPoolSnapshots(pool: string) {
    return this.get<{ snapshots: { name: string; created: string; used: number; referenced: number; auto: boolean }[] }>(
      `/snapshot-schedules/snapshots?pool=${encodeURIComponent(pool)}`,
      { cache: false },
    );
  }

  // ==============================================
  // What's New (release highlights)
  // ==============================================

  /** Running version's release highlights + whether this user has seen them. */
  async getWhatsNew() {
    return this.get('/whats-new', { cache: false });
  }

  /** Acknowledge the current release so the modal stops showing. */
  async markWhatsNewSeen() {
    return this.post('/whats-new/seen', {});
  }

  // ==============================================
  // Client/server contract
  // ==============================================

  /**
   * Fetch the server's contract + build versions. Unauthenticated.
   * Returns `null` on a network error so callers can stay silent.
   */
  async getMeta(): Promise<{ contract_version: number; server_version: string } | null> {
    const res = await this.get<{ contract_version: number; server_version: string }>(
      '/meta',
      { cache: false },
    );
    return res.success && res.data ? res.data : null;
  }

  // ==============================================
  // VPN
  // ==============================================

  async getVpnCapabilities() {
    return this.get('/vpn/capabilities', { ttl: 60000 });
  }

  async listVpnConnections() {
    return this.get('/vpn/connections', { ttl: 5000 });
  }

  // WireGuard — one always-on server interface + provisioned peers
  // (roadwarrior devices). See src/vpn.rs's WireGuard section doc comment
  // for the full design; creating a peer needs nothing but a name.
  async getWireGuardServerStatus() {
    return this.get('/vpn/wireguard/server', { ttl: 10000 });
  }

  async listWireGuardPeers() {
    return this.get('/vpn/wireguard/peers', { ttl: 5000 });
  }

  async createWireGuardPeer(name: string) {
    return this.post('/vpn/wireguard/peers', { name });
  }

  async revokeWireGuardPeer(id: string) {
    return this.delete(`/vpn/wireguard/peers/${encodeURIComponent(id)}`);
  }

  async getWireGuardPeerConfig(id: string) {
    return this.get(`/vpn/wireguard/peers/${encodeURIComponent(id)}`, { cache: false });
  }

  // OpenVPN
  async createOpenVPNConnection(data: {
    name: string;
    config_content: string;
    username?: string;
    password?: string;
  }) {
    return this.post('/vpn/openvpn', data);
  }

  async startOpenVPNConnection(name: string, credentials?: { username: string; password: string }) {
    return this.post(`/vpn/openvpn/${encodeURIComponent(name)}/start`, credentials || {});
  }

  async stopOpenVPNConnection(name: string) {
    return this.post(`/vpn/openvpn/${encodeURIComponent(name)}/stop`, {});
  }

  async deleteOpenVPNConnection(name: string) {
    return this.delete(`/vpn/openvpn/${encodeURIComponent(name)}`);
  }

  // ==============================================
  // Cloud Sync (rclone)
  // ==============================================

  async getRcloneStatus() {
    return this.get('/rclone/status', { ttl: 10000 });
  }

  async getCloudRemotes() {
    return this.get('/rclone/remotes', { ttl: 10000 });
  }

  async getCloudProviders() {
    return this.get('/rclone/providers', { ttl: 60000 });
  }

  async getCloudMounts() {
    return this.get('/rclone/mounts', { ttl: 5000 });
  }

  async addCloudRemote(data: { name: string; provider: string; config?: string }) {
    return this.post('/rclone/remotes', data);
  }

  async deleteCloudRemote(name: string) {
    return this.delete(`/rclone/remotes/${encodeURIComponent(name)}`);
  }

  async mountCloudRemote(remote: string, mountPoint: string, path?: string) {
    return this.post('/rclone/mount/start', { remote, mount_point: mountPoint, path });
  }

  async unmountCloudRemote(mountPoint: string) {
    return this.post('/rclone/mount/stop', { mount_point: mountPoint });
  }

  async syncCloud(data: {
    source_type: 'local' | 'cloud';
    source: string;
    source_path?: string;
    dest_type: 'local' | 'cloud';
    destination: string;
    dest_path?: string;
    delete: boolean;
    dry_run: boolean;
    update?: boolean;
  }) {
    return this.post('/rclone/sync', data);
  }

  async getCloudConfig() {
    return this.get('/rclone/config', { cache: false });
  }

  async saveCloudConfig(config: string) {
    return this.post('/rclone/config', { config });
  }

  // ─── Sync jobs (server-side persistence) ───────────────────────────────────

  async listSyncJobs() {
    return this.get('/rclone/jobs', { ttl: 5000 });
  }

  async createSyncJob(data: {
    name: string;
    source_type: 'local' | 'cloud';
    source: string;
    source_path?: string;
    dest_type: 'local' | 'cloud';
    destination: string;
    dest_path?: string;
  }) {
    return this.post('/rclone/jobs', data);
  }

  async deleteSyncJob(id: number) {
    return this.delete(`/rclone/jobs/${id}`);
  }

  async runSyncJob(id: number) {
    return this.post(`/rclone/jobs/${id}/run`, {});
  }

  // ─── Progress SSE ───────────────────────────────────────────────────────────

  /**
   * Get SSE URL for job progress
   */
  getProgressUrl(jobId: string): string {
    return `${this.baseUrl}/progress/${encodeURIComponent(jobId)}`;
  }

  /**
   * Connect to SSE stream for job progress
   */
  subscribeToProgress(
    jobId: string,
    onProgress: (event: ProgressEvent) => void,
    onDone?: (event: ProgressEvent) => void,
    onError?: (error: Event) => void
  ): () => void {
    const url = this.getProgressUrl(jobId);
    const eventSource = new EventSource(url);

    eventSource.addEventListener('progress', (e: MessageEvent) => {
      try {
        const event = JSON.parse(e.data) as ProgressEvent;
        onProgress(event);
      } catch {
        console.error('Failed to parse progress event');
      }
    });

    eventSource.addEventListener('done', (e: MessageEvent) => {
      try {
        const event = JSON.parse(e.data) as ProgressEvent;
        onDone?.(event);
        eventSource.close();
      } catch {
        console.error('Failed to parse done event');
      }
    });

    eventSource.addEventListener('error', (e: MessageEvent) => {
      onError?.(e);
      eventSource.close();
    });

    eventSource.addEventListener('heartbeat', () => {
      // Connection is alive
    });

    // Return cleanup function
    return () => {
      eventSource.close();
    };
  }

  /** Unified background-activity feed for the desktop taskbar. */
  async getActivity() {
    return this.get<ActivityTask[]>('/activity', { cache: false });
  }

  // ==============================================
  // File Explorer
  // ==============================================

  async listDirectory(path: string = '/') {
    return this.get(`/explorer/list?path=${encodeURIComponent(path)}`, { cache: false });
  }

  async searchFiles(path: string, query: string, maxResults: number = 100, fuzzy = false) {
    const qs = `path=${encodeURIComponent(path)}&query=${encodeURIComponent(query)}&max_results=${maxResults}${fuzzy ? '&fuzzy=true' : ''}`;
    return this.get(`/explorer/search?${qs}`, { cache: false });
  }

  async getDisks() {
    return this.get('/explorer/disks', { cache: false });
  }

  /** Low-level Explorer mutation wrapper. Every `/explorer/*` action
   *  handler reads its arguments from the query string (`Query<…>`), not a
   *  JSON body, so `params` are serialised as `?key=value`. */
  async explorerAction(endpoint: string, params: Record<string, string>) {
    const qs = new URLSearchParams(params).toString();
    return this.post(`${endpoint}?${qs}`);
  }

  /** Low-level Explorer GET wrapper for endpoints that take query params. */
  async explorerGet(endpoint: string, params: Record<string, string>) {
    const qs = new URLSearchParams(params).toString();
    return this.get(`${endpoint}?${qs}`, { cache: false });
  }

  async mkdir(path: string) {
    return this.explorerAction('/explorer/mkdir', { path });
  }

  async renameItem(path: string, newName: string) {
    return this.explorerAction('/explorer/rename', { path, new_name: newName });
  }

  async copyItem(source: string, destination: string, strategy?: 'overwrite' | 'keep_both' | 'skip') {
    const p: Record<string, string> = { source, destination };
    if (strategy) p.strategy = strategy;
    return this.explorerAction('/explorer/copy', p);
  }

  async moveItem(source: string, destination: string, strategy?: 'overwrite' | 'keep_both' | 'skip') {
    const p: Record<string, string> = { source, destination };
    if (strategy) p.strategy = strategy;
    return this.explorerAction('/explorer/move', p);
  }

  /** Overwrite a text file's contents from the inline editor. */
  async saveFile(path: string, content: string) {
    return this.post('/explorer/save', { path, content });
  }

  /** Labelled sidebar roots ("My files", "Shares", …). */
  async getExplorerRoots() {
    return this.get('/explorer/roots', { cache: false });
  }

  /** Safely unmount + power-off an external/removable drive. */
  async ejectDrive(mountPoint: string) {
    return this.explorerAction('/explorer/eject', { path: mountPoint });
  }

  /** ZFS snapshots that cover a directory, with read-only browse paths. */
  async getExplorerSnapshots(path: string) {
    return this.explorerGet('/explorer/snapshots', { path });
  }

  async getExplorerFavorites() {
    return this.get('/explorer/favorites', { cache: false });
  }

  async addExplorerFavorite(path: string, label?: string) {
    return this.post('/explorer/favorites', { path, label });
  }

  async removeExplorerFavorite(path: string) {
    return this.delete('/explorer/favorites', { path });
  }

  /** Recently opened locations (server-side, per user, newest first). */
  async getExplorerRecents() {
    return this.get('/explorer/recents', { cache: false });
  }

  async touchExplorerRecent(path: string) {
    return this.post('/explorer/recents', { path });
  }

  async clearExplorerRecents() {
    return this.delete('/explorer/recents');
  }

  /** Per-file version timeline for the Rewind drawer. */
  async getExplorerHistory(path: string) {
    return this.explorerGet('/explorer/history', { path });
  }

  /** Nearby SMB/mDNS file servers (best-effort, never an error). */
  async getNetworkDiscovery() {
    return this.get('/explorer/network-discovery', { cache: false });
  }

  /** Wipe + mkfs an external drive (admin-only, ext4/exfat). */
  async formatDrive(device: string, fstype: 'ext4' | 'exfat', label?: string) {
    return this.post('/explorer/format', { device, fstype, label });
  }

  /** A file "share by link" row returned by the backend. */
  async listShareLinks() {
    return this.get('/explorer/shares', { cache: false });
  }

  async createShareLink(path: string, mode: 'anyone' | 'signed_in', expiresDays: number) {
    return this.post('/explorer/shares', { path, mode, expires_days: expiresDays });
  }

  async revokeShareLink(token: string) {
    return this.delete(`/explorer/shares/${encodeURIComponent(token)}`);
  }

  /** Absolute URL of a share link, for copy / QR display. */
  getShareLinkUrl(sharePath: string) {
    return `${window.location.origin}${sharePath}`;
  }

  async getExplorerWatchTicket() {
    return this.post('/explorer/watch/ticket');
  }

  getExplorerWatchUrl(ticket: string, path: string) {
    return `${this.baseUrl}/explorer/watch?ticket=${encodeURIComponent(ticket)}&path=${encodeURIComponent(path)}`;
  }

  async setPermissions(path: string, mode: string) {
    return this.explorerAction('/explorer/permissions', { path, mode });
  }

  async previewFile(path: string) {
    return this.explorerGet('/explorer/preview', { path });
  }

  async extractArchive(path: string, destination: string) {
    return this.explorerAction('/explorer/extract', { path, destination });
  }

  async compressItems(paths: string[], destination: string, format: 'tar.gz' | 'zip' = 'tar.gz') {
    return this.post('/explorer/compress', { paths, destination, format });
  }

  async trashItem(path: string) {
    return this.explorerAction('/explorer/trash', { path });
  }

  async getTrash() {
    return this.get('/explorer/trash', { cache: false });
  }

  async restoreTrashItem(id: string) {
    return this.explorerAction('/explorer/trash/restore', { id });
  }

  async purgeTrashItem(id: string) {
    return this.delete(`/explorer/trash/${encodeURIComponent(id)}`);
  }

  async emptyTrash() {
    return this.delete('/explorer/trash');
  }

  /**
   * Mint a short-lived, single-use download ticket for `path`. The server
   * checks the caller's download quota here, so an over-quota rejection
   * surfaces as a real error instead of a silently-failed browser download.
   */

  /**
   * Mint a multi-use, directory-scoped thumbnail ticket. One is fetched per
   * directory listing and reused for every thumbnail `<img>`, so the session
   * token never appears in a URL.
   */
  async getThumbnailTicket(dirPath: string): Promise<string | null> {
    const r = await this.post<{ ticket: string }>(
      `/explorer/thumbnail-ticket?path=${encodeURIComponent(dirPath)}`
    );
    return r.success && r.data ? r.data.ticket : null;
  }

  getDownloadUrlWithTicket(ticket: string) {
    return `${this.baseUrl}/explorer/download?ticket=${encodeURIComponent(ticket)}`;
  }

  /**
   * Downloads a file by first minting a short-lived ticket, then handing the
   * ticket URL to the browser's download manager. Streaming through the
   * browser (instead of `fetch` → `blob()`) means a multi-gigabyte file is
   * never buffered whole in tab memory, and HTTP range/resume works.
   *
   * The ticket endpoint is what carries the auth header, so a quota or
   * permission rejection is still reported back to the caller.
   */
  async downloadFile(path: string, filename: string): Promise<{ success: boolean; error?: string }> {
    try {
      const ticket = await this.issueDownloadTicket(path);
      if (!ticket.success || !ticket.data) {
        return { success: false, error: ticket.message || ticket.error || 'Download not allowed' };
      }

      const a = document.createElement('a');
      a.href = this.getDownloadUrlWithTicket(ticket.data.ticket);
      a.download = filename;
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      return { success: true };
    } catch (e) {
      return { success: false, error: e instanceof Error ? e.message : 'Network error during download' };
    }
  }

  /** Mint a single-use download ticket for `path` (quota + access checked
   *  server-side). The ticket powers `getDownloadUrlWithTicket`. */
  async issueDownloadTicket(path: string) {
    return this.post(`/explorer/download-ticket?path=${encodeURIComponent(path)}`, undefined);
  }

  /**
   * Thumbnail URL for an image file. `ticket` is the directory-scoped ticket
   * from `getThumbnailTicket`; without it the request is unauthenticated and
   * will 401 (the caller's `onerror` then hides the image).
   */
  getThumbnailUrl(path: string, ticket?: string) {
    const base = `${this.baseUrl}/explorer/thumbnail?path=${encodeURIComponent(path)}`;
    return ticket ? `${base}&ticket=${encodeURIComponent(ticket)}` : base;
  }

  // ---- Resumable chunked uploads ----------------------------------------
  //
  // A file is streamed in fixed-size chunks through a server-side session so
  // an interrupted upload can resume from the server's authoritative offset
  // instead of restarting from zero.
  private static readonly UPLOAD_CHUNK_SIZE = 4 * 1024 * 1024;

  /** Open a server-side resumable upload slot for one file.
   *  Returns `{ session_id }` on success. */
  async uploadSessionStart(path: string, filename: string, size: number, strategy?: string) {
    const qs = new URLSearchParams({ path, filename, size: String(size) });
    if (strategy) qs.set('strategy', strategy);
    return this.post(`/explorer/upload/session?${qs.toString()}`, undefined);
  }

  /** Send one raw chunk at `offset`. On a 409 the response carries the
   *  server's authoritative `offset` so the upload can resume from there. */
  async uploadChunk(sessionId: string, offset: number, chunk: Blob) {
    try {
      const response = await fetch(
        `${this.baseUrl}/explorer/upload/chunk?session_id=${encodeURIComponent(sessionId)}&offset=${offset}`,
        {
          method: 'POST',
          headers: this.getHeaders(),
          body: chunk,
        },
      );
      return await response.json();
    } catch (error) {
      return {
        success: false,
        error: error instanceof Error ? error.message : 'Network error',
      };
    }
  }

  /** Abort an upload session and discard its temp file. */
  async uploadSessionAbort(sessionId: string) {
    return this.delete(`/explorer/upload/session?session_id=${encodeURIComponent(sessionId)}`);
  }

  /** Finalise an upload session (renames the temp file into place). */
  async uploadComplete(sessionId: string) {
    return this.post(`/explorer/upload/complete?session_id=${encodeURIComponent(sessionId)}`, undefined);
  }

  /** Sends one chunk; on a 409 the response carries the server's `offset`. */

  /**
   * Upload a file via the resumable chunked-upload flow, reporting progress
   * per chunk. A failed chunk is retried; if the server reports a different
   * offset (a partial write), the upload resumes from there.
   */
  async uploadFile(
    path: string,
    file: File,
    onProgress?: (loaded: number, total: number) => void,
    strategy?: 'overwrite' | 'keep_both' | 'skip',
  ): Promise<{ success: boolean; message?: string; error?: string; data?: any }> {
    const chunkSize = ApiClient.UPLOAD_CHUNK_SIZE;
    const total = file.size;

    const started = await this.uploadSessionStart(path, file.name, total, strategy);
    if (!started.success || !started.data) {
      return { success: false, error: started.message || started.error || 'Could not start upload' };
    }
    const sessionId = started.data.session_id;

    try {
      let offset = 0;
      while (offset < total) {
        let slice = file.slice(offset, Math.min(offset + chunkSize, total));
        let sent = false;
        for (let attempt = 0; attempt < 4 && !sent; attempt++) {
          const r = await this.uploadChunk(sessionId, offset, slice);
          if (r.success) {
            sent = true;
          } else if (typeof r.offset === 'number' && r.offset !== offset) {
            // Server is authoritative about what landed — resume from there.
            offset = r.offset;
            slice = file.slice(offset, Math.min(offset + chunkSize, total));
          }
        }
        if (!sent) {
          await this.uploadSessionAbort(sessionId).catch(() => {});
          return { success: false, error: 'Upload interrupted — please retry' };
        }
        offset += slice.size;
        onProgress?.(offset, total);
      }

      const done = await this.uploadComplete(sessionId);
      if (!done.success) {
        await this.uploadSessionAbort(sessionId).catch(() => {});
      }
      return done;
    } catch (e) {
      await this.uploadSessionAbort(sessionId).catch(() => {});
      return { success: false, error: e instanceof Error ? e.message : 'Network error during upload' };
    }
  }

  // ==============================================
  // Syncthing
  // ==============================================

  async getSyncthingStatus() {
    return this.get('/syncthing/status', { ttl: 5000 });
  }

  async getSyncthingDevices() {
    return this.get('/syncthing/devices', { ttl: 3000 });
  }

  async getSyncthingFolders() {
    return this.get('/syncthing/folders', { ttl: 3000 });
  }

  async restartSyncthing() {
    return this.post('/syncthing/restart');
  }

  async pauseSyncthingDevice(device: string) {
    return this.post('/syncthing/pause', { device });
  }

  async resumeSyncthingDevice(device: string) {
    return this.post('/syncthing/resume', { device });
  }

  async scanSyncthingFolder(folder: string) {
    return this.post('/syncthing/scan', { folder });
  }

  // ==============================================
  // System Journal (centralized host logs & events)
  // ==============================================

  async getJournalSources() {
    return this.get('/journal/sources', { cache: false });
  }

  async getJournalEntries(params: {
    source?: string;
    unit?: string;
    priority?: number;
    limit?: number;
    search?: string;
  }) {
    const qs = new URLSearchParams();
    if (params.source) qs.set('source', params.source);
    if (params.unit) qs.set('unit', params.unit);
    if (params.priority !== undefined) qs.set('priority', String(params.priority));
    if (params.limit !== undefined) qs.set('limit', String(params.limit));
    if (params.search) qs.set('search', params.search);
    const query = qs.toString();
    return this.get(`/journal/entries${query ? `?${query}` : ''}`, { cache: false });
  }

  async clearJournalLogs() {
    return this.delete('/journal/clear');
  }

  async getJournalCleanupSettings() {
    return this.get('/journal/cleanup-settings', { cache: false });
  }

  async updateJournalCleanupSettings(settings: { auto_cleanup_enabled: boolean; interval_days: number }) {
    return this.put('/journal/cleanup-settings', settings);
  }

  // ==============================================
  // Transfer quotas (upload/download caps for Users/Groups pages)
  // ==============================================

  async getTransferQuota(subjectType: 'user' | 'group', subjectName: string) {
    return this.get(`/transfer-quotas/${subjectType}/${encodeURIComponent(subjectName)}`, { cache: false });
  }

  /** Every configured quota for every subject of one type — used by the "All quotas" overview. */
  async listTransferQuotas(subjectType: 'user' | 'group') {
    return this.get(`/transfer-quotas/${subjectType}`, { cache: false });
  }

  async setTransferQuota(
    subjectType: 'user' | 'group',
    subjectName: string,
    body: { upload_enabled: boolean; upload_limit_bytes: number; download_enabled: boolean; download_limit_bytes: number }
  ) {
    return this.put(`/transfer-quotas/${subjectType}/${encodeURIComponent(subjectName)}`, body);
  }

  // ==============================================
  // Avatars (DiceBear Miniavs, generated offline server-side)
  // ==============================================

  /**
   * Build the URL of a user's persistent avatar SVG.
   * Each seed change replaces the cached avatar on the server.
   */
  userAvatarUrl(username: string, size = 64): string {
    const qs = size ? `?size=${size}` : '';
    return `${this.baseUrl}/users/${encodeURIComponent(username)}/avatar.svg${qs}`;
  }

  /**
   * Fetch the URL of a freshly-generated preview SVG for a given seed.
   * The endpoint returns the raw SVG bytes (Content-Type: image/svg+xml),
   * so this returns a Blob URL the caller can plug into an `<img src>`.
   *
   * Each call passes `&t=<ts>` to bust any browser/proxy cache.
   */
  async previewAvatarSvg(seed: string, size = 128): Promise<string> {
    const params = new URLSearchParams({ seed, size: String(size), fresh: 'true' });
    const response = await fetch(`${this.baseUrl}/avatars/preview?${params.toString()}`, {
      method: 'GET',
      headers: this.getHeaders(false),
    });
    if (!response.ok) {
      throw new Error(`Avatar preview failed: ${response.status}`);
    }
    const svg = await response.text();
    // Encode the SVG into a data URI so the caller can use it as an
    // `<img src>` without leaking a Blob URL across navigations.
    return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
  }

  /**
   * Persist a new seed for the current user. The server regenerates the
   * avatar (deterministically from the seed) and returns the new metadata.
   */
  async updateMyAvatar(seed: string) {
    return this.post('/auth/me/avatar', { seed });
  }

  // ==============================================
  // Storage (Disks, Partitions, RAID)
  // ==============================================

  async getStorageDisks() {
    return this.get('/storage/disks', { ttl: 10000 });
  }

  async getDiskSmartCapabilities(name: string) {
    return this.get(`/storage/disks/${encodeURIComponent(name)}/smart/capabilities`, { cache: false });
  }

  async startDiskSmartTest(name: string, testType: string) {
    const res = await this.post(`/storage/disks/${encodeURIComponent(name)}/smart/test`, { test_type: testType });
    return res;
  }

  async getDiskSmartTestStatus(name: string) {
    return this.get(`/storage/disks/${encodeURIComponent(name)}/smart/test`, { cache: false });
  }

  async getStoragePartitions() {
    return this.get('/storage/partitions', { ttl: 10000 });
  }

  async getStorageRemovable() {
    return this.get('/storage/removable', { ttl: 5000 });
  }

  async getRaidArrays() {
    return this.get('/storage/raid', { ttl: 10000 });
  }

  async getRaidAvailableDevices() {
    return this.get('/storage/raid/devices', { ttl: 10000 });
  }

  async createRaid(data: {
    name: string;
    level: string;
    devices: string[];
    filesystem: string;
    /** Extra disks attached as hot spares (mdadm techs only). */
    spares?: string[];
    /** Create raid1/5/6 with `missing` slots filling up to the level min. */
    allow_missing?: boolean;
  }) {
    return this.post('/storage/raid', data);
  }

  /**
   * Grow an mdadm virtual disk onto an additional free device. The reshape
   * runs in the background — poll the scrub/sync status endpoint for
   * progress.
   */
  async growVirtualDisk(name: string, device: string) {
    const res = await this.post(`/storage/virtual-disks/${encodeURIComponent(name)}/grow`, {
      device
    });
    this.invalidateCache('/storage/virtual-disks');
    return res;
  }

  async rebuildRaid(name: string, replacementDevice?: string) {
    const res = await this.post(`/storage/raid/${encodeURIComponent(name)}/rebuild`, {
      replacement_device: replacementDevice ?? null
    });
    this.invalidateCache('/storage/raid');
    return res;
  }

  // --- Virtual disks (STORAGE's unified mdadm + ZFS + Btrfs management) ---

  async listVirtualDisks() {
    return this.get('/storage/virtual-disks', { ttl: 8000 });
  }

  async deleteVirtualDisk(name: string) {
    const res = await this.delete(`/storage/virtual-disks/${encodeURIComponent(name)}`);
    this.invalidateCache('/storage/virtual-disks');
    this.invalidateCache('/storage/raid');
    return res;
  }

  async scrubVirtualDisk(name: string) {
    return this.post(`/storage/virtual-disks/${encodeURIComponent(name)}/scrub`, {});
  }

  /** Current scrub / consistency-check status of a virtual disk. */
  async getVirtualDiskScrubStatus(name: string) {
    return this.get(`/storage/virtual-disks/${encodeURIComponent(name)}/scrub`, { cache: false });
  }

  /** Pools present on attached disks but not yet imported ("foreign" pools). */
  async getImportablePools() {
    return this.get('/storage/importable-pools', { cache: false });
  }

  /** Import a foreign ZFS/Btrfs pool (admin). `new_name` renames on import (ZFS). */
  async importPool(name: string, poolType: string, newName?: string) {
    const res = await this.post('/storage/import-pool', {
      name,
      pool_type: poolType,
      new_name: newName ?? null,
    });
    this.invalidateCache('/storage/virtual-disks');
    this.invalidateCache('/storage/importable-pools');
    return res;
  }

  // ==============================================
  // Disk encryption (LUKS2, manual unlock)
  // ==============================================

  async getEncryptedVolumes() {
    return this.get('/encryption/volumes', { ttl: 5000 });
  }

  async getEncryptionAvailableDevices() {
    return this.get('/encryption/available-devices', { ttl: 5000 });
  }

  async createEncryptedVolume(data: { device: string; filesystem: string; label: string; passphrase: string }) {
    const res = await this.post('/encryption/volumes', data);
    this.invalidateCache('/encryption/volumes');
    return res;
  }

  async unlockEncryptedVolume(id: string, passphrase: string) {
    const res = await this.post(`/encryption/volumes/${encodeURIComponent(id)}/unlock`, { passphrase });
    this.invalidateCache('/encryption/volumes');
    return res;
  }

  async lockEncryptedVolume(id: string) {
    const res = await this.post(`/encryption/volumes/${encodeURIComponent(id)}/lock`, {});
    this.invalidateCache('/encryption/volumes');
    return res;
  }

  async deleteEncryptedVolume(id: string) {
    const res = await this.delete(`/encryption/volumes/${encodeURIComponent(id)}`, { confirm: true });
    this.invalidateCache('/encryption/volumes');
    return res;
  }

  /**
   * Streams a chat reply as it's generated. EventSource can't send a POST
   * body, so this reads `/chat/send/stream`'s `text/event-stream` response
   * manually via `fetch` + a stream reader instead. Returns a function the
   * caller can invoke to abort generation early.
   */
  chatSendStream(
    message: string,
    history: { role: string; content: string }[],
    conversationId: string | undefined,
    language: string | undefined,
    onChunk: (content: string) => void,
    onDone: (data: { conversation_id: string; model: string }) => void,
    onError: (message: string) => void
  ): () => void {
    const controller = new AbortController();

    (async () => {
      try {
        const response = await fetch(`${this.baseUrl}/chat/send/stream`, {
          method: 'POST',
          headers: this.getHeaders(),
          body: JSON.stringify({
            message,
            history,
            conversation_id: conversationId,
            language,
          }),
          signal: controller.signal,
        });

        if (!response.ok || !response.body) {
          const text = await response.text().catch(() => '');
          onError(text || `HTTP ${response.status}`);
          return;
        }

        const reader = response.body.getReader();
        const decoder = new TextDecoder();
        let buffer = '';

        while (true) {
          const { value, done } = await reader.read();
          if (done) break;
          buffer += decoder.decode(value, { stream: true });

          let sep: number;
          while ((sep = buffer.indexOf('\n\n')) !== -1) {
            const rawEvent = buffer.slice(0, sep);
            buffer = buffer.slice(sep + 2);

            let eventName = 'message';
            let data = '';
            for (const line of rawEvent.split('\n')) {
              if (line.startsWith('event:')) eventName = line.slice(6).trim();
              else if (line.startsWith('data:')) data += line.slice(5).trim();
            }
            if (!data) continue;

            if (eventName === 'chunk') {
              try {
                onChunk((JSON.parse(data) as { content: string }).content);
              } catch {
                // Ignore a malformed chunk rather than aborting the whole stream
              }
            } else if (eventName === 'done') {
              try {
                onDone(JSON.parse(data));
              } catch {
                onError('Malformed done event');
              }
            } else if (eventName === 'error') {
              onError(data);
            }
          }
        }
      } catch (error) {
        if (error instanceof DOMException && error.name === 'AbortError') return;
        onError(error instanceof Error ? error.message : 'Network error');
      }
    })();

    return () => controller.abort();
  }

  async chatListModels() {
    return this.get('/chat/models', { cache: false });
  }

  async chatPullModel(name: string) {
    return this.post('/chat/models/pull', { name });
  }

  async chatSetModel(model: string) {
    return this.put('/chat/model', { model });
  }

  async chatRecommendModels() {
    return this.get('/chat/models/recommend', { cache: false });
  }

  // ==============================================
  // RAG Knowledge Base
  // ==============================================

  // ==============================================
  // External Agents (MCP server)
  // ==============================================

  async mcpStatus() {
    // Never cached: the token list must reflect creates/revokes
    // immediately (a `ttl: 0` would fall back to the default TTL).
    return this.get('/mcp/status', { cache: false });
  }

  async mcpSetEnabled(enabled: boolean) {
    return this.put('/mcp/status', { enabled });
  }

  async mcpSetPermissions(permissions: Record<string, unknown>) {
    return this.put('/mcp/permissions', permissions);
  }

  async mcpCreateToken(name: string) {
    return this.post('/mcp/tokens', { name });
  }

  async mcpRevokeToken(id: string) {
    return this.delete(`/mcp/tokens/${encodeURIComponent(id)}`);
  }

  // ==============================================
  // Restic
  // ==============================================

  async resticAvailable() {
    return this.get<{ available: boolean }>('/restic/available', { ttl: 60000 });
  }

  async resticVersion() {
    return this.get<{ version: string }>('/restic/version');
  }

  async resticListRepos(includePassword = false) {
    return this.get<ResticRepo[]>(`/restic/repos?include_password=${includePassword}`);
  }

  async resticCreateRepo(data: {
    name: string;
    repository: string;
    password: string;
    retention_policy_id?: string;
    source_paths: string[];
    exclude_patterns?: string;
    tags?: string;
    snapshot_source?: string;
    snapshot_pool?: string;
    snapshot_dataset?: string;
    snapshot_create_before_backup?: boolean;
  }) {
    return this.post('/restic/repos', data);
  }

  async resticDeleteRepo(id: string) {
    return this.delete(`/restic/repos/${encodeURIComponent(id)}`);
  }

  async resticListSnapshots(repoId: string) {
    return this.get<ResticSnapshotInfo[]>(`/restic/repos/${encodeURIComponent(repoId)}/snapshots`);
  }

  async resticBackup(data: {
    repo_id: string;
    tags?: string;
    exclude_patterns?: string;
    snapshot_name?: string;
  }) {
    return this.post('/restic/backup', data);
  }

  async resticRestore(data: {
    repo_id: string;
    snapshot_id: string;
    target: string;
    exclude_patterns?: string;
    include_paths?: string[];
  }) {
    return this.post('/restic/restore', data);
  }

  /** Pre-flight space check before a restore (required/available/fits). */
  async resticRestorePlan(repoId: string, snapshotId: string, target: string) {
    const qs = `repo_id=${encodeURIComponent(repoId)}&snapshot_id=${encodeURIComponent(snapshotId)}&target=${encodeURIComponent(target)}`;
    return this.get(`/restic/restore-plan?${qs}`, { cache: false });
  }

  /** Repos with no successful backup in the last 7 days. */
  async resticReminders() {
    return this.get('/restic/reminders', { cache: false });
  }

  async resticCheck(data: {
    repo_id: string;
    read_data?: boolean;
  }) {
    return this.post('/restic/check', data);
  }

  async resticForget(data: {
    repo_id: string;
    keep_last?: number;
    keep_hourly?: number;
    keep_daily?: number;
    keep_weekly?: number;
    keep_monthly?: number;
    keep_yearly?: number;
    keep_within?: string;
    prune?: boolean;
  }) {
    return this.post('/restic/forget', data);
  }

  async resticPrune(repoId: string) {
    return this.post(`/restic/repos/${encodeURIComponent(repoId)}/prune`);
  }

  async resticUnlock(repoId: string) {
    return this.post(`/restic/repos/${encodeURIComponent(repoId)}/unlock`);
  }

  async resticStats(repoId: string) {
    return this.get<ResticStats>(`/restic/repos/${encodeURIComponent(repoId)}/stats`);
  }

  async resticBackupJobs(repoId: string) {
    return this.get(`/restic/repos/${encodeURIComponent(repoId)}/jobs`);
  }

  async resticDeleteSnapshot(repoId: string, snapshotId: string) {
    return this.post('/restic/snapshots/delete', { repo_id: repoId, snapshot_id: snapshotId });
  }

  // ==============================================
  // Restic backup schedules & retention policies
  // ==============================================

  async resticListSchedules(repoId?: string) {
    const qs = repoId ? `?repo_id=${encodeURIComponent(repoId)}` : '';
    return this.get(`/restic/schedules${qs}`, { cache: false });
  }

  async resticCreateSchedule(data: { repo_id: string; name: string; cron_expression: string; enabled?: boolean }) {
    return this.post('/restic/schedules', data);
  }

  async resticSetScheduleEnabled(id: string, enabled: boolean) {
    return this.put(`/restic/schedules/${encodeURIComponent(id)}/enabled`, { enabled });
  }

  async resticDeleteSchedule(id: string) {
    return this.delete(`/restic/schedules/${encodeURIComponent(id)}`);
  }

  async resticListRetentionPolicies() {
    return this.get('/restic/retention-policies', { cache: false });
  }

  async resticCreateRetentionPolicy(data: { name: string; max_count?: number; max_age_days?: number; max_size_gb?: number; apply_to_all?: boolean }) {
    return this.post('/restic/retention-policies', data);
  }

  async resticDeleteRetentionPolicy(id: string) {
    return this.delete(`/restic/retention-policies/${encodeURIComponent(id)}`);
  }

  // ==============================================
  // Downloader (aria2)
  // ==============================================

  async getDownloaderStatus() {
    return this.get<{ available: boolean; running: boolean }>('/downloader/status', { cache: false });
  }

  // ==============================================
  // Cache tier (HomeVault write caching)
  // ==============================================

  async getCacheSettings() {
    return this.get<CacheSettings>('/cache/settings', { cache: false });
  }

  async updateCacheSettings(data: Partial<CacheSettings>) {
    return this.put('/cache/settings', data);
  }

  async getCachePools() {
    return this.get<CachePoolInfo[]>('/cache/pools', { cache: false });
  }

  async getCacheShares() {
    return this.get<CacheShare[]>('/cache/shares', { cache: false });
  }

  async createCacheShare(data: { name: string; cache_mode: string; target_pool: string }) {
    return this.post('/cache/shares', data);
  }

  async updateCacheShare(name: string, data: { cache_mode?: string; target_pool?: string; enabled?: boolean }) {
    return this.put(`/cache/shares/${encodeURIComponent(name)}`, data);
  }

  async deleteCacheShare(name: string) {
    return this.delete(`/cache/shares/${encodeURIComponent(name)}`);
  }

  async runCacheMover() {
    return this.post('/cache/mover/run');
  }

  async getCacheMoverRuns() {
    return this.get<CacheMoverRun[]>('/cache/mover/runs', { cache: false });
  }

  async startDownloader() {
    return this.post('/downloader/start');
  }

  async getDownloaderGlobalStat() {
    return this.get('/downloader/global-stat', { cache: false });
  }

  async getDownloaderTasks() {
    return this.get('/downloader/tasks', { cache: false });
  }

  async addDownloaderUrl(uri: string, dir?: string, out?: string) {
    return this.post('/downloader/add', { uri, dir, out });
  }

  async pauseDownloader(gid: string) {
    return this.post('/downloader/pause', { gid });
  }

  async unpauseDownloader(gid: string) {
    return this.post('/downloader/unpause', { gid });
  }

  async removeDownloader(gid: string) {
    return this.post('/downloader/remove', { gid });
  }

  // ==============================================
  // Rescue Tools (ddrescue, TestDisk, PhotoRec, mdadm)
  // ==============================================

  async getRescueTools() {
    return this.get<RescueToolsResponse>('/rescue/tools', { cache: true, ttl: 10000 });
  }

  async getRescueDevices() {
    return this.get<RescueBlockDevice[]>('/rescue/devices', { cache: true, ttl: 5000 });
  }

  async ddrescueClone(source: string, destination: string, options?: {
    logfile?: string;
    block_size?: number;
    retry_passes?: number;
    direct_io?: boolean;
    reverse?: boolean;
    verbose?: boolean;
  }) {
    return this.post('/rescue/ddrescue/clone', { source, destination, ...(options || {}) });
  }

  async testdiskAnalyze(device: string) {
    // No options: the scripted analyse always deep-scans and writes its own
    // log, and the API now *rejects* `deep_search`/`log_path` instead of
    // accepting them and silently ignoring them.
    return this.post('/rescue/testdisk/analyze', { device });
  }

  async testdiskRebuild(device: string) {
    // Same: the partition table is never written back automatically, so there
    // is no `force` to send — the API rejects it.
    return this.post('/rescue/testdisk/rebuild', { device });
  }

  async photorecRecover(device: string, output_dir: string, options?: {
    file_types?: string[];
    partition_type?: string;
  }) {
    // `keep_broken` and `max_size` are gone: neither exists in PhotoRec's
    // scripted /cmd grammar, which is the only mode that works headless.
    return this.post('/rescue/photorec/recover', { device, output_dir, ...(options || {}) });
  }

  async mdadmDiagnose(name: string) {
    return this.post('/rescue/mdadm/diagnose', { name });
  }

  async mdadmRepair(name: string, options?: {
    action?: string;
    force?: boolean;
    verbose?: boolean;
  }) {
    return this.post('/rescue/mdadm/repair', { name, ...(options || {}) });
  }

  /** Cancel a running background job for real.
   *
   * `DELETE /api/progress/{job_id}` runs the job producer's registered
   * cancellation hook — for rescue jobs that SIGTERMs the tool's whole
   * process group, so a multi-hour `ddrescue` clone or a PhotoRec carve can
   * actually be stopped from the UI. The response says what happened:
   * `{ job_id, cancelled, message }`, and an unknown id is a 400. */
  async cancelJob(jobId: string) {
    return this.delete<{ job_id: string; cancelled: boolean; message: string }>(
      `/progress/${encodeURIComponent(jobId)}`
    );
  }

  // ==============================================
  // Share permissions (per-user/group ACLs on a share)
  // ==============================================

  async getSharePermissions(name: string, shareType: string) {
    return this.get(`/shares/${encodeURIComponent(name)}/${encodeURIComponent(shareType)}/permissions`, { cache: false });
  }

  async grantSharePermission(name: string, shareType: string, body: {
    grantee_type: string;
    grantee_name: string;
    permission: string;
    recursive?: boolean;
  }) {
    return this.post(`/shares/${encodeURIComponent(name)}/${encodeURIComponent(shareType)}/permissions`, body);
  }

  async revokeSharePermission(name: string, shareType: string, body: { grantee_type: string; grantee_name: string }) {
    return this.delete(`/shares/${encodeURIComponent(name)}/${encodeURIComponent(shareType)}/permissions`, body);
  }

  async syncSharePermissions(name: string, shareType: string) {
    return this.post(`/shares/${encodeURIComponent(name)}/${encodeURIComponent(shareType)}/permissions/sync`);
  }

  // ==============================================
  // Antivirus (ClamAV)
  // ==============================================

  async getAntivirusStatus() {
    return this.get('/antivirus/status', { ttl: 5000 });
  }

  async getAntivirusSettings() {
    return this.get('/antivirus/settings', { cache: false });
  }

  async updateAntivirusSettings(data: {
    quarantine_dir: string;
    scheduled_scan_enabled: boolean;
    scheduled_scan_cron: string;
  scheduled_scan_path: string | undefined;
    scan_archives: boolean;
  }) {
    return this.put('/antivirus/settings', data);
  }

  async startAntivirusScan(path: string) {
    return this.post('/antivirus/scan', { path });
  }

  async cancelAntivirusScan() {
    return this.post('/antivirus/scan/cancel');
  }

  async getAntivirusScanHistory() {
    return this.get('/antivirus/scans', { cache: false });
  }

  async getAntivirusThreats() {
    return this.get('/antivirus/threats', { cache: false });
  }

  async resolveAntivirusThreat(id: string, deleteFile: boolean) {
    return this.post(`/antivirus/threats/${encodeURIComponent(id)}/resolve`, { delete: deleteFile });
  }

  // ==============================================
  // Anti-Ransomware Protection
  // ==============================================

  async getRansomwareStatus() {
    return this.get('/ransomware/status', { ttl: 5000 });
  }

  async getRansomwareSettings() {
    return this.get('/ransomware/settings', { cache: false });
  }

  async updateRansomwareSettings(data: {
    enabled: boolean;
    honeypots_enabled: boolean;
    rate_threshold_events: number;
    rate_threshold_seconds: number;
    auto_response: 'alert_only' | 'lock_account' | 'disable_share';
  }) {
    return this.put('/ransomware/settings', data);
  }

  async listProtectedPaths() {
    return this.get('/ransomware/protected-paths', { cache: false });
  }

  async addProtectedPath(data: {
    path: string;
    share_name?: string;
    share_type?: string;
    place_honeypot?: boolean;
  }) {
    return this.post('/ransomware/protected-paths', data);
  }

  async deleteProtectedPath(id: string) {
    return this.delete(`/ransomware/protected-paths/${encodeURIComponent(id)}`);
  }

  async getRansomwareEvents() {
    return this.get('/ransomware/events', { cache: false });
  }

  // ==============================================
  // Firewall (ufw)
  // ==============================================

  async getFirewallStatus() {
    return this.get('/firewall/status', { ttl: 5000 });
  }

  async getFirewallEnablePreview() {
    return this.get('/firewall/enable/preview', { cache: false });
  }

  async enableFirewall() {
    return this.post('/firewall/enable', { confirmed: true });
  }

  /**
   * Disabling a firewall opens every port it was blocking, so the server
   * requires the same explicit confirmation as enabling.
   */
  async disableFirewall() {
    return this.post('/firewall/disable', { confirmed: true });
  }

  async addFirewallRule(data: { port: string; protocol?: string; action?: string; source?: string }) {
    return this.post('/firewall/rules', data);
  }

  /**
   * Delete by the rule's own live `spec` (preferred: it identifies one rule
   * exactly, including its `from <source>` clause) or by its identity fields.
   * The server looks the rule up in the live firewall and refuses (404) if it
   * is gone, so a stale page can never delete the wrong rule.
   */
  async deleteFirewallRule(data: {
    spec?: string;
    port?: string;
    protocol?: string;
    action?: string;
    source?: string;
  }) {
    return this.delete('/firewall/rules', data);
  }

  async setFirewallDefaultPolicy(data: { direction: 'incoming' | 'outgoing' | 'routed'; policy: 'allow' | 'deny' | 'reject' }) {
    return this.post('/firewall/default-policy', data);
  }

  async getFirewallHistory(limit = 50) {
    return this.get(`/firewall/history?limit=${limit}`, { cache: false });
  }

  // --- Built-in HTTPS/TLS (admin-only; applies after a restart) ---
  async getTlsSettings() {
    return this.get('/settings/tls', { cache: false });
  }

  async setTlsSettings(data: {
    enabled: boolean;
    https_port?: number | null;
    mode: 'self-signed' | 'manual';
    cert_path?: string | null;
    key_path?: string | null;
    redirect: boolean;
  }) {
    return this.put('/settings/tls', data);
  }

  async regenerateTlsCert() {
    return this.post('/settings/tls/regenerate');
  }

}

// Types for Rescue Tools
interface RescueToolStatus {
  name: string;
  available: boolean;
  version: string | null;
}

interface RescueToolsResponse {
  tools: RescueToolStatus[];
}

interface RescueBlockDevice {
  device: string;
  name: string;
  size: number;
  model: string;
  transport: string;
  removable: boolean;
}

interface DdrescueLogStatus {
  job_id: string;
  progress: number;
  message: string;
  bytes_copied: number | null;
  bytes_total: number | null;
  speed: string | null;
  running: boolean;
}
interface ResticRepo {
  id: string;
  name: string;
  repository: string;
  password: string;
  retention_policy_id: string | null;
  source_paths: string[];
  exclude_patterns: string | null;
  tags: string | null;
  snapshot_source: string;
  snapshot_pool: string | null;
  snapshot_dataset: string | null;
  snapshot_create_before_backup: boolean;
  created_at: string;
  updated_at: string;
}

interface ResticSnapshotInfo {
  id: string;
  short_id: string;
  time: string;
  tree: string;
  paths: string[];
  hostname: string;
  program_version: string;
  tags: string[] | null;
}

interface ResticStats {
  total_size: number;
  total_file_count: number;
  snapshots_count: number;
  total_blob_count: number;
  compressed_size: number;
}

// ── Cache tier (disk write caching) ──
export interface CacheSettings {
  cache_pool: string | null;
  mover_cron: string;
  mover_enabled: boolean;
  min_free_percent: number;
  mover_last_run_at: string | null;
  mover_age_days: number;
  mover_excludes: string | null;
  mover_bwlimit: number;
}

export interface CachePoolInfo {
  name: string;
  pool_type: string;
  mount_point: string | null;
  size_bytes: number;
  used_bytes: number;
  available_bytes: number;
}

export interface CacheShare {
  id: string;
  name: string;
  cache_mode: 'yes' | 'no' | 'prefer' | 'only';
  target_pool: string;
  enabled: boolean;
  created_at: string;
  updated_at: string;
}

export interface CacheMoverRun {
  id: string;
  status: string;
  started_at: string;
  finished_at: string | null;
  files_moved: number;
  bytes_moved: number;
  error_message: string | null;
}

// ── Antivirus (ClamAV) ──
export interface AvSettings {
  quarantine_dir: string;
  scheduled_scan_enabled: boolean;
  scheduled_scan_cron: string;
  scheduled_scan_path: string | undefined;
  scan_archives: boolean;
  last_scan_at: string | null;
}

export interface AvScan {
  id: string;
  scan_path: string;
  trigger: string;
  status: string;
  files_scanned: number;
  threats_found: number;
  started_at: string;
  finished_at: string | null;
  error_message: string | null;
}

export interface AvThreat {
  id: string;
  scan_id: string;
  file_path: string;
  signature_name: string;
  action: string;
  quarantine_path: string | null;
  detected_at: string;
  resolved_at: string | null;
}

// ── Anti-Ransomware Protection ──
export interface RansomwareSettings {
  enabled: boolean;
  honeypots_enabled: boolean;
  rate_threshold_events: number;
  rate_threshold_seconds: number;
  auto_response: 'alert_only' | 'lock_account' | 'disable_share';
}

export interface ProtectedPath {
  id: string;
  path: string;
  share_name: string | null;
  share_type: string | null;
  honeypot_filename: string | null;
  created_at: string;
}

export interface RansomwareEvent {
  id: string;
  protected_path_id: string | null;
  event_type: string;
  detail: string | null;
  suspected_username: string | null;
  response_taken: string;
  created_at: string;
}

// ── Firewall (ufw) ──
export interface FirewallRule {
  number: number;
  port: string;
  protocol: string;
  action: string;
  source: string | null;
  comment: string | null;
  is_mandatory: boolean;
  /** The rule exactly as `ufw show added` printed it, comment stripped —
   *  what the delete endpoint takes to remove this rule and no other. */
  spec: string;
}

export interface FirewallDefaultPolicy {
  incoming: string;
  outgoing: string;
  routed: string;
}

export interface FirewallHistoryEntry {
  id: string;
  action: string;
  rule_description: string | null;
  performed_by: string;
  created_at: string;
}

export interface FileEntry {
  name: string;
  path: string;
  is_directory: boolean;
  size_bytes: number;
  modified: string | null;
  permissions: string | null;
  owner: string | null;
  filesystem: string | null;
}

export interface DirectoryListing {
  path: string;
  parent: string | null;
  entries: FileEntry[];
  total_size: number;
  file_count: number;
  directory_count: number;
}

// Scheduled tasks (generic cron engine — see docs/guides/scheduled-tasks.md)
export interface ScheduledTask {
  id: string;
  name: string;
  task_type: 'command' | 'snapshot' | 'smart_test' | 'power';
  cron: string;
  payload: string;
  enabled: boolean;
  last_run_at: string | null;
  last_status: string | null;
  last_error: string | null;
  next_run_at: string | null;
  created_at: string;
  updated_at: string;
}

export interface ScheduledTaskRun {
  id: string;
  task_id: string;
  status: 'running' | 'success' | 'failed';
  started_at: string;
  finished_at: string | null;
  output: string | null;
}

export interface SnapshotSchedule {
  id: string;
  pool: string;
  enabled: boolean;
  cron: string;
  prefix: string;
  recursive: boolean;
  gfs_hourly: number;
  gfs_daily: number;
  gfs_weekly: number;
  gfs_monthly: number;
  gfs_yearly: number;
  last_run_at: string | null;
  last_status: string | null;
  last_error: string | null;
  next_run_at: string | null;
  created_at: string;
  updated_at: string;
}

export interface MetricsPoint {
  ts: number;
  cpu: number | null;
  mem: number | null;
  load1: number | null;
  net_rx: number | null;
  net_tx: number | null;
  temp: number | null;
}

export interface DiskSpindownSetting {
  device: string;
  apm_level: number | null;
  spindown_minutes: number | null;
}

// Export singleton instance
export const api = new ApiClient();

