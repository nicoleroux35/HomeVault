/* =============================================================================
 * File          : snapUtils.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Window/widget drag helpers: clamp a dragged rectangle to the true
 *   viewport edges and snap it when it comes near an edge. The desktop-chrome
 *   keep-out geometry (menubar, dock) is mirrored here as constants because
 *   it only matters at initial placement; a lockstep E2E test fails if the
 *   CSS tokens and this mirror drift apart.
 *
 * Dependencies:
 *   - none
 *
 * Usage:
 *   `applySnap(x, y, w, h)` from the drag handlers in Desktop/WM code.
 * =============================================================================
 */

export interface SnapConfig {
  topbarHeight: number
  dockBottom: number
  dockHeight: number
  snapThreshold: number
  edgeMargin: number
}

/* Hand-maintained MIRROR of the chrome keep-out geometry, which lives in CSS
   as `--hv-chrome-topbar-keepout` / `--hv-chrome-dock-keepout` (tokens.css,
   DESKTOP CHROME). It is mirrored rather than read because the keep-out is
   only meaningful at INITIAL placement time, before any layout is available —
   and because the dock auto-hides above 900px, so this is the <=900px case.
   Every value below has to be re-derived whenever those tokens move; the
   desktop E2E suite has a lockstep test that fails if they drift apart.
   The menubar is a floating pill at top:8px with height:62px (see
   .hv-menubar in Desktop.svelte) — its bottom edge sits at y=70, so window and
   monitor-widget content must start below that. The dock is an 84px pill at
   bottom:16px — the two pills are deliberately different heights. */
export const DEFAULT_SNAP_CONFIG: SnapConfig = {
  topbarHeight: 70,
  dockBottom: 16,
  dockHeight: 84,
  snapThreshold: 20,
  edgeMargin: 8,
}

export interface SnapResult {
  x: number
  y: number
  snapped: boolean
  snapEdge: 'top' | 'bottom' | 'left' | 'right' | 'none'
}

/**
 * Clamp a dragged window / widget position to the viewport and snap it when a
 * side comes within `snapThreshold` of a screen edge.
 *
 * The clamp is to the *true* screen edges (0 / vw-width / vh-height) so a
 * window or widget can be parked flush into any corner or along any side —
 * the floating menubar pill and the dock are only kept clear on *initial*
 * placement (`computeInitialGeometry` in windowManager.ts), never here.
 */
export function applySnap(
  x: number,
  y: number,
  width: number,
  height: number,
  config: SnapConfig = DEFAULT_SNAP_CONFIG,
): SnapResult {
  const vw = window.innerWidth
  const vh = window.innerHeight
  const t = config.snapThreshold

  // A window can be wider/taller than the viewport (small screens); keep the
  // max bound at 0 in that case so it stays pinned to the top-left.
  const maxX = Math.max(0, vw - width)
  const maxY = Math.max(0, vh - height)

  let newX = Math.max(0, Math.min(x, maxX))
  let newY = Math.max(0, Math.min(y, maxY))

  let snapEdge: SnapResult['snapEdge'] = 'none'

  if (newY <= t) {
    newY = 0
    snapEdge = 'top'
  } else if (maxY - newY <= t) {
    newY = maxY
    snapEdge = 'bottom'
  }

  if (newX <= t) {
    newX = 0
    if (snapEdge === 'none') snapEdge = 'left'
  } else if (maxX - newX <= t) {
    newX = maxX
    if (snapEdge === 'none') snapEdge = 'right'
  }

  return { x: newX, y: newY, snapped: snapEdge !== 'none', snapEdge }
}
