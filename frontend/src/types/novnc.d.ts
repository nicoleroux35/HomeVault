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
