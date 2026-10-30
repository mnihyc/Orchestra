/** One display drawn by noVNC, a viewer that exists and is used as it is, on a socket this view opens itself with its ticket and subprotocol and hands over.
 *
 * Until "Work in it" the viewer is told to send nothing a person does (the server drops it in any case;
 * the view does not pretend), and its socket is one that only watches. A socket that ends with a reason
 * in its close frame says why; a line that is lost is taken up again with a new ticket and a new viewer,
 * after a pause that grows. The picture's quality follows updates that fall behind (`viewer.ts`). The
 * viewer's code is fetched only when the view is first drawn, so no other screen carries it.
 */
import { type Ref, useEffect, useImperativeHandle, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { displayTicket } from '@/api/displays';

import { adapt, BEHIND_AFTER_MS, CLOSE_WITH_REASON, displayAddress, DISPLAY_SUBPROTOCOL, type DisplayEnd, endOf, FIRST_QUALITY, QUALITY_BEST } from './viewer';

/** What the view uses of the viewer: the shape of noVNC's `RFB`. */
export interface RfbLike extends EventTarget {
  viewOnly: boolean;
  focusOnClick: boolean;
  scaleViewport: boolean;
  resizeSession: boolean;
  qualityLevel: number;
  disconnect: () => void;
  sendCtrlAltDel: () => void;
  focus: (options?: FocusOptions) => void;
  blur: () => void;
  clipboardPasteFrom: (text: string) => void;
}

export type RfbConstructor = new (target: HTMLElement, channel: WebSocket, options?: { shared?: boolean }) => RfbLike;

/** Fetches the viewer, once, when a display is first drawn. */
export async function loadNoVnc(): Promise<RfbConstructor> {
  const loaded = await import('@novnc/novnc');
  return loaded.default;
}

/** The pauses before a lost line is tried again. */
const RECONNECT_PAUSES_MS = [500, 1_000, 2_000, 4_000, 8_000] as const;
/** How often the view looks whether updates fall behind. */
const LOOK_EVERY_MS = 1_000;
/** How long the session is asked to take the panel's size for, before the panel's later changes are its own again. */
const FIT_FOR_MS = 1_000;

export interface ViewerState {
  phase: 'loading' | 'connecting' | 'open' | 'lost' | 'ended' | 'failed';
  ended: DisplayEnd | 'unknown' | null;
  quality: number;
}

export interface ViewerControls {
  paste: (text: string) => void;
  ctrlAltDel: () => void;
  fit: () => void;
}

export interface ViewerProps {
  displayId: string;
  control: boolean;
  label: string;
  onState: (state: ViewerState) => void;
  /** Ctrl+Alt+Esc while working in it: the keys go back to Orch. */
  onGiveBack: () => void;
  controls?: Ref<ViewerControls>;
  load?: () => Promise<RfbConstructor>;
  ticket?: (displayId: string, control: boolean) => Promise<string>;
  connect?: (url: string, protocol: string) => WebSocket;
}

/** A ticket for the display's socket, from the API. */
async function serverTicket(displayId: string, control: boolean): Promise<string> {
  return (await displayTicket(displayId, control)).ticket;
}

/** The browser's own socket. */
function browserSocket(url: string, protocol: string): WebSocket {
  return new WebSocket(url, protocol);
}

export function Viewer({ displayId, control, label, onState, onGiveBack, controls, load = loadNoVnc, ticket = serverTicket, connect = browserSocket }: ViewerProps) {
  const { t } = useTranslation();
  const target = useRef<HTMLDivElement>(null);
  const viewer = useRef<RfbLike | null>(null);
  const [attempt, setAttempt] = useState(0);
  // The view's latest listener, read by the connection whenever it has news: kept current after each render.
  const report = useRef(onState);
  useEffect(() => {
    report.current = onState;
  });

  useImperativeHandle(controls, () => ({
    paste: (text) => viewer.current?.clipboardPasteFrom(text),
    ctrlAltDel: () => viewer.current?.sendCtrlAltDel(),
    fit: () => {
      const current = viewer.current;
      if (current === null) return;
      current.resizeSession = true;
      setTimeout(() => {
        if (viewer.current === current) current.resizeSession = false;
      }, FIT_FOR_MS);
    },
  }), []);

  useEffect(() => {
    let alive = true;
    // Effect cleanup may run while the viewer or its ticket is still loading.
    const isAlive = (): boolean => alive;
    let socket: WebSocket | null = null;
    let rfb: RfbLike | null = null;
    let looking: ReturnType<typeof setInterval> | undefined;
    let retry: ReturnType<typeof setTimeout> | undefined;
    let quality = FIRST_QUALITY;
    let latest = 0;
    let lastLook = performance.now();
    report.current({ phase: attempt === 0 ? 'loading' : 'lost', ended: null, quality: QUALITY_BEST });
    void (async () => {
      let Rfb: RfbConstructor;
      let value: string;
      try {
        [Rfb, value] = await Promise.all([load(), ticket(displayId, control)]);
      } catch {
        if (!isAlive()) return;
        report.current({ phase: attempt === 0 ? 'failed' : 'lost', ended: null, quality: QUALITY_BEST });
        if (attempt > 0) retry = setTimeout(() => setAttempt((count) => count + 1), RECONNECT_PAUSES_MS[Math.min(attempt, RECONNECT_PAUSES_MS.length - 1)] ?? 8_000);
        return;
      }
      if (!isAlive() || target.current === null) return;
      report.current({ phase: 'connecting', ended: null, quality: quality.level });
      const made = connect(displayAddress(displayId, value), DISPLAY_SUBPROTOCOL);
      made.binaryType = 'arraybuffer';
      socket = made;
      made.addEventListener('message', (event) => {
        latest = Math.max(latest, performance.now() - event.timeStamp);
      });
      made.addEventListener('close', (event) => {
        if (!alive) return;
        clearInterval(looking);
        const end = endOf(event.code, event.reason);
        if (end !== null || event.code === CLOSE_WITH_REASON) {
          report.current({ phase: 'ended', ended: end, quality: quality.level });
          return;
        }
        report.current({ phase: 'lost', ended: null, quality: quality.level });
        retry = setTimeout(() => setAttempt((count) => count + 1), RECONNECT_PAUSES_MS[Math.min(attempt, RECONNECT_PAUSES_MS.length - 1)] ?? 8_000);
      });
      const drawing = new Rfb(target.current, made, { shared: true });
      drawing.viewOnly = !control;
      drawing.focusOnClick = control;
      drawing.scaleViewport = true;
      drawing.qualityLevel = quality.level;
      drawing.addEventListener('connect', () => {
        if (alive) report.current({ phase: 'open', ended: null, quality: quality.level });
      });
      rfb = drawing;
      viewer.current = drawing;
      if (control) drawing.focus({ preventScroll: true });
      looking = setInterval(() => {
        const now = performance.now();
        const late = now - lastLook - LOOK_EVERY_MS;
        lastLook = now;
        const behind = latest > BEHIND_AFTER_MS || late > BEHIND_AFTER_MS;
        latest = 0;
        const next = adapt(quality, behind);
        if (next.level !== quality.level) {
          drawing.qualityLevel = next.level;
          report.current({ phase: 'open', ended: null, quality: next.level });
        }
        quality = next;
      }, LOOK_EVERY_MS);
    })();
    return () => {
      alive = false;
      clearInterval(looking);
      clearTimeout(retry);
      if (viewer.current === rfb) viewer.current = null;
      try {
        rfb?.disconnect();
      } catch {
        // A viewer that never connected has nothing to end.
      }
      if (socket !== null && socket.readyState <= 1) socket.close(1000);
    };
  }, [displayId, control, attempt, load, ticket, connect]);

  return (
    <div
      ref={target}
      role="application"
      aria-label={label}
      aria-roledescription={t('vnc.view.title')}
      className="relative min-h-0 flex-1 overflow-hidden bg-bg-muted"
      data-vnc-viewer={control ? 'working' : 'watching'}
      onKeyDownCapture={(event) => {
        if (control && event.ctrlKey && event.altKey && event.key === 'Escape') {
          event.preventDefault();
          event.stopPropagation();
          viewer.current?.blur();
          onGiveBack();
        }
      }}
    />
  );
}
