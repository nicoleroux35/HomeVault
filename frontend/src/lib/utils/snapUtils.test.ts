/**
 * snapUtils — window/widget drag clamping and edge snapping.
 *
 * The clamp is to the true viewport (a window may be parked flush in a
 * corner); only the initial placement keeps clear of the menubar/dock.
 */
import { beforeEach, describe, expect, it } from 'vitest';
import { applySnap, DEFAULT_SNAP_CONFIG } from '$lib/utils/snapUtils';

function viewport(width: number, height: number) {
  Object.defineProperty(window, 'innerWidth', { configurable: true, writable: true, value: width });
  Object.defineProperty(window, 'innerHeight', { configurable: true, writable: true, value: height });
}

describe('applySnap', () => {
  beforeEach(() => viewport(1280, 800));

  it('keeps a freely positioned window where it is', () => {
    expect(applySnap(200, 200, 400, 300)).toEqual({
      x: 200,
      y: 200,
      snapped: false,
      snapEdge: 'none',
    });
  });

  it('clamps to the true viewport edges', () => {
    expect(applySnap(-50, -50, 400, 300)).toMatchObject({ x: 0, y: 0, snapped: true, snapEdge: 'top' });
    expect(applySnap(2000, 2000, 400, 300)).toMatchObject({ x: 880, y: 500, snapped: true, snapEdge: 'bottom' });
  });

  it('snaps within the threshold and reports the dominant edge', () => {
    const t = DEFAULT_SNAP_CONFIG.snapThreshold;
    expect(applySnap(5, 300, 400, 300)).toMatchObject({ x: 0, y: 300, snapped: true, snapEdge: 'left' });
    expect(applySnap(t, 300, 400, 300)).toMatchObject({ x: 0, snapEdge: 'left' });
    // One pixel past the threshold is already free movement.
    expect(applySnap(t + 1, 300, 400, 300).snapEdge).toBe('none');
    expect(applySnap(1280 - 400 - 5, 300, 400, 300)).toMatchObject({
      x: 880,
      snapEdge: 'right',
      snapped: true,
    });
  });

  it('pins an oversized window to the top-left instead of a negative max', () => {
    viewport(500, 400);
    expect(applySnap(30, 120, 900, 700)).toMatchObject({ x: 0, y: 0, snapped: true, snapEdge: 'top' });
  });

  it('honours a caller-supplied threshold and margin', () => {
    expect(
      applySnap(10, 300, 400, 300, { ...DEFAULT_SNAP_CONFIG, snapThreshold: 5 }).snapEdge,
    ).toBe('none');
  });
});
