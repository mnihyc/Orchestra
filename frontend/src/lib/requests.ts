/** Requests over a slow link: what was done shown at once as pending and never as done, a wait given up after thirty seconds of silence, a repeat that is safe, and a failure kept to the view it happened in.
 *
 * A slow connection makes things take longer; it never makes a request fail for having taken long.
 * So a request is given up when nothing has moved for a while, and never because of its length; a
 * read that failed for the network is asked again a few times; and a write is asked again only when
 * it carries a key that makes the repeat harmless.
 */

/**
 * How long nothing may move before a request is given up: a hundred round trips of a slow link, and
 * half of the silence a live connection is allowed.
 */
export const SILENCE_LIMIT_MS = 30_000;

/** The pauses before a failed read is asked again; after the last one the view says that it could not load. */
export const READ_RETRY_PAUSES_MS = [1_000, 2_000, 4_000] as const;

/** The random part added to each pause, as a share of it, so that many views do not ask again at one instant. */
const RETRY_SPREAD = 0.25;

/** Why a request got no answer that could be read. */
export type NetworkFailure =
  /** The browser says there is no connection. */
  | 'offline'
  /** Nothing moved, in either direction, for the silence limit. */
  | 'silent'
  /** The connection failed before an answer arrived. */
  | 'failed'
  /** An answer arrived that is no answer of the API: a proxy's own page, or a body that cannot be read. */
  | 'malformed'
  /** Whoever asked no longer wants the answer. */
  | 'aborted';

/** A request that got no answer of the API. Nothing is known about whether it was carried out. */
export class NetworkError extends Error {
  override readonly name = 'NetworkError';
  readonly failure: NetworkFailure;
  /** The HTTP status, when something answered that was not the API. */
  readonly status: number | null;

  constructor(failure: NetworkFailure, status: number | null = null, options?: ErrorOptions) {
    super(`network: ${failure}`, options);
    this.failure = failure;
    this.status = status;
  }

  /** Whether asking again could help: everything but a request that was called off. */
  get transient(): boolean {
    return this.failure !== 'aborted';
  }
}

/** The statuses whose answer never has a body. */
const BODILESS = new Set([101, 103, 204, 205, 304]);

class Silence extends Error {
  override readonly name = 'Silence';
}

/**
 * Sends `request` and gives it up when nothing has moved for `limitMs`: no answer began, or the body
 * of the answer stopped arriving. An answer that keeps arriving is never cut short, however long it is.
 */
export async function sendUntilSilent(
  request: Request,
  send: (request: Request) => Promise<Response>,
  limitMs: number = SILENCE_LIMIT_MS,
): Promise<Response> {
  const quiet = new AbortController();
  let timer: ReturnType<typeof setTimeout> | undefined;
  const moved = () => {
    clearTimeout(timer);
    timer = setTimeout(() => quiet.abort(new Silence()), limitMs);
  };
  const settled = () => clearTimeout(timer);

  // Given up for silence, or called off by whoever asked: either way the wait ends here, whether or
  // not what carries the request listens to the signal itself.
  const signal = AbortSignal.any([request.signal, quiet.signal]);
  const ended = new Promise<never>((_, reject) => {
    if (signal.aborted) reject(signal.reason as Error);
    else signal.addEventListener('abort', () => reject(signal.reason as Error), { once: true });
  });
  // Nobody may be waiting on it any more when it ends.
  ended.catch(() => undefined);

  moved();
  let response: Response;
  try {
    response = await Promise.race([send(new Request(request, { signal })), ended]);
  } catch (cause) {
    settled();
    throw asNetworkError(cause, request.signal);
  }
  // An answer that has no body by its status is complete as it is. Some browsers hand out an empty
  // stream for one all the same, and no answer of such a status can be made round a stream.
  if (response.body === null || BODILESS.has(response.status)) {
    settled();
    return response;
  }

  moved();
  const reader = response.body.getReader();
  // A read that is waiting when the wait ends is let go, and the body ends in the error below.
  const release = () => void reader.cancel(signal.reason).catch(() => undefined);
  signal.addEventListener('abort', release, { once: true });
  const finished = () => {
    settled();
    signal.removeEventListener('abort', release);
  };
  const body = new ReadableStream<Uint8Array>({
    async pull(controller) {
      try {
        const chunk = await reader.read();
        if (signal.aborted) {
          finished();
          controller.error(asNetworkError(signal.reason, request.signal));
        } else if (chunk.done) {
          finished();
          controller.close();
        } else {
          moved();
          controller.enqueue(chunk.value);
        }
      } catch (cause) {
        finished();
        controller.error(asNetworkError(cause, request.signal));
      }
    },
    cancel(reason) {
      finished();
      return reader.cancel(reason);
    },
  });
  return new Response(body, { status: response.status, statusText: response.statusText, headers: response.headers });
}

function asNetworkError(cause: unknown, callerSignal: AbortSignal): NetworkError {
  if (cause instanceof NetworkError) return cause;
  if (cause instanceof Silence) return new NetworkError('silent', null, { cause });
  if (callerSignal.aborted) return new NetworkError('aborted', null, { cause });
  if (isAbort(cause)) {
    // The silence timer aborts with its own reason, which some runtimes wrap.
    const reason: unknown = cause instanceof Error ? cause.cause : undefined;
    return new NetworkError(reason instanceof Silence ? 'silent' : 'aborted', null, { cause });
  }
  const offline = typeof navigator !== 'undefined' && !navigator.onLine;
  return new NetworkError(offline ? 'offline' : 'failed', null, { cause });
}

function isAbort(cause: unknown): boolean {
  return cause instanceof Error && cause.name === 'AbortError';
}

/** Whether a read that failed `failures` times with `error` is asked again. */
export function retryRead(failures: number, error: unknown): boolean {
  return error instanceof NetworkError && error.transient && failures < READ_RETRY_PAUSES_MS.length;
}

/** How long to wait before attempt `attempt` (counted from zero) of a failed read. */
export function retryPause(attempt: number, random: () => number = Math.random): number {
  const pause = READ_RETRY_PAUSES_MS[Math.min(attempt, READ_RETRY_PAUSES_MS.length - 1)] ?? 0;
  return pause + pause * RETRY_SPREAD * random();
}

/**
 * A key for one act of a person: made when they act, and sent with every attempt of the request that
 * carries the act out, so that a second attempt after a lost answer makes nothing twice.
 */
export function newActKey(): string {
  if (typeof crypto.randomUUID === 'function') return crypto.randomUUID();
  // A page that is not served securely has no `randomUUID`; the same value is made from random bytes.
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = ((bytes[6] ?? 0) & 0x0f) | 0x40;
  bytes[8] = ((bytes[8] ?? 0) & 0x3f) | 0x80;
  const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}
