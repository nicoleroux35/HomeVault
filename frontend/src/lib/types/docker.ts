/* =============================================================================
 * File          : docker.ts
 * Author        : Nicolas Le Roux
 * Created       : 24/10/2026
 * Last modified : 24/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The wire shapes of the Docker app surface (containers, images, networks,
 *   volumes, disk usage, Compose projects, the app catalog and its community
 *   stores), plus the two request bodies that carry more than one field.
 *   They used to live inside `routes/Docker.svelte`; they moved here so the
 *   wrapper methods of `services/api.ts` and the screen share one definition
 *   (ROADMAP 31.1). They mirror `src/handlers/docker.rs`,
 *   `src/handlers/compose.rs` and `src/app_catalog/` — keep them in sync with
 *   the Rust serializers, not with what a screen happens to read.
 *
 * Dependencies:
 *   - none (types only)
 *
 * Usage:
 *   `import type { DockerContainer } from '$lib/types/docker'` from
 *   `services/api.ts` and from any screen that renders the payload.
 * =============================================================================
 */

/** One published port, as `docker inspect` reports it. */
export interface DockerContainerPort {
  host_port: number;
  container_port: number;
  protocol: string;
}

/** One row of `GET /docker/containers` (with or without `?stats=true`). */
export interface DockerContainer {
  id: string;
  name: string;
  image: string;
  status: string;
  state: 'Running' | 'Exited' | 'Paused' | 'Restarting' | 'Dead' | 'Created' | 'Unknown';
  created: string;
  ports: DockerContainerPort[];
  /** `null` unless the request asked for live stats (`?stats=true`). */
  memory_mb: number | null;
  cpu_percent: number | null;
  net_rx_bytes: number | null;
  net_tx_bytes: number | null;
  blk_read_bytes: number | null;
  blk_write_bytes: number | null;
  update_available: boolean;
  compose_project: string | null;
  networks: string[];
}

/** One row of `GET /docker/images`. */
export interface DockerImage {
  id: string;
  repository: string;
  tag: string;
  size: string;
  created: string;
}

/** One row of `GET /docker/networks`. */
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

/** One row of `GET /docker/hub/search`. */
export interface DockerHubResult {
  name: string;
  description: string;
  star_count: number;
  pull_count: number;
  is_official: boolean;
}

/** One row of `GET /docker/compose/projects`. */
export interface DockerComposeProject {
  name: string;
  config_path: string;
  services: string[];
  status: string;
}

/** One row of `GET /docker/volumes`. */
export interface DockerVolume {
  name: string;
  driver: string;
  scope: string;
  mountpoint: string;
  in_use: boolean;
  used_by: string[];
}

/** One `docker system df` line, already humanised by the backend. */
export interface DockerDiskUsageRow {
  kind: string;
  total: number;
  active: number;
  size: string;
  reclaimable: string;
}

/** `GET /docker/system/df`. */
export interface DockerDiskUsage {
  rows: DockerDiskUsageRow[];
  total_size: string;
  total_reclaimable: string;
}

/**
 * `GET /docker/port-check` — is a host port free to publish on? Checked
 * against the host's bound ports, other containers and HomeVault's own port.
 */
export interface DockerPortStatus {
  available: boolean;
  /** What holds it when unavailable: `container`, `homevault` or `host`. */
  used_by: string | null;
  /** Extra context for `used_by` — e.g. the container name. */
  detail: string | null;
}

/** `POST /docker/prune`. */
export interface DockerPruneReport {
  target: string;
  reclaimed: string;
  message: string;
}

/** One mount of `GET /docker/containers/{name}/inspect`. */
export interface DockerContainerMount {
  kind: string;
  source: string;
  destination: string;
  read_only: boolean;
  name: string | null;
}

/** `GET /docker/containers/{name}/inspect` — the container's real settings. */
export interface DockerContainerDetail {
  id: string;
  name: string;
  image: string;
  image_id: string;
  image_digest: string | null;
  created: string;
  state: string;
  status: string;
  exit_code: number | null;
  started_at: string | null;
  finished_at: string | null;
  restart_count: number;
  pid: number | null;
  command: string[];
  entrypoint: string[];
  working_dir: string;
  hostname: string;
  user: string;
  restart_policy: string;
  privileged: boolean;
  memory_limit: number | null;
  nano_cpus: number | null;
  env: string[];
  labels: [string, string][];
  mounts: DockerContainerMount[];
  ports: DockerContainerPort[];
  networks: string[];
  ip_address: string | null;
  health: string | null;
  log_driver: string | null;
  log_path: string | null;
  compose_project: string | null;
  compose_service: string | null;
}

/** `POST /docker/containers/{name}/exec`. */
export interface DockerExecResult {
  stdout: string;
  stderr: string;
  exit_code: number;
}

/** One mount row of the create-container request. */
export interface DockerCreateVolume {
  host_path: string;
  container_path: string;
  read_only: boolean;
}

/** One env row of the create-container request. */
export interface DockerCreateEnvVar {
  key: string;
  value: string;
}

/**
 * `POST /docker/containers`. Every field is optional on the wire except
 * `name` and `image`; the screen sends the whole object and lets the backend
 * validate it.
 */
export interface DockerCreateContainerRequest {
  name: string;
  image: string;
  ports?: DockerContainerPort[];
  volumes?: DockerCreateVolume[];
  env?: DockerCreateEnvVar[];
  network?: string | null;
  restart_policy?: string;
  memory_limit?: string | null;
  cpu_limit?: string | null;
  privileged?: boolean;
  auto_remove?: boolean;
}

/** `GET/POST /docker/volumes`. */
export interface DockerCreateVolumeRequest {
  name: string;
  driver?: string | null;
}

/** `POST /docker/networks`. */
export interface DockerCreateNetworkRequest {
  name: string;
  driver: string;
  subnet?: string | null;
  gateway?: string | null;
  internal?: boolean;
}

/** One `[[environment]]` declaration of an app package manifest. */
export interface DockerEnvVarDecl {
  name: string;
  services: string[];
  default: string | null;
  options: string[];
  required: boolean;
  note: string | null;
}

/**
 * One catalog row. `id` is the **namespaced** `<store>:<app>` key used in
 * every catalog URL — never the install key (see `docs/guides/docker.md`).
 */
export interface DockerCatalogApp {
  id: string;
  /** Bare app id, for the Lucide-icon fallback lookup. */
  app_id: string;
  name: string;
  description: string;
  tagline: string | null;
  category: string;
  icon: string | null;
  has_icon: boolean;
  gallery_count: number;
  default_port: number;
  version: string;
  developer: string | null;
  website: string | null;
  repo: string | null;
  dependencies: string[];
  implements: string[];
  environment: DockerEnvVarDecl[];
  missing_dependencies: string[];
  incompatibility_reason: string | null;
  store_id: string;
  store_name: string;
  installed: boolean;
  running: boolean;
  installed_port: number | null;
  proxy_port: number | null;
  proxy_auth_required: boolean;
  unsupported_on_host: boolean;
}

/** One row of `GET /docker/stores` (a community package store). */
export interface DockerAppStore {
  id: string;
  name: string;
  git_url: string;
  enabled: boolean;
  last_synced_at: number | null;
  last_error: string | null;
  app_count: number;
}

/** `GET /docker/catalog/updates`. */
export interface DockerAppUpdateStatus {
  id: string;
  name: string;
  installed_version: string | null;
  available_version: string;
  package_update_available: boolean;
  image_update_available: boolean;
}

/** One dependency line of the pre-install review. */
export interface DockerInstallReviewDep {
  dependency: string;
  satisfied_by: string[];
  providers: string[];
  selected: string | null;
}

/** `GET /docker/catalog/{id}/install-review`. */
export interface DockerInstallReview {
  id: string;
  name: string;
  version: string;
  incompatibility_reason: string | null;
  dependencies: DockerInstallReviewDep[];
  environment: DockerEnvVarDecl[];
  default_port: number;
}

/** `POST /docker/catalog/{id}/install`. */
export interface DockerCatalogInstallRequest {
  port?: number;
  alternatives?: Record<string, string>;
  environment?: Record<string, string>;
}

/** `POST /docker/catalog/update-all`. */
export interface DockerCatalogUpdateAllReport {
  updated: string[];
  failed: { id: string; error: string }[];
}

/** `POST /docker/images/pull`. */
export interface DockerPullImageRequest {
  image: string;
  tag: string;
}

/** `POST /docker/images/build`. */
export interface DockerBuildImageRequest {
  context: string;
  dockerfile: string | null;
  tag: string;
  no_cache: boolean;
  pull: boolean;
  build_args: string[];
}

/** `POST /docker/containers/{name}/update` — live resource limits. */
export interface DockerContainerUpdateRequest {
  memory_limit: string | null;
  cpu_limit: string | null;
  restart_policy: string | null;
}

/** The four targets `POST /docker/prune` accepts. */
export type DockerPruneTarget = 'images' | 'volumes' | 'containers' | 'networks' | 'builders' | 'system';

/** The lifecycle verbs of a container / a Compose project. */
export type DockerContainerAction = 'start' | 'stop' | 'restart';
export type DockerComposeAction = 'start' | 'stop' | 'restart' | 'update';
