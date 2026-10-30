/** The part of noVNC's viewer (`@novnc/novnc`, which ships no types of its own) that the VNC view uses, as its API document describes it. */
declare module '@novnc/novnc' {
  export interface RfbOptions {
    shared?: boolean;
    credentials?: { username?: string; password?: string; target?: string };
    repeaterID?: string;
    wsProtocols?: string[];
  }

  export default class RFB extends EventTarget {
    constructor(target: HTMLElement, urlOrChannel: string | WebSocket, options?: RfbOptions);
    viewOnly: boolean;
    focusOnClick: boolean;
    clipViewport: boolean;
    dragViewport: boolean;
    scaleViewport: boolean;
    resizeSession: boolean;
    background: string;
    qualityLevel: number;
    compressionLevel: number;
    readonly capabilities: { power: boolean };
    disconnect(): void;
    sendCtrlAltDel(): void;
    sendKey(keysym: number, code: string | null, down?: boolean): void;
    focus(options?: FocusOptions): void;
    blur(): void;
    clipboardPasteFrom(text: string): void;
  }
}
