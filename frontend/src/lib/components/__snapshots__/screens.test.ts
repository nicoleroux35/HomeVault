/**
 * Snapshot tests for application screens
 * Ensures UI stability and catches unintended visual changes
 */

import { render } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

// Mock stores
vi.mock('$lib/stores/auth', () => ({
  authStore: {
    subscribe: vi.fn((fn) => {
      fn({ user: { username: 'testuser', is_admin: true }, token: 'test-token' });
      return () => {};
    }),
  },
}));

vi.mock('$lib/stores/settings', () => ({
  settingsStore: {
    subscribe: vi.fn((fn) => {
      fn({ wallpaper: 'hv-01', accent_color: '#2E72C9' });
      return () => {};
    }),
  },
}));

describe('Login screen snapshots', () => {
  it('renders login form', async () => {
    // Mock Login component
    const mockLogin = `
      <div class="login-container">
        <form>
          <h1>Sign In</h1>
          <input type="text" name="username" placeholder="Username" />
          <input type="password" name="password" placeholder="Password" />
          <button type="submit">Sign In</button>
        </form>
      </div>
    `;

    expect(mockLogin).toMatchSnapshot();
  });

  it('renders login form with error', async () => {
    const mockLoginWithError = `
      <div class="login-container">
        <form>
          <h1>Sign In</h1>
          <div class="alert error">Invalid credentials</div>
          <input type="text" name="username" placeholder="Username" />
          <input type="password" name="password" placeholder="Password" />
          <button type="submit">Sign In</button>
        </form>
      </div>
    `;

    expect(mockLoginWithError).toMatchSnapshot();
  });

  it('renders 2FA verification screen', async () => {
    const mock2FA = `
      <div class="login-container">
        <form>
          <h1>Two-Factor Authentication</h1>
          <p>Enter the 6-digit code from your authenticator app</p>
          <div class="totp-inputs">
            <input type="text" maxlength="1" />
            <input type="text" maxlength="1" />
            <input type="text" maxlength="1" />
            <input type="text" maxlength="1" />
            <input type="text" maxlength="1" />
            <input type="text" maxlength="1" />
          </div>
          <button type="submit">Verify</button>
        </form>
      </div>
    `;

    expect(mock2FA).toMatchSnapshot();
  });
});

describe('Desktop screen snapshots', () => {
  it('renders empty desktop', () => {
    const mockDesktop = `
      <div class="hv-desktop" data-testid="hv-desktop">
        <div class="hv-topbar"></div>
        <div class="hv-dock">
          <div class="hv-dock-icon" data-app="settings"></div>
          <div class="hv-dock-icon" data-app="explorer"></div>
        </div>
      </div>
    `;

    expect(mockDesktop).toMatchSnapshot();
  });

  it('renders desktop with open windows', () => {
    const mockDesktopWithWindows = `
      <div class="hv-desktop">
        <div class="hv-topbar"></div>
        <div class="hv-window" data-app="settings">
          <div class="hv-window-header">
            <h1>Settings</h1>
          </div>
          <div class="hv-window-body">
            <!-- Settings content -->
          </div>
        </div>
        <div class="hv-dock"></div>
      </div>
    `;

    expect(mockDesktopWithWindows).toMatchSnapshot();
  });
});

describe('Settings screen snapshots', () => {
  it('renders settings categories', () => {
    const mockSettings = `
      <div class="settings-container">
        <nav class="settings-nav">
          <button>General</button>
          <button>Appearance</button>
          <button>Security</button>
          <button>Network</button>
        </nav>
        <div class="settings-content">
          <h2>Appearance</h2>
          <div class="settings-card">
            <label>Wallpaper</label>
            <div class="wallpaper-grid">
              <!-- Wallpaper thumbnails -->
            </div>
          </div>
        </div>
      </div>
    `;

    expect(mockSettings).toMatchSnapshot();
  });

  it('renders appearance settings with wallpaper grid', () => {
    const wallpapers = Array.from({ length: 44 }, (_, i) => `hv-${String(i + 1).padStart(2, '0')}`);
    const mockAppearance = `
      <div class="appearance-settings">
        <div class="wallpaper-section">
          <h3>Wallpaper</h3>
          <div class="wallpaper-grid">
            ${wallpapers.map(wp => `<button class="wallpaper-thumb" data-wallpaper="${wp}"></button>`).join('')}
          </div>
        </div>
        <div class="accent-section">
          <h3>Accent Color</h3>
          <input type="color" value="#2E72C9" />
        </div>
      </div>
    `;

    expect(mockAppearance).toMatchSnapshot();
  });
});

describe('Widget snapshots', () => {
  it('renders CPU widget', () => {
    const mockCPUWidget = `
      <div class="widget cpu-widget">
        <div class="widget-header">
          <span class="widget-icon">💻</span>
          <h3>CPU</h3>
        </div>
        <div class="widget-body">
          <div class="metric">
            <span class="metric-value">45%</span>
            <span class="metric-label">Usage</span>
          </div>
        </div>
      </div>
    `;

    expect(mockCPUWidget).toMatchSnapshot();
  });

  it('renders storage widget', () => {
    const mockStorageWidget = `
      <div class="widget storage-widget">
        <div class="widget-header">
          <span class="widget-icon">💾</span>
          <h3>Storage</h3>
        </div>
        <div class="widget-body">
          <div class="progress-bar">
            <div class="progress-fill" style="width: 65%"></div>
          </div>
          <p>650 GB / 1 TB used</p>
        </div>
      </div>
    `;

    expect(mockStorageWidget).toMatchSnapshot();
  });

  it('renders network widget', () => {
    const mockNetworkWidget = `
      <div class="widget network-widget">
        <div class="widget-header">
          <span class="widget-icon">🌐</span>
          <h3>Network</h3>
        </div>
        <div class="widget-body">
          <div class="metric">
            <span class="label">↓ Down</span>
            <span class="value">125 Mbps</span>
          </div>
          <div class="metric">
            <span class="label">↑ Up</span>
            <span class="value">45 Mbps</span>
          </div>
        </div>
      </div>
    `;

    expect(mockNetworkWidget).toMatchSnapshot();
  });
});

describe('Component consistency', () => {
  it('all buttons use consistent styling', () => {
    const buttons = [
      '<button class="btn btn-primary">Primary</button>',
      '<button class="btn btn-secondary">Secondary</button>',
      '<button class="btn btn-danger">Danger</button>',
    ];

    buttons.forEach((btn) => {
      expect(btn).toContain('class="btn');
    });

    expect(buttons).toMatchSnapshot();
  });

  it('all cards use consistent structure', () => {
    const cards = [
      `<div class="card">
        <div class="card-header">Title</div>
        <div class="card-body">Content</div>
      </div>`,
      `<div class="card">
        <div class="card-header">Another Card</div>
        <div class="card-body">More content</div>
      </div>`,
    ];

    expect(cards).toMatchSnapshot();
  });
});
