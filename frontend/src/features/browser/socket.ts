/** The socket of one browser: the picture of the tab watched, its tabs and who is there, what a page asks, and for a person who drives, what they do in it.
 *
 * One socket shows one tab at a time, which its person chooses, and carries the newest picture only:
 * the server sends the next when the one before was acknowledged, so a slow screen sees fewer pictures
 * and never an old one. A picture is a JPEG with a header of twelve bytes (its number, the page's
 * viewport, the image's size), drawn by whoever owns the connection, which then says it was drawn. A
 * ticket to watch carries nothing to the page; driving is a socket of its own, on a ticket that asks for
 * it. A line that is lost is taken up again with a new ticket, after a pause that grows.
 */
import { createStore, type StoreApi } from 'zustand/vanilla';

import { browserTicket } from '@/api/browsers';
import type { UserRef } from '@/generated/api-types';

/** The one subprotocol a browser's socket speaks. */
export const BROWSER_SUBPROTOCOL = 'orch.browser.v1';

export interface BrowserTab {
  id: string;
  title: string;
  url: string;
  loading: boolean;
  active: boolean;
  opener: string | null;
}

export type BrowserCloseReason = 'quit' | 'closed_by_agent' | 'idle' | 'exited' | 'environment_stopped' | 'link_lost' | 'switched_off' | 'revoked';

export interface PageDialog {
  tab: string;
  kind: 'alert' | 'confirm' | 'prompt' | 'beforeunload';
  message: string;
  default: string | null;
}

/** What the server sends as text. */
export type BrowserServerFrame =
  | { op: 'ready'; control: boolean; tabs: BrowserTab[]; viewport: { width: number; height: number }; people: { watching: UserRef[]; driving: UserRef[] } }
  | { op: 'tabs'; tabs: BrowserTab[] }
  | { op: 'people'; watching: UserRef[]; driving: UserRef[] }
  | ({ op: 'dialog' } & PageDialog)
  | { op: 'dialog_closed'; tab: string }
  | { op: 'chooser'; tab: string; multiple: boolean }
  | { op: 'quality'; level: 0 | 1 | 2 | 3 | 4; why: 'link' | 'clear' }
  | { op: 'copied'; text: string }
  | { op: 'error'; code: 'view_only' | 'no_such_tab' | 'not_allowed' | 'invalid'; detail: string }
  | { op: 'closed'; reason: BrowserCloseReason }
  | { op: 'pong'; n: number };

/** What a person who drives sends. */
export type BrowserControlFrame =
  | { op: 'pointer'; tab: string; kind: 'move' | 'down' | 'up' | 'wheel'; x: number; y: number; button?: 'left' | 'middle' | 'right'; buttons?: number; clicks?: number; dx?: number; dy?: number; mod?: number }
  | { op: 'key'; tab: string; kind: 'down' | 'up'; key: string; code: string; text?: string; mod?: number; repeat?: boolean; location?: number }
  | { op: 'text'; tab: string; text: string }
  | { op: 'navigate'; tab: string; url: string }
  | { op: 'history'; tab: string; to: 'back' | 'forward' | 'reload' | 'stop' }
  | { op: 'tab'; do: 'new' | 'close' | 'activate'; tab?: string; url?: string }
  | { op: 'dialog'; tab: string; accept: boolean; text?: string }
  | { op: 'files'; tab: string; paths: string[] | null }
  | { op: 'viewport'; width: number; height: number }
  | { op: 'copy'; tab: string };

/** One picture as it came: its number, the page's size in CSS pixels, the image's size, and the JPEG. */
export interface BrowserPicture {
  n: number;
  viewport: { width: number; height: number };
  image: { width: number; height: number };
  jpeg: Uint8Array<ArrayBuffer>;
}

/** The bytes of the header before the JPEG: a number of four bytes and four sizes of two, big-endian. */
export const PICTURE_HEADER_BYTES = 12;

/** Reads a binary frame; `null` for one too short to be a picture. */
export function readPicture(data: ArrayBuffer): BrowserPicture | null {
  if (data.byteLength <= PICTURE_HEADER_BYTES) return null;
  const view = new DataView(data);
  return {
    n: view.getUint32(0),
    viewport: { width: view.getUint16(4), height: view.getUint16(6) },
    image: { width: view.getUint16(8), height: view.getUint16(10) },
    jpeg: new Uint8Array(data, PICTURE_HEADER_BYTES),
  };
}

/** The modifier bits the browser's protocol takes: Alt 1, Ctrl 2, Meta 4, Shift 8. */
export function modifiers(event: { altKey: boolean; ctrlKey: boolean; metaKey: boolean; shiftKey: boolean }): number {
  return (event.altKey ? 1 : 0) | (event.ctrlKey ? 2 : 0) | (event.metaKey ? 4 : 0) | (event.shiftKey ? 8 : 0);
}

export type BrowserPhase = 'connecting' | 'open' | 'lost' | 'closed';

export interface BrowserSocketState {
  phase: BrowserPhase;
  /** Whether this socket was given to drive. */
  control: boolean;
  reason: BrowserCloseReason | null;
  tabs: BrowserTab[];
  /** The tab this socket shows. */
  tab: string | null;
  viewport: { width: number; height: number } | null;
  watching: UserRef[];
  driving: UserRef[];
  dialog: PageDialog | null;
  chooser: { tab: string; multiple: boolean } | null;
  /** The picture's level, 0 the best; `null` before the server said one. */
  quality: number | null;
  /** The last thing refused, until the next thing is done. */
  refused: { code: string; detail: string } | null;
}

/** The parts of a WebSocket this uses, so that a test can stand in for one. */
export interface BrowserSocketLike {
  readyState: number;
  binaryType: string;
  onopen: (() => void) | null;
  onmessage: ((event: { data: unknown }) => void) | null;
  onclose: ((event: { code: number; reason?: string }) => void) | null;
  onerror: (() => void) | null;
  send: (data: string) => void;
  close: (code?: number) => void;
}

const OPEN = 1;
/** The pauses before a lost line is tried again: soon at first, since a person is looking at it. */
export const RECONNECT_PAUSES_MS = [500, 1_000, 2_000, 4_000, 8_000] as const;
/** The close codes after which another ticket cannot help: not authorised, or nothing to show. */
const FINAL_CODES: ReadonlySet<number> = new Set([4401, 4403, 4404]);

export interface BrowserConnectionOptions {
  browserId: string;
  control: boolean;
  /** A picture arrived; the owner draws it and then calls `drawn`. */
  onPicture: (picture: BrowserPicture) => void;
  /** What `copy` answered: what is selected in the tab. */
  onCopied?: (text: string) => void;
  connect?: (url: string, protocol: string) => BrowserSocketLike;
  address?: (browserId: string, ticket: string) => string;
  ticket?: (browserId: string, control: boolean) => Promise<string>;
}

export interface BrowserConnection {
  state: StoreApi<BrowserSocketState>;
  /** Says which tab this socket shows and how large its panel is. */
  watch: (tab: string, width: number, height: number) => void;
  /** Says a picture was drawn, so that the next may come. */
  drawn: (n: number) => void;
  /** Sends what a person does; dropped by the server on a socket that only watches. */
  send: (frame: BrowserControlFrame) => void;
  /** Ends the socket for good. */
  close: () => void;
}

function defaultAddress(browserId: string, ticket: string): string {
  const scheme = location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${scheme}//${location.host}/api/v1/ws/browsers/${encodeURIComponent(browserId)}?ticket=${encodeURIComponent(ticket)}`;
}

function isTabs(value: unknown): value is BrowserTab[] {
  return Array.isArray(value);
}

/** Connects to a browser and keeps the connection through a line that comes and goes, until it ends. */
export function connectBrowser(options: BrowserConnectionOptions): BrowserConnection {
  const connect = options.connect ?? ((url, protocol) => new WebSocket(url, protocol) as unknown as BrowserSocketLike);
  const address = options.address ?? defaultAddress;
  const askTicket = options.ticket ?? (async (id: string, control: boolean) => (await browserTicket(id, control)).ticket);
  const state = createStore<BrowserSocketState>(() => ({ phase: 'connecting', control: options.control, reason: null, tabs: [], tab: null, viewport: null, watching: [], driving: [], dialog: null, chooser: null, quality: null, refused: null }));
  let socket: BrowserSocketLike | null = null;
  let wanted = true;
  // A caller may close this connection while its ticket request is awaiting an answer.
  const isWanted = (): boolean => wanted;
  let attempts = 0;
  let retry: ReturnType<typeof setTimeout> | undefined;
  let watching: { tab: string; width: number; height: number } | null = null;

  const sendText = (frame: object) => {
    if (socket?.readyState === OPEN) socket.send(JSON.stringify(frame));
  };
  const sendWatch = () => {
    if (watching !== null) sendText({ op: 'watch', tab: watching.tab, width: Math.round(watching.width), height: Math.round(watching.height), dpr: globalThis.devicePixelRatio || 1 });
  };
  const chooseTab = (tabs: BrowserTab[]) => {
    const current = state.getState().tab;
    if (current !== null && tabs.some((tab) => tab.id === current)) return current;
    return tabs.find((tab) => tab.active)?.id ?? tabs[0]?.id ?? null;
  };

  const lose = () => {
    socket = null;
    if (!wanted || state.getState().phase === 'closed') return;
    state.setState({ phase: 'lost' });
    const pause = RECONNECT_PAUSES_MS[Math.min(attempts, RECONNECT_PAUSES_MS.length - 1)] ?? 8_000;
    attempts += 1;
    clearTimeout(retry);
    retry = setTimeout(() => void open(), pause);
  };

  const receive = (frame: BrowserServerFrame) => {
    switch (frame.op) {
      case 'ready': {
        const tabs = isTabs(frame.tabs) ? frame.tabs : [];
        state.setState({ phase: 'open', control: frame.control, reason: null, tabs, tab: chooseTab(tabs), viewport: frame.viewport, watching: frame.people.watching, driving: frame.people.driving });
        const tab = state.getState().tab;
        if (tab !== null && watching !== null) watching = { ...watching, tab };
        sendWatch();
        return;
      }
      case 'tabs': {
        const tab = chooseTab(frame.tabs);
        const moved = tab !== state.getState().tab;
        state.setState({ tabs: frame.tabs, tab });
        if (moved && tab !== null && watching !== null) {
          watching = { ...watching, tab };
          sendWatch();
        }
        return;
      }
      case 'people':
        state.setState({ watching: frame.watching, driving: frame.driving });
        return;
      case 'dialog':
        state.setState({ dialog: { tab: frame.tab, kind: frame.kind, message: frame.message, default: frame.default } });
        return;
      case 'dialog_closed':
        if (state.getState().dialog?.tab === frame.tab) state.setState({ dialog: null });
        return;
      case 'chooser':
        state.setState({ chooser: { tab: frame.tab, multiple: frame.multiple } });
        return;
      case 'quality':
        state.setState({ quality: frame.level });
        return;
      case 'copied':
        options.onCopied?.(frame.text);
        return;
      case 'error':
        state.setState({ refused: { code: frame.code, detail: frame.detail } });
        return;
      case 'closed':
        wanted = false;
        state.setState({ phase: 'closed', reason: frame.reason });
        return;
      case 'pong':
        return;
    }
  };

  const open = async () => {
    if (!isWanted()) return;
    state.setState({ phase: state.getState().phase === 'lost' ? 'lost' : 'connecting' });
    let ticket: string;
    try {
      ticket = await askTicket(options.browserId, options.control);
    } catch {
      lose();
      return;
    }
    if (!isWanted()) return;
    const made = connect(address(options.browserId, ticket), BROWSER_SUBPROTOCOL);
    made.binaryType = 'arraybuffer';
    socket = made;
    made.onopen = () => {
      attempts = 0;
    };
    made.onmessage = (event) => {
      if (socket !== made) return;
      if (typeof event.data === 'string') {
        let frame: unknown;
        try {
          frame = JSON.parse(event.data);
        } catch {
          return;
        }
        if (typeof frame === 'object' && frame !== null && 'op' in frame) receive(frame as BrowserServerFrame);
        return;
      }
      if (event.data instanceof ArrayBuffer) {
        const picture = readPicture(event.data);
        if (picture === null) return;
        // The window's size travels with each picture: somebody who drives may have changed it.
        const known = state.getState().viewport;
        if (known?.width !== picture.viewport.width || known.height !== picture.viewport.height) state.setState({ viewport: picture.viewport });
        options.onPicture(picture);
      }
    };
    made.onclose = (event) => {
      if (socket !== made) return;
      if (FINAL_CODES.has(event.code)) {
        wanted = false;
        state.setState({ phase: 'closed', reason: state.getState().reason ?? 'revoked' });
        socket = null;
        return;
      }
      lose();
    };
    made.onerror = () => undefined;
  };

  void open();

  return {
    state,
    watch: (tab, width, height) => {
      watching = { tab, width, height };
      if (state.getState().tab !== tab) state.setState({ tab });
      sendWatch();
    },
    drawn: (n) => sendText({ op: 'ack', n }),
    send: (frame) => {
      if (state.getState().refused !== null) state.setState({ refused: null });
      if (frame.op === 'dialog' && state.getState().dialog?.tab === frame.tab) state.setState({ dialog: null });
      if (frame.op === 'files' && state.getState().chooser?.tab === frame.tab) state.setState({ chooser: null });
      sendText(frame);
    },
    close: () => {
      wanted = false;
      clearTimeout(retry);
      const held = socket;
      socket = null;
      held?.close(1000);
    },
  };
}
