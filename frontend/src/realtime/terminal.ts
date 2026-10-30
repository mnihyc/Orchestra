/** Terminal byte stream, the replay a shell is taken up with, and a shell that was taken to another screen.
 *
 * A shell's own socket, which carries raw bytes and not the rows of the feeds: typed bytes go up as binary
 * frames, what the shell prints comes down the same way, and a few small text frames say that the shell is ready
 * (with how many bytes of what it printed come first, so that a shell taken up again shows its screen), that
 * it exited, that it was closed and why, and answer a ping, from which the round trip is measured. Nothing
 * is drawn here: the screen that owns the connection draws what arrives, and sends what is typed.
 */
import { createStore, type StoreApi } from 'zustand/vanilla';

import { shellTicket } from '@/api/terminals';

/** The one subprotocol a shell's socket speaks. */
export const TERMINAL_SUBPROTOCOL = 'orch.terminal.v1';

/** Why a shell's socket ended, as the server says it. */
export type CloseReason = 'exited' | 'closed_by_user' | 'detached' | 'environment_stopped' | 'link_lost' | 'attached_elsewhere' | 'revoked';

export type ShellPhase =
  /** A ticket is being asked for, or the socket is opening. */
  | 'connecting'
  /** The shell is ready and its screen is coming or has come. */
  | 'attached'
  /** The line went; the screen stays, and a new attachment is made by itself. */
  | 'lost'
  /** Another screen took the shell: this one can take it back. */
  | 'elsewhere'
  /** The shell is over. */
  | 'closed';

export interface ShellState {
  phase: ShellPhase;
  /** How the shell ended, once `phase` is `closed`. */
  reason: CloseReason | null;
  /** The shell's exit code, where it exited. */
  exitCode: number | null;
  /** The middle one of the last few round trips, in milliseconds; `null` before one was measured. */
  roundTripMs: number | null;
}

/** What an owner of a connection is told besides the state. */
export interface ShellListener {
  /** The shell's screen is about to come: what is drawn is drawn anew from the replay. */
  onReady: (size: { cols: number; rows: number; replayBytes: number }) => void;
  /** Bytes the shell printed. */
  onData: (bytes: Uint8Array) => void;
}

/** The parts of a WebSocket this uses, so that a test can stand in for one. */
export interface ShellSocketLike {
  readyState: number;
  binaryType: string;
  onopen: (() => void) | null;
  onmessage: ((event: { data: unknown }) => void) | null;
  onclose: ((event: { code: number }) => void) | null;
  onerror: (() => void) | null;
  send: (data: string | Uint8Array) => void;
  close: (code?: number) => void;
}

const OPEN = 1;

/** How often the round trip is measured: often enough that a slow line shows within a few seconds. */
export const PING_EVERY_MS = 5_000;
/** How many round trips the figure is the middle one of, so that one late packet changes nothing. */
const ROUND_TRIPS_KEPT = 3;
/** The pauses before a lost line is tried again, in milliseconds: it is a person's shell, which they want back soon. */
export const RECONNECT_PAUSES_MS = [500, 1_000, 2_000, 4_000, 8_000] as const;
/** The close codes of the socket that mean the shell cannot be had with this ticket: not authorised, or malformed. */
const CLOSE_UNAUTHORISED = 4401;

export interface ShellConnectionOptions {
  terminalId: string;
  listener: ShellListener;
  /** Makes the socket; the browser's own when absent. */
  connect?: (url: string, protocol: string) => ShellSocketLike;
  /** The address of the socket for a shell and a ticket; this page's own origin when absent. */
  address?: (terminalId: string, ticket: string) => string;
  /** A ticket for attaching. The first is the one the opening gave; the next are asked for. */
  firstTicket?: string;
  tickets?: (terminalId: string) => Promise<string>;
}

function defaultAddress(terminalId: string, ticket: string): string {
  const scheme = location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${scheme}//${location.host}/api/v1/ws/terminals/${encodeURIComponent(terminalId)}?ticket=${encodeURIComponent(ticket)}`;
}

/** The middle of the last few figures. */
function middle(values: readonly number[]): number {
  const sorted = [...values].sort((one, other) => one - other);
  return sorted[Math.floor(sorted.length / 2)] ?? 0;
}

export interface ShellConnection {
  state: StoreApi<ShellState>;
  /** Types: bytes go to the shell as they are. */
  send: (bytes: Uint8Array) => void;
  /** The screen changed size. */
  resize: (cols: number, rows: number) => void;
  /** Takes the shell back from another screen. */
  attach: () => void;
  /** Lets the shell go: it runs on. */
  detach: () => void;
}

/** Attaches to a shell and keeps it attached through a line that comes and goes, until it ends or is taken. */
export function connectShell(options: ShellConnectionOptions): ShellConnection {
  const connect = options.connect ?? ((url, protocol) => new WebSocket(url, protocol) as unknown as ShellSocketLike);
  const address = options.address ?? defaultAddress;
  const askTicket = options.tickets ?? (async (id: string) => (await shellTicket(id)).ticket);
  const state = createStore<ShellState>(() => ({ phase: 'connecting', reason: null, exitCode: null, roundTripMs: null }));
  let socket: ShellSocketLike | null = null;
  let ticket: string | null = options.firstTicket ?? null;
  const life = { wanted: true };
  /** Read through a function, since it changes while a ticket is being asked for. */
  const wanted = () => life.wanted;
  let attempts = 0;
  let retry: ReturnType<typeof setTimeout> | undefined;
  let heartbeat: ReturnType<typeof setInterval> | undefined;
  let pingN = 0;
  const pings = new Map<number, number>();
  let trips: number[] = [];
  let cols = 80;
  let rows = 24;

  const stop = () => {
    clearInterval(heartbeat);
    pings.clear();
  };

  const finish = (reason: CloseReason, exitCode: number | null) => {
    life.wanted = false;
    stop();
    state.setState({ phase: reason === 'attached_elsewhere' ? 'elsewhere' : 'closed', reason, exitCode: exitCode ?? state.getState().exitCode });
  };

  const lose = () => {
    stop();
    socket = null;
    if (!wanted() || state.getState().phase === 'closed' || state.getState().phase === 'elsewhere') return;
    state.setState({ phase: 'lost' });
    const pause = RECONNECT_PAUSES_MS[Math.min(attempts, RECONNECT_PAUSES_MS.length - 1)] ?? 8_000;
    attempts += 1;
    clearTimeout(retry);
    retry = setTimeout(() => void open(), pause);
  };

  const open = async () => {
    if (!wanted()) return;
    try {
      const use = ticket ?? (await askTicket(options.terminalId));
      ticket = null;
      if (!wanted()) return;
      const made = connect(address(options.terminalId, use), TERMINAL_SUBPROTOCOL);
      made.binaryType = 'arraybuffer';
      socket = made;
      made.onopen = () => {
        attempts = 0;
        clearInterval(heartbeat);
        const ping = () => {
          if (made.readyState !== OPEN) return;
          pingN += 1;
          pings.set(pingN, performance.now());
          made.send(JSON.stringify({ op: 'ping', n: pingN }));
        };
        ping();
        heartbeat = setInterval(ping, PING_EVERY_MS);
      };
      made.onmessage = (event) => {
        if (socket !== made) return;
        if (typeof event.data === 'string') {
          const frame = JSON.parse(event.data) as { op: string; cols?: number; rows?: number; replay_bytes?: number; code?: number; reason?: CloseReason; n?: number };
          switch (frame.op) {
            case 'ready':
              state.setState({ phase: 'attached', reason: null });
              options.listener.onReady({ cols: frame.cols ?? cols, rows: frame.rows ?? rows, replayBytes: frame.replay_bytes ?? 0 });
              // The screen may have another size than the shell was made with.
              made.send(JSON.stringify({ op: 'resize', cols, rows }));
              return;
            case 'exit':
              state.setState({ exitCode: frame.code ?? null });
              return;
            case 'closed':
              finish(frame.reason ?? 'link_lost', null);
              return;
            case 'pong': {
              const sent = pings.get(frame.n ?? -1);
              pings.delete(frame.n ?? -1);
              if (sent === undefined) return;
              trips = [...trips, Math.round(performance.now() - sent)].slice(-ROUND_TRIPS_KEPT);
              state.setState({ roundTripMs: middle(trips) });
              return;
            }
            default:
          }
          return;
        }
        options.listener.onData(new Uint8Array(event.data as ArrayBuffer));
      };
      made.onclose = (event) => {
        if (socket !== made) return;
        // A ticket the server did not take is not tried again with the same: the next is asked for.
        if (event.code === CLOSE_UNAUTHORISED && attempts > RECONNECT_PAUSES_MS.length) {
          finish('revoked', null);
          return;
        }
        lose();
      };
      made.onerror = () => undefined;
    } catch {
      lose();
    }
  };

  void open();

  return {
    state,
    send: (bytes) => {
      if (socket?.readyState === OPEN) socket.send(bytes);
    },
    resize: (nextCols, nextRows) => {
      cols = nextCols;
      rows = nextRows;
      if (socket?.readyState === OPEN) socket.send(JSON.stringify({ op: 'resize', cols, rows }));
    },
    attach: () => {
      life.wanted = true;
      attempts = 0;
      clearTimeout(retry);
      state.setState({ phase: 'connecting', reason: null });
      void open();
    },
    detach: () => {
      life.wanted = false;
      clearTimeout(retry);
      stop();
      const closing = socket;
      socket = null;
      closing?.close(1000);
    },
  };
}
