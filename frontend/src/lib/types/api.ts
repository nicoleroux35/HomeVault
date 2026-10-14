/* =============================================================================
 * File          : api.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 12/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Shared frontend type definitions mirroring the backend's API shapes:
 *   the standard response envelope plus the request/response types for
 *   auth, setup, users and the feature areas consumed by services/api.ts.
 *
 * Dependencies:
 *   - none (types only)
 *
 * Usage:
 *   `import type { ApiResponse, ... } from '$lib/types/api'` across the
 *   frontend; keep in sync with the Rust handlers (see docs/API.md).
 * =============================================================================
 */

/** Standard API response envelope – every endpoint returns this shape. */
export interface ApiResponse<T = any> {
  success: boolean;
  data?: T;
  error?: string;
  message?: string;
}

// ── Auth ────────────────────────────────────────────────────────────────────

export interface LoginRequest {
  username: string;
  password: string;
  totp_code?: string;
  remember_me?: boolean;
}

export interface LoginResponse {
  token: string;
  csrf_token: string;
  username: string;
  is_admin: boolean;
  requires_2fa?: boolean;
  /** Only on `POST /auth/setup`: whether the TOTP enrolled at the wizard's
   *  password step was persisted for the new account. */
  totp_enabled?: boolean;
  /** Only on `POST /auth/setup` with TOTP: the one-time backup codes. */
  backup_codes?: string[];
}

export interface SetupAdminRequest {
  username: string;
  password: string;
  // Optional server-side (`SetupRequest.fullname: Option<String>` in
  // src/storage/models.rs) — omitting it entirely (not an empty string)
  // matters: the backend only runs its fullname validation when the field
  // is `Some`, so sending `""` instead of omitting it turns a legitimately
  // blank full name into a validation error.
  fullname?: string;
  avatar_seed?: string;
  /** Optional TOTP enrollment done at the wizard's password step: the server
   *  validates the code against this client-generated secret before creating
   *  the account and enables 2FA on it. Both fields must be sent together. */
  totp_secret?: string;
  totp_code?: string;
}

export interface SetupStatus {
  is_first_run: boolean;
  is_setup_valid: boolean;
  hardware_changed: boolean;
  /** An admin exists but the recorded hardware fingerprint no longer matches
   *  this host — the app routes to the recovery screen, never the wizard. */
  needs_recovery: boolean;
  server_machine_id: string;
  sudo_available?: boolean;
}

export interface User {
  username: string;
  is_admin: boolean;
  role?: string;
  role_id?: number;
  email?: string;
  fullname?: string;
  created_at?: string;
  avatar_seed?: string;
  avatar_url?: string;
  two_fa_enabled?: boolean;
}

// ── System ──────────────────────────────────────────────────────────────────

export interface UserHomeConfig {
  enabled: boolean;
  disk: string | null;
  path: string | null;
  created_at: string | null;
}

export interface AvailableHomeDisk {
  device: string;
  mount_point: string;
  filesystem: string;
  available_bytes: number;
  total_bytes: number;
}

export interface SystemInfo {
  hostname: string;
  os: string;
  kernel: string;
  uptime: number;
  boot_time: number;
}

export interface CpuInfo {
  model: string;
  cores: number;
  threads: number;
  frequency: number;
  usage: number;
}

export interface MemoryInfo {
  total: number;
  used: number;
  available: number;
  cached: number;
  percent: number;
}

export interface DiskInfo {
  device: string;
  mount_point: string;
  filesystem: string;
  total: number;
  used: number;
  available: number;
  percent: number;
}

// ── Docker ──────────────────────────────────────────────────────────────────

export interface DockerContainer {
  id: string;
  name: string;
  image: string;
  status: string;
  state: string;
  created: string;
  ports: Array<{ host_port: number; container_port: number; protocol: string }>;
}

export interface DockerImage {
  id: string;
  tags: string[];
  size: number;
  created: string;
}

export interface DockerNetwork {
  id: string;
  name: string;
  driver: string;
  scope: string;
  subnet: string | null;
  gateway: string | null;
  internal: boolean;
  containers: string[];
}

// ── Network ─────────────────────────────────────────────────────────────────

export interface NetworkInterface {
  name: string;
  mac: string;
  ip: string;
  status: string;
  speed?: number;
  type: 'ethernet' | 'wifi';
}

// ── Widgets ─────────────────────────────────────────────────────────────────

export interface Widget {
  id: string;
  type: string;
  position: number;
  config?: any;
}

// ── Storage ─────────────────────────────────────────────────────────────────

export interface PhysicalDisk {
  name: string;
  device: string;
  size: number;
  model: string;
  vendor?: string;
  serial: string;
  type: string;
  role: string;
  connection_type: string;
  smart_status: string;
  temperature?: number | null;
  partition_count: number;
  errors: number;
  partitions?: PartitionInfo[];
}

export interface PartitionInfo {
  device: string;
  mount_point: string;
  filesystem: string;
  size: number;
  used: number;
  available: number;
  percent: number;
  label?: string;
  uuid?: string;
}

export interface RaidArray {
  name: string;
  level: string;
  devices: string[];
  status: string;
  state: string;
  size: number;
  used: number;
  raid_devices: number;
  active_devices: number;
  failed_devices: number;
  resync_percent?: number;
  chunk_size?: number;
}

export interface RemovableDisk {
  name: string;
  device: string;
  size: number;
  model: string;
  filesystem?: string;
  mount_point?: string;
  label?: string;
  mounted: boolean;
}

export interface AvailableDevice {
  name: string;
  size: number;
  model: string;
  in_use: boolean;
}

export interface PoolInfo {
  name: string;
  type: string;
  size: number;
  used: number;
  available: number;
  status: string;
  health: string;
  devices: string[];
}
