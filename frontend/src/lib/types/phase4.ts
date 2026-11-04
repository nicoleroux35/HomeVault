// Phase 4 TypeScript Type Definitions

// ============================================================================
// UPS Server Types
// ============================================================================

export interface UpsServerConfig {
  enabled: boolean;
  bind_interface: string;
  bind_address: string | null;
  port: number;
  allowed_clients: UpsClient[];
  shutdown_policy: ShutdownPolicy;
  max_clients: number;
}

export interface UpsClient {
  name: string;
  address: string;
  username: string;
  password_hash: string;
  master: boolean;
}

export interface ShutdownPolicy {
  stop_containers: boolean;
  shutdown_vms: boolean;
  flush_restic: boolean;
  stop_shares: boolean;
  stage_timeout_secs: Record<string, number>;
}

export interface UpsServerState {
  running: boolean;
  connected_clients: number;
  last_error: string | null;
  shutdown_in_progress: boolean;
  shutdown_stage: string | null;
}

export interface UpsServerGetResponse {
  available: boolean;
  config: UpsServerConfig;
  state: UpsServerState;
}

// ============================================================================
// USB Copy Types
// ============================================================================

export interface UsbCopyProfile {
  id: string;
  name: string;
  device_uuid: string;
  device_label: string | null;
  source_path: string;
  destination_path: string;
  verify_checksums: boolean;
  auto_eject: boolean;
  enabled: boolean;
  last_run: string | null;
  last_status: CopyStatus | null;
}

export type CopyStatus =
  | { Success: { files_copied: number; bytes_copied: number } }
  | { Failed: { error: string } }
  | 'Cancelled';

export interface RemovableDevice {
  device: string;
  uuid: string | null;
  label: string | null;
  mount_point: string | null;
  size: number;
  mounted: boolean;
  matching_profile: string | null;
}

// ============================================================================
// DHCP Server Types
// ============================================================================

export interface DhcpServerConfig {
  dhcp_enabled: boolean;
  tftp_enabled: boolean;
  interface: string;
  dhcp_range_start: string;
  dhcp_range_end: string;
  netmask: string;
  gateway: string | null;
  dns_servers: string[];
  lease_time_secs: number;
  reservations: Record<string, string>;
  tftp_root: string;
  pxe_boot_file: string | null;
  confirmed: boolean;
}

export interface DhcpServerState {
  running: boolean;
  leases: DhcpLease[];
  conflict_detected: boolean;
  last_error: string | null;
}

export interface DhcpLease {
  mac: string;
  ip: string;
  hostname: string | null;
  expires_at: number;
}

export interface DhcpServerGetResponse {
  config: DhcpServerConfig;
  state: DhcpServerState;
  available: boolean;
}

// ============================================================================
// Local DNS Server Types (phase C1 — the same dnsmasq instance as DHCP)
// ============================================================================

/** What kind of answer a local name record produces. */
export type DnsRecordKind = 'a' | 'cname';

export interface DnsRecord {
  name: string;
  kind: DnsRecordKind;
  /** An IPv4 address for `a`, the alias target for `cname`. */
  value: string;
}

export interface DnsDomainOverride {
  /** The domain, optionally prefixed with `.` to match subdomains only. */
  domain: string;
  address: string;
}

export interface LocalDnsConfig {
  enabled: boolean;
  interface: string;
  serve_dhcp_clients: boolean;
  forwarders: string[];
  records: DnsRecord[];
  overrides: DnsDomainOverride[];
  confirmed: boolean;
}

export interface LocalDnsState {
  /** `dnsmasq` is running (it also serves DHCP/TFTP). */
  running: boolean;
  /** Running *and* this feature is enabled: queries are being answered. */
  serving: boolean;
}

export interface LocalDnsGetResponse {
  config: LocalDnsConfig;
  state: LocalDnsState;
  available: boolean;
  interfaces: string[];
  system_forwarders: string[];
  dhcp_enabled: boolean;
}

export interface LocalDnsUpdateResponse {
  config: LocalDnsConfig;
  state: LocalDnsState;
  message: string;
}

// ============================================================================
// Fan Control Types
// ============================================================================

export type FanProfile = 'auto' | 'quiet' | 'balanced' | 'performance' | 'manual';

export interface FanConfig {
  profile: FanProfile;
  manual_duty: number;
  channel_overrides: Record<string, FanChannelConfig>;
  interval_secs: number;
}

export interface FanChannelConfig {
  profile: FanProfile | null;
  manual_duty: number | null;
  temp_sensor: string | null;
}

export interface FanChannel {
  id: string;
  name: string;
  hwmon_path: string;
  pwm_num: number;
  supports_auto: boolean;
  current_pwm: number;
  current_mode: number;
  temp_sensor: string | null;
  current_temp: number | null;
}

export interface FanChannelsResponse {
  channels: FanChannel[];
  is_laptop: boolean;
  message: string | null;
}

// ============================================================================
// Guided Redundancy Types
// ============================================================================

export type RedundancyLevel = 'none' | 'single' | 'double';
export type StorageTech = 'mdadm' | 'zfs' | 'btrfs';
export type RiskLevel = 'low' | 'medium' | 'high';
export type RebuildTime = 'fast' | 'moderate' | 'slow' | 'very_slow';

export interface LayoutRecommendation {
  id: string;
  name: string;
  description: string;
  technology: StorageTech;
  level: string;
  redundancy: RedundancyLevel;
  usable_bytes: number;
  raw_bytes: number;
  efficiency_pct: number;
  risk: RiskAssessment;
  layout: DiskLayout;
  warnings: string[];
  score: number;
}

export interface RiskAssessment {
  data_loss_risk: RiskLevel;
  rebuild_time: RebuildTime;
  failure_tolerance: number;
  explanation: string;
}

export interface DiskLayout {
  groups: DiskGroup[];
  metadata_bytes: number;
}

export interface DiskGroup {
  disks: string[];
  role: string;
}

export interface RecommendationsResponse {
  recommendations: LayoutRecommendation[];
  total_disks: number;
  available_disks: number;
}
