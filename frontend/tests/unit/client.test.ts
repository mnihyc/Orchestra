// @vitest-environment node
/** The API client: the token renewed once and the refused request sent again, errors as typed values, the header of the cookie requests, and a wait that ends on silence alone. */
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ApiError, createApiClient, done, ok, ReceiptAcknowledged, RENEW_BEFORE_END_MS, repeatable } from '@/api/client';
import type { AuthResult, Me } from '@/generated/api-types';
import { NetworkError, retryRead } from '@/lib/requests';
import { beginOpening, openingId } from '@/lib/opening';
import { i18n, setLocale } from '@/i18n';
import { formatSpan } from '@/lib/time';
import { errorText } from '@/ui/errorText';
import type { HtmlReads } from '@/api/htmlReads';
import { createServerClock } from '@/lib/serverClock';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { runInNewContext } from 'node:vm';

const BASE = 'http://orch.test/api/v1';

const me = { id: 'u1', username: 'ada', display_name: 'Ada' } as Me;

function grant(token: string, lifeMs = 15 * 60_000): AuthResult {
  return { access_token: token, access_expires_at: new Date(Date.now() + lifeMs).toISOString(), user: me };
}

function json(status: number, body: unknown, headers: Record<string, string> = {}): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': status >= 400 ? 'application/problem+json' : 'application/json', ...headers },
  });
}

function problem(status: number, code: string, extra: Record<string, unknown> = {}, headers: Record<string, string> = {}): Response {
  return json(status, { type: `orch:error:${code}`, code, title: code, status, detail: code, request_id: 'req-1', ...extra }, headers);
}

/** A server that answers by a function of the request, and remembers what it was asked. */
function server(answer: (request: Request, seen: Request[]) => Response | Promise<Response>) {
  const seen: Request[] = [];
  const fetch = async (request: Request): Promise<Response> => {
    seen.push(request.clone());
    return answer(request, seen);
  };
  return { fetch, seen, paths: () => seen.map((request) => `${request.method} ${new URL(request.url).pathname.replace('/api/v1', '')}`) };
}

it('uses cookie discovery for the addressed thread even when this client holds a bearer', async () => {
  const { fetch, seen } = server(() => json(200, null));
  const client = createApiClient({ baseUrl: BASE, fetch });
  client.adopt(grant('previous-credential'));
  await client.api.POST('/auth/session/thread/{id}', { params: { path: { id: '01000000-0000-7000-8000-000000000001' } } });
  expect(seen[0]?.headers.get('x-orch-csrf')).toBe('1');
  expect(seen[0]?.headers.has('authorization')).toBe(false);
});

function htmlStart(fetch: (request: Request) => Promise<Response>, address = '/thread/one'): HtmlReads {
  const window: { __orchStartupReads?: HtmlReads; fetch: (path: string, options?: RequestInit) => Promise<Response>; location: URL; document: { querySelectorAll: () => never[] }; AbortController: typeof AbortController; Intl: typeof Intl; Date: typeof Date; TextDecoder: typeof TextDecoder } = {
    fetch: (path, options) => fetch(new Request(new URL(path, BASE), options)), location: new URL(address, BASE),
    document: { querySelectorAll: () => [] }, AbortController, Intl, Date, TextDecoder,
  };
  const script = readFileSync(resolve(process.cwd(), 'src/shell/startup.inline.js'), 'utf8');
  runInNewContext(`${script}(window,{common:[],pages:[]});`, { window, URL, Request, Response, Headers, AbortController, Promise, Date, Intl });
  if (window.__orchStartupReads === undefined) throw new Error('The HTML must own its early reads.');
  return window.__orchStartupReads;
}

/** A page's opening as the server streams it (POST /auth/session/open): the session's line now, each later line when given. */
function streamedOpening(session: unknown, headers: Record<string, string> = {}) {
  let push!: (line: unknown) => void, end!: () => void;
  const body = new ReadableStream<Uint8Array>({ start(controller) {
    push = (line) => { controller.enqueue(new TextEncoder().encode(`${JSON.stringify(line)}\n`)); };
    end = () => { controller.close(); };
  } });
  push({ session });
  return { response: new Response(body, { headers: { 'Content-Type': 'application/x-ndjson', ...headers } }), push, end };
}

/** A page's whole opening at once. */
function opened(session: unknown, ...lines: unknown[]): Response {
  const answer = streamedOpening(session);
  for (const line of lines) answer.push(line);
  answer.end();
  return answer.response;
}

const isOpening = (request: Request) => new URL(request.url).pathname.endsWith('/auth/session/open');

it('restores the reader and supplies the native thread opening through one document request', async () => {
  const id = '01000000-0000-7000-8000-000000000001';
  const session = grant('document-credential');
  const opening = { thread: { id } };
  const document = server(() => json(200, { session, opening: { status: 'ready', data: opening } }));
  const ordinary = server(() => problem(500, 'unexpected'));
  const startupReads = htmlStart(document.fetch, `/thread/${id}`);
  const client = createApiClient({ baseUrl: BASE, fetch: ordinary.fetch, startupReads });
  await expect(client.restore()).resolves.toBe(true);
  expect(client.accessToken()).toBe(session.access_token);
  expect(await ok(client.api.GET('/threads/{id}/opening', { params: { path: { id } } }))).toEqual(opening);
  expect(document.paths()).toEqual([`POST /auth/session/thread/${id}`]);
  expect(ordinary.paths()).toEqual([]);
  startupReads.cancel();
});

it('adopts the HTML session and bootstrap without another exchange or a false clock sample from script loading', async () => {
  vi.useFakeTimers(); vi.setSystemTime(0);
  const earlyGrant = grant('synthetic-html-token');
  const first = { person: me, state: { fromHtml: true } };
  const early = server(() => {
    const answer = streamedOpening(earlyGrant, { Date: new Date().toUTCString() });
    answer.push({ read: 0, status: 'ready', data: first }); answer.end();
    return answer.response;
  });
  const startupReads = htmlStart(early.fetch);
  await vi.waitFor(() => expect(early.paths()).toEqual(['POST /auth/session/open']));
  const fresh = { person: me, state: { fromExplicitRead: true } };
  const ordinary = server((request) => request.url.endsWith('/auth/session') ? json(200, grant('synthetic-unexpected-token')) : json(200, fresh));
  const clock = createServerClock(), observe = vi.spyOn(clock, 'observe');
  vi.setSystemTime(10_000);
  const options = { baseUrl: BASE, fetch: ordinary.fetch, clock, startupReads };
  const client = createApiClient(options);
  await expect(client.restore()).resolves.toBe(true);
  expect(client.accessToken()).toBe(earlyGrant.access_token);
  expect(await ok(client.api.GET('/me/bootstrap'))).toEqual(first);
  expect(ordinary.paths()).toEqual([]);
  expect(observe.mock.calls.every(([, sent, received]) => sent < 10_000 && received < 10_000)).toBe(true);
  // The one-shot handoff cannot serve a stale answer to a later explicit read.
  expect(await ok(client.api.GET('/me/bootstrap'))).toEqual(fresh);
  expect(ordinary.paths()).toEqual(['GET /me/bootstrap']);
});

it('shows the addressed storage totals from one authorized read while the shell answer is still pending', async () => {
  const admin = { ...grant('synthetic-storage'), user: { ...me, is_system_admin: true } };
  const totals = { blob_count: 817 };
  let finishBootstrap!: (shell: unknown) => void;
  const early = server((request) => {
    if (!isOpening(request)) return json(200, totals);
    const answer = streamedOpening(admin);
    finishBootstrap = (shell) => { answer.push({ read: 0, status: 'ready', data: shell }); answer.end(); };
    return answer.response;
  });
  const ordinary = server(() => json(200, { blob_count: 900 }));
  const client = createApiClient({ baseUrl: BASE, fetch: ordinary.fetch, startupReads: htmlStart(early.fetch, '/admin/storage') });
  await client.restore();
  await vi.waitFor(() => expect(early.paths()).toContain('GET /admin/storage'));
  expect(await ok(client.api.GET('/admin/storage'))).toEqual(totals);
  finishBootstrap({ initial: true });
  expect(await ok(client.api.GET('/me/bootstrap'))).toEqual({ initial: true });
  expect(ordinary.paths()).toEqual([]);
  expect(early.seen.find((req) => req.url.endsWith('/admin/storage'))?.headers.get('authorization')).toBe('Bearer synthetic-storage');
  expect(await ok(client.api.GET('/admin/storage'))).toEqual({ blob_count: 900 });
  expect(ordinary.paths()).toEqual(['GET /admin/storage']);
});

it('drops an unclaimed storage opening when the same person changes authority', async () => {
  const admin = { ...grant('synthetic-storage'), user: { ...me, is_system_admin: true } };
  const early = server((request) => isOpening(request) ? opened(admin, { read: 0, status: 'ready', data: {} }) : json(200, { blob_count: 817 }));
  const ordinary = server(() => problem(403, 'authz.denied'));
  const client = createApiClient({ baseUrl: BASE, fetch: ordinary.fetch, startupReads: htmlStart(early.fetch, '/admin/storage/') });
  await client.restore();
  await vi.waitFor(() => expect(early.paths()).toContain('GET /admin/storage'));
  client.session.setState({ readerVersion: (client.session.getState().readerVersion ?? 0) + 1 });
  await expect(ok(client.api.GET('/admin/storage'))).rejects.toMatchObject({ status: 403 });
  expect(ordinary.paths()).toEqual(['GET /admin/storage']);
});

it('leaves the storage read behind when navigating away and still adopts the original shell answer', async () => {
  const admin = { ...grant('synthetic-storage'), user: { ...me, is_system_admin: true } };
  const early = server((request) => isOpening(request) ? opened(admin, { read: 0, status: 'ready', data: { initial: true } }) : json(200, { initial: true }));
  const ordinary = server(() => json(200, { blob_count: 901 }));
  const opening = htmlStart(early.fetch, '/admin/storage');
  const client = createApiClient({ baseUrl: BASE, fetch: ordinary.fetch, startupReads: opening });
  await client.restore();
  await vi.waitFor(() => expect(early.paths()).toContain('GET /admin/storage'));
  opening.visit('/admin/overview');
  opening.visit('/admin/storage');
  expect(await ok(client.api.GET('/me/bootstrap'))).toEqual({ initial: true });
  expect(await ok(client.api.GET('/admin/storage'))).toEqual({ blob_count: 901 });
  expect(ordinary.paths()).toEqual(['GET /admin/storage']);
});

it('drops HTML replies when an explicit replacement arrives even for the same person', async () => {
  vi.useFakeTimers();
  let finish!: (shell: unknown) => void;
  const early = server(() => {
    const answer = streamedOpening(grant('synthetic-html-old'));
    finish = (shell) => { answer.push({ read: 0, status: 'ready', data: shell }); answer.end(); };
    return answer.response;
  });
  const startupReads = htmlStart(early.fetch);
  await vi.waitFor(() => expect(early.seen).toHaveLength(1));
  const ordinary = server(() => json(200, { person: me, state: { current: true } }));
  const options = { baseUrl: BASE, fetch: ordinary.fetch, startupReads };
  const client = createApiClient(options);
  client.adopt(grant('synthetic-explicit-replacement'));
  finish({ person: me, state: { formerPrivateChoice: true } });
  expect(await ok(client.api.GET('/me/bootstrap'))).toEqual({ person: me, state: { current: true } });
  expect(ordinary.seen[0]?.headers.get('Authorization')).toBe('Bearer synthetic-explicit-replacement');
});

it('keeps a refused HTML session factual and sends no bootstrap or automatic replacement request', async () => {
  vi.useFakeTimers();
  const early = server(() => problem(503, 'internal'));
  const startupReads = htmlStart(early.fetch);
  const ordinary = server(() => json(200, grant('synthetic-retry')));
  const options = { baseUrl: BASE, fetch: ordinary.fetch, startupReads };
  const client = createApiClient(options);
  await expect(client.restore()).rejects.toMatchObject({ status: 503 });
  expect(early.paths()).toEqual(['POST /auth/session/open']);
  expect(ordinary.paths()).toEqual([]);
  await expect(client.restore()).resolves.toBe(true);
  expect(ordinary.paths()).toEqual(['POST /auth/session']);
});

afterEach(async () => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  await setLocale('en');
});

it('records the actual renewal schedule, attempt and completion without borrowing a later grant’s timing', async () => {
  vi.useFakeTimers(); const now = Date.now(); let finish!: (reply: Response) => void;
  const { fetch, seen } = server(() => new Promise<Response>((resolve) => { finish = resolve; }));
  const client = createApiClient({ baseUrl: BASE, fetch }); client.adopt(grant('synthetic-old', RENEW_BEFORE_END_MS + 5_000));
  const original = client.session.getState();
  expect(original).toMatchObject({ renewal: { phase: 'scheduled', scheduled_at: now + 5_000, renewed_at: null } });
  await vi.advanceTimersByTimeAsync(4_999); expect(seen).toHaveLength(0);
  await vi.advanceTimersByTimeAsync(1); expect(seen).toHaveLength(1);
  expect(client.session.getState()).toMatchObject({ renewal: { phase: 'in_progress', scheduled_at: null } });
  finish(json(200, grant('synthetic-fresh'))); await vi.advanceTimersByTimeAsync(0);
  expect(client.session.getState()).toMatchObject({ renewal: { phase: 'completed', renewed_at: now + 5_000 } });
  expect(original).toMatchObject({ renewal: { scheduled_at: now + 5_000 } });
});

it('keeps a temporary renewal failure factual while the held session remains usable', async () => {
  vi.useFakeTimers(); const next = Date.now() + 15 * 60_000 - RENEW_BEFORE_END_MS;
  const { fetch, paths } = server(() => { throw new TypeError('Synthetic connection failed'); });
  const client = createApiClient({ baseUrl: BASE, fetch }); client.adopt(grant('synthetic-held'));
  await expect(client.renew()).rejects.toBeInstanceOf(NetworkError);
  expect(client.session.getState()).toMatchObject({ status: 'signed_in', userId: me.id, renewal: { phase: 'failed', scheduled_at: next, renewed_at: null } });
  expect(paths()).toEqual(['POST /auth/refresh']);
});

it('keeps a long admitted renewal deadline instead of letting the browser timer overflow renew immediately', async () => {
  vi.useFakeTimers(); const now = Date.now(), life = 35 * 24 * 60 * 60_000;
  const { fetch, seen } = server(() => json(200, grant('synthetic-later')));
  const client = createApiClient({ baseUrl: BASE, fetch }); client.adopt(grant('synthetic-long', life));
  const deadline = now + life - RENEW_BEFORE_END_MS;
  await vi.advanceTimersByTimeAsync(1); expect(seen).toHaveLength(0);
  await vi.advanceTimersByTimeAsync(2_147_483_647 - 1); expect(seen).toHaveLength(0);
  expect(client.session.getState()).toMatchObject({ renewal: { phase: 'scheduled', scheduled_at: deadline } });
  await vi.advanceTimersByTimeAsync(deadline - Date.now()); expect(seen).toHaveLength(1);
});

it('does not let a former renewal overwrite an explicit fresh grant for the same person', async () => {
  let finish!: (reply: Response) => void;
  const { fetch } = server(() => new Promise<Response>((resolve) => { finish = resolve; }));
  const client = createApiClient({ baseUrl: BASE, fetch }); client.adopt(grant('synthetic-former'));
  const old = client.renew(); await new Promise<void>((resolve) => setTimeout(resolve, 0));
  client.adopt(grant('synthetic-explicit'));
  finish(problem(401, 'auth.invalid_token')); await old;
  expect(client.accessToken()).toBe('synthetic-explicit');
  expect(client.session.getState()).toMatchObject({ status: 'signed_in', renewal: { phase: 'scheduled', renewed_at: null } });
});

it.each([false, true])('does not dispatch a former request through an explicit replacement after renewal waits (proactive=%s)', async (proactive) => {
  let finish!: (reply: Response) => void;
  const { fetch, paths } = server((request) => new URL(request.url).pathname.endsWith('/auth/refresh')
    ? new Promise<Response>((resolve) => { finish = resolve; }) : problem(401, 'auth.invalid_token'));
  const client = createApiClient({ baseUrl: BASE, fetch });
  client.adopt(grant('synthetic-former', proactive ? RENEW_BEFORE_END_MS - 1 : 15 * 60_000));
  const request = client.api.GET('/me'); await new Promise<void>((resolve) => setTimeout(resolve, 0));
  client.adopt(grant('synthetic-explicit'));
  finish(json(200, grant('synthetic-retired'))); await request;
  expect(paths()).toEqual(proactive ? ['POST /auth/refresh'] : ['GET /me', 'POST /auth/refresh']);
  expect(client.accessToken()).toBe('synthetic-explicit'); expect(client.session.getState()).toMatchObject({ status: 'signed_in' });
});

it.each(['expired', 'revoked', 'account_unavailable'] as const)('retains only a proven own %s fact and clears it on replacement', async (cause) => {
  const at = cause === 'account_unavailable' ? undefined : '2026-11-15T04:00:00Z';
  const { fetch } = server(() => problem(401, cause === 'account_unavailable' ? 'auth.suspended' : 'auth.invalid_token', {
    session_end: { cause, at_if_known: at, user_id: 'private-prior-user', diagnostic: 'private session data' },
  }));
  const client = createApiClient({ baseUrl: BASE, fetch }); client.adopt(grant('synthetic-held'));
  await expect(client.renew()).resolves.toBe(false);
  expect(client.session.getState()).toMatchObject({ status: 'signed_out', sessionEnded: true, sessionEnd: { cause, at_if_known: at ?? null } });
  expect(JSON.stringify(client.session.getState())).not.toMatch(/private-prior-user|private session data/);
  client.adopt({ ...grant('synthetic-next'), user: { ...me, id: 'next-end-reader' } });
  expect(client.session.getState()).toMatchObject({ status: 'signed_in', sessionEnded: false, sessionEnd: null });
});

it('keeps an unreported end generic and ignores unexpected end data on a request with no held reader', async () => {
  const plain = server(() => problem(401, 'auth.invalid_token'));
  const held = createApiClient({ baseUrl: BASE, fetch: plain.fetch }); held.adopt(grant('synthetic-held'));
  await held.renew(); expect(held.session.getState()).toMatchObject({ sessionEnded: true, sessionEnd: null });
  const publicWire = server(() => problem(401, 'auth.invalid_token', { session_end: { cause: 'revoked' } }));
  const visitor = createApiClient({ baseUrl: BASE, fetch: publicWire.fetch }); await visitor.api.GET('/me');
  expect(visitor.session.getState()).toMatchObject({ sessionEnded: false, sessionEnd: null }); expect(publicWire.seen).toHaveLength(1);
});

it('retains a proven bearer end when the later cookie response has no remaining session record', async () => {
  const at = '2026-11-15T04:00:00Z';
  const { fetch } = server((request) => new URL(request.url).pathname.endsWith('/auth/refresh') ? problem(401, 'auth.required')
    : problem(401, 'auth.invalid_token', { session_end: { cause: 'expired', at_if_known: at } }));
  const client = createApiClient({ baseUrl: BASE, fetch }); client.adopt(grant('synthetic-held')); await client.api.GET('/me');
  expect(client.session.getState()).toMatchObject({ status: 'signed_out', sessionEnd: { cause: 'expired', at_if_known: at } });
});

it('lets a replacement reader own renewal while the former reader’s renewal still waits', async () => {
  const replies: ((reply: Response) => void)[] = [];
  const { fetch } = server(() => new Promise<Response>((resolve) => replies.push(resolve)));
  const client = createApiClient({ baseUrl: BASE, fetch }); client.adopt(grant('synthetic-former'));
  const former = client.renew(); await new Promise<void>((resolve) => setTimeout(resolve, 0));
  client.adopt({ ...grant('synthetic-next'), user: { ...me, id: 'next-reader' } });
  const next = client.renew(); await new Promise<void>((resolve) => setTimeout(resolve, 0));
  try {
    expect(replies).toHaveLength(2);
    replies[0]?.(problem(401, 'auth.invalid_token')); await former;
    expect(client.session.getState()).toMatchObject({ userId: 'next-reader', renewal: { phase: 'in_progress' } });
    replies[1]?.(json(200, { ...grant('synthetic-complete'), user: { ...me, id: 'next-reader' } })); await next;
    expect(client.session.getState()).toMatchObject({ userId: 'next-reader', renewal: { phase: 'completed' } });
  } finally { for (const reply of replies) reply(json(200, grant('synthetic-final'))); await Promise.all([former, next]); }
});

it('shares one logical opening across HTTP reads and uses a fresh one for the next navigation', async () => {
  const { fetch, seen } = server(() => json(200, me));
  const client = createApiClient({ baseUrl: BASE, fetch });
  beginOpening();
  const first = openingId();
  await client.api.GET('/me');
  await client.api.GET('/me');
  expect(seen.map((request) => request.headers.get('x-orch-opening'))).toEqual([first, first]);
  beginOpening();
  await client.api.GET('/me');
  expect(seen[2]?.headers.get('x-orch-opening')).toBe(openingId());
  expect(openingId()).not.toBe(first);
});

describe('a token that is no longer good', () => {
  it('is renewed once, and the request that was refused is sent again with the new one', async () => {
    const { fetch, seen, paths } = server((request) => {
      const path = new URL(request.url).pathname;
      if (path.endsWith('/auth/refresh')) return json(200, grant('new'));
      return request.headers.get('Authorization') === 'Bearer new' ? json(200, me) : problem(401, 'auth.invalid_token');
    });
    const client = createApiClient({ baseUrl: BASE, fetch });
    client.adopt(grant('old'));

    await expect(ok(client.api.GET('/me'))).resolves.toMatchObject({ username: 'ada' });

    expect(paths()).toEqual(['GET /me', 'POST /auth/refresh', 'GET /me']);
    expect(seen[2]?.headers.get('Authorization')).toBe('Bearer new');
    expect(client.accessToken()).toBe('new');
  });

  it('is renewed once for every request that was refused with it', async () => {
    let renewals = 0;
    const { fetch } = server(async (request) => {
      const path = new URL(request.url).pathname;
      if (path.endsWith('/auth/refresh')) {
        renewals += 1;
        await new Promise((resolve) => setTimeout(resolve, 5));
        return json(200, grant('new'));
      }
      return request.headers.get('Authorization') === 'Bearer new' ? json(200, me) : problem(401, 'auth.invalid_token');
    });
    const client = createApiClient({ baseUrl: BASE, fetch });
    client.adopt(grant('old'));

    await Promise.all([ok(client.api.GET('/me')), ok(client.api.GET('/me')), ok(client.api.GET('/me'))]);

    expect(renewals).toBe(1);
  });

  it('signs the page out when the session is gone too, and gives the refusal back', async () => {
    const { fetch, paths } = server((request) =>
      new URL(request.url).pathname.endsWith('/auth/refresh') ? problem(401, 'auth.required') : problem(401, 'auth.invalid_token'),
    );
    const client = createApiClient({ baseUrl: BASE, fetch });
    const events: string[] = [];
    client.onSession((event) => events.push(event.type));
    client.adopt(grant('old'));

    await expect(ok(client.api.GET('/me'))).rejects.toMatchObject({ code: 'auth.invalid_token' });

    // One renewal, no second try of the request: a refusal is not asked about twice.
    expect(paths()).toEqual(['GET /me', 'POST /auth/refresh']);
    expect(client.session.getState().status).toBe('signed_out');
    expect(events).toEqual(['signed_in', 'signed_out']);
  });

  it('is renewed before it ends, so that the request is spared the refusal', async () => {
    const { fetch, paths } = server((request) =>
      new URL(request.url).pathname.endsWith('/auth/refresh') ? json(200, grant('new')) : json(200, me),
    );
    const client = createApiClient({ baseUrl: BASE, fetch });
    // Less of its life is left than the margin a renewal is given.
    client.adopt(grant('old', RENEW_BEFORE_END_MS / 2));

    await ok(client.api.GET('/me'));

    expect(paths().slice(-2)).toEqual(['POST /auth/refresh', 'GET /me']);
  });
});

describe('the two requests the session cookie authenticates', () => {
  it('carry the header that says they come from this page, and no token', async () => {
    const { fetch, seen } = server((request) =>
      new URL(request.url).pathname.endsWith('/auth/refresh') ? json(200, grant('new')) : new Response(null, { status: 204 }),
    );
    const client = createApiClient({ baseUrl: BASE, fetch });
    client.adopt(grant('held'));

    await client.renew();
    await client.signOut();

    for (const request of seen) {
      expect(request.headers.get('X-Orch-Csrf')).toBe('1');
      expect(request.headers.get('Authorization')).toBeNull();
    }
    expect(seen.map((request) => new URL(request.url).pathname)).toEqual(['/api/v1/auth/refresh', '/api/v1/auth/logout']);
  });

  it('leave every other request without that header, and with the token', async () => {
    const { fetch, seen } = server(() => json(200, me));
    const client = createApiClient({ baseUrl: BASE, fetch });
    client.adopt(grant('held'));

    await ok(client.api.GET('/me'));

    expect(seen[0]?.headers.get('X-Orch-Csrf')).toBeNull();
    expect(seen[0]?.headers.get('Authorization')).toBe('Bearer held');
  });
});

describe('an error of the API', () => {
  it('is a typed error with its code, what the code adds, when to try again and the request id', async () => {
    const { fetch } = server(() => problem(423, 'auth.locked', { way: 'password' }, { 'Retry-After': '120' }));
    const client = createApiClient({ baseUrl: BASE, fetch });
    const before = Date.now();

    const error = await ok(client.api.POST('/auth/login', { body: { login: 'ada', password: 'x' } })).catch((caught: unknown) => caught);

    expect(error).toBeInstanceOf(ApiError);
    const refused = error as ApiError;
    expect(refused.code).toBe('auth.locked');
    expect(refused.status).toBe(423);
    expect(refused.requestId).toBe('req-1');
    expect(refused.extra('auth.locked')).toEqual({ way: 'password' });
    expect(refused.extra('auth.second_step_required')).toBeNull();
    expect(refused.retryAt?.getTime()).toBeGreaterThanOrEqual(before + 120_000);
    expect(refused.retryAt?.getTime()).toBeLessThan(before + 125_000);
  });

  it('names the second step that is missing', async () => {
    const { fetch } = server(() => problem(401, 'auth.second_step_required', { methods: ['totp', 'recovery_code', 'email_code', 'smoke_signal'] }));
    const client = createApiClient({ baseUrl: BASE, fetch });

    const error = (await ok(client.api.POST('/auth/login', { body: { login: 'grace', password: 'x' } })).catch((caught: unknown) => caught)) as ApiError;

    // A proof this build does not know is left out, and the ones it knows are kept.
    expect(error.extra('auth.second_step_required')).toEqual({ methods: ['totp', 'recovery_code', 'email_code'] });
  });

  it('carries the faults of a request that was not valid, by the member each is about', async () => {
    const errors = [{ path: 'password', message: 'must be at least 10 characters' }, { broken: true }];
    const { fetch } = server(() => problem(400, 'request.invalid', { errors }));
    const client = createApiClient({ baseUrl: BASE, fetch });

    const error = (await ok(client.api.POST('/setup', { body: { username: 'a', display_name: 'A', password: 'b' } })).catch((caught: unknown) => caught)) as ApiError;

    expect(error.faults()).toEqual([{ path: 'password', message: 'must be at least 10 characters' }]);
  });

  it('takes the request id from the header when the body has none', async () => {
    const { fetch } = server(() => json(404, { code: 'not_found', status: 404, detail: 'no', title: 'no', type: 'x', request_id: '' }, { 'X-Request-Id': 'from-header' }));
    const client = createApiClient({ baseUrl: BASE, fetch });

    const error = (await ok(client.api.GET('/me')).catch((caught: unknown) => caught)) as ApiError;

    expect(error.requestId).toBe('from-header');
  });

  it('places a public-name conflict at its supplied field and invents no field for a private account conflict', async () => {
    const { fetch } = server(() => problem(409, 'conflict.exists', { errors: [
      { path: 'username', message: 'Choose a different name.' }, { path: 'email', message: 7 },
    ] }));
    const client = createApiClient({ baseUrl: BASE, fetch });
    const error = (await ok(client.api.POST('/auth/register', { body: {
      username: 'taken-name', display_name: 'New Person', password: 'a sufficiently long secret', accept_invitation: false,
    } })).catch((caught: unknown) => caught)) as ApiError;
    expect(error.faults()).toEqual([{ path: 'username', message: 'Choose a different name.' }]);
    const privateConflict = new ApiError({ type: 'orch:error:conflict.exists', code: 'conflict.exists', status: 409,
      title: 'Conflict', detail: 'The account change could not be completed.', request_id: 'private-conflict' }, null);
    expect(privateConflict.faults()).toEqual([]);
  });

  it('is told apart from an answer that is not the API at all', async () => {
    const { fetch } = server(() => new Response('<html>Bad gateway</html>', { status: 502, headers: { 'Content-Type': 'text/html' } }));
    const client = createApiClient({ baseUrl: BASE, fetch });

    const error = await ok(client.api.GET('/meta')).catch((caught: unknown) => caught);

    expect(error).toBeInstanceOf(NetworkError);
    expect(error).toMatchObject({ failure: 'malformed', status: 502 });
  });
});

describe('signing out', () => {
  it('keeps the session when the server could not be told', async () => {
    const { fetch } = server(() => {
      throw new TypeError('fetch failed');
    });
    const client = createApiClient({ baseUrl: BASE, fetch });
    client.adopt(grant('held'));

    await expect(client.signOut()).rejects.toBeInstanceOf(NetworkError);

    expect(client.session.getState().status).toBe('signed_in');
    expect(client.accessToken()).toBe('held');
  });

  it('forgets the token, which was never anywhere but in memory', async () => {
    const { fetch } = server(() => new Response(null, { status: 204 }));
    const client = createApiClient({ baseUrl: BASE, fetch });
    client.adopt(grant('held'));

    await client.signOut();

    expect(client.accessToken()).toBeNull();
    expect(client.session.getState()).toMatchObject({ status: 'signed_out', userId: null });
  });
});

describe('a page that was just loaded', () => {
  it('finds its session with one request, however many ask at once', async () => {
    const { fetch, paths } = server(() => json(200, grant('restored')));
    const client = createApiClient({ baseUrl: BASE, fetch });

    const answers = await Promise.all([client.restore(), client.restore(), client.restore()]);

    expect(answers).toEqual([true, true, true]);
    expect(paths()).toEqual(['POST /auth/session']);
    expect(client.session.getState().userId).toBe('u1');
  });

  it('learns that nobody is signed in from the refusal of that request', async () => {
    const { fetch } = server(() => problem(401, 'auth.required'));
    const client = createApiClient({ baseUrl: BASE, fetch });

    await expect(client.restore()).resolves.toBe(false);
    expect(client.session.getState().status).toBe('signed_out');
  });

  it('discovers a first visitor quietly without guessing whether an HttpOnly cookie exists', async () => {
    const { fetch, seen, paths } = server(() => json(200, null));
    const client = createApiClient({ baseUrl: BASE, fetch });
    await expect(Promise.all([client.restore(), client.restore()])).resolves.toEqual([false, false]);
    expect(paths()).toEqual(['POST /auth/session']);
    expect(seen[0]?.headers.get('x-orch-csrf')).toBe('1');
    expect(seen[0]?.headers.has('authorization')).toBe(false);
    expect(client.session.getState().status).toBe('signed_out');
    await expect(client.restore()).resolves.toBe(false); expect(seen).toHaveLength(1);
  });

  it('ignores an anonymous discovery answer after a later sign-in became current', async () => {
    let finish: (reply: Response) => void = () => { throw new Error('The discovery was not started'); };
    const { fetch } = server(() => new Promise<Response>((resolve) => { finish = resolve; }));
    const client = createApiClient({ baseUrl: BASE, fetch });
    const restoring = client.restore(); await new Promise<void>((resolve) => setTimeout(resolve, 0));
    client.adopt(grant('current-sign-in')); finish(json(200, null));
    await expect(restoring).resolves.toBe(true);
    expect(client.accessToken()).toBe('current-sign-in'); expect(client.session.getState().userId).toBe('u1');
  });
});

describe('a slow answer', () => {
  /** A body that arrives a piece at a time, with a pause before each piece. */
  function trickle(pieces: number, pauseMs: number): Response {
    let sent = 0;
    const body = new ReadableStream<Uint8Array>({
      async pull(controller) {
        await new Promise((resolve) => setTimeout(resolve, pauseMs));
        sent += 1;
        if (sent === pieces) {
          controller.enqueue(new TextEncoder().encode('"end"]'));
          controller.close();
        } else {
          controller.enqueue(new TextEncoder().encode(sent === 1 ? '["piece",' : '"piece",'));
        }
      },
    });
    return new Response(body, { status: 200, headers: { 'Content-Type': 'application/json' } });
  }

  it('is not given up for taking long, as long as something keeps arriving', async () => {
    // Six pieces, each within the limit of silence; together far beyond it.
    const { fetch } = server(() => trickle(6, 30));
    const client = createApiClient({ baseUrl: BASE, fetch, silenceLimitMs: 80 });

    const answer = await ok(client.api.GET('/meta'));

    expect(answer).toHaveLength(6);
  });

  it('is given up when nothing has moved for the limit of silence', async () => {
    const { fetch } = server(() => trickle(3, 200));
    const client = createApiClient({ baseUrl: BASE, fetch, silenceLimitMs: 40 });

    const error = await ok(client.api.GET('/meta')).catch((caught: unknown) => caught);

    expect(error).toBeInstanceOf(NetworkError);
    expect(error).toMatchObject({ failure: 'silent' });
  });
});

for (const locale of ['en', 'zh-CN'] as const) for (const stage of ['response', 'body'] as const) {
  it(`names the ${stage} silence stage and the admitted request's own duration in ${locale}`, async () => {
    await setLocale(locale);
    vi.useFakeTimers();
    const limit = stage === 'response' ? 45_000 : 120_000;
    const { fetch, paths } = server(() => stage === 'response' ? new Promise<Response>(() => undefined)
      : new Response(new ReadableStream<Uint8Array>({ start(controller) { controller.enqueue(new TextEncoder().encode('{')); } }),
        { headers: { 'Content-Type': 'application/json' } }));
    const client = createApiClient({ baseUrl: BASE, fetch, silenceLimitMs: limit });
    const pending = ok(client.api.GET('/meta')).catch((error: unknown) => error);
    await vi.advanceTimersByTimeAsync(limit);
    const error = await pending;
    expect(error).toBeInstanceOf(NetworkError);
    if (!(error instanceof NetworkError)) throw new Error('The admitted request must keep its native network failure');
    const duration = formatSpan(limit, { locale });
    const expected = locale === 'en' ? stage === 'response' ? `No response for ${duration}` : `Transfer stalled for ${duration}`
      : stage === 'response' ? `等待响应已达 ${duration}` : `传输停滞已达 ${duration}`;
    expect(errorText(error, i18n.getFixedT(locale), { locale }).message).toBe(expected);
    expect(error.failure).toBe('silent'); expect(retryRead(0, error)).toBe(true);
    expect(paths()).toEqual(['GET /meta']);
    expect(client.session.getState().status).toBe('unknown');
    await setLocale('en');
  });
}

for (const locale of ['en', 'zh-CN'] as const) for (const stage of ['response', 'body'] as const) {
  it(`names a connection failure during ${stage} without changing its read retry in ${locale}`, async () => {
    await setLocale(locale);
    vi.stubGlobal('navigator', { onLine: true });
    const { fetch, paths } = server(() => stage === 'response' ? Promise.reject(new TypeError('private connection diagnostic'))
      : new Response(new ReadableStream<Uint8Array>({ start(controller) { controller.error(new TypeError('private transfer diagnostic')); } }),
        { headers: { 'Content-Type': 'application/json' } }));
    const client = createApiClient({ baseUrl: BASE, fetch });
    const error = await ok(client.api.GET('/meta')).catch((failure: unknown) => failure);
    expect(error).toBeInstanceOf(NetworkError);
    if (!(error instanceof NetworkError)) throw new Error('The native connection failure must be preserved');
    const expected = locale === 'en' ? stage === 'response' ? 'Connection failed before a response' : 'Connection failed during transfer'
      : stage === 'response' ? '响应前连接失败' : '传输期间连接失败';
    expect(errorText(error, i18n.getFixedT(locale), { locale }).message).toBe(expected);
    expect(error.failure).toBe('failed'); expect(retryRead(0, error)).toBe(true);
    expect(paths()).toEqual(['GET /meta']);
    expect(client.session.getState().status).toBe('unknown');
  });
}

describe('a request that creates something', () => {
  it('carries the key of the act, so that a second attempt makes nothing twice', async () => {
    const { fetch, seen } = server(() => new Response(null, { status: 204 }));
    const client = createApiClient({ baseUrl: BASE, fetch });

    await done(client.api.POST('/auth/recovery', { body: { email: 'ada@example.org' }, ...repeatable('act-1') }));

    expect(seen[0]?.headers.get('Idempotency-Key')).toBe('act-1');
  });
});


describe('an acknowledged mutation without full response content', () => {
  it('keeps the committed receipt and request identity, tells act listeners once, and never retries', async () => {
    const receipt = { projection: 'receipt', chat_id: 'c1', message_id: 'm1', seq: 7 };
    const { fetch, paths } = server(() => json(201, receipt, { 'X-Request-Id': 'accepted-1' }));
    const client = createApiClient({ baseUrl: BASE, fetch });
    const changed = vi.fn();
    client.onAct(changed);
    const result: unknown = await ok(client.api.POST('/chats/{id}/messages', {
      params: { path: { id: 'c1' } },
      body: { client_id: 'once', parts: [{ type: 'text', text: 'A submitted message' }] },
    })).catch((caught: unknown) => caught);
    expect(result).toBeInstanceOf(ReceiptAcknowledged);
    if (!(result instanceof ReceiptAcknowledged)) throw new Error('Expected the committed acknowledgement');
    expect(result.committed).toBe(true);
    expect(result.status).toBe(201);
    expect(result.requestId).toBe('accepted-1');
    expect(result.receipt).toEqual(receipt);
    expect(result).not.toBeInstanceOf(ApiError);
    expect(result).not.toBeInstanceOf(NetworkError);
    expect(retryRead(0, result)).toBe(false);
    expect(paths()).toEqual(['POST /chats/c1/messages']);
    expect(changed).toHaveBeenCalledTimes(1);
    expect(changed.mock.calls[0]?.[0]).toMatchObject({ answer: receipt });
  });

  it('preserves a newly issued secret for its recipient without copying it into error serialization', async () => {
    const receipt = { projection: 'receipt' as const, token_id: 'k1', token: 'new-secret-visible-once', already_created: false };
    const result: unknown = await ok(Promise.resolve({ data: receipt, response: json(200, receipt) }))
      .catch((caught: unknown) => caught);
    expect(result).toBeInstanceOf(ReceiptAcknowledged);
    if (!(result instanceof ReceiptAcknowledged)) throw new Error('Expected the committed acknowledgement');
    expect(result.receipt.token).toBe('new-secret-visible-once');
    expect(JSON.stringify(result)).not.toContain('new-secret-visible-once');
    expect(result.message).not.toContain('new-secret-visible-once');
    expect(result.stack).not.toContain('new-secret-visible-once');
  });
});


describe('credential creation act observers', () => {
  it.each(['full', 'receipt'] as const)('keeps %s token/proof material in the dedicated result only', async (projection) => {
    const info = { id: 'new-key', name: 'A key', scopes: [], node: null, created_at: '2026-11-01T00:00:00Z', last_used_at: null, expires_at: null };
    const answer = projection === 'full'
      ? { token: 'issued-secret-once', info, already_created: false }
      : { projection: 'receipt', token: 'issued-secret-once', token_id: info.id, created_at: info.created_at, expires_at: null, already_created: false };
    const { fetch, paths } = server(() => json(200, answer));
    const client = createApiClient({ baseUrl: BASE, fetch });
    const changed = vi.fn();
    client.onAct(changed);
    const result = await client.api.POST('/me/tokens', { body: { name: 'A key', proof: { password: 'fresh-secret-proof' } } });
    expect(result.data).toEqual(answer);
    expect(changed).toHaveBeenCalledTimes(1);
    const observed: unknown = changed.mock.calls[0]?.[0];
    expect(observed).toMatchObject({ path: '/me/tokens', answer: { token_id: 'new-key', already_created: false } });
    expect(observed).not.toHaveProperty('input');
    expect(JSON.stringify(observed)).not.toContain('issued-secret-once');
    expect(JSON.stringify(observed)).not.toContain('fresh-secret-proof');
    expect(paths()).toEqual(['POST /me/tokens']);
  });
});

it.each([{ stalled: false, html: false }, { stalled: true, html: false }, { stalled: false, html: true }])('file-piece silence follows actual upload progress, stalled=$stalled, HTML startup=$html', async ({ stalled, html }) => {
  vi.useFakeTimers();
  let opened: () => void = () => undefined;
  const started = new Promise<void>((resolve) => { opened = resolve; });
  const held: { headers: Map<string, string>; bytes: string; aborted: boolean }[] = [];
  class UploadXhr {
    readonly upload = { onprogress: null as (() => void) | null, onload: null as (() => void) | null };
    onload: (() => void) | null = null; onerror: (() => void) | null = null; onabort: (() => void) | null = null;
    status = 200; statusText = 'OK'; responseType = ''; withCredentials = false;
    response = new TextEncoder().encode('{"received":8}').buffer;
    readonly entry = { headers: new Map<string, string>(), bytes: '', aborted: false };
    private progress: ReturnType<typeof setInterval> | undefined;
    private done: ReturnType<typeof setTimeout> | undefined;
    constructor() { held.push(this.entry); opened(); }
    open(method: string, url: string) { expect(method).toBe('PUT'); expect(url).toContain('/uploads/sessions/'); }
    setRequestHeader(name: string, value: string) { this.entry.headers.set(name.toLowerCase(), value); }
    getAllResponseHeaders() { return 'Content-Type: application/json\r\n'; }
    async send(body: Blob) {
      this.entry.bytes = await body.text();
      if (!stalled) {
        this.progress = setInterval(() => this.upload.onprogress?.(), 20);
        setTimeout(() => this.upload.onload?.(), 100);
      }
      this.done = setTimeout(() => { clearInterval(this.progress); this.onload?.(); }, 140);
    }
    abort() { this.entry.aborted = true; clearInterval(this.progress); clearTimeout(this.done); this.onabort?.(); }
  }
  vi.stubGlobal('XMLHttpRequest', UploadXhr);
  const startup: HtmlReads = { visit: () => undefined, take: () => null, scriptsReady: Promise.resolve(), painted: () => undefined, cancel: () => undefined };
  const client = createApiClient({ baseUrl: BASE, silenceLimitMs: 40, ...(html ? { startupReads: startup } : {}) });
  client.adopt(grant('synthetic-piece'));
  const sent = vi.fn();
  const running = (html
    ? client.putFilePiece('owned', 0, new Blob(['contents']), new AbortController().signal, sent)
    : ok(client.api.PUT('/uploads/sessions/{upload_id}', {
    params: { path: { upload_id: 'owned' }, query: { offset: 0 } },
    body: '', bodySerializer: () => new Blob(['contents']), headers: { 'Content-Type': 'application/octet-stream', ...repeatable('piece-once').headers },
  }))).catch((error: unknown) => error);
  await started;
  await vi.advanceTimersByTimeAsync(160);
  expect(held).toHaveLength(1);
  expect(held[0]?.bytes).toBe('contents');
  expect(held[0]?.headers.get('authorization')).toBe('Bearer synthetic-piece');
  if (html) expect(sent).toHaveBeenCalledExactlyOnceWith(expect.any(Number));
  else expect(held[0]?.headers.get('idempotency-key')).toBeTruthy();
  if (stalled) { expect(await running).toMatchObject({ failure: 'silent' }); expect(held[0]?.aborted).toBe(true); }
  else { expect(await running).toEqual({ received: 8 }); expect(held[0]?.aborted).toBe(false); }
});


it.each(['expired','revoked','account_unavailable'] as const)('retains only its proven native %s outcome and clears it when another reader signs in',async(cause)=>{
  const fact={cause,...(cause==='account_unavailable'?{}:{at_if_known:'2026-11-15T04:00:00Z'})};
  const {fetch}=server(()=>problem(401,cause==='account_unavailable'?'auth.suspended':'auth.invalid_token',{session_end:{...fact,user_id:'not-a-public-fact'}}));
  const client=createApiClient({baseUrl:BASE,fetch});client.adopt(grant('former'));
  await expect(client.renew()).resolves.toBe(false);
  expect(client.session.getState()).toMatchObject({status:'signed_out',sessionEnded:true,sessionEnd:{cause,at_if_known:fact.at_if_known??null}});
  expect(JSON.stringify(client.session.getState())).not.toContain('not-a-public-fact');
  client.adopt({...grant('replacement'),user:{...me,id:'replacement'}});
  expect(client.session.getState()).toMatchObject({status:'signed_in',userId:'replacement',sessionEnded:false,sessionEnd:null});
});

it('an old account refusal cannot sign out a replacement or retain its former private outcome',async()=>{
  let finish:(reply:Response)=>void=()=>{throw new Error('The request was not started');};
  const {fetch}=server(()=>new Promise<Response>((resolve)=>{finish=resolve;}));
  const client=createApiClient({baseUrl:BASE,fetch});client.adopt(grant('former'));
  const pending=client.api.GET('/me');await new Promise<void>((resolve)=>setTimeout(resolve,0));
  client.adopt({...grant('replacement'),user:{...me,id:'replacement'}});
  finish(problem(401,'auth.suspended',{session_end:{cause:'account_unavailable'}}));await pending;
  expect(client.accessToken()).toBe('replacement');
  expect(client.session.getState()).toMatchObject({status:'signed_in',userId:'replacement',sessionEnded:false});
});

it('a late renewal outcome cannot cross readers and a replacement can own its separate renewal',async()=>{
  const replies:((reply:Response)=>void)[]=[];
  const {fetch}=server(()=>new Promise<Response>((resolve)=>replies.push(resolve)));
  const client=createApiClient({baseUrl:BASE,fetch});client.adopt(grant('former'));
  const former=client.renew();await new Promise<void>((resolve)=>setTimeout(resolve,0));
  client.adopt({...grant('replacement'),user:{...me,id:'replacement'}});
  const next=client.renew();await new Promise<void>((resolve)=>setTimeout(resolve,0));
  expect(replies).toHaveLength(2);
  replies[0]?.(problem(401,'auth.invalid_token',{session_end:{cause:'revoked',at_if_known:'2026-11-15T04:00:00Z'}}));
  await expect(former).resolves.toBe(true);
  expect(client.session.getState()).toMatchObject({status:'signed_in',userId:'replacement'});
  replies[1]?.(json(200,{...grant('fresh-replacement'),user:{...me,id:'replacement'}}));await expect(next).resolves.toBe(true);
  expect(client.accessToken()).toBe('fresh-replacement');
  expect(client.session.getState()).toHaveProperty('sessionEnd',null);
});

it('an unreported old refusal remains generic instead of borrowing a current expiry rule',async()=>{
  const {fetch}=server(()=>problem(401,'auth.invalid_token'));
  const client=createApiClient({baseUrl:BASE,fetch});client.adopt(grant('former'));
  await expect(client.renew()).resolves.toBe(false);
  expect(client.session.getState()).toMatchObject({status:'signed_out',sessionEnded:true,sessionEnd:null});
});

it('keeps a proven bearer end when the cookie proof is no longer retained',async()=>{
  const at='2026-11-15T04:00:00Z';
  const {fetch}=server((request)=>new URL(request.url).pathname.endsWith('/auth/refresh')?problem(401,'auth.required'):problem(401,'auth.invalid_token',{session_end:{cause:'expired',at_if_known:at}}));
  const client=createApiClient({baseUrl:BASE,fetch});client.adopt(grant('former'));
  await client.api.GET('/me');
  expect(client.session.getState()).toMatchObject({status:'signed_out',sessionEnd:{cause:'expired',at_if_known:at}});
});
