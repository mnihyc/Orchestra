/** The fake of an environment's displays: the list as the real server keeps it, and a socket that speaks enough of RFB 3.8 (no password, raw pixels) for a real viewer to connect and draw, and that tells a test which messages reached the display.
 *
 * The socket's bytes are the protocol's own, from the server's version on, as the real one carries them.
 * On a ticket to watch, what acts (a key, the pointer, pasted text, a resize, power) is dropped as the
 * real server's filter drops it, and noted as dropped; on a ticket to work in it, it reaches the display.
 * A test changes the picture, sends what was copied on a display an agent announced, ends a display, or
 * switches displays off, through `/__double/displays/...`.
 */
import { randomUUID } from 'node:crypto';

import type { WebSocket } from 'ws';

import type { Display, EnvironmentDisplays, EnvironmentRef } from '../../src/generated/api-types.ts';
import { ACME, RUNS_ON, userRef } from './fixtures.ts';
import { type Answer, type Part, type Person, Refusal, type World } from './world.ts';

const TICKET_MS = 30_000;
/** The size the fake draws: small, so that a whole update in raw pixels stays light for a test. */
const FRAME = { width: 320, height: 200 } as const;
/** The close code that carries why a display's socket ended, as the real server sends it. */
const CLOSE_WITH_REASON = 4000;

type Off = EnvironmentDisplays['off'];

interface PixelFormat { bits: number; bigEndian: boolean; redMax: number; greenMax: number; blueMax: number; redShift: number; greenShift: number; blueShift: number }

interface Client {
  socket: WebSocket;
  person: Person;
  control: boolean;
  stage: 'version' | 'security' | 'init' | 'normal';
  pending: Buffer;
  format: PixelFormat;
  /** An update asked for and not answered yet: answered when the picture changes. */
  waiting: boolean;
  seen: number;
}

interface Kept {
  row: Display;
  change: number;
  clients: Set<Client>;
  received: { by: string; control: boolean; dropped: boolean; kind: number; length: number }[];
}

/** The pixel format a server says first: 32 bits, true colour, little-endian, red in the high byte. */
const SERVER_FORMAT: PixelFormat = { bits: 32, bigEndian: false, redMax: 255, greenMax: 255, blueMax: 255, redShift: 16, greenShift: 8, blueShift: 0 };

/** The colour of a point of the fake display: a desktop with a window whose title bar moves with each change. */
function colourAt(x: number, y: number, change: number): [number, number, number] {
  const left = 40 + (change % 10) * 8;
  if (x >= left && x < left + 180 && y >= 40 && y < 56) return [40, 80, 160];
  if (x >= left && x < left + 180 && y >= 56 && y < 150) return [245, 245, 245];
  return [(x * 255) / FRAME.width, 90, (y * 255) / FRAME.height];
}

export class DisplaysWorld implements Part {
  readonly world: World;
  private places = new Map<string, { off: Off; servers: { name: string; path: string; version: string | null }[]; windowManager: string | null }>();
  private displays = new Map<string, Kept>();
  private tickets = new Map<string, { display: string; user: string; control: boolean; expires: number }>();

  constructor(world: World) {
    this.world = world;
    this.reset();
  }

  reset(): void {
    for (const kept of this.displays.values()) for (const client of kept.clients) client.socket.terminate();
    this.places.clear();
    this.displays.clear();
    this.tickets.clear();
    const xvnc = [{ name: 'Xvnc', path: '/usr/bin/Xvnc', version: '1.14.1' }];
    this.places.set(RUNS_ON.workspace, { off: null, servers: xvnc, windowManager: 'openbox' });
    this.places.set(RUNS_ON.docsCheckout, { off: 'environment_offline', servers: xvnc, windowManager: null });
    this.places.set(RUNS_ON.sandbox, { off: null, servers: [], windowManager: null });
    const ada = this.world.people.get('ada');
    this.add(RUNS_ON.workspace, 'Desktop', 'started', ada === undefined ? null : userRef(ada.me), null, ':1', null);
    this.add(RUNS_ON.workspace, 'emulator', 'announced', null, { id: ACME.mainThread, kind: 'thread', name: 'Main thread', redacted: false }, null, 'The Android emulator the tests run on');
  }

  /** What an environment has to serve a display, for its page and its list. */
  supportOf(environmentId: string): { servers: { name: string; path: string; version: string | null }[]; window_manager: string | null } {
    const place = this.places.get(environmentId);
    return { servers: place?.servers ?? [], window_manager: place?.windowManager ?? null };
  }

  /** Whether displays are on for an environment, for its page. */
  switchedOn(environmentId: string): boolean {
    const off = this.places.get(environmentId)?.off ?? null;
    return off !== 'environment_switch_off' && off !== 'disabled_by_policy';
  }

  private add(environmentId: string, name: string, origin: Display['origin'], by: Display['opened_by'], thread: Display['thread'], xDisplay: string | null, note: string | null): Kept {
    const now = new Date().toISOString();
    const environment = { id: environmentId, name: this.world.resources.environments.find((record) => record.id === environmentId)?.name ?? 'Workspace' } as EnvironmentRef;
    const row: Display = {
      id: randomUUID(), environment, name, origin, opened_by: by, thread, endpoint: origin === 'started' ? '127.0.0.1:5901' : '127.0.0.1:5554', x_display: xDisplay,
      geometry: `${FRAME.width}x${FRAME.height}`, watching: 0, driving: [], note, browsers: [], state: 'open', close_reason: null, opened_at: now, last_used_at: now, closed_at: null,
    };
    const kept: Kept = { row, change: 0, clients: new Set(), received: [] };
    this.displays.set(row.id, kept);
    return kept;
  }

  private may(person: Person, environmentId: string, permission: 'vnc.view' | 'vnc.manage'): void {
    if (!this.world.access.permissionsOnEnvironment(person, environmentId).includes(permission)) throw new Refusal(403, 'authz.denied', `${permission} is needed on this environment`, { required: permission });
  }

  private rowOf(kept: Kept, manage: boolean): Display {
    return { ...kept.row, endpoint: manage ? kept.row.endpoint : null, watching: kept.clients.size, driving: [...new Map([...kept.clients].filter((client) => client.control).map((client) => [client.person.me.username, userRef(client.person.me)])).values()] };
  }

  handle(person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>): Answer | null {
    let found: RegExpExecArray | null;
    if ((found = /^\/environments\/([^/]+)\/displays$/.exec(path)) !== null) {
      const environmentId = found[1] ?? '';
      const place = this.places.get(environmentId) ?? { off: null, servers: [], windowManager: null };
      if (method === 'GET') {
        this.may(person, environmentId, 'vnc.view');
        const manage = this.world.access.permissionsOnEnvironment(person, environmentId).includes('vnc.manage');
        const displays = [...this.displays.values()].filter((kept) => kept.row.environment.id === environmentId).map((kept) => this.rowOf(kept, manage));
        const list: EnvironmentDisplays = { off: place.off, support: this.supportOf(environmentId), displays, limit: { open: displays.filter((one) => one.state !== 'closed' && one.origin === 'started').length, max: 4 } };
        return { status: 200, body: list };
      }
      if (method === 'POST') {
        this.may(person, environmentId, 'vnc.manage');
        if (place.off !== null) throw new Refusal(403, 'vnc.not_allowed', 'displays are off here', { reason: place.off });
        if (place.servers.length === 0) throw new Refusal(403, 'vnc.not_allowed', 'nothing here serves a display', { reason: 'unsupported' });
        const taken = [...this.displays.values()].filter((kept) => kept.row.environment.id === environmentId && kept.row.x_display !== null).length;
        const xDisplay = `:${taken + 1}`;
        const threadId = query.get('thread');
        const kept = this.add(environmentId, typeof body.name === 'string' && body.name !== '' ? body.name : xDisplay, 'started', userRef(person.me), threadId === null ? null : { id: threadId, kind: 'thread', name: this.world.places.rowOf(threadId)?.name ?? null, redacted: false }, xDisplay, null);
        return { status: 201, body: this.rowOf(kept, true) };
      }
    }
    if ((found = /^\/displays\/([^/]+)$/.exec(path)) !== null && method === 'DELETE') {
      const kept = this.displays.get(found[1] ?? '');
      if (kept === undefined) throw new Refusal(404, 'not_found', 'no such display');
      this.may(person, kept.row.environment.id, 'vnc.manage');
      this.end(kept, 'quit');
      return { status: 200, body: this.rowOf(kept, true) };
    }
    if ((found = /^\/displays\/([^/]+)\/ticket$/.exec(path)) !== null && method === 'POST') {
      const kept = this.displays.get(found[1] ?? '');
      if (kept === undefined) throw new Refusal(404, 'not_found', 'no such display');
      const control = body.control === true;
      this.may(person, kept.row.environment.id, control ? 'vnc.manage' : 'vnc.view');
      const ticket = randomUUID();
      this.tickets.set(ticket, { display: kept.row.id, user: person.me.username, control, expires: Date.now() + TICKET_MS });
      return { status: 200, body: { ticket, ticket_expires_at: new Date(Date.now() + TICKET_MS).toISOString() } };
    }
    return null;
  }

  control(path: string, body: Record<string, unknown>): Answer | null {
    if (path === '/place') {
      const environmentId = typeof body.environment === 'string' ? body.environment : RUNS_ON.workspace;
      const place = this.places.get(environmentId);
      if (place === undefined) return { status: 404, body: { detail: 'no such environment' } };
      if ('off' in body) place.off = (body.off ?? null) as Off;
      if (Array.isArray(body.servers)) place.servers = body.servers as typeof place.servers;
      return { status: 204 };
    }
    if (path === '/list') {
      const environmentId = typeof body.environment === 'string' ? body.environment : RUNS_ON.workspace;
      return { status: 200, body: [...this.displays.values()].filter((kept) => kept.row.environment.id === environmentId).map((kept) => ({ id: kept.row.id, name: kept.row.name, origin: kept.row.origin, state: kept.row.state, clients: kept.clients.size })) };
    }
    const kept = typeof body.display === 'string' ? this.displays.get(body.display) : [...this.displays.values()].find((one) => one.row.name === body.name);
    if (kept === undefined) return path.startsWith('/') ? { status: 404, body: { detail: 'no such display' } } : null;
    switch (path) {
      case '/received':
        return { status: 200, body: kept.received };
      case '/change':
        this.changed(kept);
        return { status: 204 };
      case '/cut': {
        // What a program copied on a display an agent announced: its own server sends it to every viewer.
        const text = Buffer.from(typeof body.text === 'string' ? body.text : 'copied text', 'latin1');
        const message = Buffer.alloc(8);
        message.writeUInt8(3, 0);
        message.writeUInt32BE(text.length, 4);
        for (const client of kept.clients) if (client.stage === 'normal') client.socket.send(Buffer.concat([message, text]), { binary: true });
        return { status: 204 };
      }
      case '/end':
        this.end(kept, typeof body.reason === 'string' ? body.reason : 'closed_by_agent');
        return { status: 204 };
      default:
        return null;
    }
  }

  // ---------------------------------------------------------------- the socket, as RFB 3.8

  attach(socket: WebSocket, displayId: string, ticketValue: string): void {
    const ticket = this.tickets.get(ticketValue);
    this.tickets.delete(ticketValue);
    const kept = this.displays.get(displayId);
    const person = this.world.people.get(ticket?.user ?? '');
    if (ticket?.display !== displayId || ticket.expires < Date.now() || kept === undefined || person === undefined) {
      socket.close(CLOSE_WITH_REASON, 'revoked');
      return;
    }
    if (kept.row.state === 'closed') {
      socket.close(CLOSE_WITH_REASON, kept.row.close_reason ?? 'quit');
      return;
    }
    const client: Client = { socket, person, control: ticket.control, stage: 'version', pending: Buffer.alloc(0), format: SERVER_FORMAT, waiting: false, seen: -1 };
    kept.clients.add(client);
    socket.send(Buffer.from('RFB 003.008\n', 'latin1'), { binary: true });
    socket.on('message', (data) => {
      client.pending = Buffer.concat([client.pending, Buffer.from(data as Buffer)]);
      this.read(kept, client);
    });
    socket.on('close', () => kept.clients.delete(client));
  }

  private read(kept: Kept, client: Client): void {
    for (;;) {
      const bytes = client.pending;
      if (client.stage === 'version') {
        if (bytes.length < 12) return;
        client.pending = bytes.subarray(12);
        client.stage = 'security';
        client.socket.send(Buffer.from([1, 1]), { binary: true });
        continue;
      }
      if (client.stage === 'security') {
        if (bytes.length < 1) return;
        client.pending = bytes.subarray(1);
        client.stage = 'init';
        client.socket.send(Buffer.from([0, 0, 0, 0]), { binary: true });
        continue;
      }
      if (client.stage === 'init') {
        if (bytes.length < 1) return;
        client.pending = bytes.subarray(1);
        client.stage = 'normal';
        const name = Buffer.from(kept.row.name, 'utf8');
        const init = Buffer.alloc(24);
        init.writeUInt16BE(FRAME.width, 0);
        init.writeUInt16BE(FRAME.height, 2);
        init.writeUInt8(32, 4);
        init.writeUInt8(24, 5);
        init.writeUInt8(0, 6);
        init.writeUInt8(1, 7);
        init.writeUInt16BE(255, 8);
        init.writeUInt16BE(255, 10);
        init.writeUInt16BE(255, 12);
        init.writeUInt8(16, 14);
        init.writeUInt8(8, 15);
        init.writeUInt8(0, 16);
        init.writeUInt32BE(name.length, 20);
        client.socket.send(Buffer.concat([init, name]), { binary: true });
        continue;
      }
      const length = messageLength(bytes);
      if (length === null) return;
      if (length < 0) {
        client.socket.close(CLOSE_WITH_REASON, 'protocol');
        return;
      }
      const message = bytes.subarray(0, length);
      client.pending = bytes.subarray(length);
      this.message(kept, client, message);
    }
  }

  private message(kept: Kept, client: Client, message: Buffer): void {
    const kind = message.readUInt8(0);
    if (kind === 0) {
      client.format = {
        bits: message.readUInt8(4), bigEndian: message.readUInt8(6) !== 0, redMax: message.readUInt16BE(8), greenMax: message.readUInt16BE(10), blueMax: message.readUInt16BE(12),
        redShift: message.readUInt8(14), greenShift: message.readUInt8(15), blueShift: message.readUInt8(16),
      };
      return;
    }
    if (kind === 3) {
      const incremental = message.readUInt8(1) !== 0;
      if (!incremental) client.seen = -1;
      client.waiting = true;
      this.update(kept, client);
      return;
    }
    if (kind === 2 || kind === 150 || kind === 248) return;
    // What acts: dropped on a ticket to watch, as the real server's filter drops it, and noted either way.
    kept.received.push({ by: client.person.me.username, control: client.control, dropped: !client.control, kind, length: message.length });
    if (client.control) {
      kept.row = { ...kept.row, last_used_at: new Date().toISOString() };
      if (kind === 4 || kind === 5) this.changed(kept);
    }
  }

  private changed(kept: Kept): void {
    kept.change += 1;
    for (const client of kept.clients) this.update(kept, client);
  }

  /** A whole picture in raw pixels, in the format the viewer asked for. */
  private update(kept: Kept, client: Client): void {
    if (!client.waiting || client.seen === kept.change || client.stage !== 'normal') return;
    client.waiting = false;
    client.seen = kept.change;
    const format = client.format;
    const bytesPerPixel = format.bits / 8;
    const header = Buffer.alloc(16);
    header.writeUInt8(0, 0);
    header.writeUInt16BE(1, 2);
    header.writeUInt16BE(0, 4);
    header.writeUInt16BE(0, 6);
    header.writeUInt16BE(FRAME.width, 8);
    header.writeUInt16BE(FRAME.height, 10);
    header.writeInt32BE(0, 12);
    const pixels = Buffer.alloc(FRAME.width * FRAME.height * bytesPerPixel);
    for (let y = 0; y < FRAME.height; y += 1) {
      for (let x = 0; x < FRAME.width; x += 1) {
        const [red, green, blue] = colourAt(x, y, kept.change);
        const value = ((Math.round((red * format.redMax) / 255) << format.redShift) | (Math.round((green * format.greenMax) / 255) << format.greenShift) | (Math.round((blue * format.blueMax) / 255) << format.blueShift)) >>> 0;
        const at = (y * FRAME.width + x) * bytesPerPixel;
        for (let byte = 0; byte < bytesPerPixel; byte += 1) {
          const shift = format.bigEndian ? (bytesPerPixel - 1 - byte) * 8 : byte * 8;
          pixels[at + byte] = (value >>> shift) & 0xff;
        }
      }
    }
    client.socket.send(Buffer.concat([header, pixels]), { binary: true });
  }

  private end(kept: Kept, reason: string): void {
    kept.row = { ...kept.row, state: 'closed', close_reason: reason, closed_at: new Date().toISOString() };
    for (const client of kept.clients) client.socket.close(CLOSE_WITH_REASON, reason);
    kept.clients.clear();
  }
}

/** The length of the client's next message, `null` while it is incomplete, -1 for one the double cannot follow. */
function messageLength(bytes: Buffer): number | null {
  if (bytes.length < 1) return null;
  const need = (count: number, length: () => number): number | null => (bytes.length < count ? null : length());
  switch (bytes.readUInt8(0)) {
    case 0: return 20;
    case 2: return need(4, () => 4 + 4 * bytes.readUInt16BE(2));
    case 3: case 150: return 10;
    case 4: return 8;
    case 5: return need(2, () => ((bytes.readUInt8(1) & 0x80) === 0 ? 6 : 7));
    case 6: return need(8, () => 8 + Math.abs(bytes.readInt32BE(4)));
    case 248: return need(9, () => 9 + bytes.readUInt8(8));
    case 250: return 4;
    case 251: return need(8, () => 8 + 16 * bytes.readUInt8(6));
    case 255: return 12;
    default: return -1;
  }
}
