/**
 * Unified Icon Mapping
 * Generated from: Phosphor Icons, Tabler Icons
 *
 * Usage:
 * import { getIcon, iconNames } from '$lib/icons/mapping-unified';
 */

export const iconLibraries = ['phosphor', 'tabler'] as const;
export type IconLibrary = typeof iconLibraries[number];

export interface IconEntry {
  phosphor?: string;
  tabler?: string;
}

export const iconCount = {
  phosphor: 53,
  tabler: 84,
  total: 137
};

export const iconLookup: Record<string, IconEntry> = {
  "archive": {
    "phosphor": "phosphor-archive",
    "tabler": "tabler-archive"
  },
  "article": {
    "phosphor": "phosphor-article",
    "tabler": "tabler-article"
  },
  "book-open": {
    "phosphor": "phosphor-book-open"
  },
  "calendar": {
    "phosphor": "phosphor-calendar",
    "tabler": "tabler-calendar"
  },
  "camera": {
    "phosphor": "phosphor-camera",
    "tabler": "tabler-camera"
  },
  "caret-down": {
    "phosphor": "phosphor-caret-down",
    "tabler": "tabler-caret-down"
  },
  "caret-up": {
    "phosphor": "phosphor-caret-up",
    "tabler": "tabler-caret-up"
  },
  "certificate": {
    "phosphor": "phosphor-certificate",
    "tabler": "tabler-certificate"
  },
  "chart-line": {
    "phosphor": "phosphor-chart-line",
    "tabler": "tabler-chart-line"
  },
  "chat": {
    "phosphor": "phosphor-chat"
  },
  "clock": {
    "phosphor": "phosphor-clock",
    "tabler": "tabler-clock"
  },
  "cloud": {
    "phosphor": "phosphor-cloud",
    "tabler": "tabler-cloud"
  },
  "command": {
    "phosphor": "phosphor-command",
    "tabler": "tabler-command"
  },
  "copy": {
    "phosphor": "phosphor-copy",
    "tabler": "tabler-copy"
  },
  "cpu": {
    "phosphor": "phosphor-cpu",
    "tabler": "tabler-cpu"
  },
  "database": {
    "phosphor": "phosphor-database",
    "tabler": "tabler-database"
  },
  "desktop": {
    "phosphor": "phosphor-desktop"
  },
  "download": {
    "phosphor": "phosphor-download",
    "tabler": "tabler-download"
  },
  "export": {
    "phosphor": "phosphor-export"
  },
  "file": {
    "phosphor": "phosphor-file",
    "tabler": "tabler-file"
  },
  "function": {
    "phosphor": "phosphor-function",
    "tabler": "tabler-function"
  },
  "gauge": {
    "phosphor": "phosphor-gauge",
    "tabler": "tabler-gauge"
  },
  "ghost": {
    "phosphor": "phosphor-ghost",
    "tabler": "tabler-ghost"
  },
  "gif": {
    "phosphor": "phosphor-gif",
    "tabler": "tabler-gif"
  },
  "git-branch": {
    "phosphor": "phosphor-git-branch",
    "tabler": "tabler-git-branch"
  },
  "heartbeat": {
    "phosphor": "phosphor-heartbeat",
    "tabler": "tabler-heartbeat"
  },
  "house": {
    "phosphor": "phosphor-house"
  },
  "image": {
    "phosphor": "phosphor-image"
  },
  "info": {
    "phosphor": "phosphor-info"
  },
  "key": {
    "phosphor": "phosphor-key",
    "tabler": "tabler-key"
  },
  "layout": {
    "phosphor": "phosphor-layout",
    "tabler": "tabler-layout"
  },
  "list": {
    "phosphor": "phosphor-list",
    "tabler": "tabler-list"
  },
  "list-checks": {
    "phosphor": "phosphor-list-checks"
  },
  "lock": {
    "phosphor": "phosphor-lock",
    "tabler": "tabler-lock"
  },
  "log": {
    "phosphor": "phosphor-log"
  },
  "megaphone": {
    "phosphor": "phosphor-megaphone"
  },
  "memory": {
    "phosphor": "phosphor-memory",
    "tabler": "tabler-memory"
  },
  "network": {
    "phosphor": "phosphor-network",
    "tabler": "tabler-network"
  },
  "notification": {
    "phosphor": "phosphor-notification",
    "tabler": "tabler-notification"
  },
  "option": {
    "phosphor": "phosphor-option",
    "tabler": "tabler-option"
  },
  "power": {
    "phosphor": "phosphor-power",
    "tabler": "tabler-power"
  },
  "rewind": {
    "phosphor": "phosphor-rewind"
  },
  "rss": {
    "phosphor": "phosphor-rss",
    "tabler": "tabler-rss"
  },
  "share": {
    "phosphor": "phosphor-share",
    "tabler": "tabler-share"
  },
  "stop": {
    "phosphor": "phosphor-stop"
  },
  "strategy": {
    "phosphor": "phosphor-strategy"
  },
  "sun": {
    "phosphor": "phosphor-sun",
    "tabler": "tabler-sun"
  },
  "terminal": {
    "phosphor": "phosphor-terminal",
    "tabler": "tabler-terminal"
  },
  "upload": {
    "phosphor": "phosphor-upload",
    "tabler": "tabler-upload"
  },
  "user": {
    "phosphor": "phosphor-user",
    "tabler": "tabler-user"
  },
  "users": {
    "phosphor": "phosphor-users",
    "tabler": "tabler-users"
  },
  "warning": {
    "phosphor": "phosphor-warning"
  },
  "x": {
    "phosphor": "phosphor-x",
    "tabler": "tabler-x"
  },
  "activity": {
    "tabler": "tabler-activity"
  },
  "ai": {
    "tabler": "tabler-ai"
  },
  "alert-triangle": {
    "tabler": "tabler-alert-triangle"
  },
  "battery": {
    "tabler": "tabler-battery"
  },
  "bmp": {
    "tabler": "tabler-bmp"
  },
  "box": {
    "tabler": "tabler-box"
  },
  "checkbox": {
    "tabler": "tabler-checkbox"
  },
  "container": {
    "tabler": "tabler-container"
  },
  "csv": {
    "tabler": "tabler-csv"
  },
  "dashboard": {
    "tabler": "tabler-dashboard"
  },
  "folder": {
    "tabler": "tabler-folder"
  },
  "history": {
    "tabler": "tabler-history"
  },
  "home": {
    "tabler": "tabler-home"
  },
  "html": {
    "tabler": "tabler-html"
  },
  "id": {
    "tabler": "tabler-id"
  },
  "jpg": {
    "tabler": "tabler-jpg"
  },
  "json": {
    "tabler": "tabler-json"
  },
  "language": {
    "tabler": "tabler-language"
  },
  "layout-dashboard": {
    "tabler": "tabler-layout-dashboard"
  },
  "layout-grid": {
    "tabler": "tabler-layout-grid"
  },
  "login": {
    "tabler": "tabler-login"
  },
  "logout": {
    "tabler": "tabler-logout"
  },
  "mail": {
    "tabler": "tabler-mail"
  },
  "menu": {
    "tabler": "tabler-menu"
  },
  "message": {
    "tabler": "tabler-message"
  },
  "movie": {
    "tabler": "tabler-movie"
  },
  "music": {
    "tabler": "tabler-music"
  },
  "number": {
    "tabler": "tabler-number"
  },
  "png": {
    "tabler": "tabler-png"
  },
  "progress": {
    "tabler": "tabler-progress"
  },
  "refresh": {
    "tabler": "tabler-refresh"
  },
  "restore": {
    "tabler": "tabler-restore"
  },
  "search": {
    "tabler": "tabler-search"
  },
  "server": {
    "tabler": "tabler-server"
  },
  "settings": {
    "tabler": "tabler-settings"
  },
  "svg": {
    "tabler": "tabler-svg"
  },
  "timezone": {
    "tabler": "tabler-timezone"
  },
  "txt": {
    "tabler": "tabler-txt"
  },
  "wallpaper": {
    "tabler": "tabler-wallpaper"
  },
  "webhook": {
    "tabler": "tabler-webhook"
  },
  "wheel": {
    "tabler": "tabler-wheel"
  },
  "wifi": {
    "tabler": "tabler-wifi"
  },
  "window": {
    "tabler": "tabler-window"
  },
  "world": {
    "tabler": "tabler-world"
  },
  "zip": {
    "tabler": "tabler-zip"
  }
};

export function getIcon(baseName: string, library?: IconLibrary): string | null {
  const entry = iconLookup[baseName];
  if (!entry) return null;

  if (library) {
    return entry[library] || null;
  }

  return entry.tabler || entry.phosphor || null;
}

export function hasIcon(baseName: string): boolean {
  return baseName in iconLookup;
}

export function getAllIcons(baseName: string): string[] {
  const entry = iconLookup[baseName];
  if (!entry) return [];
  return Object.values(entry).filter(Boolean);
}

export const iconNames = Object.keys(iconLookup);
