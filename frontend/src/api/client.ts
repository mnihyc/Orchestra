/** API client for Fetch, auth refresh, error and request-id handling, and a key on every act so that a repeat is safe.
 *
 * The one place the application speaks HTTP. Everything else asks through the typed client made here,
 * whose paths, bodies and replies are the generated ones.
 *
 * What it takes care of, so that no caller has to:
 * - the access token, which is kept in memory only and never written anywhere: it is sent as a
 *   bearer, renewed shortly before it ends, and renewed once when the API says it is no longer good,
 *   after which the request that was refused is sent once more;
 * - the header that the two requests authenticated by the session cookie alone must carry;
 * - an error of the API as an `ApiError` that carries its code, the members that code adds, and the
 *   request id to show beside it;
 * - a request given up after silence, never for its length (`lib/requests.ts`).
 */
import createClient, { type Client } from 'openapi-fetch';
import { createStore, type StoreApi } from 'zustand/vanilla';

import type { AuthResult, ErrorCode, MissingAccess, NodeRef, paths, Problem, SignInProof } from '@/generated/api-types';
import { NetworkError, sendUntilSilent } from '@/lib/requests';
import { openingId } from '@/lib/opening';
import { apiErrorSentence, fieldErrorSentence } from '@/i18n/errors';

/** Where every request of the API lives. */
export const BASE_PATH = '/api/v1';

const CSRF_HEADER = 'X-Orch-Csrf';
const REQUEST_ID_HEADER = 'X-Request-Id';
const IDEMPOTENCY_HEADER = 'Idempotency-Key';

/** The two requests that the session cookie alone authenticates. */
const COOKIE_REQUESTS = [`${BASE_PATH}/auth/refresh`, `${BASE_PATH}/auth/logout`];

/**
 * How long before its end an access token is renewed: twice the silence a request is allowed, so that
 * a renewal on a slow link still lands while the old token is good.
 */
export const RENEW_BEFORE_END_MS = 60_000;

// ---------------------------------------------------------------- errors

/** A proof the API names; a saved recovery code is a second factor only. */
export type { SignInProof } from '@/generated/api-types';

/** One fault of a request that was not valid. */
export interface FieldFault {
  /** Where in the body: `email`, `content[0].text`. */
  path: string;
  message: string;
}

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null;
const isProof = (value: unknown): value is SignInProof =>
  value === 'password' || value === 'email_code' || value === 'totp' || value === 'recovery_code';
const isFault = (value: unknown): value is FieldFault =>
  isRecord(value) && typeof value.path === 'string' && typeof value.message === 'string';

/**
 * What a code adds to its error, read from the body and checked: a member that is not as documented
 * is as good as absent, and the screen falls back on the plain sentence of the code.
 */
const EXTRAS = {
  'request.invalid': (problem: Problem) =>
    Array.isArray(problem.errors) ? { errors: problem.errors.filter(isFault) } : null,
  'request.value_refused': (problem: Problem) =>
    Array.isArray(problem.errors) ? { errors: problem.errors.filter(isFault) } : null,
  'auth.second_step_required': (problem: Problem) =>
    Array.isArray(problem.methods) ? { methods: problem.methods.filter(isProof) } : null,
  'auth.locked': (problem: Problem) => (isProof(problem.way) ? { way: problem.way } : null),
  'authz.denied': (problem: Problem) => ({
    required: typeof problem.required === 'string' ? problem.required : null,
    requestable: problem.requestable === true,
    override: problem.override === true,
    missing: (Array.isArray(problem.missing) ? problem.missing : []) as MissingAccess[],
  }),
  'org.suspended': (problem: Problem) => (isRecord(problem.org) ? { org: problem.org as NodeRef } : null),
  // The most projects one inside another that a path holds, where the system says it.
  'project.too_deep': (problem: Problem) => (typeof problem.limit === 'number' ? { limit: problem.limit } : null),
  // A value another thing holds: for a short name, the org of the same place that holds it where the caller may see it,
  // and a short name that is free.
  'conflict.exists': (problem: Problem) => ({
    errors: Array.isArray(problem.errors) ? problem.errors.filter(isFault) : [],
    existingId: typeof problem.existing_id === 'string' ? problem.existing_id : null,
    existing:
      isRecord(problem.existing) && typeof problem.existing.name === 'string'
        ? { name: problem.existing.name, deleted: problem.existing.deleted === true }
        : null,
    suggestion: typeof problem.suggestion === 'string' ? problem.suggestion : null,
  }),
} satisfies Partial<Record<ErrorCode, (problem: Problem) => object | null>>;

/** The members each code adds, for the codes this application acts on. */
export type ProblemExtras = { [Code in keyof typeof EXTRAS]: NonNullable<ReturnType<(typeof EXTRAS)[Code]>> };

/** An error the API answered with. */
export class ApiError extends Error {
  override readonly name = 'ApiError';
  readonly problem: Problem;
  /** When the API says to try again, for a locked name or a rate that was passed. */
  readonly retryAt: Date | null;

  constructor(problem: Problem, retryAt: Date | null) {
    super(apiErrorSentence(problem.code, problem.status));
    this.problem = problem;
    this.retryAt = retryAt;
  }

  /** The stable value a client acts on, and by which the message is found in the catalogue. */
  get code(): ErrorCode {
    return this.problem.code;
  }

  get status(): number {
    return this.problem.status;
  }

  /** The id of the request, to show beside the error and to find it in the server's logs. */
  get requestId(): string {
    return this.problem.request_id;
  }

  /** Whether this is `code`. */
  is(code: ErrorCode): boolean {
    return this.problem.code === code;
  }

  /** What `code` adds to the error, when this is that code and the members are as documented. */
  extra<Code extends keyof ProblemExtras>(code: Code): ProblemExtras[Code] | null {
    if (this.problem.code !== code) return null;
    return EXTRAS[code](this.problem) as ProblemExtras[Code] | null;
  }

  /** The faults of a request that was not valid, by the member each is about. */
  faults(): FieldFault[] {
    return (this.extra('request.invalid')?.errors ?? this.extra('request.value_refused')?.errors ?? this.extra('conflict.exists')?.errors ?? [])
      .map((fault) => ({ ...fault, message: fieldErrorSentence(fault.message, undefined, fault.path) }));
  }
}

/** Whether the invitation's maker can no longer authorize its grant. */
export function invitationIssuerAuthorityLost(error: unknown): boolean {
  return error instanceof ApiError && error.problem.reason === 'invitation_issuer_authority_lost';
}

function isProblem(value: unknown): value is Problem {
  return isRecord(value) && typeof value.code === 'string' && typeof value.status === 'number';
}

/** `Retry-After` as a moment: it is a number of seconds, or a date. */
function retryAt(response: Response, now: number): Date | null {
  const header = response.headers.get('Retry-After');
  if (header === null) return null;
  const seconds = Number(header);
  if (Number.isFinite(seconds)) return new Date(now + seconds * 1_000);
  const date = Date.parse(header);
  return Number.isNaN(date) ? null : new Date(date);
}

function toError(body: unknown, response: Response): ApiError | NetworkError {
  if (!isProblem(body)) return new NetworkError('malformed', response.status);
  const requestId = body.request_id || (response.headers.get(REQUEST_ID_HEADER) ?? '');
  return new ApiError({ ...body, request_id: requestId }, retryAt(response, Date.now()));
}

interface Answer {
  data?: unknown;
  error?: unknown;
  response: Response;
}

/** The body of a request that succeeded; an `ApiError` or a `NetworkError` is thrown otherwise. */
export async function ok<A extends Answer>(answer: Promise<A>): Promise<Exclude<A['data'], undefined>> {
  const { data, error, response } = await answer;
  if (!response.ok) throw toError(error, response);
  return data as Exclude<A['data'], undefined>;
}

/** The same for a request that answers with nothing. */
export async function done(answer: Promise<Answer>): Promise<void> {
  const { error, response } = await answer;
  if (!response.ok) throw toError(error, response);
}

/** The header that makes a creating request safe to repeat: `key` is made once, when the person acts. */
export function repeatable(key: string): { headers: Record<string, string> } {
  return { headers: { [IDEMPOTENCY_HEADER]: key } };
}

// ---------------------------------------------------------------- the session

/** Whether somebody is signed in, as far as this page knows. */
export type SessionStatus =
  /** Not asked yet: the page was just loaded and holds no token. */
  | 'unknown'
  | 'signed_in'
  | 'signed_out';

export interface SessionState {
  /** Changes only when the human/session authority changes, not when its token is renewed. */
  readerVersion?: number;
  status: SessionStatus;
  userId: string | null;
  /** When the access token ends, on this device's clock. */
  accessEndsAt: number | null;
}

export type SessionEvent =
  | { type: 'signed_in'; result: AuthResult }
  /** A token was renewed for the person who was signed in already. */
  | { type: 'renewed'; result: AuthResult }
  | { type: 'signed_out' };

/** A request that changes something, answered with success: what the registry of acts (`src/api/acts.ts`) is told. */
export interface Act {
  /** Native response causation, shared with an echoed live hint; never a permission or request body. */
  requestId?: string;
  /** The reader that sent this act; a late answer cannot populate another reader's caches. */
  readerVersion?: number;
  method: 'POST' | 'PUT' | 'PATCH' | 'DELETE';
  /** The path below the API's base, without its query: `/nodes/1f…/roles`. */
  path: string;
  /** The JSON sent by the caller, when there was one. Never retained beyond the act listeners. */
  input?: unknown;
  /** The successful JSON answer: views apply these rows without reading their lists again. */
  answer?: unknown;
}

/** The methods of a request that changes something. */
const WRITES: ReadonlySet<string> = new Set<Act['method']>(['POST', 'PUT', 'PATCH', 'DELETE']);

export interface ApiClientOptions {
  /** The origin and base path requests go to; this page's own when absent. */
  baseUrl?: string;
  /** What sends a request; the browser's `fetch` when absent. */
  fetch?: (request: Request) => Promise<Response>;
  /** How long nothing may move before a request is given up. */
  silenceLimitMs?: number;
}

export interface ApiClient {
  /** The typed requests of the API. Wrap a call in `ok` or `done`. */
  api: Client<paths>;
  /** Who is signed in. Read it with `useStore`, or `getState()` outside a component. */
  session: StoreApi<SessionState>;
  /** The access token, for the socket's first frame. Nothing may store it. */
  accessToken: () => string | null;
  /** Takes the answer of a sign-in, a registration or a setup as the session of this page. */
  adopt: (result: AuthResult) => void;
  /**
   * Asks for a new access token with the session cookie. Answers whether somebody is signed in;
   * throws when the API could not be asked, which says nothing about the session.
   */
  renew: () => Promise<boolean>;
  /** Finds out, once for this page, whether its browser holds a session. */
  restore: () => Promise<boolean>;
  /** Ends the session on the server and then forgets it here. Throws, with the session kept, when the server was not reached. */
  signOut: () => Promise<void>;
  /** Hears of every sign-in, renewal and sign-out. What it returns stops the listening. */
  onSession: (listener: (event: SessionEvent) => void) => () => void;
  /**
   * Hears of every act that succeeded, before its caller does: the request's answer is handed on once every listener is
   * done, so that what an act changed is shown fresh by the time its caller goes on. A listener that fails does not fail
   * the act. What it returns stops the listening.
   */
  onAct: (listener: (act: Act) => void | Promise<void>) => () => void;
}

/** Makes a client with a session of its own. The application has one; a test makes as many as it needs. */
export function createApiClient(options: ApiClientOptions = {}): ApiClient {
  const origin = typeof location === 'undefined' ? '' : location.origin;
  const baseUrl = options.baseUrl ?? `${origin}${BASE_PATH}`;
  const basePath = new URL(baseUrl, origin || 'http://localhost').pathname;
  const cookieRequests = COOKIE_REQUESTS.map((path) => path.replace(BASE_PATH, basePath));
  const send = options.fetch ?? ((request: Request) => fetch(request));

  let token: string | null = null;
  /** How far the server's clock is ahead of this device's, from the date of its last answer. */
  let serverAhead = 0;
  let renewTimer: ReturnType<typeof setTimeout> | undefined;
  let renewing: Promise<boolean> | null = null;
  let restoring: Promise<boolean> | null = null;
  const listeners = new Set<(event: SessionEvent) => void>();
  const actListeners = new Set<(act: Act) => void | Promise<void>>();
  const session = createStore<SessionState>(() => ({ status: 'unknown', userId: null, accessEndsAt: null, readerVersion: 0 }));

  const emit = (event: SessionEvent) => {
    for (const listener of listeners) listener(event);
  };

  function forget(): void {
    clearTimeout(renewTimer);
    const wasSignedIn = session.getState().status === 'signed_in';
    token = null;
    session.setState({ status: 'signed_out', userId: null, accessEndsAt: null, readerVersion: (session.getState().readerVersion ?? 0) + 1 });
    if (wasSignedIn) emit({ type: 'signed_out' });
  }

  function adopt(result: AuthResult): void {
    const before = session.getState();
    // The life of the token is counted on the server's clock, so that a device whose own clock is
    // wrong still renews in time.
    const life = Date.parse(result.access_expires_at) - (Date.now() + serverAhead);
    const accessEndsAt = Date.now() + life;
    token = result.access_token;
    const sameReader = before.status === 'signed_in' && before.userId === result.user.id;
    session.setState({ status: 'signed_in', userId: result.user.id, accessEndsAt, readerVersion: (before.readerVersion ?? 0) + (sameReader ? 0 : 1) });

    clearTimeout(renewTimer);
    renewTimer = setTimeout(
      () => {
        // A renewal that could not be made is made by the next request, or by the answer that says so.
        renew().catch(() => undefined);
      },
      Math.max(0, life - RENEW_BEFORE_END_MS),
    );

    const same = before.status === 'signed_in' && before.userId === result.user.id;
    emit({ type: same ? 'renewed' : 'signed_in', result });
  }

  function isCookieRequest(request: Request): boolean {
    return cookieRequests.includes(new URL(request.url).pathname);
  }

  function carry(request: Request, bearer: string | null): Request {
    const headers = new Headers(request.headers);
    if (isCookieRequest(request)) headers.set(CSRF_HEADER, '1');
    else if (bearer !== null) headers.set('Authorization', `Bearer ${bearer}`);
    return new Request(request, { headers });
  }

  async function dispatch(request: Request): Promise<Response> {
    const response = await sendUntilSilent(request, send, options.silenceLimitMs);
    const date = Date.parse(response.headers.get('Date') ?? '');
    if (!Number.isNaN(date)) serverAhead = date - Date.now();
    return response;
  }

  async function codeOf(response: Response): Promise<string | null> {
    try {
      const body: unknown = await response.clone().json();
      return isProblem(body) ? body.code : null;
    } catch {
      return null;
    }
  }

  /** Tells every listener of an act that succeeded, and waits for them; a listener's fault is reported and stops nothing. */
  async function tellAct(request: Request, response: Response, input: Promise<unknown>, readerVersion: number): Promise<void> {
    const pathname = new URL(request.url).pathname;
    const answer: unknown = response.status !== 204 && response.headers.get('content-type')?.includes('json') === true
      ? await response.clone().json().catch((): undefined => undefined)
      : undefined;
    const sent = await input;
    if (readerVersion !== (session.getState().readerVersion ?? 0)) return;
    const requestId = response.headers.get(REQUEST_ID_HEADER);
    const act: Act = { ...(requestId ? { requestId } : {}), readerVersion, method: request.method as Act['method'], path: pathname.startsWith(basePath) ? pathname.slice(basePath.length) : pathname, input: sent, answer };
    await Promise.all(
      [...actListeners].map(async (listener) => {
        try {
          await listener(act);
        } catch (error) {
          console.error('What an act changed could not be shown', error);
        }
      }),
    );
  }

  async function transport(request: Request): Promise<Response> {
    const readerVersion = session.getState().readerVersion ?? 0;
    const headers = new Headers(request.headers);
    if (!headers.has('x-orch-opening')) headers.set('x-orch-opening', openingId());
    if (request.method === 'GET' && typeof location !== 'undefined') {
      headers.set('x-orch-view', `${location.pathname}${location.search}`);
      if (new URL(request.url).pathname === `${basePath}/me/bootstrap`) headers.set('x-orch-timezone', Intl.DateTimeFormat().resolvedOptions().timeZone);
    }
    request = new Request(request, { headers });
    // Clone before fetch consumes the body. Upload streams are never buffered for cache updates.
    const input: Promise<unknown> = WRITES.has(request.method) && request.headers.get('content-type')?.includes('json') === true
      ? request.clone().json().catch((): undefined => undefined)
      : Promise.resolve(undefined);
    const response = await authorized(request);
    if (response.ok && WRITES.has(request.method) && !isCookieRequest(request)) await tellAct(request, response, input, readerVersion);
    return response;
  }

  async function authorized(request: Request): Promise<Response> {
    if (isCookieRequest(request)) return dispatch(carry(request, null));
    const readerVersion = session.getState().readerVersion ?? 0;

    const endsAt = session.getState().accessEndsAt;
    if (token !== null && endsAt !== null && Date.now() >= endsAt - RENEW_BEFORE_END_MS) {
      // Due, or overdue after a sleep: renew first and spare the request a refusal.
      await renew().catch(() => undefined);
    }
    if (readerVersion !== (session.getState().readerVersion ?? 0)) return Response.json({ type: 'orch:error:auth.invalid_token', code: 'auth.invalid_token', title: 'Sign in again', detail: 'The requesting session changed.', status: 401, request_id: '' }, { status: 401 });

    const used = token;
    const again = used === null ? null : request.clone();
    const response = await dispatch(carry(request, used));
    if (again === null || response.status !== 401) return response;
    const code = await codeOf(response);
    if (code === 'auth.suspended') {
      // The person was suspended: nothing of theirs answers any more, and the page says so by signing out.
      forget();
      return response;
    }
    if (code !== 'auth.invalid_token') return response;
    if (readerVersion !== (session.getState().readerVersion ?? 0)) return response;
    // The token is no longer good. One renewal serves every request that was refused with it, and
    // each of them is sent again exactly once.
    const signedIn = token !== used ? token !== null : await renew().catch(() => false);
    if (!signedIn || token === null || readerVersion !== (session.getState().readerVersion ?? 0)) return response;
    return dispatch(carry(again, token));
  }

  const api = createClient<paths>({ baseUrl, fetch: transport, credentials: 'same-origin' });

  async function renewOnce(): Promise<boolean> {
    const answer = await api.POST('/auth/refresh');
    if (answer.data !== undefined) {
      adopt(answer.data);
      return true;
    }
    const error = toError(answer.error, answer.response);
    if (error instanceof ApiError && (error.status === 401 || error.status === 403)) {
      // No session, an ended one, or a suspended person: nobody is signed in on this page.
      forget();
      return false;
    }
    throw error;
  }

  function renew(): Promise<boolean> {
    renewing ??= renewOnce().finally(() => {
      renewing = null;
    });
    return renewing;
  }

  function restore(): Promise<boolean> {
    if (session.getState().status !== 'unknown') return Promise.resolve(session.getState().status === 'signed_in');
    restoring ??= renew().finally(() => {
      restoring = null;
    });
    return restoring;
  }

  async function signOut(): Promise<void> {
    try {
      await done(api.POST('/auth/logout'));
    } catch (error) {
      // "Not signed in" is the answer of a session that is gone already. Anything else leaves the
      // session as it is: saying "signed out" while the cookie still works would be untrue.
      if (!(error instanceof ApiError) || error.status !== 401) throw error;
    }
    forget();
  }

  return {
    api,
    session,
    accessToken: () => token,
    adopt,
    renew,
    restore,
    signOut,
    onSession: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    onAct: (listener) => {
      actListeners.add(listener);
      return () => actListeners.delete(listener);
    },
  };
}

/** The client of the application. */
export const client = createApiClient();

/** The typed requests of the API, on the application's client. */
export const api = client.api;

/** A build asset (fonts and binary decoders), served from this origin and
 * governed by the same silence limit as requests. It needs no bearer token. */
export async function assetBytes(url: string, signal?: AbortSignal): Promise<Uint8Array> {
  const request = new Request(new URL(url, window.location.href), { credentials: 'same-origin', ...(signal === undefined ? {} : { signal }) });
  const response = await sendUntilSilent(request, (sent) => fetch(sent));
  if (!response.ok) throw new NetworkError('failed', response.status);
  return new Uint8Array(await response.arrayBuffer());
}

/** Reads the public installation observation while the authenticated API is unavailable. */
export async function readInstallationProgress(signal?: AbortSignal): Promise<unknown> {
  const response = await fetch('/update-status', { cache: 'no-store', credentials: 'omit', ...(signal === undefined ? {} : { signal }) });
  if (!response.ok) return null;
  const value: unknown = await response.json();
  return value;
}
