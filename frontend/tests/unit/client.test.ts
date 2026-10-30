// @vitest-environment node
/** The API client: the token renewed once and the refused request sent again, errors as typed values, the header of the cookie requests, and a wait that ends on silence alone. */
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ApiError, createApiClient, done, ok, RENEW_BEFORE_END_MS, repeatable } from '@/api/client';
import type { AuthResult, Me } from '@/generated/api-types';
import { NetworkError } from '@/lib/requests';
import { beginOpening, openingId } from '@/lib/opening';

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

afterEach(() => {
  vi.useRealTimers();
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
    expect(paths()).toEqual(['POST /auth/refresh']);
    expect(client.session.getState().userId).toBe('u1');
  });

  it('learns that nobody is signed in from the refusal of that request', async () => {
    const { fetch } = server(() => problem(401, 'auth.required'));
    const client = createApiClient({ baseUrl: BASE, fetch });

    await expect(client.restore()).resolves.toBe(false);
    expect(client.session.getState().status).toBe('signed_out');
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

describe('a request that creates something', () => {
  it('carries the key of the act, so that a second attempt makes nothing twice', async () => {
    const { fetch, seen } = server(() => new Response(null, { status: 204 }));
    const client = createApiClient({ baseUrl: BASE, fetch });

    await done(client.api.POST('/auth/recovery', { body: { email: 'ada@example.org' }, ...repeatable('act-1') }));

    expect(seen[0]?.headers.get('Idempotency-Key')).toBe('act-1');
  });
});
