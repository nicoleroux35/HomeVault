/**
 * windowManager — the desktop windowing store.
 *
 * These are the invariants the shell relies on and that were previously only
 * asserted through the 3 000-line desktop Playwright spec: opening cascades,
 * focus raises z-order, minimize/restore, maximize round-trips, and a narrow
 * viewport opens maximized so the title-bar controls stay reachable.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';

type WindowManagerModule = typeof import('$lib/stores/windowManager');

function viewport(width: number, height: number) {
  Object.defineProperty(window, 'innerWidth', { configurable: true, writable: true, value: width });
  Object.defineProperty(window, 'innerHeight', { configurable: true, writable: true, value: height });
}

async function loadManager(): Promise<WindowManagerModule> {
  vi.resetModules();
  return import('$lib/stores/windowManager');
}

describe('windowManager', () => {
  beforeEach(() => viewport(1280, 800));

  it('opens a window with geometry that fits the viewport', async () => {
    const { windowManager } = await loadManager();

    const id = windowManager.openWindow('dashboard', 'Dashboard', 'dashboard');
    const [win] = get(windowManager);

    expect(id).toBe('window-1');
    expect(win).toMatchObject({ appId: 'dashboard', title: 'Dashboard', icon: 'dashboard', isMaximized: false });
    expect(win.x).toBeGreaterThanOrEqual(0);
    expect(win.y).toBeGreaterThanOrEqual(0);
    expect(win.x + win.width).toBeLessThanOrEqual(1280);
    expect(win.y + win.height).toBeLessThanOrEqual(800);
  });

  it('cascades successive windows and gives each one a higher z-index', async () => {
    const { windowManager } = await loadManager();

    windowManager.openWindow('dashboard', 'Dashboard', 'dashboard');
    windowManager.openWindow('storage', 'Storage', 'storage');
    const [first, second] = get(windowManager);

    expect(second.x).toBeGreaterThan(first.x);
    expect(second.y).toBeGreaterThan(first.y);
    expect(second.zIndex).toBeGreaterThan(first.zIndex);
  });

  it('opens maximized below the 820px breakpoint', async () => {
    viewport(700, 900);
    const { windowManager } = await loadManager();

    windowManager.openWindow('dashboard', 'Dashboard', 'dashboard');
    expect(get(windowManager)[0].isMaximized).toBe(true);
  });

  it('raises a focused window above the others', async () => {
    const { windowManager } = await loadManager();

    const first = windowManager.openWindow('dashboard', 'Dashboard', 'dashboard');
    windowManager.openWindow('storage', 'Storage', 'storage');
    windowManager.focusWindow(first);

    const [firstWin, secondWin] = get(windowManager);
    expect(firstWin.zIndex).toBeGreaterThan(secondWin.zIndex);
  });

  it('closes a window by id and leaves the rest untouched', async () => {
    const { windowManager } = await loadManager();

    const first = windowManager.openWindow('dashboard', 'Dashboard', 'dashboard');
    windowManager.openWindow('storage', 'Storage', 'storage');
    windowManager.closeWindow(first);

    const remaining = get(windowManager);
    expect(remaining).toHaveLength(1);
    expect(remaining[0].appId).toBe('storage');
  });

  it('minimizes, restores and keeps the restore above the others', async () => {
    const { windowManager } = await loadManager();

    const first = windowManager.openWindow('dashboard', 'Dashboard', 'dashboard');
    windowManager.openWindow('storage', 'Storage', 'storage');
    windowManager.minimizeWindow(first);
    expect(get(windowManager)[0].isMinimized).toBe(true);

    windowManager.restoreWindow(first);
    const [firstWin, secondWin] = get(windowManager);
    expect(firstWin.isMinimized).toBe(false);
    expect(firstWin.zIndex).toBeGreaterThan(secondWin.zIndex);
  });

  it('round-trips maximize back to the saved geometry', async () => {
    const { windowManager } = await loadManager();

    const id = windowManager.openWindow('dashboard', 'Dashboard', 'dashboard');
    const before = get(windowManager)[0];

    windowManager.maximizeWindow(id);
    expect(get(windowManager)[0].isMaximized).toBe(true);
    expect(get(windowManager)[0].savedWidth).toBe(before.width);

    windowManager.maximizeWindow(id);
    const after = get(windowManager)[0];
    expect(after.isMaximized).toBe(false);
    expect(after.width).toBe(before.width);
    expect(after.height).toBe(before.height);
  });

  it('reflows windows inside a smaller viewport', async () => {
    const { windowManager } = await loadManager();
    windowManager.openWindow('dashboard', 'Dashboard', 'dashboard');

    windowManager.reflow(1000, 700);
    const win = get(windowManager)[0];
    expect(win.x + win.width).toBeLessThanOrEqual(1000);
    expect(win.y + win.height).toBeLessThanOrEqual(700);
  });

  it('marks a user resize so content auto-fit stops fighting it', async () => {
    const { windowManager } = await loadManager();
    const id = windowManager.openWindow('dashboard', 'Dashboard', 'dashboard');

    windowManager.updateWindowSize(id, 500, 400);
    windowManager.markUserResized(id);

    expect(get(windowManager)[0]).toMatchObject({ width: 500, height: 400, userResized: true });
  });
});
