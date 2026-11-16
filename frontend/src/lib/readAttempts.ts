/** Browser observations belong to the actual read attempt, never to a later default or another reader. */
export type ReadAttemptPhase = 'reading' | 'waiting' | 'failed';
export interface ReadAttemptFacts {
  reader: string;
  operation: 'read';
  read_attempt: number;
  scheduled_retry_at: number | null;
  phase: ReadAttemptPhase;
}
interface Episode { flight: unknown; facts: ReadAttemptFacts; error: object | null; cleanup?: () => void }
const episodes = new WeakMap<object, Episode>();
const identities = new WeakMap<object, object>();
const errors = new WeakMap<object, Set<Episode>>();
const delays = new WeakMap<object, number>();
const listeners = new WeakMap<object, Set<() => void>>();
const object = (value: unknown): value is object => typeof value === 'object' && value !== null;
function identity(flight: unknown): unknown {
  if (!object(flight)) return flight;
  let token = identities.get(flight);
  if (token === undefined) { token = {}; identities.set(flight, token); }
  return token;
}

export const requestReader = (user: string | null, version: number | undefined): string => JSON.stringify([user, version ?? 0]);
function changed(error: object): void { for (const listener of listeners.get(error) ?? []) listener(); }
function detach(episode: Episode): void {
  const error = episode.error;
  if (error === null) return;
  const held = errors.get(error); held?.delete(episode);
  if (held?.size === 0) errors.delete(error);
  episode.error = null; changed(error);
}
function update(episode: Episode, more: Partial<ReadAttemptFacts>): void {
  const next = { ...episode.facts, ...more };
  if (next.phase === episode.facts.phase && next.read_attempt === episode.facts.read_attempt
    && next.scheduled_retry_at === episode.facts.scheduled_retry_at) return;
  episode.facts = next;
  if (episode.error !== null) changed(episode.error);
}

/** Multiple observers share one attempt; a replaced flight starts a new observation. */
export function beginReadAttempt(owner: object, flight: unknown, reader: string): void {
  const token = identity(flight);
  let episode = episodes.get(owner);
  if (episode !== undefined && episode.flight === token && episode.facts.reader !== reader) return;
  if (episode === undefined || episode.flight !== token || episode.facts.reader !== reader) {
    if (episode !== undefined) { episode.cleanup?.(); detach(episode); }
    episode = { flight: token, error: null, facts: { reader, operation: 'read', read_attempt: 0, scheduled_retry_at: null, phase: 'reading' } };
    episodes.set(owner, episode);
  }
  update(episode, { read_attempt: episode.facts.read_attempt + 1, phase: 'reading', scheduled_retry_at: null });
}

export function noteReadError(owner: object, flight: unknown, error: unknown): void {
  const episode = episodes.get(owner);
  if (episode === undefined || episode.flight !== identity(flight) || !object(error)) return;
  if (episode.error !== error) {
    detach(episode); episode.error = error;
    const held = errors.get(error) ?? new Set<Episode>(); held.add(episode); errors.set(error, held); changed(error);
  }
}

/** Delay evaluation alone does not establish that a retry was admitted. */
export function observedRetryDelay<Failure>(delay: number | ((attempt: number, error: Failure) => number)) {
  return (attempt: number, error: Failure): number => {
    const duration = typeof delay === 'number' ? delay : delay(attempt, error);
    if (object(error) && Number.isFinite(duration)) delays.set(error, Math.max(0, duration));
    return duration;
  };
}

/** Called only once the real scheduler has accepted this safe read's retry. */
export function scheduleReadRetry(owner: object, error: unknown, duration?: number): void {
  const episode = episodes.get(owner);
  if (episode === undefined || !object(error) || episode.error !== error) return;
  const pause = duration ?? delays.get(error);
  const at = pause === undefined ? NaN : Date.now() + Math.max(0, pause);
  update(episode, { phase: 'waiting', scheduled_retry_at: Number.isFinite(new Date(at).getTime()) ? at : null });
}

export function failReadAttempt(owner: object, error: unknown): void {
  const episode = episodes.get(owner);
  if (episode !== undefined && episode.error === error) update(episode, { phase: 'failed', scheduled_retry_at: null });
}

export function retireReadAttempt(owner: object, flight?: unknown): void {
  const episode = episodes.get(owner);
  if (episode === undefined || flight !== undefined && episode.flight !== identity(flight)) return;
  episodes.delete(owner); episode.cleanup?.(); detach(episode);
}

/** The one abort listener follows the current native attempt, including its final retained failure. */
export function watchReadAbort(signal: AbortSignal, flight: unknown): void {
  const episode = episodes.get(signal);
  if (episode === undefined || episode.flight !== identity(flight) || episode.cleanup !== undefined) return;
  const abort = () => retireReadAttempt(signal, flight);
  episode.cleanup = () => signal.removeEventListener('abort', abort);
  signal.addEventListener('abort', abort, { once: true });
  if (signal.aborted) abort();
}

/** A shared error with several current owners has no unambiguous attempt to attribute. */
export function readAttemptFacts(error: unknown, reader: string): ReadAttemptFacts | null {
  if (!object(error)) return null;
  let found: ReadAttemptFacts | null = null;
  for (const episode of errors.get(error) ?? []) {
    if (episode.facts.reader !== reader) continue;
    if (found !== null) return null;
    found = episode.facts;
  }
  return found;
}

export function subscribeReadAttempt(error: unknown, listener: () => void): () => void {
  if (!object(error)) return () => undefined;
  const held = listeners.get(error) ?? new Set<() => void>(); held.add(listener); listeners.set(error, held);
  return () => { held.delete(listener); if (held.size === 0) listeners.delete(error); };
}

/** Native feed backoff ends when its owner leaves, without making another hidden read. */
export function waitForReadRetry(signal: AbortSignal, duration: number): Promise<void> {
  return new Promise((resolve, reject) => {
    const done = () => { signal.removeEventListener('abort', abort); resolve(); };
    const timer = setTimeout(done, duration);
    const abort = () => { clearTimeout(timer); signal.removeEventListener('abort', abort); reject(new DOMException('The read was cancelled', 'AbortError')); };
    signal.addEventListener('abort', abort, { once: true });
    if (signal.aborted) abort();
  });
}
