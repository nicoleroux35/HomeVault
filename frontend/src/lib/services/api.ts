/* =============================================================================
 * File          : api.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 24/10/2026 by Nicolas Le Roux
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
 *   Since ROADMAP 31.1 it is the **only** HTTP path of the frontend: the
 *   Docker app used to carry its own local `authedFetch` helper, and every
 *   Docker/Compose/catalog call now goes through this class, so the token
 *   injection, the contract header, the 426 reload prompt, the GET cache and
 *   the mutation invalidation are shared by every screen.
 *
 * Dependencies:
 *   - none (browser fetch + internal $lib modules)
 *
 * Usage:
 *   `import { api } from '$lib/services/api'` — used by every service call
 *   site; tokens are set once after login via api.setTokens(...).
 * =============================================================================
 */

import type { ApiResponse, DbBackupEntry, DbBackupStatus, LoginRequest, LoginResponse, RestoreDrillReport, SetupAdminRequest, SetupStatus } from '$lib/types/api';
import type { DownloaderFeedRefresh, DownloaderRssFeed } from '$lib/types/downloader';
import type { RemoteMountRequest, RemoteMountView } from '$lib/types/explorer';
import type {
  DockerAppStore,
  DockerAppUpdateStatus,
  DockerBuildImageRequest,
  DockerCatalogApp,
  DockerCatalogInstallRequest,
  DockerCatalogUpdateAllReport,
  DockerComposeProject,
  DockerContainer,
  DockerContainerAction,
  DockerContainerDetail,
  DockerContainerUpdateRequest,
  DockerCreateContainerRequest,
  DockerCreateNetworkRequest,
  DockerCreateVolumeRequest,
  DockerDiskUsage,
  DockerExecResult,
  DockerHubResult,
  DockerImage,
  DockerInstallReview,
  DockerNetwork,
  DockerPruneReport,
  DockerPruneTarget,
  DockerPortStatus,
  DockerPullImageRequest,
  DockerVolume,
} from '$lib/types/docker';
import { notifyContractMismatch } from '$lib/stores/contract';

/** One row of the desktop background-activity taskbar (`GET /api/activity`). */
export interface ActivityTask {
  /** `PROGRESS_MANAGER` job uuid, or `"aria2:<gid>"`. */
  id: string;
  /** Stable slug — localised on the client via `desktop.activity_kind_<kind>`. */
  kind: string;
  /**
   * Lifecycle slug: `pending` | `running` | `failed` | `cancelled`.
   *
   * A failed or cancelled job stays in the feed for one minute after its last
   * event, so the row the user was watching reports its outcome instead of
   * vanishing between two polls (ROADMAP 28.2). The badge counts only the
   * active states.
   */
  state: 'pending' | 'running' | 'failed' | 'cancelled' | 'completed';
  /** Dynamic name (torrent / file); empty for a generic job. */
  label: string;
  /** `0..100`, or `null` for an indeterminate job (sliding bar, no `%`). */
  progress: number | null;
  /** Latest message / "12/40 files" / "8.4 MB/s". */
  detail: string | null;
  /**
   * Structured transfer detail (HV-UX-02). `null` when the producer reports no
   * byte counter — the client then renders an indeterminate bar and no ETA
   * rather than a fabricated "0 s left".
   */
  bytes_processed: number | null;
  bytes_total: number | null;
  /** Bytes per second over the producer's rolling window. */
  speed_bps: number | null;
  /** Seconds remaining, computed server-side from `speed_bps`. */
  eta_seconds: number | null;
  /** Copy method / digest slug (`clone`, `kernel_copy`, `stream`, `sha256`). */
  method: string | null;
}

/** Which zone of the Explorer's transfer tray a row belongs to. */
export type TransferZone = 'upload' | 'download' | 'copy';

/** One in-flight chunked upload (`upload_sessions`, server-side). */
export interface ActiveUpload {
  session_id: string;
  filename: string;
  dir: string;
  bytes_received: number;
  bytes_total: number;
  /** `0..100`, or `null` when the client declared no size. */
  progress: number | null;
  speed_bps: number | null;
  eta_seconds: number | null;
  state: 'running';
  /** Unix seconds. */
  created_at: number;
  last_chunk_at: number;
}

/** One live copy / move / checksum / download job. */
export interface LiveTransfer {
  /** Progress-manager job id. */
  id: string;
  zone: TransferZone;
  /** Stable slug, localised via `explorer.transfer_kind_<kind>`. */
  kind: string;
  state: 'pending' | 'running' | 'failed' | 'cancelled' | 'completed';
  label: string;
  progress: number | null;
  bytes_processed: number | null;
  bytes_total: number | null;
  speed_bps: number | null;
  eta_seconds: number | null;
  method: string | null;
  detail: string | null;
  /**
   * API-relative `DELETE` path that stops this transfer (`/progress/{id}`),
   * or `null` for an upload — those are aborted with the session id instead.
   */
  cancel_path: string | null;
}

export interface ZoneCounts {
  uploads: number;
  downloads: number;
  copies: number;
  total: number;
}

/** `GET /api/explorer/transfers/active`. */
export interface ActiveTransfers {
  uploads: ActiveUpload[];
  transfers: LiveTransfer[];
  counts: ZoneCounts;
}

/** One finished transfer (`GET /api/explorer/transfers/history`). */
export interface TransferHistoryEntry {
  id: number;
  job_id: string | null;
  username: string;
  operation: 'upload' | 'download' | 'copy' | 'move' | 'checksum';
  source_path: string | null;
  dest_path: string | null;
  bytes_total: number;
  duration_ms: number | null;
  method: string | null;
  verified: boolean;
  status: 'complete' | 'failed' | 'cancelled';
  error_message: string | null;
  /** Unix seconds. */
  created_at: number;
  completed_at: number | null;
}

export interface TransferHistoryPage {
  entries: TransferHistoryEntry[];
  has_more: boolean;
  limit: number;
  offset: number;
}

/** Result of an inline `POST /api/explorer/checksum`. */
export interface ChecksumResult {
  path: string;
  algorithm: 'sha256' | 'sha512' | 'blake3';
  checksum: string;
  file_size: number;
  duration_ms: number;
  compare_with?: { path: string | null; checksum: string; file_size: number };
  match?: boolean;
}

/** What the filesystem actually does about one user's home size limit. */
export interface HomeQuotaState {
  /**
   * True only when the limit (or, with no limit, the accounting) was read
   * back from the filesystem — never because the save succeeded.
   */
  enforced: boolean;
  /** `ext4`, `xfs`, `zfs`, `btrfs`, … */
  filesystem: string;
  /** Mount point (or ZFS dataset) the home folder lives on. */
  mount_point: string;
  /** Configured hard limit in bytes; `null` = unlimited. */
  limit_bytes: number | null;
  /** Usage reported by the filesystem when accounting is active. */
  used_bytes: number | null;
  /**
   * Stable machine-readable cause (`not_mounted_with_quota`,
   * `unsupported_filesystem`, `accounting_inactive`, `no_entry`,
   * `limit_mismatch`, `command_failed`, `inspect_failed`,
   * `invalid_username`) — the UI localises this.
   */
  reason_code: string | null;
  /** English diagnostic detail — logs and last-resort fallback text. */
  reason: string | null;
}

/** `GET /api/users/{username}/home` — the stored config plus the live state. */
export interface HomeFolderInfo {
  enabled: boolean;
  disk: string | null;
  path: string | null;
  created_at: string | null;
  size_bytes: number | null;
  /** `null` when no home folder is enabled for this user. */
  quota: HomeQuotaState | null;
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
export const CONTRACT_VERSION = 2;

/** Cadence of a persisted Cloud Sync job (ROADMAP 16.4). */
export type CloudSyncScheduleKind = 'none' | 'interval' | 'cron';

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
   *
   * A Docker mutation evicts the whole `/docker` family for the same reason:
   * one click has several readers — a container action changes
   * `GET:/docker/containers?stats=false` **and** `…?stats=true` **and** the
   * catalog's `installed`/`running` flags, an image pull changes
   * `GET:/docker/system/df`, a compose action changes the project list. The
   * Docker screen re-read 8 of those endpoints after every action, and a
   * cached 3 s answer would have shown the pre-action state back (ROADMAP
   * 31.1). The screen already asked for the freshness explicitly; this
   * keeps that behaviour when the cache is in the path.
   */
  private invalidateForMutation(endpoint: string) {
    this.invalidateCache(endpoint);
    if (endpoint.startsWith('/settings/')) {
      this.invalidateCache('/settings');
    }
    if (endpoint.startsWith('/docker')) {
      this.invalidateCache('/docker');
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
    // `ttl ?? default` and not `ttl || default`: the Docker screen needs a
    // `ttl: 0` ("never serve a cached row") and `0 || 5000` would silently
    // turn that into the 5 s default (ROADMAP 31.1).
    const ttl = options?.ttl ?? this.defaultTTL;
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
      // Expose the status so polling stores can back off on a 5xx instead of
      // treating every `success:false` as a benign refusal.
      data.http_status = response.status;

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

  /** Generic PATCH request (partial update; the passkey label uses it). */
  async patch<T = any>(endpoint: string, data?: any): Promise<ApiResponse<T>> {
    this.invalidateForMutation(endpoint);

    try {
      const response = await fetch(`${this.baseUrl}${endpoint}`, {
        method: 'PATCH',
        headers: this.getHeaders(),
        body: JSON.stringify(data ?? {}),
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
  async loginWith2FA(
    username: string,
    password: string,
    totpCode: string,
    rememberMe = false,
    rememberDevice = false,
  ) {
    return this.post<LoginResponse>(
      '/auth/2fa/verify',
      {
        username,
        password,
        totp_code: totpCode,
        remember_me: rememberMe,
        // Opt-in: the server answers with `hv_trusted_device` (HttpOnly) and
        // this browser may skip the second factor for 30 days.
        remember_device: rememberDevice,
      },
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

  /**
   * Which sign-in methods this appliance currently offers.
   *
   * Public on purpose: the login screen has to decide whether to draw a
   * single sign-on button, and only the server knows. Never cached, so an
   * administrator switching the integration on does not need a reload.
   */
  async getAuthMethods() {
    return this.get<{
      password: boolean;
      oidc: { enabled: boolean; label: string };
      saml?: { enabled: boolean; label: string };
    }>('/auth/methods', { cache: false });
  }

  /**
   * Start a single sign-on. The answer is the provider URL to visit; the
   * server has already bound the flow to this browser with a cookie.
   */
  async startOidcSignIn() {
    return this.post<{ authorize_url: string; state: string }>('/auth/oidc/start', {}, false);
  }

  /**
   * Trade the one-shot ticket the callback left in the URL fragment for the
   * session the desktop needs. `includeAuth: false` — the caller is not signed
   * in yet, and a stale token must not be attached to the exchange.
   */
  async completeOidcSignIn(code: string) {
    return this.post<LoginResponse>('/auth/oidc/complete', { code }, false);
  }

  /**
   * Start a SAML sign-in. Same shape as the OpenID Connect one: the answer is
   * the identity provider URL, and the flow is already bound to this browser.
   */
  async startSamlSignIn() {
    return this.post<{ authorize_url: string; state: string }>('/auth/saml/start', {}, false);
  }

  /** Trade the one-shot ticket the SAML ACS left in the fragment. */
  async completeSamlSignIn(code: string) {
    return this.post<LoginResponse>('/auth/saml/complete', { code }, false);
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

  /**
   * Current home-folder config plus the *real* quota state of the filesystem
   * hosting it (`enforced`, `used_bytes`, `reason`). The Users page renders
   * this verbatim: a limit that is stored but not enforced must never look
   * applied.
   */
  async getUserHome(username: string): Promise<HomeFolderInfo> {
    const res = await this.get(`/users/${username}/home`);
    return (res?.data ?? res) as HomeFolderInfo;
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
  // Docker (containers, images, networks, volumes, compose, app catalog)
  // ==============================================
  //
  // One wrapper per endpoint the Docker screen calls (ROADMAP 31.1). The
  // screen used to hold its own `authedFetch` helper; every call now goes
  // through this class, so a Docker request carries the same contract header,
  // 426 prompt, cache and invalidation as the rest of the app.
  //
  // **Caching contract.** The wrappers default to `cache: false` because the
  // screen's behaviour before the migration was "no cache": rows must be the
  // live daemon state after every action, and the ~20 mutations rely on the
  // read that follows them. The two exceptions are the original read methods
  // (`getDockerContainers`, `getDockerCatalog`, `isDockerAvailable`) whose
  // 3 s / 5 s / 10 s TTLs the desktop Docker monitor already depends on —
  // they keep their exact signature and TTL. Every mutation also evicts the
  // whole `/docker` cache family (see `invalidateForMutation`), so a cached
  // read can never survive the write that invalidates it.

  async getDockerContainers() {
    return this.get<DockerContainer[]>('/docker/containers', { ttl: 3000 }); // Cache 3s
  }

  /**
   * Containers WITHOUT stats — what the Docker app paints first (a plain
   * `docker ps`, no `docker stats` round-trip).
   */
  async getDockerContainerList() {
    return this.get<DockerContainer[]>('/docker/containers?stats=false', { cache: false });
  }

  /**
   * Containers WITH live `docker stats` sampling (CPU %, memory, net/block
   * IO). `stats=true` makes the backend run `docker stats --no-stream`, which
   * blocks ~1s, so this is deliberately uncached and used by the Dashboard's
   * live-usage card and by the Docker screen's background stats fill — never
   * by its first paint.
   */
  async getDockerContainersWithStats() {
    return this.get<DockerContainer[]>('/docker/containers?stats=true', { cache: false });
  }

  async isDockerAvailable() {
    return this.get<{ available: boolean }>('/docker/available', { ttl: 10000 });
  }

  async isDockerComposeAvailable() {
    return this.get<{ available: boolean }>('/docker/compose/available', { cache: false });
  }

  // -- Containers ------------------------------------------------------------

  async createDockerContainer(payload: DockerCreateContainerRequest) {
    return this.post('/docker/containers', payload);
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

  /** Start/stop/restart — the shape `Desktop.svelte` switches on. */
  async dockerContainerAction(name: string, action: DockerContainerAction) {
    return this.put(`/docker/containers/${encodeURIComponent(name)}/${action}`);
  }

  /** Pause/resume a running container. */
  async dockerContainerPauseAction(name: string, action: 'pause' | 'unpause') {
    return this.put(`/docker/containers/${encodeURIComponent(name)}/${action}`);
  }

  /** Send a signal. The backend whitelists the accepted signals. */
  async killDockerContainer(name: string, signal = 'SIGKILL') {
    return this.post(`/docker/containers/${encodeURIComponent(name)}/kill`, { signal });
  }

  async renameDockerContainer(name: string, newName: string) {
    return this.put(`/docker/containers/${encodeURIComponent(name)}/rename`, { name: newName });
  }

  /** Remove (uninstall) a container. `force` also stops a running one. */
  async removeDockerContainer(name: string, force = false) {
    return this.delete(`/docker/containers/${encodeURIComponent(name)}?force=${force ? 'true' : 'false'}`);
  }

  async inspectDockerContainer(name: string) {
    return this.get<DockerContainerDetail>(`/docker/containers/${encodeURIComponent(name)}/inspect`, { cache: false });
  }

  /** Apply memory/CPU/restart-policy limits in place (no recreate). */
  async updateDockerContainer(name: string, payload: DockerContainerUpdateRequest) {
    return this.post(`/docker/containers/${encodeURIComponent(name)}/update`, payload);
  }

  /**
   * Recreate a container from its own settings after re-pulling its image
   * (the backend renames the old one aside and rolls back on failure).
   */
  async recreateDockerContainer(name: string) {
    return this.post(`/docker/containers/${encodeURIComponent(name)}/recreate`);
  }

  async execDockerContainer(name: string, command: string[], timeoutSecs = 30) {
    return this.post<DockerExecResult>(`/docker/containers/${encodeURIComponent(name)}/exec`, {
      command,
      timeout_secs: timeoutSecs,
    });
  }

  /**
   * Mint a single-use ticket for the live-log SSE stream. `EventSource`
   * cannot send `X-Session-Token`, so the session never travels in a URL —
   * the returned `url` carries the ticket instead.
   */
  async createDockerLogTicket(
    name: string,
    options: { tail: number; follow: boolean; timestamps: boolean },
  ) {
    return this.post<{ ticket: string; url: string }>(
      `/docker/containers/${encodeURIComponent(name)}/logs/ticket`,
      { tail: options.tail, follow: options.follow, timestamps: options.timestamps },
    );
  }

  // -- Images ----------------------------------------------------------------

  async getDockerImages() {
    return this.get<DockerImage[]>('/docker/images', { cache: false });
  }

  /** `image` is the repository, `tag` defaults to `latest` server-side. */
  async pullDockerImage(payload: DockerPullImageRequest) {
    return this.post('/docker/images/pull', payload);
  }

  async buildDockerImage(payload: DockerBuildImageRequest) {
    return this.post<{ log: string }>('/docker/images/build', payload);
  }

  async removeDockerImage(id: string, force = true) {
    return this.delete(`/docker/images/${encodeURIComponent(id)}?force=${force ? 'true' : 'false'}`, { force });
  }

  async searchDockerHub(query: string) {
    return this.get<DockerHubResult[]>(`/docker/hub/search?q=${encodeURIComponent(query)}`, { cache: false });
  }

  // -- Networks --------------------------------------------------------------

  async getDockerNetworks() {
    return this.get<DockerNetwork[]>('/docker/networks', { cache: false });
  }

  async createDockerNetwork(payload: DockerCreateNetworkRequest) {
    return this.post('/docker/networks', payload);
  }

  async removeDockerNetwork(name: string) {
    return this.delete(`/docker/networks/${encodeURIComponent(name)}`);
  }

  // -- Volumes, disk usage, prune -------------------------------------------

  async getDockerVolumes() {
    return this.get<DockerVolume[]>('/docker/volumes', { cache: false });
  }

  async createDockerVolume(payload: DockerCreateVolumeRequest) {
    return this.post('/docker/volumes', payload);
  }

  /** `force` is required to delete a volume that is still in use. */
  async removeDockerVolume(name: string, force = false) {
    return this.delete(`/docker/volumes/${encodeURIComponent(name)}?force=${force ? 'true' : 'false'}`);
  }

  async getDockerDiskUsage() {
    return this.get<DockerDiskUsage>('/docker/system/df', { cache: false });
  }

  /** `all` also prunes unused (not only dangling) resources. */
  async pruneDocker(target: DockerPruneTarget, all = false) {
    return this.post<DockerPruneReport>('/docker/prune', { target, all });
  }

  // -- Private registries ----------------------------------------------------

  async getDockerRegistries() {
    return this.get<string[]>('/docker/registries', { cache: false });
  }

  /** The password goes in the body; it is never logged nor echoed back. */
  async dockerRegistryLogin(registry: string, username: string, password: string) {
    return this.post('/docker/registries/login', { registry, username, password });
  }

  async dockerRegistryLogout(registry: string) {
    return this.post('/docker/registries/logout', { registry });
  }

  // -- Daemon ----------------------------------------------------------------

  async startDockerDaemon() {
    return this.post('/docker/daemon/start');
  }

  async getDockerDaemonLogs() {
    return this.get<{ logs: string }>('/docker/daemon/logs', { cache: false });
  }

  async getDockerDaemonConfig() {
    return this.get<{ config: string; path: string }>('/docker/daemon/config', { cache: false });
  }

  async saveDockerDaemonConfig(config: string) {
    return this.put('/docker/daemon/config', { config });
  }

  // -- Compose ---------------------------------------------------------------

  async getDockerComposeProjects() {
    return this.get<DockerComposeProject[]>('/docker/compose/projects', { cache: false });
  }

  async createDockerComposeProject(name: string, config: string) {
    return this.post('/docker/compose/projects', { name, config });
  }

  async updateDockerComposeProject(project: string, config: string) {
    return this.put(`/docker/compose/projects/${encodeURIComponent(project)}`, { config });
  }

  /** The raw `docker-compose.yml` text of a managed project. */
  async getDockerComposeConfig(project: string) {
    return this.get<{ config: string }>(
      `/docker/compose/projects/${encodeURIComponent(project)}/config`,
      { cache: false },
    );
  }

  /** Validate a compose file without deploying it. */
  async validateDockerCompose(config: string) {
    return this.post('/docker/compose/validate', { config });
  }

  async dockerComposeAction(project: string, action: 'start' | 'stop' | 'restart') {
    return this.post(`/docker/compose/projects/${encodeURIComponent(project)}/${action}`);
  }

  /** Pull the newest images and redeploy the stack. */
  async updateDockerComposeProjectImages(project: string) {
    return this.post(`/docker/compose/projects/${encodeURIComponent(project)}/update`);
  }

  async removeDockerComposeProject(project: string) {
    return this.delete(`/docker/compose/projects/${encodeURIComponent(project)}`);
  }

  // -- Community package stores ---------------------------------------------

  async getDockerStores() {
    return this.get<DockerAppStore[]>('/docker/stores', { cache: false });
  }

  async addDockerStore(gitUrl: string) {
    return this.post('/docker/stores', { git_url: gitUrl });
  }

  async syncDockerStore(id: string) {
    return this.post(`/docker/stores/${encodeURIComponent(id)}/sync`);
  }

  async removeDockerStore(id: string) {
    return this.delete(`/docker/stores/${encodeURIComponent(id)}`);
  }

  // -- App catalog -----------------------------------------------------------

  async getDockerCatalog() {
    return this.get<DockerCatalogApp[]>('/docker/catalog', { ttl: 5000 });
  }

  /** Uncached catalog read — the catalog tab, where an install must show up. */
  async getDockerCatalogList() {
    return this.get<DockerCatalogApp[]>('/docker/catalog', { cache: false });
  }

  async getDockerCatalogUpdates() {
    return this.get<DockerAppUpdateStatus[]>('/docker/catalog/updates', { cache: false });
  }

  /** P1 — opt-in scheduled catalog updates. */
  async getDockerUpdateSchedule() {
    return this.get('/docker/updates/schedule', { cache: false });
  }

  async saveDockerUpdateSchedule(enabled: boolean, intervalHours: number) {
    return this.put('/docker/updates/schedule', { enabled, interval_hours: intervalHours });
  }

  async runDockerUpdateNow() {
    return this.post('/docker/updates/run-now');
  }

  /** The pre-install review: dependencies, settings, compatibility. */
  async getDockerInstallReview(id: string) {
    return this.get<DockerInstallReview>(`/docker/catalog/${encodeURIComponent(id)}/install-review`, { cache: false });
  }

  async installDockerCatalogApp(id: string, payload: DockerCatalogInstallRequest = {}) {
    return this.post(`/docker/catalog/${encodeURIComponent(id)}/install`, payload);
  }

  async updateDockerCatalogApp(id: string) {
    return this.post(`/docker/catalog/${encodeURIComponent(id)}/update`);
  }

  /** Grouped update queue: per-app outcomes, one failure never hides the rest. */
  async updateAllDockerCatalogApps() {
    return this.post<DockerCatalogUpdateAllReport>('/docker/catalog/update-all');
  }

  async getDockerCatalogAppLogs(id: string, tail = 200) {
    return this.get<{ logs: string }>(`/docker/catalog/${encodeURIComponent(id)}/logs?tail=${tail}`, { cache: false });
  }

  /** Toggle the per-app reverse proxy's own sign-in page. */
  async setDockerCatalogProxyAuth(id: string, authRequired: boolean) {
    return this.post(`/docker/catalog/${encodeURIComponent(id)}/proxy`, { auth_required: authRequired });
  }

  /** Uninstall — the app's data under `apps_dir()` is kept on purpose. */
  async uninstallDockerCatalogApp(id: string) {
    return this.delete(`/docker/catalog/${encodeURIComponent(id)}`);
  }

  /** Is a host port free? `used_by` names what holds it (`container`,
   * `homevault`, `host`) and `detail` gives the container name when known. */
  async checkDockerPort(port: number) {
    return this.get<DockerPortStatus>(`/docker/port-check?port=${Number(port)}`, { cache: false });
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
  async grantResourceAccess(member: string, kind: 'app' | 'folder' | 'share', resource: string, permission: 'deny' | 'read' | 'write' | 'full' = 'read') {
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
  async grantGroupResourceAccess(group: string, kind: 'app' | 'folder' | 'share', resource: string, permission: 'deny' | 'read' | 'write' | 'full' = 'read') {
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

  /** Active disk-health findings (`GET /api/health/disk-health`). */
  async diskHealthFindings() {
    return this.get('/health/disk-health', { cache: false });
  }

  /** Per-disk S.M.A.R.T. history (`GET /api/health/disk-health/samples`). */
  async diskHealthSamples(serial: string, limit = 500) {
    return this.get(
      `/health/disk-health/samples?serial=${encodeURIComponent(serial)}&limit=${limit}`,
      { cache: false },
    );
  }

  /** Run one disk-health sample now (admin). */
  async diskHealthSample() {
    return this.post('/health/disk-health/sample', {});
  }

  /** Regression trends for every sampled disk (`GET /api/health/disk-health/predictions`). */
  async diskHealthPredictions() {
    return this.get('/health/disk-health/predictions', { cache: false });
  }

  /** Supervised background loops (FIAB-02, admin). */
  async schedulerHealth() {
    return this.get('/health/schedulers', { cache: false });
  }

  /** Per-path Btrfs compression policies (`GET /api/storage/btrfs/policies`). */
  async btrfsPolicies() {
    return this.get('/storage/btrfs/policies', { cache: false });
  }

  /** Create or update one Btrfs compression policy (admin). */
  async saveBtrfsPolicy(path: string, level: string, defrag: boolean) {
    return this.put('/storage/btrfs/policies', { path, level, defrag });
  }

  /** Delete a Btrfs compression policy (admin). */
  async deleteBtrfsPolicy(path: string) {
    return this.delete(`/storage/btrfs/policies?path=${encodeURIComponent(path)}`);
  }

  /** Apply a Btrfs compression policy now (admin). */
  async applyBtrfsPolicy(path: string) {
    return this.post('/storage/btrfs/policies/apply', { path });
  }

  /** Snapshot quiescence hooks (`GET /api/storage/snapshot-hooks`). */
  async snapshotHooks() {
    return this.get('/storage/snapshot-hooks', { cache: false });
  }

  /** Create a snapshot quiescence hook (admin). */
  async createSnapshotHook(name: string, pre: string[], post: string[]) {
    return this.post('/storage/snapshot-hooks', { name, pre, post });
  }

  /** Update a snapshot quiescence hook (admin). */
  async updateSnapshotHook(id: string, name: string, pre: string[], post: string[], enabled: boolean) {
    return this.put(`/storage/snapshot-hooks/${id}`, { name, pre, post, enabled });
  }

  /** Delete a snapshot quiescence hook (admin). */
  async deleteSnapshotHook(id: string) {
    return this.delete(`/storage/snapshot-hooks/${id}`);
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

  /** Every IANA zone name the server can evaluate — the Settings picker's
   *  list, and the same names a task's own `timezone` accepts. Cached for an
   *  hour: it is the IANA database, not user data. */
  async getTimezones() {
    return this.get<{ timezones: string[] }>('/settings/timezones', { ttl: 3_600_000 });
  }

  // General

  async updateGeneralSettings(data: {
    server_name?: string;
    timezone?: string;
    mdns_enabled?: boolean;
    terminal_enabled?: boolean;
  }) {
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

  // --- SMTP OAuth2 (P1-16) --------------------------------------------------

  /** Read the OAuth2 status. Never includes the client secret or a token. */
  async getEmailOAuth() {
    return this.get<{
      oauth: {
        provider: 'google' | 'microsoft';
        client_id: string;
        redirect_url: string;
        configured: boolean;
        connected: boolean;
        connected_at: string | null;
        last_error: string | null;
        auth_method: string;
        smtp_host: string;
        smtp_port: number;
      };
      providers: Array<{
        id: 'google' | 'microsoft';
        smtp_host: string;
        smtp_port: number;
        auth_method: string;
      }>;
    }>('/settings/email/oauth', { cache: false });
  }

  /** Save the client registration. A blank `client_secret` keeps the stored one. */
  async setEmailOAuth(data: {
    provider?: 'google' | 'microsoft';
    client_id?: string;
    client_secret?: string;
    redirect_url?: string;
  }) {
    return this.put('/settings/email/oauth', data);
  }

  /** Forget the registration and the refresh token. */
  async disconnectEmailOAuth() {
    return this.delete('/settings/email/oauth');
  }

  /** Build the provider URL and bind the flow to this browser. */
  async startEmailOAuth() {
    return this.post<{ authorization_url: string; expires_in: number }>(
      '/settings/email/oauth/start',
      {}
    );
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
    /**
     * SMS channel (phase B3). `token` is the credential in clear on the wire
     * and is sealed server-side; the read path only ever answers `token_set`,
     * so an empty value means "keep the stored one" rather than "erase it".
     */
    sms?: {
      enabled: boolean;
      endpoint: string;
      body_template: string;
      recipient: string;
      extra_headers: Array<[string, string]>;
      token_sealed: string;
    };
  }) {
    return this.put('/settings/notifications', data);
  }

  // Appearance

  /**
   * The appliance's look (wallpaper + accent). **Any** signed-in session may
   * read it — the desktop's boot hydration needs it, and it carries nothing
   * but those two fields (ROADMAP 26.1). Writing it stays admin-only.
   *
   * `cache: false`: the Settings window re-reads it after a save and the
   * hydration reads it on every boot, so a cached copy would show the
   * previous look.
   */
  async getAppearanceSettings() {
    return this.get<{ wallpaper: string | null; accent_color: string | null }>(
      '/settings/appearance',
      { cache: false },
    );
  }

  async updateAppearanceSettings(data: {
    wallpaper?: string;
    accent_color?: string;
  }) {
    return this.put('/settings/appearance', data);
  }

  // Updates

  async updateUpdateSettings(data: { check_frequency?: string; release_manifest?: string }) {
    return this.put('/settings/update', data);
  }

  // --- Signed update application (P2-8, UI half) ---------------------------

  /**
   * Whether the update engine can run, the queued request, the last engine
   * exit and the tail of its log. Polled after an apply, because the server
   * restarts in the middle of it.
   */
  async getUpdateApplyStatus() {
    return this.get<{
      available: boolean;
      configured: boolean;
      running: boolean;
      current_version: string;
      request: { manifest: string; actor: string | null; requested_at: string | null } | null;
      exit: { status: string; exit_code: number | null; finished_at: string } | null;
      log_tail: string;
      restarts_service: boolean;
    }>('/settings/update/status', { cache: false });
  }

  /** Queue a signed update and start the engine (admin). */
  async applyUpdate(payload: {
    manifest?: string;
    signature?: string;
    pubkey?: string;
    confirm: string;
  }) {
    return this.post('/settings/update/apply', payload);
  }

  /** Read the version info. The server only contacts the update endpoint
   *  here when the admin's saved frequency says a check is due — opening
   *  Settings is never an outbound call by itself. */
  async checkForUpdates() {
    return this.get('/settings/version', { cache: false });
  }

  /** The explicit "Check now" action — the only path that forces an outbound
   *  call, whatever the saved frequency (admin only, server-enforced). */
  async checkForUpdatesNow() {
    return this.post('/settings/version/check');
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

  /** Reads the last non-destructive restore drill (P2-9). */
  async getRestoreDrill(): Promise<ApiResponse<{ report: RestoreDrillReport | null }>> {
    return this.get<{ report: RestoreDrillReport | null }>('/settings/backup/restore-drill', { cache: false });
  }

  /**
   * Runs the restore drill. The optional passphrase only verifies an encrypted
   * configuration archive and travels in a header, never a URL.
   */
  async runRestoreDrill(passphrase?: string): Promise<ApiResponse<{ report: RestoreDrillReport; warning?: string }>> {
    try {
      const headers: Record<string, string> = { 'Content-Type': 'application/json' };
      const token = localStorage.getItem('session-token');
      if (token) headers['x-session-token'] = token;
      const csrf = localStorage.getItem('csrf-token');
      if (csrf) headers['x-csrf-token'] = csrf;
      if (passphrase) headers['x-backup-passphrase'] = passphrase;
      const response = await fetch(`${this.baseUrl}/settings/backup/restore-drill`, {
        method: 'POST',
        headers,
        body: '{}',
      });
      return await this.parseResponse<{ report: RestoreDrillReport; warning?: string }>(response);
    } catch (error) {
      return {
        success: false,
        error: error instanceof Error ? error.message : 'Network error',
      };
    }
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

  // SNMP *agent* (the appliance's own snmpd — the client above queries remote
  // devices). `503` when `snmpd` is not installed.
  async getSnmpAgent() {
    return this.get('/snmp/agent', { cache: false });
  }

  async updateSnmpAgent(data: {
    enabled?: boolean;
    scope?: 'loopback' | 'lan';
    lan_address?: string;
    version?: 'v2c' | 'v3';
    community?: string;
    v3_username?: string;
    v3_auth_secret?: string;
    v3_priv_secret?: string;
    location?: string;
    contact?: string;
  }) {
    return this.put('/snmp/agent', data);
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

  /** Revoke sessions by their opaque public id (`SessionInfo.id`), never a
   *  live token — the server resolves ids against the caller's own sessions. */
  async revokeSessions(ids: string[]) {
    return this.post('/security/sessions/revoke', { ids });
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

  // --- Link aggregation and VLAN devices (phase B5 / B6) ------------------

  /** The host's bonds, the NICs that may join one, and the supported modes. */
  async getNetworkBonds() {
    return this.get('/network/bonds', { ttl: 5000 });
  }

  /** Create a bond, or attach more members to a live one. */
  async createNetworkBond(data: { name?: string; mode: string; members: string[] }) {
    return this.post('/network/bonds', data);
  }

  async deleteNetworkBond(name: string) {
    return this.delete(`/network/bonds/${encodeURIComponent(name)}`);
  }

  /** The host's VLAN devices and the parents a new one may use. */
  async getNetworkVlans() {
    return this.get('/network/vlans', { ttl: 5000 });
  }

  async createNetworkVlan(data: { parent: string; vid: number }) {
    return this.post('/network/vlans', data);
  }

  async deleteNetworkVlan(name: string) {
    return this.delete(`/network/vlans/${encodeURIComponent(name)}`);
  }

  /** Saved static routes + whether `ip` is available. */
  async getNetworkRoutes() {
    return this.get('/network/routes', { ttl: 5000 });
  }

  async addNetworkRoute(data: {
    destination: string;
    via: string;
    metric?: number;
    interface: string;
  }) {
    return this.post('/network/routes', data);
  }

  async deleteNetworkRoute(data: { destination: string; interface: string }) {
    return this.delete('/network/routes', data);
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

  /**
   * Paginated notification feed for the caller, newest first (ROADMAP 21.3).
   * Answers `{ notifications, total, limit, offset }`; `read_at` is the
   * caller's per-user read timestamp.
   */
  async getNotifications(limit = 50, offset = 0) {
    return this.get(`/notifications?limit=${limit}&offset=${offset}`, { cache: false });
  }

  async markNotificationRead(id: string) {
    return this.post(`/notifications/${encodeURIComponent(id)}/read`, {});
  }

  async markAllNotificationsRead() {
    return this.post('/notifications/read-all', {});
  }

  /** Deletes one notification — admin-only (the feed is broadcast). */
  async deleteNotification(id: string) {
    return this.delete(`/notifications/${encodeURIComponent(id)}`);
  }

  /**
   * Clears the whole notification feed in one awaited call (ROADMAP 21.3) —
   * replaces the old per-row delete loop. Admin-only.
   */
  async clearNotifications() {
    return this.delete('/notifications');
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
    name: string; task_type: string; cron: string; payload: Record<string, unknown>;
    enabled: boolean; timezone?: string | null;
  }) {
    return this.post<ScheduledTask>('/scheduled-tasks', task);
  }

  async updateScheduledTask(id: string, task: {
    name: string; task_type: string; cron: string; payload: Record<string, unknown>;
    enabled: boolean; timezone?: string | null;
  }) {
    return this.put<ScheduledTask>(`/scheduled-tasks/${encodeURIComponent(id)}`, task);
  }

  /** Enable/disable only — a full PUT would reset the run summary. */
  async setScheduledTaskEnabled(id: string, enabled: boolean) {
    return this.post<ScheduledTask>(`/scheduled-tasks/${encodeURIComponent(id)}/enabled`, { enabled });
  }

  async deleteScheduledTask(id: string) {
    return this.delete(`/scheduled-tasks/${encodeURIComponent(id)}`);
  }

  /** Starts a run and returns its id: the execution continues on the server
   *  (ROADMAP 12.4), so the caller polls `GET /scheduled-tasks` (the row's
   *  `last_status`) or `GET /scheduled-tasks/{id}/runs` for the outcome. */
  async runScheduledTaskNow(id: string) {
    return this.post<{ run_id: string; task_id: string; status: string }>(
      `/scheduled-tasks/${encodeURIComponent(id)}/run`,
      {},
    );
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

  // --- Per-share snapshots (P1-12) ---

  async getShares() {
    return this.get<{ name: string; share_type: string; path: string; enabled: boolean }[]>('/shares', {
      cache: false,
    });
  }

  async listShareSnapshots(name: string, shareType: string) {
    return this.get<{
      share: { name: string; share_type: string; path: string };
      pool: string;
      snapshots: { name: string; created: string; used: number; referenced: number; browse_path: string }[];
    }>(`/shares/${encodeURIComponent(name)}/${encodeURIComponent(shareType)}/snapshots`, {
      cache: false,
    });
  }

  async createShareSnapshot(name: string, shareType: string) {
    return this.post(`/shares/${encodeURIComponent(name)}/${encodeURIComponent(shareType)}/snapshots`, {});
  }

  async deleteShareSnapshot(name: string, shareType: string, snapshot: string) {
    return this.delete(
      `/shares/${encodeURIComponent(name)}/${encodeURIComponent(shareType)}/snapshots/${encodeURIComponent(snapshot)}`,
    );
  }

  async restoreShareSnapshot(name: string, shareType: string, snapshot: string, path: string) {
    return this.post(
      `/shares/${encodeURIComponent(name)}/${encodeURIComponent(shareType)}/snapshots/${encodeURIComponent(snapshot)}/restore`,
      { path, confirm: true },
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

  // WireGuard client tunnels — imported provider configurations (P1-17).
  async listWireGuardClients() {
    return this.get('/vpn/wireguard/clients', { ttl: 5000 });
  }

  async importWireGuardClient(data: {
    name: string;
    config_content: string;
    kill_switch?: boolean;
    autostart?: boolean;
  }) {
    return this.post('/vpn/wireguard/clients', data);
  }

  async startWireGuardClient(id: string) {
    return this.post(`/vpn/wireguard/clients/${encodeURIComponent(id)}/start`, {});
  }

  async stopWireGuardClient(id: string) {
    return this.post(`/vpn/wireguard/clients/${encodeURIComponent(id)}/stop`, {});
  }

  async deleteWireGuardClient(id: string) {
    return this.delete(`/vpn/wireguard/clients/${encodeURIComponent(id)}`);
  }

  async setWireGuardClientAutostart(id: string, autostart: boolean) {
    return this.put(`/vpn/wireguard/clients/${encodeURIComponent(id)}`, { autostart });
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
    /**
     * Cadence (ROADMAP 16.4). Omitted = `none`: the job only runs when asked,
     * which is what every job created before the scheduler existed does.
     */
    schedule_kind?: CloudSyncScheduleKind;
    interval_minutes?: number;
    cron_expr?: string;
  }) {
    return this.post('/rclone/jobs', data);
  }

  async deleteSyncJob(id: number) {
    return this.delete(`/rclone/jobs/${id}`);
  }

  /**
   * "Run now". The run happens inside this request and the response carries the
   * **updated job**, so the caller renders what the server recorded
   * (`last_status`, `last_run_at`, `next_run_at`) instead of assuming `200`
   * meant "it worked".
   */
  async runSyncJob(id: number) {
    return this.post(`/rclone/jobs/${id}/run`, {});
  }

  /**
   * Set (or clear) a persisted job's cadence. `none` stops the scheduler
   * picking it up and keeps the last-run history.
   */
  async updateSyncJobSchedule(
    id: number,
    data: {
      schedule_kind: CloudSyncScheduleKind;
      interval_minutes?: number;
      cron_expr?: string;
    },
  ) {
    return this.put(`/rclone/jobs/${id}/schedule`, data);
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

  /**
   * Live transfer list for the Explorer's tray (HV-UX-01): in-flight chunked
   * uploads plus the copy/move/checksum/download jobs, grouped into zones by
   * the server so the client never has to guess which zone a job belongs to.
   */
  async getActiveTransfers() {
    return this.get<ActiveTransfers>('/explorer/transfers/active', { cache: false });
  }

  /**
   * Finished-transfer audit trail (HV-ADV-02), newest first. A non-admin only
   * ever sees their own rows; an administrator sees every user's.
   */
  async getTransferHistory(
    params: {
      limit?: number;
      offset?: number;
      operation?: 'upload' | 'download' | 'copy' | 'move' | 'checksum';
      status?: 'complete' | 'failed' | 'cancelled';
      since?: number;
    } = {},
  ) {
    const qs = new URLSearchParams();
    for (const [key, value] of Object.entries(params)) {
      // `String(value)` on both sides: the parameter values are a union of
      // string and numeric slugs, so comparing the raw value with `''` does
      // not typecheck.
      if (value !== undefined && value !== null && String(value) !== '') {
        qs.set(key, String(value));
      }
    }
    const suffix = qs.toString();
    return this.get<TransferHistoryPage>(
      `/explorer/transfers/history${suffix ? `?${suffix}` : ''}`,
      { cache: false },
    );
  }

  /**
   * On-demand checksum (HV-INT-01). A small file answers inline; a large one
   * (or `async: true`) answers `{ job_id, sse_url }` and the digest is read
   * back through `getChecksumResult`.
   */
  async computeChecksum(body: {
    path: string;
    algorithm?: 'sha256' | 'sha512' | 'blake3';
    compare_with?: string;
    async?: boolean;
  }) {
    return this.post<ChecksumResult | { job_id: string; sse_url: string; status: string }>(
      '/explorer/checksum',
      body,
    );
  }

  /** The digest of a finished asynchronous checksum job. */
  async getChecksumResult(jobId: string) {
    return this.explorerGet('/explorer/checksum/result', { job_id: jobId });
  }

  async listDirectory(path: string = '/') {
    return this.get(`/explorer/list?path=${encodeURIComponent(path)}`, { cache: false });
  }

  /**
   * Cross-root file search for the desktop's ⌘K overlay (QW-12). Confined
   * server-side to the roots this session's Explorer would list; `files_only`
   * drops directories from the answer.
   */
  async searchAllFiles(query: string, limit = 8, filesOnly = false, tag?: string) {
    const qs = `q=${encodeURIComponent(query)}&limit=${limit}${filesOnly ? '&files_only=true' : ''}${tag ? `&tag=${encodeURIComponent(tag)}` : ''}`;
    return this.get(`/search/files?${qs}`, { cache: false });
  }

  /**
   * Full-text content search (phase D1). The answer is already filtered to
   * the roots this session's Explorer would list; `available: false` means
   * the administrator has not enabled the index.
   */
  async searchContent(query: string, limit = 6) {
    const qs = `q=${encodeURIComponent(query)}&limit=${limit}`;
    return this.get(`/search/content?${qs}`, { cache: false });
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

  // --- Read-only ISO/IMG mounting (phase B2) -------------------------------

  /**
   * What is mounted, and whether this host can mount at all. `available:
   * false` means `losetup`/`mount` are missing — the screen says so rather
   * than offering a button that always fails (docs/API.md, capability
   * contract).
   */
  async listIsoMounts() {
    return this.get<{
      available: boolean;
      iso_root: string;
      mounts: Array<{
        id: string;
        image_path: string;
        label: string;
        mount_point: string;
        loop_device: string | null;
        size_bytes: number;
      }>;
    }>('/explorer/iso', { cache: false });
  }

  /** Mounts an image read-only under the appliance-managed `iso/` folder. */
  async mountIso(path: string) {
    return this.post('/explorer/iso', { path });
  }

  /** Unmounts one image by its id (never by a path). */
  async unmountIso(id: string) {
    return this.delete(`/explorer/iso/${encodeURIComponent(id)}`);
  }

  // --- Network locations (CIFS/NFS remote mounts) --------------------------

  /**
   * Every configured network location with its live state, plus which
   * protocols this host can mount. `available: false` means neither
   * `mount.cifs` nor `mount.nfs` is installed — the panel says so instead of
   * offering a button that always fails (docs/API.md, capability contract).
   */
  async listRemoteMounts() {
    return this.get<{
      available: boolean;
      kinds: string[];
      root: string;
      mounts: RemoteMountView[];
    }>('/remote-mounts', { cache: false });
  }

  /** Adds a network location; `mount_now` brings it up in the same call. */
  async createRemoteMount(payload: RemoteMountRequest) {
    return this.post<RemoteMountView>('/remote-mounts', payload);
  }

  /** Edits one location. A blank password keeps the stored one. */
  async updateRemoteMount(name: string, payload: RemoteMountRequest) {
    return this.put<RemoteMountView>(`/remote-mounts/${encodeURIComponent(name)}`, payload);
  }

  /** Unmounts (when up) and forgets one location, credentials included. */
  async deleteRemoteMount(name: string) {
    return this.delete(`/remote-mounts/${encodeURIComponent(name)}`);
  }

  async mountRemoteMount(name: string) {
    return this.post<RemoteMountView>(`/remote-mounts/${encodeURIComponent(name)}/mount`);
  }

  async unmountRemoteMount(name: string) {
    return this.post(`/remote-mounts/${encodeURIComponent(name)}/unmount`);
  }

  /** Mounts every enabled location that is not up yet (the boot pass). */
  async reconcileRemoteMounts() {
    return this.post<{
      failed: Array<{ name: string; error: string }>;
      mounts: RemoteMountView[];
    }>('/remote-mounts/reconcile');
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

  async getExplorerTags(path?: string) {
    return this.get(`/explorer/tags${path ? `?path=${encodeURIComponent(path)}` : ''}`, { cache: false });
  }

  async addExplorerTag(path: string, tag: string) {
    return this.post('/explorer/tags', { path, tag });
  }

  async removeExplorerTag(path: string, tag: string) {
    return this.delete('/explorer/tags', { path, tag });
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

  async createShareLink(
    path: string,
    mode: 'anyone' | 'signed_in',
    expiresDays: number,
    password?: string,
    maxDownloads?: number,
    allowedUsers?: string,
    allowedGroups?: string,
  ) {
    return this.post('/explorer/shares', {
      path,
      mode,
      expires_days: expiresDays,
      password,
      max_downloads: maxDownloads,
      allowed_users: allowedUsers,
      allowed_groups: allowedGroups,
    });
  }

  async revokeShareLink(token: string) {
    return this.delete(`/explorer/shares/${encodeURIComponent(token)}`);
  }

  /** Public file-request links (upload into a folder). */
  async listFileRequests() {
    return this.get('/explorer/file-requests', { cache: false });
  }

  async createFileRequest(
    path: string,
    message: string,
    expiresDays: number,
    password?: string,
    maxUploads?: number,
  ) {
    return this.post('/explorer/file-requests', {
      path,
      message,
      expires_days: expiresDays,
      password,
      max_uploads: maxUploads,
    });
  }

  async revokeFileRequest(token: string) {
    return this.delete(`/explorer/file-requests/${encodeURIComponent(token)}`);
  }

  getFileRequestUrl(requestPath: string) {
    return `${window.location.origin}${requestPath}`;
  }

  /** Absolute URL of a share link, for copy / QR display. */
  getShareLinkUrl(sharePath: string) {
    return `${window.location.origin}${sharePath}`;
  }

  async getExplorerWatchTicket(path: string) {
    return this.post('/explorer/watch/ticket', { path });
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

  /** Appliance-wide trash retention policy (age limit + size ceiling). */
  async getTrashPolicy() {
    return this.get('/explorer/trash/policy', { cache: false });
  }

  async setTrashPolicy(retentionDays: number, maxBytes: number) {
    return this.put('/explorer/trash/policy', {
      retention_days: retentionDays,
      max_bytes: maxBytes,
    });
  }

  /** Run the retention pass now, with the saved policy. */
  async purgeTrashNow() {
    return this.post('/explorer/trash/purge-now', {});
  }

  /**
   * The instant-copy (reflink) policy, plus which mounts under the user's
   * roots can actually clone — evidence for the settings panel, never a gate.
   */
  async getInstantCopySettings() {
    return this.get<{ mode: 'auto' | 'off'; mounts: { mount_point: string; fs_type: string; reflink: boolean }[] }>(
      '/explorer/instant-copy',
      { cache: false }
    );
  }

  /**
   * Save the instant-copy policy (admin only). `auto` tries a clone per file
   * and silently falls back; `off` always uses the verified byte stream.
   */
  async setInstantCopyMode(mode: 'auto' | 'off') {
    return this.put('/explorer/instant-copy', { mode });
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

  /**
   * Add a folder to the daemon's configuration (ROADMAP 18.1).
   *
   * The server owns every rule (ID charset, absolute path inside a managed
   * storage root, sync mode, known devices), so the screen sends what the last
   * dialog choice was and shows the refusal verbatim.
   */
  async createSyncthingFolder(body: {
    id: string;
    label: string;
    path: string;
    type: string;
    devices: string[];
  }) {
    return this.post<{ id: string }>('/syncthing/folders', body);
  }

  /** Remove a folder from the daemon's configuration; files on disk are kept. */
  async deleteSyncthingFolder(id: string) {
    return this.delete(`/syncthing/folders/${encodeURIComponent(id)}`);
  }

  /** Pair a device by its 56-character ID (dashes optional, case-insensitive). */
  async createSyncthingDevice(body: { device_id: string; name: string }) {
    return this.post<{ device_id: string }>('/syncthing/devices', body);
  }

  /** Remove a pairing; folders and their files on disk are kept. */
  async deleteSyncthingDevice(deviceId: string) {
    return this.delete(`/syncthing/devices/${encodeURIComponent(deviceId)}`);
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
    /** Audit source only: structured filters (P1-18). */
    from?: string;
    to?: string;
    severity?: string;
    actor?: string;
    action?: string;
    offset?: number;
  }) {
    const qs = new URLSearchParams();
    if (params.source) qs.set('source', params.source);
    if (params.unit) qs.set('unit', params.unit);
    if (params.priority !== undefined) qs.set('priority', String(params.priority));
    if (params.limit !== undefined) qs.set('limit', String(params.limit));
    if (params.search) qs.set('search', params.search);
    if (params.from) qs.set('from', params.from);
    if (params.to) qs.set('to', params.to);
    if (params.severity) qs.set('severity', params.severity);
    if (params.actor) qs.set('actor', params.actor);
    if (params.action) qs.set('action', params.action);
    if (params.offset !== undefined) qs.set('offset', String(params.offset));
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

  /** P1 — hourly usage samples for one subject (newest first, 90-day retention). */
  async getTransferQuotaHistory(
    subjectType: 'user' | 'group',
    subjectName: string,
    limit = 48,
  ) {
    return this.get(
      `/transfer-quotas/history?subject_type=${subjectType}&subject_name=${encodeURIComponent(subjectName)}&limit=${limit}`,
      { cache: false },
    );
  }

  async setTransferQuota(
    subjectType: 'user' | 'group',
    subjectName: string,
    body: {
      upload_enabled: boolean;
      upload_limit_bytes: number;
      download_enabled: boolean;
      download_limit_bytes: number;
      /** ROADMAP 23.2 — the window the counters are counted in. */
      period: 'lifetime' | 'day' | 'week' | 'month';
    }
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

  /** RAID resync-speed profile (FIAB-10, admin). */
  async getRaidSpeed() {
    return this.get('/storage/raid/resync-speed', { cache: false });
  }

  async setRaidSpeed(profile: 'low' | 'balanced' | 'fast') {
    const res = await this.put('/storage/raid/resync-speed', { profile });
    this.invalidateCache('/storage/raid');
    return res;
  }

  async createRaid(data: {
    name: string;
    level: string;
    devices: string[];
    filesystem: string;
    /** Extra disks attached as hot spares (mdadm, or a ZFS `spare` vdev). */
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

  /**
   * Read an mdadm array's current RAID level and every target the server's
   * transition matrix allows (each refusal carries its reason).
   */
  async getVirtualDiskLevelOptions(name: string) {
    return this.get(`/storage/virtual-disks/${encodeURIComponent(name)}/level-options`, {
      cache: false
    });
  }

  /**
   * Change an mdadm array's RAID level in place. `backupConfirmed` is the
   * explicit acknowledgement the server requires; the reshape runs in the
   * background — poll the scrub/sync status endpoint for progress.
   */
  async migrateVirtualDiskLevel(name: string, targetLevel: string, backupConfirmed: boolean) {
    const res = await this.post(`/storage/virtual-disks/${encodeURIComponent(name)}/migrate-level`, {
      target_level: targetLevel,
      backup_confirmed: backupConfirmed
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

  /** Password-confirmed destroy (the Storage UI path; DSM parity). */
  async destroyVirtualDisk(name: string, confirmName: string, password: string) {
    const res = await this.post(`/storage/virtual-disks/${encodeURIComponent(name)}/destroy`, {
      confirm_name: confirmName,
      password,
    });
    this.invalidateCache('/storage/virtual-disks');
    this.invalidateCache('/storage/raid');
    return res;
  }

  async scrubVirtualDisk(name: string) {
    return this.post(`/storage/virtual-disks/${encodeURIComponent(name)}/scrub`, {});
  }

  /** Btrfs layout status: legacy vs @/@snapshots (P1-13). */
  async getBtrfsLayout(name: string) {
    return this.get(`/storage/virtual-disks/${encodeURIComponent(name)}/btrfs-layout`, {
      cache: false
    });
  }

  /** Explicit legacy → @/@snapshots migration (admin, destructive topology). */
  async migrateBtrfsLayout(name: string) {
    const res = await this.post(`/storage/virtual-disks/${encodeURIComponent(name)}/btrfs-layout`, {});
    this.invalidateCache('/storage/virtual-disks');
    return res;
  }

  /** In-place Btrfs rollback to one of the pool's snapshots (admin). */
  async rollbackBtrfsSnapshot(name: string, snapshot: string) {
    const res = await this.post(`/storage/virtual-disks/${encodeURIComponent(name)}/btrfs-rollback`, {
      snapshot,
      confirm: true
    });
    this.invalidateCache('/storage/virtual-disks');
    return res;
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

  /**
   * Starts a LUKS2 volume creation. The work runs in a tracked job — the
   * answer is `{ job_id }` (not the volume), because `luksFormat` + `mkfs` +
   * `mount` take minutes on a real disk. Follow `/api/progress/{job_id}`.
   */
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

  /** Rotates a volume's LUKS passphrase (`luksChangeKey` + a test unlock). */
  async changeEncryptedVolumePassphrase(id: string, oldPassphrase: string, newPassphrase: string) {
    const res = await this.post(`/encryption/volumes/${encodeURIComponent(id)}/passphrase`, {
      old_passphrase: oldPassphrase,
      new_passphrase: newPassphrase,
    });
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

  /** Index status (any session): last indexed, doc/chunk counts, state. */
  async ragStatus() {
    // Never cached: the status is polled while a reindex runs and must not
    // report a stale `idle` after it finishes.
    return this.get('/rag/status', { cache: false });
  }

  /** Trigger a full reindex in the background (admin only). */
  async ragIndex() {
    return this.post('/rag/index', {});
  }

  /** Semantic search over the index, for debugging grounding (admin only). */
  async ragSearch(query: string, topK = 5) {
    return this.post('/rag/search', { query, top_k: topK });
  }

  /** List the indexed documents (admin only). */
  async ragDocuments() {
    return this.get('/rag/documents', { cache: false });
  }

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
    return this.get<{ available: boolean; fuse_available: boolean }>('/restic/available', {
      ttl: 60000,
    });
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

  /** Mount a repository read-only for browsing (admin). */
  async resticMountRepo(repoId: string) {
    return this.post(`/restic/repos/${encodeURIComponent(repoId)}/mount`);
  }

  /** Browse mounts the kernel currently confirms. */
  async resticMounts() {
    return this.get<ResticMountInfo[]>('/restic/mounts', { cache: false });
  }

  async resticUnmount(id: string) {
    return this.delete(`/restic/mounts/${encodeURIComponent(id)}`);
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
    // `dir` is the server's download root: every accepted download is confined
    // under it, and the screen shows it so the destination is never a guess.
    return this.get<{ available: boolean; running: boolean; dir?: string }>('/downloader/status', { cache: false });
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

  // --- ZFS read cache / separate log (L2ARC / SLOG) ------------------------

  /**
   * Every ZFS pool with its attached cache/log devices, plus the free disks
   * that could be attached. `available: false` means `zpool` is missing.
   */
  async getZfsCache() {
    return this.get<{
      available: boolean;
      pools: Array<{
        pool: string;
        state: string;
        devices: Array<{
          device: string;
          kind: 'l2arc' | 'slog';
          state: string;
          read_errors: number;
          write_errors: number;
          checksum_errors: number;
        }>;
      }>;
      candidates: Array<{ name: string; size: number; model: string; rotational: boolean }>;
    }>('/zfs-cache', { cache: false });
  }

  /** Attach a free flash disk as a read cache (`l2arc`) or separate log (`slog`). */
  async attachZfsCache(payload: { pool: string; kind: 'l2arc' | 'slog'; device: string }) {
    return this.post('/zfs-cache', payload);
  }

  /** Detach a cache/log device; the request cannot name a data disk. */
  async detachZfsCache(payload: { pool: string; device: string }) {
    return this.post('/zfs-cache/remove', payload);
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

  /**
   * Enqueue a download. `dir` is a sub-folder **relative to the server's
   * download root** (the server refuses absolute paths and `..`), `out` is a
   * bare file name — never a path. `limits` are per-task caps (aria2 syntax:
   * `500K`, `2M`, `0` = unlimited) and a seed ratio (`0` = no seeding).
   */
  async addDownloaderUrl(
    uri: string,
    dir?: string,
    out?: string,
    limits?: { maxDownloadLimit?: string; maxUploadLimit?: string; seedRatio?: string },
  ) {
    return this.post('/downloader/add', {
      uri,
      dir,
      out,
      max_download_limit: limits?.maxDownloadLimit,
      max_upload_limit: limits?.maxUploadLimit,
      seed_ratio: limits?.seedRatio,
    });
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

  // --- Downloader: automatic extraction + RSS subscriptions (P1) -----------

  /** Whether finished archives are unpacked automatically (off by default). */
  async getDownloaderSettings() {
    return this.get<{ auto_extract: boolean }>('/downloader/settings', { cache: false });
  }

  async updateDownloaderSettings(settings: { auto_extract: boolean }) {
    return this.put<{ auto_extract: boolean }>('/downloader/settings', settings);
  }

  /** Unpack the archives a finished download left behind; returns their names. */
  async extractDownloaderTask(gid: string) {
    return this.post<string[]>('/downloader/extract', { gid });
  }

  async listDownloaderFeeds() {
    return this.get<DownloaderRssFeed[]>('/downloader/feeds', { cache: false });
  }

  async addDownloaderFeed(payload: { url: string; name?: string; auto_download?: boolean }) {
    return this.post<DownloaderRssFeed>('/downloader/feeds', payload);
  }

  async updateDownloaderFeed(
    id: string,
    payload: { name?: string; enabled?: boolean; auto_download?: boolean },
  ) {
    return this.put<DownloaderRssFeed>(`/downloader/feeds/${encodeURIComponent(id)}`, payload);
  }

  async deleteDownloaderFeed(id: string) {
    return this.delete(`/downloader/feeds/${encodeURIComponent(id)}`);
  }

  /**
   * Fetch one feed. `queue: true` sends every new item to aria2 ("download all
   * new"); without it the call only lists what the feed currently offers.
   */
  async refreshDownloaderFeed(id: string, queue = false) {
    return this.post<DownloaderFeedRefresh>(
      `/downloader/feeds/${encodeURIComponent(id)}/refresh`,
      { queue },
    );
  }

  async refreshAllDownloaderFeeds() {
    return this.post<{ queued: number; failed: number }>('/downloader/feeds/refresh');
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

  /** Logical shares: one row per name+path with its protocol badges. */
  async getShareDefinitions() {
    return this.get('/shares/definitions', { cache: false });
  }

  async createShareDefinition(body: {
    name: string;
    path: string;
    comment?: string;
    protocols: string[];
    read_only?: boolean;
    guest_ok?: boolean;
    restrictions?: { disable_browsing: boolean; disable_modification: boolean; disable_download: boolean };
  }) {
    return this.post('/shares/definitions', body);
  }

  async bindShareProtocol(name: string, body: { share_type: string; read_only?: boolean; guest_ok?: boolean }) {
    return this.post(`/shares/${encodeURIComponent(name)}/bind`, body);
  }

  async unbindShareProtocol(name: string, shareType: string) {
    return this.delete(`/shares/${encodeURIComponent(name)}/bind/${encodeURIComponent(shareType)}`);
  }

  /** One grant applied to every protocol binding of a logical share. */
  async getSharePermissionsAll(name: string) {
    return this.get(`/shares/${encodeURIComponent(name)}/permissions`, { cache: false });
  }

  async grantSharePermissionAll(name: string, body: {
    grantee_type: string;
    grantee_name: string;
    permission: string;
    recursive?: boolean;
  }) {
    return this.post(`/shares/${encodeURIComponent(name)}/permissions`, body);
  }

  async revokeSharePermissionAll(name: string, body: { grantee_type: string; grantee_name: string }) {
    return this.delete(`/shares/${encodeURIComponent(name)}/permissions`, body);
  }

  async syncSharePermissionsAll(name: string) {
    return this.post(`/shares/${encodeURIComponent(name)}/permissions/sync`);
  }

  /** Flat matrix of every grant (admin). */
  async getSharePermissionsMatrix() {
    return this.get('/shares/permissions', { cache: false });
  }

  async getSharePermissionsReport(): Promise<Blob> {
    const res = await fetch(`${this.baseUrl}/shares/permissions/report`, {
      headers: this.getHeaders(),
    });
    if (!res.ok) throw new Error(await res.text());
    return res.blob();
  }

  async getNfsRules(name: string) {
    return this.get(`/shares/${encodeURIComponent(name)}/nfs/rules`, { cache: false });
  }

  async setNfsRules(name: string, rules: Array<{ client: string; access: string; options: string[] }>) {
    return this.put(`/shares/${encodeURIComponent(name)}/nfs/rules`, rules);
  }

  async getSmbPreviousVersions() {
    return this.get('/shares/smb/previous-versions', { cache: false });
  }

  async setSmbPreviousVersions(name: string, enabled: boolean) {
    return this.put(`/shares/${encodeURIComponent(name)}/smb/previous-versions`, { enabled });
  }

  async getShareAcl(name: string, shareType: string) {
    return this.get(`/shares/${encodeURIComponent(name)}/${encodeURIComponent(shareType)}/acl`, { cache: false });
  }

  async setShareAcl(name: string, shareType: string, body: { entries: unknown[]; default_entries: unknown[]; recursive: boolean }) {
    return this.put(`/shares/${encodeURIComponent(name)}/${encodeURIComponent(shareType)}/acl`, body);
  }

  async joinSmbDomain(body: { username: string; password: string }) {
    return this.post('/shares/smb/domain/join', body);
  }

  async leaveSmbDomain() {
    return this.post('/shares/smb/domain/leave');
  }

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

  // --- GeoIP firewall policy (P1-14) ---

  async getGeoIpStatus() {
    return this.get<GeoIpStatus>('/firewall/geoip', { cache: false });
  }

  async setGeoIpRule(country: string, action: 'allow' | 'deny') {
    return this.post('/firewall/geoip', { country, action });
  }

  async removeGeoIpRule(country: string) {
    return this.delete(`/firewall/geoip/${encodeURIComponent(country)}`);
  }

  async applyGeoIpRules() {
    return this.post('/firewall/geoip/apply', {});
  }

  // --- Built-in HTTPS/TLS (admin-only; applies after a restart) ---
  async getTlsSettings() {
    return this.get('/settings/tls', { cache: false });
  }

  async setTlsSettings(data: {
    enabled: boolean;
    https_port?: number | null;
    mode: 'self-signed' | 'manual' | 'acme';
    cert_path?: string | null;
    key_path?: string | null;
    redirect: boolean;
  }) {
    return this.put('/settings/tls', data);
  }

  async regenerateTlsCert() {
    return this.post('/settings/tls/regenerate');
  }

  // --- Certificate automation (ACME, admin-only) ---

  async getAcmeSettings() {
    return this.get('/settings/acme', { cache: false });
  }

  /**
   * `dns_api_token` is write-only on the wire: send `'set'` to keep the stored
   * value, `''` to clear it, or a new token to replace it. The response echoes
   * `'set'`/`''`, never the token.
   */
  async setAcmeSettings(data: {
    enabled: boolean;
    domain: string;
    contact_email: string;
    directory_url: string;
    tos_agreed: boolean;
    challenge: 'http-01' | 'dns-01';
    provider: 'manual' | 'cloudflare';
    dns_api_token: string;
    dns_propagation_seconds: number;
  }) {
    return this.put('/settings/acme', data);
  }

  async issueAcmeCertificate() {
    return this.post('/settings/acme/issue');
  }

  async confirmAcmeDnsRecord() {
    return this.post('/settings/acme/confirm');
  }

  async testAcmeChallenge() {
    return this.post('/settings/acme/test-challenge');
  }

  // --- Dynamic DNS (admin-only) ---

  async getDdnsSettings() {
    return this.get('/settings/ddns', { cache: false });
  }

  /** `token` is write-only here too: `'set'` keeps, `''` clears. */
  async setDdnsSettings(data: {
    enabled: boolean;
    provider:
      | 'duckdns'
      | 'dynv6'
      | 'cloudflare'
      | 'ovh'
      | 'namecheap'
      | 'desec'
      | 'custom';
    hostname: string;
    token: string;
    username: string;
    domain: string;
    custom_update_url: string;
  }) {
    return this.put('/settings/ddns', data);
  }

  async updateDdnsNow() {
    return this.post('/settings/ddns/update');
  }

  // --- Outbound HTTP(S) proxy (admin-only) ---

  async getProxySettings() {
    return this.get('/settings/proxy', { cache: false });
  }

  async setProxySettings(data: {
    enabled: boolean;
    http_proxy: string;
    https_proxy: string;
    no_proxy: string;
  }) {
    return this.put('/settings/proxy', data);
  }

  // --- Reachability advisor (admin-only, read-only) ---

  async getReachability() {
    return this.get('/settings/reachability', { cache: false });
  }

  /** Full configuration backup (admin-only). */
  async getConfigBackupState() {
    return this.get('/settings/config-backup', { cache: false });
  }

  async exportConfigBackup(includeSecrets: boolean, passphrase?: string) {
    return this.post('/settings/config-backup/export', {
      include_secrets: includeSecrets,
      passphrase: passphrase ?? null,
    });
  }

  /**
   * The archive is binary, so it is fetched raw (never inside a JSON string).
   * The session token travels in the header, as everywhere else; the browser
   * then saves the file from this URL.
   */
  getConfigBackupArchiveUrl(id: string) {
    return `${this.baseUrl}/settings/config-backup/archives/${encodeURIComponent(id)}`;
  }

  /** Uploads an archive; the passphrase travels in a header, never a URL. */
  private async configBackupUpload(path: string, file: Blob, passphrase?: string) {
    try {
      const headers: Record<string, string> = {
        'Content-Type': 'application/octet-stream',
      };
      const token = localStorage.getItem('session-token');
      if (token) headers['x-session-token'] = token;
      const csrf = localStorage.getItem('csrf-token');
      if (csrf) headers['x-csrf-token'] = csrf;
      if (passphrase) headers['x-backup-passphrase'] = passphrase;
      const response = await fetch(`${this.baseUrl}${path}`, {
        method: 'POST',
        headers,
        body: file,
      });
      const body = await response.json().catch(() => ({}));
      return {
        success: !!body?.success,
        data: body?.data,
        message: body?.message || body?.error,
        status: response.status,
      } as any;
    } catch (error) {
      return { success: false, message: String(error) } as any;
    }
  }

  async inspectConfigBackup(file: Blob, passphrase?: string) {
    return this.configBackupUpload('/settings/config-backup/inspect', file, passphrase);
  }

  async restoreConfigBackup(file: Blob, passphrase?: string) {
    return this.configBackupUpload('/settings/config-backup/restore', file, passphrase);
  }

  /** Per-share size quota (admin-only): state, usage and support. */
  async getShareQuota(name: string, shareType: string) {
    return this.get(
      `/shares/${encodeURIComponent(name)}/${encodeURIComponent(shareType)}/quota`,
      { cache: false },
    );
  }

  /** `quotaBytes: 0` removes the ceiling. */
  async setShareQuota(name: string, shareType: string, quotaBytes: number) {
    return this.put(
      `/shares/${encodeURIComponent(name)}/${encodeURIComponent(shareType)}/quota`,
      { quota_bytes: quotaBytes },
    );
  }

  async clearShareQuota(name: string, shareType: string) {
    return this.delete(
      `/shares/${encodeURIComponent(name)}/${encodeURIComponent(shareType)}/quota`,
    );
  }

  /** Who is connected over SMB / NFS / SFTP / FTP (admin-only). */
  async getConnectedClients() {
    return this.get('/security/connected-clients', { cache: false });
  }

  /** Names a client id, never a pid: the server re-collects before acting. */
  async kickConnectedClient(id: string) {
    return this.post('/security/connected-clients/kick', { id });
  }

  /** Login auto-block (admin-only): policy, the ban list, pardon. */
  async getAutoBlock() {
    return this.get('/security/auto-block', { cache: false });
  }

  /** Read-only hardening audit (admin-only). Never cached: the card must show
   *  a fixed finding on the very next run, without a restart. */
  async getSecurityAdvisor() {
    return this.get('/security/advisor', { cache: false });
  }

  async setAutoBlock(data: {
    enabled: boolean;
    threshold: number;
    window_seconds: number;
    ban_seconds: number;
    apply_firewall: boolean;
    dos_enabled: boolean;
    dos_requests_per_minute: number;
  }) {
    return this.put('/security/auto-block', data);
  }

  /** Lift one address: its row and, when installed, its firewall rule. */
  async pardonAutoBlock(ip: string) {
    return this.post('/security/auto-block/pardon', { ip });
  }

  async clearAutoBlockBans() {
    return this.post('/security/auto-block/clear');
  }

  /** Wake-on-LAN (admin-only): saved hosts, and sending a magic packet. */
  async getWolHosts() {
    return this.get('/power/wol/hosts', { cache: false });
  }

  async createWolHost(data: { label: string; mac: string; broadcast: string }) {
    return this.post('/power/wol/hosts', data);
  }

  async updateWolHost(id: number, data: { label: string; mac: string; broadcast: string }) {
    return this.put(`/power/wol/hosts/${id}`, data);
  }

  async deleteWolHost(id: number) {
    return this.delete(`/power/wol/hosts/${id}`);
  }

  /** Answers `sent`, never `woken`: the packet is unacknowledged. */
  async wakeWolHost(id: number) {
    return this.post('/power/wol/wake', { id });
  }

  /** Remote syslog forwarding (admin-only). */
  async getSyslogSettings() {
    return this.get('/settings/syslog', { cache: false });
  }

  async setSyslogSettings(data: {
    enabled: boolean;
    protocol: 'udp' | 'tcp' | 'tls';
    host: string;
    port: number;
    format: 'rfc5424' | 'rfc3164';
    facility: number;
    ca_pem: string;
    include_app_events: boolean;
  }) {
    return this.put('/settings/syslog', data);
  }

  /** Sends one real message to the configured collector. */
  async testSyslog() {
    return this.post('/settings/syslog/test');
  }

  // --- Identity: trusted devices and the rescue address (own account) ---

  // Passkeys / WebAuthn (Phase 1 P1.3). The two ceremonies are two calls
  // each: `begin` (server issues a single-use challenge) then `finish` (the
  // server verifies the signed response). The browser handles the
  // `navigator.credentials` call in between — see stores/passkey.ts.
  async getPasskeys() {
    return this.get('/passkeys', { cache: false });
  }

  async beginPasskeyRegistration() {
    return this.post('/passkeys/register/begin');
  }

  async finishPasskeyRegistration(payload: {
    client_data_json: string;
    attestation_object: string;
    credential_id: string;
    label: string;
  }) {
    return this.post('/passkeys/register/finish', payload);
  }

  async beginPasskeyLogin(username: string) {
    return this.post('/passkeys/login/begin', { username });
  }

  async finishPasskeyLogin(payload: {
    credential_id: string;
    client_data_json: string;
    authenticator_data: string;
    signature: string;
    username: string;
    remember_me?: boolean;
  }) {
    return this.post('/passkeys/login/finish', payload);
  }

  async renamePasskey(id: string, label: string) {
    return this.patch(`/passkeys/${encodeURIComponent(id)}`, { label });
  }

  async deletePasskey(id: string) {
    return this.delete(`/passkeys/${encodeURIComponent(id)}`);
  }

  async getTrustedDevices() {
    return this.get('/identity/devices', { cache: false });
  }

  async revokeTrustedDevice(id: string) {
    return this.post('/identity/devices/revoke', { id });
  }

  async revokeAllTrustedDevices() {
    return this.post('/identity/devices/revoke-all');
  }

  async getRescueEmail() {
    return this.get('/identity/rescue-email', { cache: false });
  }

  async setRescueEmail(email: string) {
    return this.post('/identity/rescue-email', { email });
  }

  async verifyRescueEmail(code: string) {
    return this.post('/identity/rescue-email/verify', { code });
  }

  async clearRescueEmail() {
    return this.delete('/identity/rescue-email');
  }

  // --- Account recovery through the rescue address (public) ---

  async requestAccountRecovery(username: string) {
    return this.post('/auth/recover', { username }, false);
  }

  /**
   * A correct six-digit code is exchanged for a fresh offline recovery code —
   * the same one the ordinary reset screen consumes.
   */
  async verifyAccountRecovery(username: string, code: string) {
    return this.post('/auth/recover/verify', { username, code }, false);
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
  last_check_ok: boolean | null;
  last_check_at: string | null;
  last_check_error: string | null;
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

interface ResticMountInfo {
  id: string;
  repo_id: string | null;
  repo_name: string | null;
  mount_point: string;
  started_at: string;
  active: boolean;
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

export interface CacheMoverRunShare {
  share_name: string;
  mode: string;
  status: 'moved' | 'skipped' | 'failed';
  files_moved: number;
  bytes_moved: number;
  message: string | null;
}

export interface CacheMoverRun {
  id: string;
  status: string;
  started_at: string;
  finished_at: string | null;
  files_moved: number;
  bytes_moved: number;
  error_message: string | null;
  shares: CacheMoverRunShare[];
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

export interface GeoIpCountryRule {
  country: string;
  action: 'allow' | 'deny';
  cidr_count: number;
}

export interface GeoIpStatus {
  available: boolean;
  ufw_available: boolean;
  database_present: boolean;
  database_path: string;
  database_error: string | null;
  countries: GeoIpCountryRule[];
  managed_rules: number;
  max_rules: number;
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
  task_type: 'command' | 'snapshot' | 'smart_test' | 'scrub' | 'restic_check' | 'power';
  cron: string;
  payload: string;
  enabled: boolean;
  /** IANA zone the cron is evaluated in; `null` = the appliance timezone. */
  timezone: string | null;
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
  share: string | null;
  share_type: string | null;
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

