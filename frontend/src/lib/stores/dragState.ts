import { writable } from 'svelte/store';

/**
 * Tracks an ongoing drag-and-drop operation from the app launcher.
 * Allows the desktop area to react while the user is dragging an app icon.
 */
export interface DragState {
  isDragging: boolean;
  appId: string | null;
  appName: string | null;
}

export const dragState = writable<DragState>({
  isDragging: false,
  appId: null,
  appName: null,
});

/** Begin dragging an app – the desktop can now listen for a drop. */
export function startDrag(appId: string, appName: string) {
  dragState.set({ isDragging: true, appId, appName });
}

/** End the drag (whether dropped or cancelled). */
export function endDrag() {
  dragState.set({ isDragging: false, appId: null, appName: null });
}
