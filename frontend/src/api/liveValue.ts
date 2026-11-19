/** A query's first native live snapshot answers its pending read, without a duplicate HTTP request.
 *
 * Where the answer can also be read over HTTP, `read` is the second way: it is used at once while the live connection is not up,
 * and when the snapshot has not come within `LIVE_VALUE_PATIENCE_MS` while it is, so that a connection that never carries the
 * feed does not leave the screen waiting for ever.
 */
import type { QueryKey } from '@tanstack/react-query';

import { liveConnectionIsUp } from '@/api/readAgain';

/** A live snapshot follows its subscription within a moment; after this long it is not coming, and the read is made. */
export const LIVE_VALUE_PATIENCE_MS = 10_000;

interface Waiting { resolve: (data: unknown) => void; reject: (error: Error) => void }
const waiting = new Map<string, Set<Waiting>>();

export function liveValue<Value>(key: QueryKey, signal: AbortSignal, read?: () => Promise<Value>): Promise<Value> {
  const name = JSON.stringify(key);
  return new Promise((resolve, reject) => {
    const listeners = waiting.get(name) ?? new Set<Waiting>();
    let patience: ReturnType<typeof setTimeout> | undefined;
    const remove = () => { clearTimeout(patience); listeners.delete(listener); if (listeners.size === 0) waiting.delete(name); signal.removeEventListener('abort', abort); };
    const listener: Waiting = { resolve: (data) => { remove(); resolve(data as Value); }, reject: (error) => { remove(); reject(error); } };
    const abort = () => listener.reject(new DOMException('The view was closed', 'AbortError'));
    if (signal.aborted) { reject(new DOMException('The view was closed', 'AbortError')); return; }
    listeners.add(listener); waiting.set(name, listeners); signal.addEventListener('abort', abort, { once: true });
    if (read === undefined) return;
    const second = () => { read().then((data) => listener.resolve(data), (error: unknown) => listener.reject(error instanceof Error ? error : new Error('The read failed'))); };
    if (liveConnectionIsUp()) patience = setTimeout(second, LIVE_VALUE_PATIENCE_MS); else second();
  });
}

export function deliverLiveValue(key: QueryKey, data: unknown): void {
  for (const listener of [...waiting.get(JSON.stringify(key)) ?? []]) listener.resolve(data);
}

export function refuseLiveValue(key: QueryKey, error: Error): void {
  for (const listener of [...waiting.get(JSON.stringify(key)) ?? []]) listener.reject(error);
}
