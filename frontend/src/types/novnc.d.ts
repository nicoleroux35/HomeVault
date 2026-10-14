/* =============================================================================
 * File          : novnc.d.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Minimal hand-written type declarations for the VNC client library
 *   (which ships no .d.ts): only the surface the VM screen uses.
 *
 * Dependencies:
 *   - none (ambient declarations)
 *
 * Usage:
 *   Picked up by TypeScript automatically for VmScreen.svelte.
 * =============================================================================
 */

// Minimal typings for @novnc/novnc 1.7 (ships no .d.ts). Only what VmScreen uses.
declare module '@novnc/novnc' {
  export interface RFBOptions {
    shared?: boolean;
    credentials?: { username?: string; password?: string; target?: string };
    wsProtocols?: string[];
  }
  export default class RFB extends EventTarget {
    constructor(target: Element, url: string | URL, options?: RFBOptions);
    scaleViewport: boolean;
    clipViewport: boolean;
    resizeSession: boolean;
    background: string;
    viewOnly: boolean;
    disconnect(): void;
    sendCtrlAltDel(): void;
    sendKey(keysym: number, code: string, down?: boolean): void;
    focus(): void;
    blur(): void;
  }
}
