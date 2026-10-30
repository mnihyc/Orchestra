/** The fake API as a server: the requests the screens before sign-in and the workbench make, answered from memory, and the live connection that keeps a tree current.
 *
 * Run it by itself to look at the application without a real system behind it:
 *
 *     node tests/doubles/server.ts            (listens on 127.0.0.1:41871)
 *     ORCH_DEV_API=http://127.0.0.1:41871 npm run dev
 *
 * The end-to-end tests start it the same way and steer it through the requests under `/__double`,
 * which no real system has: put it into a scenario, read what it "mailed", change a tree.
 */
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import { randomUUID } from 'node:crypto';

import { WebSocketServer, type WebSocket } from 'ws';
import type { Job } from '../../src/generated/api-types.ts';

import { type SceneName, sceneOf } from './thread-scenes.ts';
import { type Answer, type Person, Refusal, type Subscriber, World } from './world.ts';
import { bootstrap } from './bootstrap.ts';
import { threadOpening } from './thread-opening.ts';
import { chatOpening, nodeChatOpening, documentPages } from './screen-openings.ts';

const BASE = '/api/v1';
const HOST = '127.0.0.1';
const PORT = Number(process.env.ORCH_DOUBLE_PORT ?? 41871);

const world = new World(process.argv.includes('--fresh') ? { scenario: 'fresh' } : {});
const requestIds = new WeakMap<IncomingMessage, string>();
const receivedRequests: { id: string; method: string; path: string; body: Record<string, unknown>; reply?: { status: number; body: unknown } }[] = [];

function cookiesOf(request: IncomingMessage): Record<string, string> {
  const jar: Record<string, string> = {};
  for (const part of (request.headers.cookie ?? '').split(';')) {
    const at = part.indexOf('=');
    if (at > 0) jar[part.slice(0, at).trim()] = part.slice(at + 1).trim();
  }
  return jar;
}

async function bodyOf(request: IncomingMessage): Promise<Record<string, unknown>> {
  const chunks: Buffer[] = [];
  for await (const chunk of request) chunks.push(chunk as Buffer);
  try {
    const value: unknown = chunks.length === 0 ? {} : JSON.parse(Buffer.concat(chunks).toString('utf8'));
    const body = typeof value === 'object' && value !== null ? (value as Record<string, unknown>) : {};
    const path = new URL(request.url ?? '/', `http://${HOST}`).pathname;
    // A browser cannot expose a streamed upload's body to Playwright. Keep what the server
    // actually read, correlated with its response, so concurrent writes stay distinguishable.
    if (path.startsWith(`${BASE}/`)) receivedRequests.push({ id: requestIds.get(request) ?? '', method: request.method ?? 'GET', path: path.slice(BASE.length), body });
    return body;
  } catch {
    throw new Refusal(400, 'request.invalid', 'the body is not JSON', { errors: [] });
  }
}

function send(response: ServerResponse, answer: Answer, requestId: string): void {
  const headers: Record<string, string | string[]> = { 'X-Request-Id': requestId, 'Cache-Control': 'no-store', ...answer.headers };
  const cookies = Object.entries(answer.cookies ?? {}).map(([name, value]) =>
    value === null
      ? `${name}=; Max-Age=0; Path=/; HttpOnly; Secure; SameSite=Strict`
      : `${name}=${value}; Max-Age=2592000; Path=/; HttpOnly; Secure; SameSite=Strict`,
  );
  if (cookies.length > 0) headers['Set-Cookie'] = cookies;
  if (answer.text !== undefined) {
    headers['Content-Type'] = 'text/plain; charset=utf-8';
    response.writeHead(answer.status, headers).end(answer.text);
    return;
  }
  if (answer.bytes !== undefined) {
    response.writeHead(answer.status, headers).end(answer.bytes);
    return;
  }
  if (answer.body === undefined) {
    response.writeHead(answer.status, headers).end();
    return;
  }
  const problem = answer.status >= 400;
  const body = problem ? { ...(answer.body as object), request_id: requestId } : answer.body;
  headers['Content-Type'] = problem ? 'application/problem+json' : 'application/json';
  const json = JSON.stringify(body);
  // Navigation can retire Chromium's copy of an auth or opening response before the test reads it.
  // Keep the exact JSON bytes' value with the same request receipt as its uploaded body.
  const received = receivedRequests.find((entry) => entry.id === requestId);
  if (received !== undefined) received.reply = { status: answer.status, body: JSON.parse(json) as unknown };
  response.writeHead(answer.status, headers).end(json);
}

/** The requests of the API that the double answers. Anything else is, honestly, not there. */
async function api(request: IncomingMessage, path: string, query: URLSearchParams, requestId: string): Promise<Answer> {
  const method = request.method ?? 'GET';
  const csrf = request.headers['x-orch-csrf'];
  const csrfHeader = Array.isArray(csrf) ? csrf[0] : csrf;
  const route = `${method} ${path}`;
  const bearer = request.headers.authorization;

  switch (route) {
    case 'GET /meta':
      return { status: 200, body: world.metaFor(bearer) };
    case 'POST /setup':
      return world.setup(await bodyOf(request));
    case 'POST /auth/login':
      return world.login(await bodyOf(request));
    case 'POST /auth/email-code':
      return world.emailCode(await bodyOf(request));
    case 'POST /auth/refresh':
      return world.refresh(cookiesOf(request), csrfHeader);
    case 'POST /auth/logout':
      return world.logout(cookiesOf(request), csrfHeader);
    case 'POST /auth/register':
      return world.register(await bodyOf(request));
    case 'GET /auth/name-availability':
      return world.nameAvailability(query.get('name') ?? '');
    case 'POST /auth/recovery':
      return world.recovery(await bodyOf(request));
    case 'POST /auth/recovery/confirm':
      return world.recoveryConfirm(await bodyOf(request));
    case 'POST /auth/verify-email':
      return world.verifyEmail(await bodyOf(request));
    case 'POST /auth/verify-email/resend':
      return world.verifyResend(await bodyOf(request));
    case 'GET /me':
      return { status: 200, body: world.caller(bearer).me };
    case 'GET /me/admin-mode':
      return { status: 200, body: world.adminMode(bearer) };
    case 'GET /me/authority':
      return { status: 200, body: { person: world.caller(bearer).me, admin_mode: world.adminMode(bearer),
        orgs: world.orgsOf(world.browserPerson(bearer)), meta: world.metaFor(bearer) } };
    case 'PUT /me/admin-mode':
      return world.setAdminMode(bearer, await bodyOf(request));
    case 'POST /me/admin-openings':
      return world.openings.cached(bearer, await bodyOf(request), requestId);
    case 'GET /me/bootstrap': {
      const person = world.caller(bearer);
      const zone = request.headers['x-orch-timezone'];
      if (typeof zone === 'string' && person.me.timezone_mode === 'device' && person.me.timezone !== zone) world.updateMe(person, { timezone: zone, timezone_mode: 'device', timezone_report: true });
      return { status: 200, body: await bootstrap(world, person, bearer) };
    }
    case 'PATCH /me':
      return world.updateMe(world.caller(bearer), await bodyOf(request));
    case 'POST /invitations/accept':
      return world.accept(world.caller(bearer), await bodyOf(request));
    case 'GET /orgs':
      return { status: 200, body: world.orgsOf(world.browserPerson(bearer)) };
    default:
  }

  const opening = /^GET \/threads\/([^/]+)\/opening$/.exec(route);
  if (opening?.[1] !== undefined) {
    receivedRequests.push({ id: requestId, method, path, body: {} });
    return { status: 200, body: await threadOpening(world, world.browserPerson(bearer), opening[1], Number(query.get('limit') ?? 200)) };
  }
  const chat = /^GET \/chats\/([^/]+)\/opening$/.exec(route);
  if (chat?.[1] !== undefined) return { status: 200, body: await chatOpening(world, world.browserPerson(bearer), chat[1], query) };
  const scopes = /^GET \/nodes\/([^/]+)\/chat-opening$/.exec(route);
  if (scopes?.[1] !== undefined) return { status: 200, body: await nodeChatOpening(world, world.browserPerson(bearer), scopes[1], query) };
  const documents = /^GET \/nodes\/([^/]+)\/document-pages$/.exec(route);
  if (documents?.[1] !== undefined) return { status: 200, body: await documentPages(world, world.browserPerson(bearer), documents[1], query) };

  const recovery = /^GET \/auth\/recovery\/([^/]+)$/.exec(route);
  if (recovery?.[1] !== undefined) return world.recoveryCheck(decodeURIComponent(recovery[1]));

  const lookup = /^GET \/invitations\/lookup\/([^/]+)$/.exec(route);
  if (lookup?.[1] !== undefined) return world.lookup(decodeURIComponent(lookup[1]));

  const bySlug = /^GET \/orgs\/by-slug\/([^/]+)$/.exec(route);
  if (bySlug?.[1] !== undefined) return world.orgBySlug(world.browserPerson(bearer), decodeURIComponent(bySlug[1]));

  const tree = /^GET \/orgs\/([^/]+)\/tree$/.exec(route);
  if (tree?.[1] !== undefined) {
    const since = query.get('since');
    const person = world.browserPerson(bearer);
    const reply = world.tree(person, tree[1], since === null ? null : Number(since), query);
    if (reply.status === 200 && since === null && !query.has('cursor')) {
      try {
        const settings = await world.requestFeeds.read(person, `/nodes/${tree[1]}/settings`);
        return { ...reply, body: { ...(reply.body as object), settings: settings.body } };
      } catch (error) { if (!(error instanceof Refusal) || error.answer.status !== 403) throw error; }
    }
    return reply;
  }

  const start = /^GET \/auth\/oidc\/([^/]+)\/start$/.exec(route);
  if (start !== null) {
    // No identity provider stands behind the double: every sign-in through one comes back refused.
    return { status: 302, headers: { Location: '/login?error=denied' } };
  }

  // Everything else is the world's parts', in their order (resources, threads, chats, places): each answers what is its own.
  const person = world.browserPerson(bearer);
  // A part that takes a body that is not JSON (a file's bytes) reads the request itself, before anything reads it as JSON.
  for (const { part } of world.parts) {
    const answer = (await part.raw?.(person, request, method, path, query)) ?? null;
    if (answer !== null) return answer;
  }
  const given = method === 'GET' ? {} : await bodyOf(request);
  for (const { part } of world.parts) {
    const header = request.headers['idempotency-key'];
    const answer = (await part.handle?.(person, method, path, query, given, typeof header === 'string' ? header : undefined, requestId)) ?? null;
    if (answer !== null) return answer;
  }
  // A request of the real API that the double does not play is still a request that needs somebody signed in.
  throw new Refusal(404, 'not_found', `the double does not answer ${route}`);
}

/** The requests only the double has, by which a test steers it. */
async function control(request: IncomingMessage, path: string): Promise<Answer> {
  const body = await bodyOf(request);
  switch (path) {
    case '/reset':
      // Nobody is who they were: every live connection ends, and each client finds out when it comes back.
      for (const socket of sockets) socket.close(1012);
      receivedRequests.length = 0;
      world.reset(body);
      return { status: 204 };
    case '/requests':
      return { status: 200, body: receivedRequests };
    case '/outbox':
      return { status: 200, body: world.outbox };
    case '/invitation/chat':
      return world.inviteToChat(typeof body.token === 'string' ? body.token : '',typeof body.chat === 'string' ? body.chat : '',typeof body.as === 'string' ? body.as : 'ada');
    case '/tree': {
      const orgId = typeof body.org === 'string' ? body.org : '';
      const nodes = Array.isArray(body.nodes) ? (body.nodes as { id: string }[]) : [];
      return { status: 200, body: { seq: world.changeTree(orgId, nodes, body.lose === true) } };
    }
    case '/deny': {
      // Takes permissions away from somebody without telling their page: what a stale page would still believe.
      const username = typeof body.username === 'string' ? body.username : '';
      const permissions = Array.isArray(body.permissions) ? body.permissions.filter((one): one is string => typeof one === 'string') : [];
      world.places.denied.set(username, new Set(permissions));
      return { status: 204 };
    }
    case '/mail':
      world.places.mailWorks = body.works === true;
      return { status: 204 };
    case '/drop':
      // Every live connection ends as it does when the real server restarts.
      for (const socket of sockets) socket.close(1012);
      return { status: 204 };
    case '/thread/emit': {
      // Rows appear in a thread as if the system made them; `lose` keeps them and sends nothing.
      const threadId = typeof body.thread === 'string' ? body.thread : '';
      return { status: 200, body: { seq: world.threads.emit(threadId, body.rows ?? {}, body.lose === true) } };
    }
    case '/thread/scene': {
      // A whole conversation at once, from the scenes the doubles know by name.
      const threadId = typeof body.thread === 'string' ? body.thread : '';
      const played = sceneOf(body.name as SceneName, threadId, typeof body.count === 'number' ? body.count : undefined);
      for (const approval of played.approvals) world.threads.addApproval(approval);
      for (const [item, text] of Object.entries(played.outputs)) world.threads.wholeOutput.set(item, text);
      let seq = 0;
      for (const step of played.steps) seq = world.threads.emit(threadId, step);
      return { status: 200, body: { seq } };
    }
    case '/thread/delta': {
      const threadId = typeof body.thread === 'string' ? body.thread : '';
      world.threads.delta(threadId, body as unknown as Parameters<typeof world.threads.delta>[1]);
      return { status: 204 };
    }
    case '/thread/presence': {
      const threadId = typeof body.thread === 'string' ? body.thread : '';
      world.threads.setPresence(threadId, (body.users ?? []) as Parameters<typeof world.threads.setPresence>[1]);
      return { status: 204 };
    }
    case '/job/snapshot': {
      const job = body.job;
      if (typeof job !== 'object' || job === null || !('id' in job) || typeof job.id !== 'string') throw new Refusal(400, 'request.invalid', 'a controlled job snapshot needs its identity');
      world.resources.jobs.set(job.id, job as Job);
      await world.requestFeeds.changed();
      return { status: 204 };
    }
    case '/thread/set': {
      const threadId = typeof body.thread === 'string' ? body.thread : '';
      world.threads.configure(threadId, body);
      return { status: 204 };
    }
    case '/thread/close': {
      const threadId = typeof body.thread === 'string' ? body.thread : '';
      world.threads.closeFeed(threadId, body.reason === 'deleted' ? 'deleted' : 'revoked');
      return { status: 204 };
    }
    case '/thread/approval':
      world.threads.addApproval(body as unknown as Parameters<typeof world.threads.addApproval>[0]);
      return { status: 204 };
    case '/thread/output': {
      if (typeof body.item === 'string' && typeof body.text === 'string') world.threads.wholeOutput.set(body.item, body.text);
      return { status: 204 };
    }
    case '/thread/state': {
      const threadId = typeof body.thread === 'string' ? body.thread : '';
      return { status: 200, body: world.threads.snapshot(threadId) };
    }
    case '/thread/looked-at':
      return { status: 200, body: world.threads.visibility() };
    case '/model/entry': {
      // A manager of the provider writes a model's entry anew: what it offers, what it opens to threads, what a thread starts with.
      const modelName = typeof body.model_name === 'string' ? body.model_name : '';
      world.threads.setEntry(modelName, typeof body.display_name === 'string' ? body.display_name : '', body.parameters ?? {}, Array.isArray(body.groups) ? body.groups.map(String) : undefined);
      return { status: 204 };
    }
    case '/model/feature': {
      // The place switches a feature of its agents on or off: access programs are one.
      if (typeof body.feature === 'string') world.threads.features[body.feature] = body.on === true;
      return { status: 204 };
    }
    case '/end-sessions':
      // Every access token stops being good at once, as it does when its quarter of an hour is over.
      for (const held of world.tokens.values()) held.endsAt = 0;
      return { status: 204 };
    default:
      // What steers a part is under its name: `/__double/chats/say` is for the chats.
      for (const { name, part } of world.parts) {
        if (!path.startsWith(`/${name}/`)) continue;
        const answer = part.control?.(path.slice(name.length + 1), body) ?? null;
        if (answer !== null) return answer;
      }
      return { status: 404, body: { detail: `no such control: ${path}` } };
  }
}

const server = createServer((request, response) => {
  const requestId = randomUUID();
  requestIds.set(request, requestId);
  const url = new URL(request.url ?? '/', `http://${HOST}`);
  const answer = async (): Promise<Answer> => {
    if (url.pathname === '/health') return { status: 200, body: 'ok' };
    if (url.pathname.startsWith('/__double/')) return control(request, url.pathname.slice('/__double'.length));
    if (!url.pathname.startsWith(`${BASE}/`)) return { status: 404, body: { detail: 'not the API' } };
    if (world.delayMs > 0) await new Promise((resolve) => setTimeout(resolve, world.delayMs));
    const path = url.pathname.slice(BASE.length);
    const result = await api(request, path, url.searchParams, requestId);
    if (result.status >= 200 && result.status < 300) {
      world.openings.finishJson(request.headers.authorization, request.method ?? 'GET', path,
        url.searchParams, result.body, request.headers['x-orch-opening'], requestId);
    }
    const view = request.headers['x-orch-view'];
    if (request.method === 'GET' && result.status < 300 && typeof view === 'string' && request.headers.authorization !== undefined) {
      try {
        const { noteView } = await import('./view-read.ts');
        await noteView(world, world.browserPerson(request.headers.authorization), view, result.body);
      } catch { /* Personal bookkeeping never changes a successful native page answer. */ }
    }
    return result;
  };
  answer()
    .catch((error: unknown): Answer => {
      if (error instanceof Refusal) return error.answer;
      console.error(error);
      return {
        status: 500,
        body: { type: 'orch:error:internal', code: 'internal', title: 'Unexpected failure', status: 500, detail: 'see the log' },
      };
    })
    .then((result) => {
      if (result.status < 300 && ['POST', 'PUT', 'PATCH', 'DELETE'].includes(request.method ?? '')) void world.requestFeeds.changed();
      if (url.pathname === '/health') response.writeHead(200, { 'Content-Type': 'text/plain' }).end('ok');
      else send(response, result, requestId);
    })
    .catch((error: unknown) => console.error(error));
});

// ---------------------------------------------------------------- the live connection

const sockets = new Set<WebSocket>();
const sessionReaders = new Map<WebSocket, { session: string; changed: () => void }>();
world.sessionAuthorityChanged = (id, ended) => {
  for (const [socket, held] of sessionReaders) if (held.session === id) {
    if (ended) socket.close(4401);
    else held.changed();
  }
};
const live = new WebSocketServer({
  noServer: true,
  handleProtocols: (protocols) => (protocols.has('orch.v1') ? 'orch.v1' : false),
});

/** How long a connection may wait before it says who it is. */
const AUTH_WITHIN_MS = 10_000;

/** The sockets of the shells: one for each, with a protocol of their own. */
const shells = new WebSocketServer({ noServer: true, handleProtocols: (protocols) => (protocols.has('orch.terminal.v1') ? 'orch.terminal.v1' : false) });
/** The sockets of browsers and of displays. */
const browserSockets = new WebSocketServer({ noServer: true, handleProtocols: (protocols) => (protocols.has('orch.browser.v1') ? 'orch.browser.v1' : false) });
const displaySockets = new WebSocketServer({ noServer: true, handleProtocols: (protocols) => (protocols.has('orch.display.v1') ? 'orch.display.v1' : false) });

server.on('upgrade', (request, socket, head) => {
  const address = new URL(request.url ?? '/', `http://${HOST}`);
  const shell = new RegExp(`^${BASE}/ws/terminals/([^/]+)$`).exec(address.pathname);
  if (shell?.[1] !== undefined) {
    const id = shell[1];
    shells.handleUpgrade(request, socket, head, (connection) => world.workspace.terminals.attach(connection, id, address.searchParams.get('ticket') ?? ''));
    return;
  }
  // A browser's socket and a display's: each with its own subprotocol, as the real server has them.
  const browser = new RegExp(`^${BASE}/ws/browsers/([^/]+)$`).exec(address.pathname);
  if (browser?.[1] !== undefined) {
    const id = browser[1];
    browserSockets.handleUpgrade(request, socket, head, (connection) => world.browsers.attach(connection, id, address.searchParams.get('ticket') ?? ''));
    return;
  }
  const display = new RegExp(`^${BASE}/ws/displays/([^/]+)$`).exec(address.pathname);
  if (display?.[1] !== undefined) {
    const id = display[1];
    displaySockets.handleUpgrade(request, socket, head, (connection) => world.displays.attach(connection, id, address.searchParams.get('ticket') ?? ''));
    return;
  }
  if (address.pathname !== `${BASE}/ws`) {
    socket.destroy();
    return;
  }
  live.handleUpgrade(request, socket, head, (connection) => live.emit('connection', connection));
});

live.on('connection', (socket: WebSocket) => {
  if (socket.protocol !== 'orch.v1') {
    socket.close(4409);
    return;
  }
  sockets.add(socket);
  let user: string | null = null;
  let credential: string | null = null;
  let session: string | null = null;
  const subs = new Map<number, string>();
  const joined = new Map<number, string>();
  let pendingOpening: { feed: string; id: string; nonce: string } | null = null;
  const subscriber: Subscriber = {
    send: (frame) => {
      if (typeof frame === 'object' && frame !== null && 'op' in frame && 'id' in frame && typeof frame.id === 'number') {
        if (frame.op === 'subscribed' && 'feed' in frame && typeof frame.feed === 'string') {
          if (pendingOpening !== null && pendingOpening.feed === frame.feed) {
            world.openings.live(credential === null ? undefined : `Bearer ${credential}`, pendingOpening.id, pendingOpening.nonce);
          }
          joined.set(frame.id, frame.feed);
        }
        if (frame.op === 'closed') joined.delete(frame.id);
      }
      socket.send(JSON.stringify(frame));
    },
    close: (code) => socket.close(code),
  };
  const late = setTimeout(() => socket.close(4401), AUTH_WITHIN_MS);

  socket.on('message', (data) => {
    let frame: Record<string, unknown>;
    try {
      const text = Array.isArray(data) ? Buffer.concat(data).toString('utf8') : Buffer.from(data as ArrayBuffer).toString('utf8');
      frame = JSON.parse(text) as Record<string, unknown>;
    } catch {
      socket.close(4400);
      return;
    }
    if (frame.op === 'auth' || frame.op === 'reauth') {
      const person = typeof frame.token === 'string' ? world.tokenUser(frame.token) : null;
      if (person === null) {
        socket.close(4401);
        return;
      }
      const actor = world.browserPerson(`Bearer ${String(frame.token)}`);
      const bound = world.browserSessionRefs(actor)?.find((entry) => entry.current)?.id ?? null;
      if (user !== null && (user !== person.me.username || session !== bound)) {
        socket.close(4401);
        return;
      }
      clearTimeout(late);
      user = person.me.username;
      credential = String(frame.token);
      session = bound;
      if (bound !== null) sessionReaders.set(socket, { session: bound, changed: () => {
        const former = [...joined];
        joined.clear();
        for (const { part } of world.parts) part.gone?.(subscriber);
        for (const feed of subs.values()) world.subscribers.get(feed)?.delete(subscriber);
        subs.clear();
        for (const [id, feed] of former) subscriber.send({ op: 'resync', id, feed });
      } });
      if (frame.op === 'auth') {
        subscriber.send({
          op: 'ready',
          connection_id: randomUUID().slice(0, 8),
          user_id: person.me.id,
          server_time: new Date().toISOString(),
          access_expires_at: new Date(Date.now() + world.accessLifeMs).toISOString(),
          heartbeat_secs: 25,
          build: world.meta.build,
        });
      }
      return;
    }
    if (user === null) {
      socket.close(4401);
      return;
    }
    let person: Person;
    try { person = world.browserPerson(credential === null ? undefined : `Bearer ${credential}`); }
    catch { socket.close(4401); return; }
    // A part serves the feeds that are its own (a thread, a chat, the person's own); what none of them takes is the tree of an org.
    const taken = (): boolean => world.parts.some(({ part }) => part.frame?.(subscriber, person, frame) === true);
    switch (frame.op) {
      case 'ping':
        subscriber.send({ op: 'pong', n: frame.n });
        return;
      case 'sub': {
        const id = Number(frame.id);
        const feed = String(frame.feed);
        let opening: string;
        try { opening = frame.opening_id === undefined ? randomUUID() : world.openings.nonce(frame.opening_id, 'opening_id'); }
        catch (error) { if (!(error instanceof Refusal)) throw error; socket.close(4400); return; }
        const doc = /^doc:([0-9a-f-]+)$/i.exec(feed);
        pendingOpening = doc?.[1] === undefined ? null : { feed, id: doc[1], nonce: opening };
        try { if (taken()) return; }
        catch (error) {
          if (!(error instanceof Refusal)) throw error;
          for (const { part } of world.parts) part.gone?.(subscriber);
          socket.close(4403); return;
        }
        finally { pendingOpening = null; }
        const orgId = /^org:(.+)$/.exec(feed)?.[1];
        const tree = orgId === undefined ? undefined : world.trees.get(orgId);
        if (orgId === undefined || tree === undefined) {
          subscriber.send({ op: 'error', id, code: orgId === undefined ? 'bad_feed' : 'not_found', detail: feed });
          return;
        }
        if (!world.orgsOf(person).some((org) => org.id === orgId)) {
          subscriber.send({ op: 'error', id, code: 'forbidden', detail: feed });
          return;
        }
        subs.set(id, feed);
        let followers = world.subscribers.get(feed);
        if (followers === undefined) {
          followers = new Map();
          world.subscribers.set(feed, followers);
        }
        followers.set(subscriber, id);
        const since = typeof frame.since === 'number' ? frame.since : null;
        // A sequence from after what the tree holds means the tree was put back: the reader loads anew.
        const reload = since !== null && since > tree.seq;
        subscriber.send({ op: 'subscribed', id, feed, seq: tree.seq, mode: reload ? 'reload' : 'delta' });
        if (!reload && since !== null && since < tree.seq) {
          // What the reader may see of the rows that changed since: a row that is not theirs says only that it is gone.
          const rows = [...tree.nodes.values()].filter((node) => node.tree_seq > since);
          subscriber.send({ op: 'sync', id, feed, prev: since, seq: tree.seq, data: { nodes: world.places.rowsFor(person, rows, since) } });
        }
        return;
      }
      case 'unsub': {
        if (taken()) return;
        const id = Number(frame.id);
        const feed = subs.get(id);
        if (feed !== undefined) {
          subs.delete(id);
          world.subscribers.get(feed)?.delete(subscriber);
          subscriber.send({ op: 'closed', id, feed, reason: 'unsubscribed' });
        }
        return;
      }
      case 'visible':
      case 'typing':
        taken();
        return;
      default:
        socket.close(4400);
    }
  });

  socket.on('close', () => {
    clearTimeout(late);
    sockets.delete(socket);
    sessionReaders.delete(socket);
    for (const feed of subs.values()) world.subscribers.get(feed)?.delete(subscriber);
    for (const { part } of world.parts) part.gone?.(subscriber);
  });
});

server.listen(PORT, HOST, () => {
  console.warn(`fake API on http://${HOST}:${PORT} (scenario: ${world.meta.needs_setup ? 'fresh' : 'team'})`);
});

for (const signal of ['SIGINT', 'SIGTERM'] as const) {
  process.on(signal, () => {
    for (const socket of sockets) socket.terminate();
    server.close(() => process.exit(0));
  });
}
