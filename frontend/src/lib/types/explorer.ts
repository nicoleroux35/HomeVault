/* =============================================================================
 * File          : explorer.ts
 * Author        : Nicolas Le Roux
 * Created       : 09/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Types of the Explorer's network locations (CIFS/NFS remote mounts):
 *   `GET /api/remote-mounts` and its mutation endpoints.
 *
 * Dependencies:
 *   - none
 *
 * Usage:
 *   Imported by $lib/services/api and the RemoteMountPanel component.
 * =============================================================================
 */

/** Which remote protocol a network location speaks. */
export type RemoteMountKind = 'cifs' | 'nfs';

/** One configured network location, as persisted on the server. */
export interface RemoteMount {
  /** Slug, also the folder name under `/mnt/homevault-remote`. */
  name: string;
  kind: RemoteMountKind;
  server: string;
  /** CIFS share name or NFS export path. */
  source: string;
  username: string | null;
  domain: string | null;
  /** Extra mount options, from the per-protocol allowlist. */
  options: string[];
  read_only: boolean;
  /** Mount again automatically after a restart. */
  enabled: boolean;
  created_at: string;
  /** Last failure, so the panel can explain a location that is not up. */
  last_error: string | null;
}

/**
 * One network location plus its live state. The server never returns the
 * password — only whether one is stored.
 */
export interface RemoteMountView extends RemoteMount {
  /** Absolute host path the share is (or would be) mounted at. */
  target: string;
  mounted: boolean;
  has_credentials: boolean;
}

/** Body of `POST /api/remote-mounts` and `PUT /api/remote-mounts/{name}`. */
export interface RemoteMountRequest {
  name?: string;
  kind?: RemoteMountKind;
  server?: string;
  source?: string;
  username?: string;
  /** Write-only: a blank value keeps the stored password. */
  password?: string;
  domain?: string;
  options?: string[];
  read_only?: boolean;
  enabled?: boolean;
  /** Bring the location up in the same call (also remounts after an edit). */
  mount_now?: boolean;
}
