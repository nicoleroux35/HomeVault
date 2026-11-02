// Phase 4 API Client Functions
// Add these to frontend/src/lib/services/api.ts

import type {
  UpsServerConfig,
  UpsServerGetResponse,
  UsbCopyProfile,
  RemovableDevice,
  DhcpServerConfig,
  DhcpServerGetResponse,
  DhcpLease,
  FanConfig,
  FanChannelsResponse,
  LayoutRecommendation,
  RecommendationsResponse,
  RedundancyLevel,
} from '$lib/types/phase4';

// ============================================================================
// UPS Server API
// ============================================================================

export async function getUpsServerConfig(): Promise<UpsServerGetResponse> {
  const response = await fetch('/api/power/ups-server', {
    headers: { Authorization: `Bearer ${getToken()}` },
  });
  if (!response.ok) throw new Error('Failed to get UPS server config');
  return response.json();
}

export async function updateUpsServerConfig(config: UpsServerConfig): Promise<void> {
  const response = await fetch('/api/power/ups-server', {
    method: 'PUT',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${getToken()}`,
    },
    body: JSON.stringify({ config }),
  });
  if (!response.ok) throw new Error('Failed to update UPS server config');
}

export async function startUpsServer(): Promise<void> {
  const response = await fetch('/api/power/ups-server/start', {
    method: 'POST',
    headers: { Authorization: `Bearer ${getToken()}` },
  });
  if (!response.ok) throw new Error('Failed to start UPS server');
}

export async function stopUpsServer(): Promise<void> {
  const response = await fetch('/api/power/ups-server/stop', {
    method: 'POST',
    headers: { Authorization: `Bearer ${getToken()}` },
  });
  if (!response.ok) throw new Error('Failed to stop UPS server');
}

export async function shutdownUpsServer(confirm: boolean = false): Promise<{ success: boolean; stages: any[] }> {
  const response = await fetch('/api/power/ups-server/shutdown', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${getToken()}`,
    },
    body: JSON.stringify({ confirm }),
  });
  if (!response.ok) throw new Error('Failed to shutdown via UPS server');
  return response.json();
}

// ============================================================================
// USB Copy API
// ============================================================================

export async function listUsbCopyProfiles(): Promise<UsbCopyProfile[]> {
  const response = await fetch('/api/usb-copy/profiles', {
    headers: { Authorization: `Bearer ${getToken()}` },
  });
  if (!response.ok) throw new Error('Failed to list USB copy profiles');
  return response.json();
}

export async function createUsbCopyProfile(profile: UsbCopyProfile): Promise<UsbCopyProfile> {
  const response = await fetch('/api/usb-copy/profiles', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${getToken()}`,
    },
    body: JSON.stringify(profile),
  });
  if (!response.ok) throw new Error('Failed to create USB copy profile');
  return response.json();
}

export async function updateUsbCopyProfile(id: string, profile: UsbCopyProfile): Promise<UsbCopyProfile> {
  const response = await fetch(`/api/usb-copy/profiles/${id}`, {
    method: 'PUT',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${getToken()}`,
    },
    body: JSON.stringify(profile),
  });
  if (!response.ok) throw new Error('Failed to update USB copy profile');
  return response.json();
}

export async function deleteUsbCopyProfile(id: string): Promise<void> {
  const response = await fetch(`/api/usb-copy/profiles/${id}`, {
    method: 'DELETE',
    headers: { Authorization: `Bearer ${getToken()}` },
  });
  if (!response.ok) throw new Error('Failed to delete USB copy profile');
}

export async function listUsbDevices(): Promise<RemovableDevice[]> {
  const response = await fetch('/api/usb-copy/devices', {
    headers: { Authorization: `Bearer ${getToken()}` },
  });
  if (!response.ok) throw new Error('Failed to list USB devices');
  return response.json();
}

export async function executeUsbCopy(profileId: string): Promise<{ status: string; message: string }> {
  const response = await fetch(`/api/usb-copy/execute/${profileId}`, {
    method: 'POST',
    headers: { Authorization: `Bearer ${getToken()}` },
  });
  if (!response.ok) throw new Error('Failed to execute USB copy');
  return response.json();
}

// ============================================================================
// DHCP Server API
// ============================================================================

export async function getDhcpServerConfig(): Promise<DhcpServerGetResponse> {
  const response = await fetch('/api/network/dhcp-server', {
    headers: { Authorization: `Bearer ${getToken()}` },
  });
  if (!response.ok) throw new Error('Failed to get DHCP server config');
  return response.json();
}

export async function updateDhcpServerConfig(config: DhcpServerConfig): Promise<void> {
  const response = await fetch('/api/network/dhcp-server', {
    method: 'PUT',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${getToken()}`,
    },
    body: JSON.stringify({ config }),
  });
  if (!response.ok) {
    const error = await response.text();
    throw new Error(error || 'Failed to update DHCP server config');
  }
}

export async function startDhcpServer(): Promise<void> {
  const response = await fetch('/api/network/dhcp-server/start', {
    method: 'POST',
    headers: { Authorization: `Bearer ${getToken()}` },
  });
  if (!response.ok) throw new Error('Failed to start DHCP server');
}

export async function stopDhcpServer(): Promise<void> {
  const response = await fetch('/api/network/dhcp-server/stop', {
    method: 'POST',
    headers: { Authorization: `Bearer ${getToken()}` },
  });
  if (!response.ok) throw new Error('Failed to stop DHCP server');
}

export async function getDhcpLeases(): Promise<DhcpLease[]> {
  const response = await fetch('/api/network/dhcp-server/leases', {
    headers: { Authorization: `Bearer ${getToken()}` },
  });
  if (!response.ok) throw new Error('Failed to get DHCP leases');
  return response.json();
}

// ============================================================================
// Fan Control API
// ============================================================================

export async function listFanChannels(): Promise<FanChannelsResponse> {
  const response = await fetch('/api/system/fans', {
    headers: { Authorization: `Bearer ${getToken()}` },
  });
  if (!response.ok) throw new Error('Failed to list fan channels');
  return response.json();
}

export async function getFanConfig(): Promise<FanConfig> {
  const response = await fetch('/api/system/fans/config', {
    headers: { Authorization: `Bearer ${getToken()}` },
  });
  if (!response.ok) throw new Error('Failed to get fan config');
  return response.json();
}

export async function updateFanConfig(config: FanConfig): Promise<void> {
  const response = await fetch('/api/system/fans/config', {
    method: 'PUT',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${getToken()}`,
    },
    body: JSON.stringify(config),
  });
  if (!response.ok) {
    const error = await response.text();
    throw new Error(error || 'Failed to update fan config');
  }
}

export async function restoreFanAuto(): Promise<void> {
  const response = await fetch('/api/system/fans/restore-auto', {
    method: 'POST',
    headers: { Authorization: `Bearer ${getToken()}` },
  });
  if (!response.ok) throw new Error('Failed to restore automatic fan control');
}

// ============================================================================
// Guided Redundancy API
// ============================================================================

export async function getRedundancyRecommendations(
  minRedundancy?: RedundancyLevel
): Promise<RecommendationsResponse> {
  const params = minRedundancy ? `?min_redundancy=${minRedundancy}` : '';
  const response = await fetch(`/api/storage/guided-redundancy${params}`, {
    headers: { Authorization: `Bearer ${getToken()}` },
  });
  if (!response.ok) throw new Error('Failed to get redundancy recommendations');
  return response.json();
}

export async function applyRedundancyLayout(id: string, confirmed: boolean): Promise<any> {
  const response = await fetch(`/api/storage/guided-redundancy/apply/${id}`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${getToken()}`,
    },
    body: JSON.stringify({ confirmed }),
  });
  if (!response.ok) {
    const error = await response.text();
    throw new Error(error || 'Failed to apply redundancy layout');
  }
  return response.json();
}

// ============================================================================
// Helper Function
// ============================================================================

function getToken(): string {
  // This should match your existing auth token retrieval
  return localStorage.getItem('auth_token') || '';
}
