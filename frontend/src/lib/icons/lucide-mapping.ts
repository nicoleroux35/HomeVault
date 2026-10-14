/* =============================================================================
 * File          : lucide-mapping.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   DEPRECATED legacy icon mapping kept only for old call sites; new code
 *   must use $lib/icons/mapping-unified instead.
 *
 * Dependencies:
 *   - none
 *
 * Usage:
 *   Legacy call sites only — do not extend.
 * =============================================================================
 */

/**
 * Legacy Icon Mapping - HomeVault (Deprecated)
 * Maps Material Design icon names to Tabler icon base names
 * NOTE: New code should use '$lib/icons/mapping-unified' instead
 *
 * @deprecated Use mapping-unified.ts instead
 */

/** Maps Material Design icon names to Tabler icon base names */
export const iconMapping: Record<string, string> = {
  // Material Design → Tabler mappings
  'progress_activity': 'loader',
  'account_circle': 'user',
  'group': 'users',
  'widgets': 'layout-grid',
  'dark_mode': 'moon',
  'notifications': 'bell',
  'create_new_folder': 'folder-plus',
  'view_list': 'list',
  'grid_view': 'grid-3x3',
  'content_copy': 'copy',
  'drive_file_move': 'folder-minus',
  'cloud_upload': 'cloud-upload',
  'cloud_done': 'cloud-check',
  'shield_check': 'shield-checkered',
  'cast_connected': 'cast',
  'play_arrow': 'player-play',
  'error_outline': 'alert-circle',
  'video_library': 'movie',
  'dns': 'globe',
  'power_settings_new': 'power',
  'close': 'x',
  'user': 'user',
  'settings': 'settings',
  'lock': 'lock',
  'shield': 'shield',
  'refresh': 'refresh',
  'wifi_off': 'wifi-off',
  'loader': 'loader',
  'bookmark': 'bookmark',
  'home': 'home',
  'arrow_upward': 'arrow-up',
  'upload': 'upload',
  'delete': 'trash',
  'download': 'download',
  'visibility': 'eye',
  'unarchive': 'archive',
  'edit': 'pencil',
  'folder_open': 'folder-open',
  'chevron_left': 'chevron-left',
  'add': 'plus',
  'cast': 'cast',
  'error': 'alert-circle',
  'pause': 'player-pause',
  'stop': 'square',
  'tabler-check-circle': 'circle-check',
  'person': 'user',
  'hard_drive': 'device-hard-drive',
  'device_usb': 'usb',
  'terminal': 'terminal',
  'power': 'power',
  'launchpad': 'layout-grid',
  'app-window': 'layout',
  'console': 'terminal',
  'dashboard': 'layout-dashboard',
  'folder': 'folder',
  'inventory_2': 'archive',
  'docker': 'container',
  'network': 'network',
  'backup': 'device-floppy',
  'people': 'users',
  'folder_shared': 'share',
  'cloud_sync': 'cloud',
  'monitor_heart': 'heartbeat',
  'key': 'key',
  'memory': 'cpu',
  'storage': 'device-hard-drive',
  'network_check': 'wifi',
  'thermometer': 'thermometer',
};

export function getIcon(name: string): string {
  return iconMapping[name] || name;
}

export function hasIcon(name: string): boolean {
  return name in iconMapping || name.startsWith('tabler-');
}
