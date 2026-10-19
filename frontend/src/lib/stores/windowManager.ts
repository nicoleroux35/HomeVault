/* =============================================================================
 * File          : windowManager.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Desktop window manager: open/close/focus/minimize/maximize, z-order and
 *   per-app accent colour for the windowing shell. Initial geometry
 *   cascades clear of the desktop chrome and clamps to the viewport; under
 *   the 820 px breakpoint new windows open maximized (a floating window
 *   could not fit a phone screen). A user resize sets userResized, which
 *   permanently disables the content-fit auto-grow for that window.
 *
 * Dependencies:
 *   - svelte (>= 5)
 *
 * Usage:
 *   windowManager from Desktop.svelte / WindowFrame.svelte / the launcher.
 * =============================================================================
 */

import { writable } from 'svelte/store';
import { applySnap, DEFAULT_SNAP_CONFIG } from '$lib/utils/snapUtils';

export interface Window {
  id: string;
  appId: string;
  title: string;
  icon: string;
  /** The app's identity colour (the shared `--hv-app-accent-grad`).
   *  WindowFrame publishes it as the `hvAppAccent` context so WindowHeader —
   *  and any app chrome — tints its icon tile to match the dock / Launchpad. */
  color?: string;
  component?: any;
  componentLoader?: () => Promise<any>;
  x: number;
  y: number;
  width: number;
  height: number;
  isMaximized: boolean;
  isMinimized: boolean;
  zIndex: number;
  // Saved dimensions for restore after un-maximize
  savedX?: number;
  savedY?: number;
  savedWidth?: number;
  savedHeight?: number;
  // Set once the user manually drags a resize handle. WindowFrame's
  // content-fit auto-grow (see checkContentOverflow there) stops touching
  // this window's size for good once this is true, so a deliberate resize
  // is never fought/overridden by a later auto-fit pass.
  userResized?: boolean;
}

// Below this viewport width, a floating 1024x720-ish window can never fit
// (it's most of a phone/small-tablet screen either way), so new windows
// open already maximized instead of landing mostly off-screen with their
// title-bar controls unreachable. Matches WindowFrame.svelte's MIN_WIDTH
// (400) plus room to actually see content around it.
const AUTO_MAXIMIZE_BREAKPOINT = 820;

// A floor well below WindowFrame.svelte's own MIN_WIDTH/MIN_HEIGHT (400/300,
// which only bounds interactive manual resizing) — kept low deliberately so
// it never exceeds a real phone's viewport width and reintroduces the same
// off-screen overflow this geometry helper exists to prevent.
const MIN_RESTORED_WIDTH = 280;
const MIN_RESTORED_HEIGHT = 200;

/** Computes an initial position/size that fits the current viewport, plus
 * whether the window should just open maximized on small screens. Falls
 * back to desktop-sized defaults when `window` isn't available. */
function computeInitialGeometry(index: number) {
  const vw = typeof window !== 'undefined' ? window.innerWidth : 1280;
  const vh = typeof window !== 'undefined' ? window.innerHeight : 800;
  const margin = DEFAULT_SNAP_CONFIG.edgeMargin;

  // The cascade start clears the floating menubar (see
  // DEFAULT_SNAP_CONFIG.topbarHeight) by 52px, and is derived from the token so
  // a taller pill can never swallow the first window's header (the literal 122
  // this replaced was pinned to a 70px keep-out).
  const cascadeGap = 52;
  const cascadeY = DEFAULT_SNAP_CONFIG.topbarHeight + cascadeGap + (index % 5) * 30;

  const width = Math.max(MIN_RESTORED_WIDTH, Math.min(1024, vw - margin * 2));
  // The height budget starts at THIS window's cascade Y, not at the viewport
  // top: a window is placed at `cascadeY`, so counting the whole band above it
  // would let a tall window overflow the bottom and open BEHIND the dock (it
  // used to overlap it by ~50px at a 900px viewport).
  const availableHeight =
    vh - cascadeY - DEFAULT_SNAP_CONFIG.dockBottom - DEFAULT_SNAP_CONFIG.dockHeight - margin;
  const height = Math.max(MIN_RESTORED_HEIGHT, Math.min(720, availableHeight));

  const { x, y } = applySnap(100 + (index % 5) * 30, cascadeY, width, height);

  return { x, y, width, height, isMaximized: vw <= AUTO_MAXIMIZE_BREAKPOINT };
}

function createWindowManager() {
  const windows = writable<Window[]>([]);
  let nextZIndex = 100;
  let windowCounter = 0;

  return {
    subscribe: windows.subscribe,

    openWindow(appId: string, title: string, icon: string, componentLoader?: any, color?: string) {
      windowCounter++;
      const geometry = computeInitialGeometry(windowCounter);
      const newWindow: Window = {
        id: `window-${windowCounter}`,
        appId,
        title,
        icon,
        color,
        componentLoader: typeof componentLoader === 'function' ? componentLoader : undefined,
        component: typeof componentLoader === 'function' ? undefined : componentLoader,
        ...geometry,
        isMinimized: false,
        zIndex: nextZIndex++,
      };

      windows.update(wins => [...wins, newWindow]);
      return newWindow.id;
    },

    closeWindow(id: string) {
      windows.update(wins => wins.filter(w => w.id !== id));
    },

    focusWindow(id: string) {
      windows.update(wins => {
        return wins.map(w => ({
          ...w,
          zIndex: w.id === id ? nextZIndex++ : w.zIndex
        }));
      });
    },

    minimizeWindow(id: string) {
      windows.update(wins => {
        return wins.map(w =>
          w.id === id ? { ...w, isMinimized: true } : w
        );
      });
    },

    restoreWindow(id: string) {
      windows.update(wins => {
        return wins.map(w =>
          w.id === id ? { ...w, isMinimized: false, zIndex: nextZIndex++ } : w
        );
      });
    },

    maximizeWindow(id: string) {
      windows.update(wins => {
        return wins.map(w => {
          if (w.id === id) {
            if (w.isMaximized) {
              // Restore to previous size. A window that opened already
              // maximized (small viewport, see computeInitialGeometry)
              // never went through the "save current size" branch below,
              // so savedX/savedWidth/etc. are still undefined here — fall
              // back to a fresh viewport-fit geometry instead of a
              // hardcoded desktop size that would overflow the same way.
              const fallback = w.savedWidth === undefined ? computeInitialGeometry(0) : null;
              return {
                ...w,
                isMaximized: false,
                // Toggling maximize is a direct interaction with this
                // window — always bring it to the front, don't rely on a
                // preceding mousedown having focused it (double-click,
                // programmatic calls, and future keyboard shortcuts skip
                // that).
                zIndex: nextZIndex++,
                x: w.savedX ?? fallback!.x,
                y: w.savedY ?? fallback!.y,
                width: w.savedWidth ?? fallback!.width,
                height: w.savedHeight ?? fallback!.height
              };
            } else {
              // Maximize - save current size first, and raise to the front.
              return {
                ...w,
                isMaximized: true,
                zIndex: nextZIndex++,
                savedX: w.x,
                savedY: w.y,
                savedWidth: w.width,
                savedHeight: w.height
              };
            }
          }
          return w;
        });
      });
    },

    updateWindowPosition(id: string, x: number, y: number) {
      windows.update(wins => {
        return wins.map(w =>
          w.id === id ? { ...w, x, y } : w
        );
      });
    },

    updateWindowSize(id: string, width: number, height: number) {
      windows.update(wins => {
        return wins.map(w =>
          w.id === id ? { ...w, width, height } : w
        );
      });
    },

    /** Marks a window as manually resized by the user, permanently opting
     * it out of WindowFrame's content-fit auto-grow. Called only from the
     * actual resize-handle drag, never from a drag-to-move or the auto-fit
     * itself. */
    markUserResized(id: string) {
      windows.update(wins => {
        return wins.map(w =>
          w.id === id ? { ...w, userResized: true } : w
        );
      });
    },
  };
}

export const windowManager = createWindowManager();
