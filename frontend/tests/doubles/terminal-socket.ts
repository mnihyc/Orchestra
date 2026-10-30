/** The fake of a person's shells and of the socket that carries one: a tiny shell that echoes what is typed and runs three commands.
 *
 * A shell is its opener's alone and stays while they are away: a ticket attaches a socket to it, the
 * socket first replays the end of what the shell printed and then carries bytes both ways, a second
 * socket takes the shell from the first, and a shell that exits, is closed or is left too long says why.
 * The fake shell understands `echo`, `pwd`, `colors` (a line in every colour), `clear` and `exit N`;
 * anything else is "command not found". A test prints into a shell, closes one, changes the limits
 * and the gates, and slows the answer to `ping`, through `/__double/workspace/terminal-*`.
 */
import { randomUUID } from 'node:crypto';

import type { WebSocket } from 'ws';

import type { TerminalSession } from '../../src/generated/api-types.ts';
import { type Answer, type Person, Refusal } from './world.ts';
import type { WorkspaceWorld } from './workspace.ts';

/** How much of what a shell printed is replayed to a screen that attaches: what the real connector keeps. */
const REPLAY_BYTES = 256 * 1024;
/** How long a ticket is good. */
const TICKET_MS = 30_000;
const PROMPT = '$ ';

interface Shell {
  session: TerminalSession;
  owner: string;
  buffer: Buffer;
  line: string;
  socket: WebSocket | null;
  /** What a recording holds, once the shell is closed. */
  kept: Buffer | null;
}

interface Ticket {
  session: string;
  expires: number;
  used: boolean;
}

export class Terminals {
  private shells = new Map<string, Shell>();
  private tickets = new Map<string, Ticket>();
  private maxPerPerson = 4;
  private maxPerEnvironment = 16;
  /** Why opening is refused though the permission is held; `null` when it is not. */
  private gate: 'disabled_by_policy' | 'machine_policy' | null = null;
  private pongDelayMs = 0;
  readonly workspace: WorkspaceWorld;

  constructor(workspace: WorkspaceWorld) {
    this.workspace = workspace;
  }

  reset(): void {
    for (const shell of this.shells.values()) shell.socket?.terminate();
    this.shells.clear();
    this.tickets.clear();
    this.maxPerPerson = 4;
    this.maxPerEnvironment = 16;
    this.gate = null;
    this.pongDelayMs = 0;
  }

  // ---------------------------------------------------------------- the requests

  private opened(person: Person, environment: string): Shell[] {
    return [...this.shells.values()].filter((shell) => shell.owner === person.me.username && shell.session.environment_id === environment && shell.session.state !== 'closed');
  }

  list(person: Person, environment: string, query: URLSearchParams): Answer {
    const wanted = query.get('state');
    const all = [...this.shells.values()].filter((shell) => shell.owner === person.me.username && shell.session.environment_id === environment);
    const items = all
      .filter((shell) => (wanted === 'closed' ? shell.session.state === 'closed' : shell.session.state !== 'closed'))
      .map((shell) => shell.session)
      .sort((one, other) => Date.parse(other.opened_at) - Date.parse(one.opened_at));
    return { status: 200, body: { items, next_cursor: null } };
  }

  open(person: Person, environment: string, thread: string | null, body: Record<string, unknown>): Answer {
    const over = this.workspace.override(environment);
    if (!over.terminal) throw new Refusal(403, 'terminal.not_allowed', 'shells are turned off in this environment', { reason: 'environment_switch_off' });
    if (this.gate !== null) throw new Refusal(403, 'terminal.not_allowed', 'a gate refuses it', { reason: this.gate });
    if (over.status !== 'running') throw new Refusal(403, 'terminal.not_allowed', 'the environment is offline', { reason: 'environment_offline' });
    const names = Array.isArray(body.close) ? body.close.filter((one): one is string => typeof one === 'string') : [];
    const closed: string[] = [];
    for (const name of names) {
      const shell = this.shells.get(name);
      if (shell?.owner === person.me.username && shell.session.state !== 'closed') {
        this.finish(shell, 'closed_by_user', null);
        closed.push(name);
      }
    }
    const mine = this.opened(person, environment).sort((one, other) => Date.parse(one.session.last_io_at) - Date.parse(other.session.last_io_at));
    if (mine.length >= this.maxPerPerson) {
      throw new Refusal(409, 'conflict.exists', 'a person has as many shells here as is allowed', { mine: mine.map((shell) => shell.session), limit: 'person' });
    }
    const everyone = [...this.shells.values()].filter((shell) => shell.session.environment_id === environment && shell.session.state !== 'closed');
    if (everyone.length >= this.maxPerEnvironment) {
      throw new Refusal(409, 'conflict.exists', 'the environment has as many shells as is allowed', { mine: mine.map((shell) => shell.session), limit: 'environment', open: everyone.length, people: new Set(everyone.map((shell) => shell.owner)).size });
    }
    const shell = this.make(person, environment, thread, typeof body.cwd === 'string' ? body.cwd : '', Number(body.cols ?? 80), Number(body.rows ?? 24));
    const ticket = this.ticketFor(shell.session.id);
    return { status: 201, body: { session: shell.session, ticket: ticket.ticket, ticket_expires_at: ticket.ticket_expires_at, closed } };
  }

  make(person: Person, environment: string, thread: string | null, cwd: string, cols: number, rows: number): Shell {
    const now = new Date().toISOString();
    const session: TerminalSession = {
      attached: false,
      busy: false,
      close_reason: null,
      closed_at: null,
      closes_at: new Date(Date.now() + 8 * 60 * 60 * 1000).toISOString(),
      cols,
      cwd,
      detached_at: now,
      environment_id: environment,
      exit_code: null,
      id: randomUUID(),
      last_io_at: now,
      opened_at: now,
      opened_by: this.workspace.userRefOf(person),
      recorded: true,
      rows,
      shell: 'bash',
      state: 'open',
      thread_id: thread,
    };
    const shell: Shell = { session, owner: person.me.username, buffer: Buffer.from(PROMPT), line: '', socket: null, kept: null };
    this.shells.set(session.id, shell);
    return shell;
  }

  private ticketFor(session: string): { ticket: string; ticket_expires_at: string } {
    const ticket = `ott_${randomUUID()}`;
    this.tickets.set(ticket, { session, expires: Date.now() + TICKET_MS, used: false });
    return { ticket, ticket_expires_at: new Date(Date.now() + TICKET_MS).toISOString() };
  }

  ticket(person: Person, id: string): Answer {
    const shell = this.shells.get(id);
    if (shell?.owner !== person.me.username) throw new Refusal(404, 'not_found', 'there is no such shell');
    if (shell.session.state === 'closed') throw new Refusal(409, 'conflict.exists', 'the shell is closed');
    return { status: 200, body: this.ticketFor(id) };
  }

  close(person: Person, id: string, reason: string): Answer {
    const shell = this.shells.get(id);
    if (shell === undefined || (shell.owner !== person.me.username && !person.me.is_system_admin)) throw new Refusal(404, 'not_found', 'there is no such shell');
    if (shell.session.state !== 'closed') this.finish(shell, reason, null);
    return { status: 200, body: shell.session };
  }

  recording(person: Person, id: string): Answer {
    const shell = this.shells.get(id);
    if (shell?.owner !== person.me.username || shell.kept === null) throw new Refusal(404, 'not_found', 'nothing was kept');
    return { status: 200, bytes: shell.kept, headers: { 'Content-Type': 'application/octet-stream' } };
  }

  /** Ends a shell, telling whatever screen is attached why. */
  finish(shell: Shell, reason: string, code: number | null): void {
    const now = new Date().toISOString();
    shell.kept = shell.buffer.subarray(Math.max(0, shell.buffer.length - REPLAY_BYTES));
    shell.session = { ...shell.session, state: 'closed', close_reason: reason, closed_at: now, exit_code: code, attached: false, busy: false };
    const socket = shell.socket;
    shell.socket = null;
    if (socket !== null) {
      if (code !== null) socket.send(JSON.stringify({ op: 'exit', code }));
      socket.send(JSON.stringify({ op: 'closed', reason }));
      socket.close(1000);
    }
  }

  // ---------------------------------------------------------------- the socket

  /** A browser attaches with a ticket. */
  attach(socket: WebSocket, id: string, ticket: string): void {
    const held = this.tickets.get(ticket);
    const shell = this.shells.get(id);
    if (held?.session !== id || held.used || held.expires < Date.now() || shell === undefined || shell.session.state === 'closed') {
      socket.close(4401);
      return;
    }
    held.used = true;
    // A shell has one screen at a time: the one that had it is told so, and the shell is untouched.
    if (shell.socket !== null) {
      shell.socket.send(JSON.stringify({ op: 'closed', reason: 'attached_elsewhere' }));
      shell.socket.close(1000);
    }
    shell.socket = socket;
    shell.session = { ...shell.session, attached: true, detached_at: null };
    const replay = shell.buffer.subarray(Math.max(0, shell.buffer.length - REPLAY_BYTES));
    socket.send(JSON.stringify({ op: 'ready', cols: shell.session.cols, rows: shell.session.rows, replay_bytes: replay.length }));
    if (replay.length > 0) socket.send(replay, { binary: true });

    socket.on('message', (data, isBinary) => {
      if (shell.socket !== socket) return;
      const bytes = Array.isArray(data) ? Buffer.concat(data) : Buffer.from(data as ArrayBuffer);
      if (isBinary) {
        this.typed(shell, bytes);
        return;
      }
      try {
        const frame = JSON.parse(bytes.toString('utf8')) as { op?: string; n?: number; cols?: number; rows?: number };
        if (frame.op === 'ping') setTimeout(() => socket.readyState === socket.OPEN && socket.send(JSON.stringify({ op: 'pong', n: frame.n })), this.pongDelayMs);
        else if (frame.op === 'resize') shell.session = { ...shell.session, cols: Number(frame.cols), rows: Number(frame.rows) };
      } catch {
        socket.close(4400);
      }
    });
    socket.on('close', () => {
      if (shell.socket === socket) {
        shell.socket = null;
        if (shell.session.state !== 'closed') shell.session = { ...shell.session, attached: false, detached_at: new Date().toISOString() };
      }
    });
  }

  private print(shell: Shell, text: string): void {
    const bytes = Buffer.from(text, 'utf8');
    shell.buffer = Buffer.concat([shell.buffer, bytes]);
    if (shell.buffer.length > REPLAY_BYTES * 2) shell.buffer = shell.buffer.subarray(shell.buffer.length - REPLAY_BYTES);
    shell.session = { ...shell.session, last_io_at: new Date().toISOString() };
    shell.socket?.send(bytes, { binary: true });
  }

  /** What the person typed: echoed, and at the end of a line run. */
  private typed(shell: Shell, bytes: Buffer): void {
    for (const byte of bytes) {
      if (byte === 13) {
        this.print(shell, '\r\n');
        const line = shell.line.trim();
        shell.line = '';
        if (this.run(shell, line)) this.print(shell, PROMPT);
      } else if (byte === 127) {
        if (shell.line.length > 0) {
          shell.line = shell.line.slice(0, -1);
          this.print(shell, '\b \b');
        }
      } else if (byte === 3) {
        shell.line = '';
        this.print(shell, `^C\r\n${PROMPT}`);
      } else if (byte >= 32) {
        shell.line += String.fromCharCode(byte);
        this.print(shell, String.fromCharCode(byte));
      }
    }
  }

  /** Runs one line; false when the shell has ended. */
  private run(shell: Shell, line: string): boolean {
    if (line === '') return true;
    const [command = '', ...rest] = line.split(/\s+/);
    switch (command) {
      case 'echo':
        this.print(shell, `${rest.join(' ')}\r\n`);
        return true;
      case 'pwd':
        this.print(shell, `/${shell.session.cwd}\r\n`);
        return true;
      case 'clear':
        this.print(shell, '\u001b[2J\u001b[H');
        return true;
      case 'colors':
        this.print(shell, '\u001b[31mred\u001b[0m \u001b[32mgreen\u001b[0m \u001b[33myellow\u001b[0m \u001b[34mblue\u001b[0m \u001b[1;36mcyan bold\u001b[0m\r\n');
        return true;
      case 'exit':
        this.finish(shell, 'exited', Number(rest[0] ?? 0));
        return false;
      default:
        this.print(shell, `bash: ${command}: command not found\r\n`);
        return true;
    }
  }

  // ---------------------------------------------------------------- what a test steers

  control(path: string, body: Record<string, unknown>): Answer {
    switch (path) {
      case '/terminal-limit':
        if (typeof body.person === 'number') this.maxPerPerson = body.person;
        if (typeof body.environment === 'number') this.maxPerEnvironment = body.environment;
        return { status: 204 };
      case '/terminal-gate':
        this.gate = body.reason === 'disabled_by_policy' || body.reason === 'machine_policy' ? body.reason : null;
        return { status: 204 };
      case '/terminal-lag':
        this.pongDelayMs = typeof body.ms === 'number' ? body.ms : 0;
        return { status: 204 };
      case '/terminal-print': {
        const shell = this.shells.get(String(body.id));
        if (shell !== undefined) this.print(shell, String(body.text));
        return { status: 204 };
      }
      case '/terminal-close': {
        const shell = this.shells.get(String(body.id));
        if (shell !== undefined && shell.session.state !== 'closed') this.finish(shell, typeof body.reason === 'string' ? body.reason : 'detached', null);
        return { status: 204 };
      }
      case '/terminal-seed': {
        // Shells of somebody that were opened and left, to meet a limit.
        const person = this.workspace.world.people.get(typeof body.person === 'string' ? body.person : 'ada');
        const environment = typeof body.environment === 'string' ? body.environment : this.workspace.environmentIds()[0] ?? '';
        const made: string[] = [];
        if (person !== undefined) {
          for (let index = 0; index < Number(body.count ?? 1); index += 1) {
            const shell = this.make(person, environment, typeof body.thread === 'string' ? body.thread : null, 'web', 80, 24);
            shell.session = { ...shell.session, last_io_at: new Date(Date.parse(shell.session.opened_at) - (Number(body.count ?? 1) - index) * 60_000).toISOString() };
            this.print(shell, `echo ${index}\r\n${index}\r\n${PROMPT}`);
            made.push(shell.session.id);
          }
        }
        return { status: 200, body: { ids: made } };
      }
      case '/terminal-state':
        return { status: 200, body: { sessions: [...this.shells.values()].map((shell) => shell.session), tickets: this.tickets.size } };
      default:
        return { status: 404, body: { detail: `no such control: ${path}` } };
    }
  }
}
