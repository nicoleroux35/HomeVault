/* =============================================================================
 * File          : api-phase4.ts
 * Author        : Nicolas Le Roux
 * Created       : 02/11/2026 by Nicolas Le Roux
 * Version       : 2.0.0
 *
 * Description:
 *   API client for the Phase 4 appliance features: UPS server mode, USB
 *   one-touch copy, the DHCP/TFTP/PXE server, the local DNS server (phase C1,
 *   the same dnsmasq instance), fan control and guided redundancy.
 *
 *   **Every call goes through the shared `api` client**, like the rest of the
 *   application. The previous version was a parallel implementation that had
 *   two defects which made all five screens dead in production:
 *
 *   1. it read the session from `localStorage.getItem('auth_token')`, a key
 *      nothing ever writes (the app stores `session-token`), so every request
 *      carried `Authorization: Bearer ` with an empty token;
 *   2. it returned the **raw response body** (`{ success, data }`) while every
 *      caller destructured the payload directly (`response.config`,
 *      `response.channels`, …), so even an authorised call produced
 *      `undefined` fields.
 *
 *   Going through `api` fixes both at once and adds the CSRF header, the
 *   contract header, the response cache and the mutation cache invalidation
 *   that every other screen already gets.
 *
 * Dependencies:
 *   - $lib/services/api (the shared ApiClient), $lib/types/api
 *
 * Usage:
 *   Imported by `routes/UsbCopy.svelte`, `Settings.svelte` (via the three
 *   cards) and `storage/GuidedRedundancyDialog.svelte`. See
 *   docs/guides/{usb-copy,dhcp,fan-control,ups-server,guided-redundancy}.md.
 * ============================================================================= */

import { api } from '$lib/services/api';
import type {
  UpsServerConfig,
  UpsServerGetResponse,
  UsbCopyProfile,
  RemovableDevice,
  DhcpServerConfig,
  DhcpServerGetResponse,
  DhcpLease,
  LocalDnsConfig,
  LocalDnsGetResponse,
  LocalDnsUpdateResponse,
  ProxyRule,
  ProxyRuleInput,
  ProxyRuleResponse,
  ProxyRulesResponse,
  FanConfig,
  FanChannelsResponse,
  LayoutRecommendation,
  RecommendationsResponse,
  RedundancyLevel
} from '$lib/types/phase4';

/**
 * Unwraps a successful `{ success, data }` envelope, or throws the server's own
 * sentence. Callers get the payload or an exception — never an `undefined`
 * field with no explanation.
 */
function unwrap<T>(res: { success: boolean; data?: T; message?: string; error?: string }): T {
  if (!res.success) {
    throw new Error(res.message || res.error || 'The request failed');
  }
  return res.data as T;
}

// ============================================================================
// UPS Server API
// ============================================================================

export async function getUpsServerConfig(): Promise<UpsServerGetResponse> {
  return unwrap(await api.get<UpsServerGetResponse>('/power/ups-server', { cache: false }));
}

export async function updateUpsServerConfig(config: UpsServerConfig): Promise<void> {
  unwrap(await api.put('/power/ups-server', { config }));
}

export async function startUpsServer(): Promise<void> {
  unwrap(await api.post('/power/ups-server/start'));
}

export async function stopUpsServer(): Promise<void> {
  unwrap(await api.post('/power/ups-server/stop'));
}

export async function shutdownUpsServer(
  confirm = false
): Promise<{ success: boolean; stages: any[] }> {
  return unwrap(await api.post('/power/ups-server/shutdown', { confirm }));
}

// ============================================================================
// USB Copy API
// ============================================================================

export async function listUsbCopyProfiles(): Promise<UsbCopyProfile[]> {
  return unwrap(await api.get<UsbCopyProfile[]>('/usb-copy/profiles', { cache: false }));
}

export async function createUsbCopyProfile(profile: UsbCopyProfile): Promise<UsbCopyProfile> {
  return unwrap(await api.post<UsbCopyProfile>('/usb-copy/profiles', profile));
}

export async function updateUsbCopyProfile(
  id: string,
  profile: UsbCopyProfile
): Promise<UsbCopyProfile> {
  return unwrap(
    await api.put<UsbCopyProfile>(`/usb-copy/profiles/${encodeURIComponent(id)}`, profile)
  );
}

export async function deleteUsbCopyProfile(id: string): Promise<void> {
  unwrap(await api.delete(`/usb-copy/profiles/${encodeURIComponent(id)}`));
}

export async function listUsbDevices(): Promise<RemovableDevice[]> {
  return unwrap(await api.get<RemovableDevice[]>('/usb-copy/devices', { cache: false }));
}

export async function executeUsbCopy(
  profileId: string
): Promise<{ status: string; message: string }> {
  return unwrap(
    await api.post(`/usb-copy/execute/${encodeURIComponent(profileId)}`)
  );
}

// ============================================================================
// DHCP / TFTP server API
// ============================================================================

export async function getDhcpServerConfig(): Promise<DhcpServerGetResponse> {
  return unwrap(await api.get<DhcpServerGetResponse>('/network/dhcp-server', { cache: false }));
}

export async function updateDhcpServerConfig(config: DhcpServerConfig): Promise<void> {
  unwrap(await api.put('/network/dhcp-server', { config }));
}

export async function startDhcpServer(): Promise<void> {
  unwrap(await api.post('/network/dhcp-server/start'));
}

export async function stopDhcpServer(): Promise<void> {
  unwrap(await api.post('/network/dhcp-server/stop'));
}

export async function getDhcpLeases(): Promise<DhcpLease[]> {
  return unwrap(await api.get<DhcpLease[]>('/network/dhcp-server/leases', { cache: false }));
}

// ============================================================================
// Local DNS server API (phase C1 — the same dnsmasq instance as DHCP)
// ============================================================================

export async function getLocalDnsConfig(): Promise<LocalDnsGetResponse> {
  return unwrap(await api.get<LocalDnsGetResponse>('/network/dns-server', { cache: false }));
}

export async function updateLocalDnsConfig(
  config: LocalDnsConfig
): Promise<LocalDnsUpdateResponse> {
  return unwrap(await api.put<LocalDnsUpdateResponse>('/network/dns-server', { config }));
}

// ============================================================================
// Generic reverse proxy API (phase C2)
// ============================================================================

export async function listProxyRules(): Promise<ProxyRulesResponse> {
  return unwrap(await api.get<ProxyRulesResponse>('/proxy/rules', { cache: false }));
}

export async function createProxyRule(input: ProxyRuleInput): Promise<ProxyRuleResponse> {
  return unwrap(await api.post<ProxyRuleResponse>('/proxy/rules', input));
}

export async function updateProxyRule(
  id: number,
  input: ProxyRuleInput
): Promise<ProxyRuleResponse> {
  return unwrap(await api.put<ProxyRuleResponse>(`/proxy/rules/${id}`, input));
}

export async function deleteProxyRule(id: number): Promise<void> {
  await api.delete(`/proxy/rules/${id}`);
}

/** Enables/disables a rule without touching the rest of its fields. */
export async function setProxyRuleEnabled(rule: ProxyRule, enabled: boolean): Promise<ProxyRuleResponse> {
  return updateProxyRule(rule.id, {
    host: rule.host,
    path: rule.path,
    upstream: rule.upstream,
    tls: rule.tls,
    auth_required: rule.auth_required,
    enabled
  });
}

// ============================================================================
// Fan control API
// ============================================================================

export async function listFanChannels(): Promise<FanChannelsResponse> {
  return unwrap(await api.get<FanChannelsResponse>('/system/fans', { cache: false }));
}

export async function getFanConfig(): Promise<FanConfig> {
  return unwrap(await api.get<FanConfig>('/system/fans/config', { cache: false }));
}

export async function updateFanConfig(config: FanConfig): Promise<void> {
  unwrap(await api.put('/system/fans/config', config));
}

export async function restoreFanAuto(): Promise<void> {
  unwrap(await api.post('/system/fans/restore-auto'));
}

// ============================================================================
// Guided redundancy API
// ============================================================================

export async function getRedundancyRecommendations(
  minRedundancy?: RedundancyLevel
): Promise<RecommendationsResponse> {
  const params = minRedundancy ? `?min_redundancy=${encodeURIComponent(minRedundancy)}` : '';
  return unwrap(
    await api.get<RecommendationsResponse>(`/storage/guided-redundancy${params}`, { cache: false })
  );
}

/**
 * Asks the server to create an array from a recommendation.
 *
 * The route answers `501` on this build — creating the array from a
 * recommendation is not implemented. The dialog states that limitation before
 * the click; this function exists so a future implementation has its client
 * half already written and typed.
 */
export async function applyRedundancyLayout(id: string, confirmed: boolean): Promise<any> {
  return unwrap(
    await api.post(`/storage/guided-redundancy/apply/${encodeURIComponent(id)}`, { confirmed })
  );
}

export type { LayoutRecommendation, RecommendationsResponse };
