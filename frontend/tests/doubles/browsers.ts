/** The fake of an environment's browsers: profiles and browsers kept as the real server keeps them, and a socket that serves pictures (real JPEGs, made here) and takes what a person does, so that a test can ask what reached the page.
 *
 * A ticket to watch carries nothing to the page: every frame of the second kind on it is dropped and
 * answered with `error view_only`, as the real server does, and is noted as dropped. One picture is in
 * flight for each viewer; the next is sent when the one before was acknowledged and the page changed. A
 * test makes the page ask (a dialog, a file), lowers the picture, adds a second person, ends a browser,
 * switches browsers off or makes nothing be found, through `/__double/browsers/...`.
 */
import { randomUUID } from 'node:crypto';

import type { WebSocket } from 'ws';

import type { Browser, BrowserProfile, BrowsersOff, EnvironmentBrowsers, EnvironmentRef, NodeRef } from '../../src/generated/api-types.ts';
import { ACME, RUNS_ON, userRef } from './fixtures.ts';
import { type Answer, type Part, type Person, Refusal, type World } from './world.ts';

/** How long a ticket is good, as for a shell. */
const TICKET_MS = 30_000;
/** How long somebody counts as driving after they last sent something: what the real server says. */
const DRIVING_FOR_MS = 10_000;
/** The size of the pictures the fake draws: half of the page, as a slow link would make them. */
const PICTURE = { width: 640, height: 400 } as const;

// ---------------------------------------------------------------- a JPEG made here

/** The standard table of DC codes for luminance (ITU T.81, Annex K.3). */
const DC_BITS = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
const DC_VALUES = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
/** The DC codes those tables make, by category. */
const DC_CODES: readonly [code: number, length: number][] = [[0b00, 2], [0b010, 3], [0b011, 3], [0b100, 3], [0b101, 3], [0b110, 3], [0b1110, 4], [0b11110, 5], [0b111110, 6], [0b1111110, 7], [0b11111110, 8], [0b111111110, 9]];

/** A grey picture of 8 by 8 blocks, each of one level: as much of a page as a test needs to see that it arrived. */
export function jpegOf(width: number, height: number, level: (column: number, row: number) => number): Uint8Array<ArrayBuffer> {
  const out: number[] = [];
  const word = (value: number) => out.push((value >> 8) & 0xff, value & 0xff);
  out.push(0xff, 0xd8);
  out.push(0xff, 0xe0); word(16); out.push(0x4a, 0x46, 0x49, 0x46, 0, 1, 1, 0); word(1); word(1); out.push(0, 0);
  out.push(0xff, 0xdb); word(67); out.push(0); for (let index = 0; index < 64; index += 1) out.push(1);
  out.push(0xff, 0xc0); word(11); out.push(8); word(height); word(width); out.push(1, 1, 0x11, 0);
  out.push(0xff, 0xc4); word(3 + 16 + DC_VALUES.length); out.push(0x00, ...DC_BITS, ...DC_VALUES);
  // The AC table holds one code alone, end of block: every block is of one level.
  out.push(0xff, 0xc4); word(3 + 16 + 1); out.push(0x10, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x00);
  out.push(0xff, 0xda); word(8); out.push(1, 1, 0x00, 0, 63, 0);
  let bits = 0;
  let count = 0;
  const put = (value: number, length: number) => {
    for (let at = length - 1; at >= 0; at -= 1) {
      bits = (bits << 1) | ((value >> at) & 1);
      count += 1;
      if (count === 8) {
        out.push(bits);
        if (bits === 0xff) out.push(0);
        bits = 0;
        count = 0;
      }
    }
  };
  let previous = 0;
  for (let row = 0; row < height / 8; row += 1) {
    for (let column = 0; column < width / 8; column += 1) {
      const dc = 8 * (Math.max(0, Math.min(255, Math.round(level(column, row)))) - 128);
      const difference = dc - previous;
      previous = dc;
      const magnitude = Math.abs(difference);
      const category = magnitude === 0 ? 0 : Math.floor(Math.log2(magnitude)) + 1;
      const [code, length] = DC_CODES[category] ?? [0, 2];
      put(code, length);
      if (category > 0) put(difference > 0 ? difference : (difference - 1) & ((1 << category) - 1), category);
      put(0, 1);
    }
  }
  if (count > 0) put((1 << (8 - count)) - 1, 8 - count);
  out.push(0xff, 0xd9);
  return new Uint8Array(out);
}

/** A page as blocks: a bar at the top, a heading, lines of text, and a mark that moves with each change. */
function pageLevels(change: number): (column: number, row: number) => number {
  const columns = PICTURE.width / 8;
  return (column, row) => {
    if (row < 4) return 200;
    if (row === 6 && column > 4 && column < columns / 2) return 40;
    if (row > 8 && row % 3 === 0 && column > 4 && column < columns - 8 - (row % 7) * 3) return 110;
    if (row === 30 && column === 10 + (change % 40)) return 0;
    return 245;
  };
}

// ---------------------------------------------------------------- the world

interface Tab { id: string; title: string; url: string; loading: boolean; active: boolean; opener: string | null }

interface Viewer {
  socket: WebSocket;
  person: Person;
  control: boolean;
  tab: string | null;
  inFlight: number | null;
  /** The change of the page the viewer saw last. */
  seen: number;
}

interface Kept {
  row: Browser;
  tabs: Tab[];
  viewport: { width: number; height: number };
  /** Bumped by everything that changes the page. */
  change: number;
  viewers: Set<Viewer>;
  drove: Map<string, number>;
  /** What reached the page, and what a watching socket sent and was dropped, in order. */
  received: { by: string; control: boolean; dropped: boolean; frame: Record<string, unknown> }[];
}

interface Place {
  off: BrowsersOff | null;
  programs: { name: string; path: string; version: string | null }[];
  sandbox: boolean | null;
  window: boolean;
  profiles: Map<string, BrowserProfile>;
}

export class BrowsersWorld implements Part {
  readonly world: World;
  private places = new Map<string, Place>();
  private browsers = new Map<string, Kept>();
  private tickets = new Map<string, { browser: string; user: string; control: boolean; expires: number }>();
  private frameNumber = 0;

  constructor(world: World) {
    this.world = world;
    this.reset();
  }

  reset(): void {
    for (const kept of this.browsers.values()) for (const viewer of kept.viewers) viewer.socket.terminate();
    this.places.clear();
    this.browsers.clear();
    this.tickets.clear();
    const at = new Date(Date.now() - 20 * 60_000).toISOString();
    const chromium = [{ name: 'chromium', path: '/usr/bin/chromium', version: '131.0.6778.85' }];
    this.places.set(RUNS_ON.workspace, { off: null, programs: chromium, sandbox: true, window: false, profiles: new Map([
      ['notion', { name: 'notion', size_bytes: 48_000_000, used_at: at, browser: null }],
      ['twitter', { name: 'twitter', size_bytes: 12_000_000, used_at: at, browser: null }],
      ['thread-9c1f', { name: 'thread-9c1f', size_bytes: 3_000_000, used_at: at, browser: null }],
    ]) });
    this.places.set(RUNS_ON.docsCheckout, { off: 'environment_offline', programs: chromium, sandbox: null, window: false, profiles: new Map() });
    this.places.set(RUNS_ON.sandbox, { off: null, programs: [], sandbox: null, window: false, profiles: new Map() });
    const ada = this.world.people.get('ada');
    this.start(RUNS_ON.workspace, 'notion', ada === undefined ? null : userRef(ada.me), null, 'https://www.notion.so/acme/Roadmap', 'Roadmap');
    this.start(RUNS_ON.workspace, 'thread-9c1f', null, { id: ACME.mainThread, kind: 'thread', name: 'Main thread', redacted: false }, 'https://github.com/acme/web/pull/7', 'Fix login · Pull request #7');
  }

  /** Whether browsers are on for an environment, for its page. */
  switchedOn(environmentId: string): boolean {
    const off = this.places.get(environmentId)?.off ?? null;
    return off !== 'environment_switch_off' && off !== 'disabled_by_policy';
  }

  /** What the environment has to run a browser and to serve a display, for its page. */
  support(environmentId: string): { looked_at: string | null; browser: EnvironmentBrowsers['support']; vnc: { servers: { name: string; path: string; version: string | null }[]; window_manager: string | null } } {
    const place = this.places.get(environmentId);
    return {
      looked_at: new Date().toISOString(),
      browser: { programs: place?.programs ?? [], window: place?.window ?? false, playwright_cli: place === undefined || place.programs.length === 0 ? null : '0.1.1', sandbox: place?.sandbox ?? null },
      vnc: this.world.displays.supportOf(environmentId),
    };
  }

  private start(environmentId: string, profile: string, by: Browser['opened_by'], thread: NodeRef | null, url: string, title: string): Kept {
    const now = new Date().toISOString();
    const id = randomUUID();
    const environment = { id: environmentId, name: this.world.resources.environments.find((record) => record.id === environmentId)?.name ?? 'Workspace' } as EnvironmentRef;
    const row: Browser = {
      id, environment, profile, origin: 'started', opened_by: by, thread, preset: 'headless', display: null,
      endpoint: 'http://127.0.0.1:41873', program: 'Chromium 131.0.6778.85', tabs: 1, note: thread === null ? null : 'Checks the pull request page',
      watching: 0, driving: [], state: 'open', close_reason: null, opened_at: now, last_used_at: now, closed_at: null,
    };
    const kept: Kept = { row, tabs: [{ id: randomUUID(), title, url, loading: false, active: true, opener: null }], viewport: { width: 1280, height: 800 }, change: 0, viewers: new Set(), drove: new Map(), received: [] };
    this.browsers.set(id, kept);
    const place = this.places.get(environmentId);
    const known = place?.profiles.get(profile);
    if (place !== undefined) place.profiles.set(profile, { ...(known ?? { name: profile, size_bytes: 0, used_at: null }), browser: id, used_at: now });
    return kept;
  }

  private may(person: Person, environmentId: string, permission: 'browser.view' | 'browser.manage'): void {
    if (!this.world.access.permissionsOnEnvironment(person, environmentId).includes(permission)) {
      throw new Refusal(403, 'authz.denied', `${permission} is needed on this environment`, { required: permission });
    }
  }

  /** Names already held in this world's browser memory, for the read-only reach projection. */
  profileNames(environmentId: string): { name: string }[] {
    return [...(this.places.get(environmentId)?.profiles.keys() ?? [])].sort().map((name) => ({ name }));
  }

  private listOf(person: Person, environmentId: string): EnvironmentBrowsers {
    const place = this.places.get(environmentId) ?? { off: null, programs: [], sandbox: null, window: false, profiles: new Map<string, BrowserProfile>() };
    const manage = this.world.access.permissionsOnEnvironment(person, environmentId).includes('browser.manage');
    const browsers = [...this.browsers.values()].filter((kept) => kept.row.environment.id === environmentId).map((kept) => this.rowOf(kept, manage));
    browsers.sort((one, other) => Number(one.state === 'closed') - Number(other.state === 'closed') || Date.parse(other.last_used_at) - Date.parse(one.last_used_at));
    return {
      off: place.off,
      support: this.support(environmentId).browser,
      presets: [{ name: 'headless', available: true, reason: null }, { name: 'window', available: place.window, reason: place.window ? null : 'No display server was found here, so a browser cannot have a window.' }],
      profiles: [...place.profiles.values()].sort((one, other) => one.name.localeCompare(other.name)),
      profiles_dir: manage ? '/home/orch/.orch/browsers' : null,
      browsers,
      limit: { open: browsers.filter((browser) => browser.state !== 'closed' && browser.origin === 'started').length, max: 4 },
    };
  }

  private rowOf(kept: Kept, manage: boolean): Browser {
    const now = Date.now();
    const driving = [...kept.drove.entries()].filter(([, at]) => now - at < DRIVING_FOR_MS).map(([username]) => this.world.people.get(username)).flatMap((person) => (person === undefined ? [] : [userRef(person.me)]));
    return { ...kept.row, endpoint: manage ? kept.row.endpoint : null, tabs: kept.tabs.length, watching: kept.viewers.size, driving };
  }

  handle(person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>): Answer | null {
    let found: RegExpExecArray | null;
    if ((found = /^\/environments\/([^/]+)\/browsers$/.exec(path)) !== null) {
      const environmentId = found[1] ?? '';
      if (method === 'GET') {
        this.may(person, environmentId, 'browser.view');
        return { status: 200, body: this.listOf(person, environmentId) };
      }
      if (method === 'POST') {
        this.may(person, environmentId, 'browser.manage');
        const place = this.places.get(environmentId);
        if (place?.off !== null && place?.off !== undefined) throw new Refusal(403, 'browser.not_allowed', 'browsers are off here', { reason: place.off });
        if (place === undefined || place.programs.length === 0) throw new Refusal(403, 'browser.not_allowed', 'nothing here can run a browser', { reason: 'unsupported' });
        const profile = typeof body.profile === 'string' ? body.profile : '';
        if (!/^[a-z0-9][a-z0-9._-]{0,62}$/.test(profile)) throw new Refusal(400, 'request.invalid', 'a profile is named as its directory', { errors: [] });
        const known = place.profiles.get(profile);
        if (known === undefined && body.create !== true) throw new Refusal(404, 'not_found', 'no such profile');
        const running = known?.browser === null || known?.browser === undefined ? undefined : this.browsers.get(known.browser);
        if (running !== undefined && running.row.state !== 'closed') return { status: 200, body: this.rowOf(running, true) };
        const url = typeof body.url === 'string' && body.url !== '' ? body.url : 'about:blank';
        const threadId = query.get('thread');
        const kept = this.start(environmentId, profile, userRef(person.me), threadId === null ? null : { id: threadId, kind: 'thread', name: this.world.places.rowOf(threadId)?.name ?? null, redacted: false }, url, url === 'about:blank' ? 'New tab' : url);
        if (typeof body.preset === 'string') kept.row = { ...kept.row, preset: body.preset === 'window' ? 'window' : 'headless' };
        return { status: 201, body: this.rowOf(kept, true) };
      }
    }
    if ((found = /^\/browsers\/([^/]+)$/.exec(path)) !== null && method === 'DELETE') {
      const kept = this.browsers.get(found[1] ?? '');
      if (kept === undefined) throw new Refusal(404, 'not_found', 'no such browser');
      this.may(person, kept.row.environment.id, 'browser.manage');
      this.end(kept, 'quit');
      return { status: 200, body: this.rowOf(kept, true) };
    }
    if ((found = /^\/browsers\/([^/]+)\/ticket$/.exec(path)) !== null && method === 'POST') {
      const kept = this.browsers.get(found[1] ?? '');
      if (kept === undefined) throw new Refusal(404, 'not_found', 'no such browser');
      const control = body.control === true;
      this.may(person, kept.row.environment.id, control ? 'browser.manage' : 'browser.view');
      const ticket = randomUUID();
      this.tickets.set(ticket, { browser: kept.row.id, user: person.me.username, control, expires: Date.now() + TICKET_MS });
      return { status: 200, body: { ticket, ticket_expires_at: new Date(Date.now() + TICKET_MS).toISOString() } };
    }
    if ((found = /^\/environments\/([^/]+)\/browser-profiles(?:\/([^/]+))?$/.exec(path)) !== null) {
      const environmentId = found[1] ?? '';
      const name = found[2] === undefined ? null : decodeURIComponent(found[2]);
      this.may(person, environmentId, 'browser.manage');
      const place = this.places.get(environmentId);
      if (place === undefined) throw new Refusal(404, 'not_found', 'no such environment');
      if (name === null && method === 'POST') {
        const made = typeof body.name === 'string' ? body.name : '';
        if (place.profiles.has(made)) throw new Refusal(409, 'conflict.exists', 'a profile has this name');
        const profile: BrowserProfile = { name: made, size_bytes: 0, used_at: null, browser: null };
        place.profiles.set(made, profile);
        return { status: 201, body: profile };
      }
      const profile = name === null ? undefined : place.profiles.get(name);
      if (profile === undefined || name === null) throw new Refusal(404, 'not_found', 'no such profile');
      if (profile.browser !== null && this.browsers.get(profile.browser)?.row.state !== 'closed') throw new Refusal(409, 'browser.profile_in_use', 'a browser runs on this profile');
      if (method === 'PATCH') {
        const next = typeof body.name === 'string' ? body.name : '';
        if (place.profiles.has(next)) throw new Refusal(409, 'conflict.exists', 'a profile has this name');
        place.profiles.delete(name);
        const renamed = { ...profile, name: next, browser: null };
        place.profiles.set(next, renamed);
        return { status: 200, body: renamed };
      }
      if (method === 'DELETE') {
        if (query.get('confirm') !== name) throw new Refusal(400, 'request.invalid', 'type the name of the profile to delete it', { errors: [] });
        place.profiles.delete(name);
        return { status: 204 };
      }
    }
    return null;
  }

  control(path: string, body: Record<string, unknown>): Answer | null {
    const kept = typeof body.browser === 'string' ? this.browsers.get(body.browser) : [...this.browsers.values()].find((one) => one.row.profile === body.profile);
    switch (path) {
      case '/list': {
        const environmentId = typeof body.environment === 'string' ? body.environment : RUNS_ON.workspace;
        return { status: 200, body: [...this.browsers.values()].filter((one) => one.row.environment.id === environmentId).map((one) => ({ id: one.row.id, profile: one.row.profile, state: one.row.state })) };
      }
      case '/received':
        return { status: 200, body: kept?.received ?? [] };
      case '/place': {
        const environmentId = typeof body.environment === 'string' ? body.environment : RUNS_ON.workspace;
        const place = this.places.get(environmentId);
        if (place === undefined) return { status: 404, body: { detail: 'no such environment' } };
        if ('off' in body) place.off = (body.off ?? null) as BrowsersOff | null;
        if (Array.isArray(body.programs)) place.programs = body.programs as Place['programs'];
        if (typeof body.sandbox === 'boolean') place.sandbox = body.sandbox;
        if (typeof body.window === 'boolean') place.window = body.window;
        return { status: 204 };
      }
      default:
    }
    if (kept === undefined) return path.startsWith('/') ? { status: 404, body: { detail: 'no such browser' } } : null;
    const tab = kept.tabs.find((one) => one.active)?.id ?? '';
    switch (path) {
      case '/dialog':
        this.toDrivers(kept, { op: 'dialog', tab, kind: typeof body.kind === 'string' ? body.kind : 'confirm', message: typeof body.message === 'string' ? body.message : 'Leave this page?', default: typeof body.default === 'string' ? body.default : null });
        return { status: 204 };
      case '/chooser':
        this.toDrivers(kept, { op: 'chooser', tab, multiple: body.multiple === true });
        return { status: 204 };
      case '/quality':
        this.toAll(kept, { op: 'quality', level: typeof body.level === 'number' ? body.level : 3, why: 'link' });
        return { status: 204 };
      case '/open-tab': {
        for (const one of kept.tabs) one.active = false;
        kept.tabs.push({ id: randomUUID(), title: typeof body.title === 'string' ? body.title : 'Sign in', url: typeof body.url === 'string' ? body.url : 'https://accounts.example.org/', loading: false, active: true, opener: tab });
        this.changed(kept);
        return { status: 204 };
      }
      case '/end':
        this.end(kept, typeof body.reason === 'string' ? body.reason : 'closed_by_agent');
        return { status: 204 };
      case '/change':
        this.changed(kept);
        return { status: 204 };
      default:
        return null;
    }
  }

  // ---------------------------------------------------------------- the socket

  attach(socket: WebSocket, browserId: string, ticketValue: string): void {
    const ticket = this.tickets.get(ticketValue);
    this.tickets.delete(ticketValue);
    const kept = this.browsers.get(browserId);
    const person = this.world.people.get(ticket?.user ?? '');
    if (ticket?.browser !== browserId || ticket.expires < Date.now() || kept === undefined || person === undefined) {
      socket.close(4401);
      return;
    }
    if (kept.row.state === 'closed') {
      socket.send(JSON.stringify({ op: 'closed', reason: kept.row.close_reason ?? 'quit' }));
      socket.close(1000);
      return;
    }
    const viewer: Viewer = { socket, person, control: ticket.control, tab: null, inFlight: null, seen: -1 };
    kept.viewers.add(viewer);
    socket.send(JSON.stringify({ op: 'ready', control: viewer.control, tabs: kept.tabs, viewport: kept.viewport, people: this.people(kept) }));
    this.toAll(kept, { op: 'people', ...this.people(kept) });
    socket.on('message', (data, binary) => {
      if (binary) return;
      let frame: Record<string, unknown>;
      try {
        frame = JSON.parse(Buffer.from(data as Buffer).toString('utf8')) as Record<string, unknown>;
      } catch {
        return;
      }
      this.receive(kept, viewer, frame);
    });
    socket.on('close', () => {
      kept.viewers.delete(viewer);
      this.toAll(kept, { op: 'people', ...this.people(kept) });
    });
  }

  private receive(kept: Kept, viewer: Viewer, frame: Record<string, unknown>): void {
    const op = frame.op;
    if (op === 'ping') {
      viewer.socket.send(JSON.stringify({ op: 'pong', n: frame.n }));
      return;
    }
    if (op === 'watch') {
      viewer.tab = typeof frame.tab === 'string' ? frame.tab : null;
      viewer.seen = -1;
      this.picture(kept, viewer);
      return;
    }
    if (op === 'ack') {
      if (viewer.inFlight === frame.n) viewer.inFlight = null;
      this.picture(kept, viewer);
      return;
    }
    if (!viewer.control) {
      kept.received.push({ by: viewer.person.me.username, control: false, dropped: true, frame });
      viewer.socket.send(JSON.stringify({ op: 'error', code: 'view_only', detail: 'this socket only watches' }));
      return;
    }
    kept.received.push({ by: viewer.person.me.username, control: true, dropped: false, frame });
    kept.drove.set(viewer.person.me.username, Date.now());
    kept.row = { ...kept.row, last_used_at: new Date().toISOString() };
    const tab = kept.tabs.find((one) => one.id === frame.tab) ?? kept.tabs.find((one) => one.active);
    switch (op) {
      case 'navigate':
        if (tab !== undefined && typeof frame.url === 'string') {
          tab.url = frame.url;
          tab.title = frame.url.replace(/^https?:\/\//, '');
          this.changed(kept);
        }
        break;
      case 'tab':
        if (frame.do === 'new') {
          for (const one of kept.tabs) one.active = false;
          kept.tabs.push({ id: randomUUID(), title: 'New tab', url: typeof frame.url === 'string' ? frame.url : 'about:blank', loading: false, active: true, opener: null });
        } else if (frame.do === 'close') {
          kept.tabs = kept.tabs.filter((one) => one.id !== frame.tab);
          if (kept.tabs.length > 0 && !kept.tabs.some((one) => one.active)) kept.tabs[0]!.active = true;
        } else if (frame.do === 'activate') {
          for (const one of kept.tabs) one.active = one.id === frame.tab;
        }
        this.changed(kept);
        break;
      case 'viewport':
        if (typeof frame.width === 'number' && typeof frame.height === 'number') {
          kept.viewport = { width: Math.round(frame.width), height: Math.round(frame.height) };
          this.changed(kept);
        }
        break;
      case 'copy':
        viewer.socket.send(JSON.stringify({ op: 'copied', text: 'Roadmap for the quarter' }));
        break;
      case 'dialog':
        this.toDrivers(kept, { op: 'dialog_closed', tab: frame.tab });
        break;
      case 'pointer':
      case 'key':
      case 'text':
        this.changed(kept);
        break;
      default:
    }
    this.toAll(kept, { op: 'people', ...this.people(kept) });
  }

  private people(kept: Kept) {
    const now = Date.now();
    const watching = [...new Map([...kept.viewers].map((viewer) => [viewer.person.me.username, userRef(viewer.person.me)])).values()];
    const driving = [...kept.drove.entries()].filter(([, at]) => now - at < DRIVING_FOR_MS).flatMap(([username]) => { const person = this.world.people.get(username); return person === undefined ? [] : [userRef(person.me)]; });
    return { watching, driving };
  }

  /** The page changed: tabs are said again, and each viewer is sent the newest picture when it has none in flight. */
  private changed(kept: Kept): void {
    kept.change += 1;
    this.toAll(kept, { op: 'tabs', tabs: kept.tabs });
    for (const viewer of kept.viewers) this.picture(kept, viewer);
  }

  private picture(kept: Kept, viewer: Viewer): void {
    if (viewer.tab === null || viewer.inFlight !== null || viewer.seen === kept.change || viewer.socket.readyState !== 1) return;
    this.frameNumber += 1;
    viewer.inFlight = this.frameNumber;
    viewer.seen = kept.change;
    const jpeg = jpegOf(PICTURE.width, PICTURE.height, pageLevels(kept.change));
    const header = Buffer.alloc(12);
    header.writeUInt32BE(this.frameNumber, 0);
    header.writeUInt16BE(kept.viewport.width, 4);
    header.writeUInt16BE(kept.viewport.height, 6);
    header.writeUInt16BE(PICTURE.width, 8);
    header.writeUInt16BE(PICTURE.height, 10);
    viewer.socket.send(Buffer.concat([header, Buffer.from(jpeg)]), { binary: true });
  }

  private toAll(kept: Kept, frame: object): void {
    for (const viewer of kept.viewers) if (viewer.socket.readyState === 1) viewer.socket.send(JSON.stringify(frame));
  }

  private toDrivers(kept: Kept, frame: object): void {
    for (const viewer of kept.viewers) if (viewer.control && viewer.socket.readyState === 1) viewer.socket.send(JSON.stringify(frame));
  }

  private end(kept: Kept, reason: string): void {
    const now = new Date().toISOString();
    kept.row = { ...kept.row, state: 'closed', close_reason: reason, closed_at: now };
    const place = this.places.get(kept.row.environment.id);
    const profile = kept.row.profile === null ? undefined : place?.profiles.get(kept.row.profile);
    if (place !== undefined && profile !== undefined && kept.row.profile !== null) place.profiles.set(kept.row.profile, { ...profile, browser: null });
    for (const viewer of kept.viewers) {
      if (viewer.socket.readyState === 1) viewer.socket.send(JSON.stringify({ op: 'closed', reason }));
      viewer.socket.close(1000);
    }
    kept.viewers.clear();
  }
}
