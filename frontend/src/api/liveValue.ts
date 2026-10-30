/** A query's first native live snapshot answers its pending read, without a duplicate HTTP request. */
import type { QueryKey } from '@tanstack/react-query';

interface Waiting { resolve: (data: unknown) => void; reject: (error: Error) => void }
const waiting = new Map<string, Set<Waiting>>();

export function liveValue<Value>(key: QueryKey, signal: AbortSignal): Promise<Value> {
  const name = JSON.stringify(key);
  return new Promise((resolve, reject) => {
    const listeners = waiting.get(name) ?? new Set<Waiting>();
    const remove = () => { listeners.delete(listener); if (listeners.size === 0) waiting.delete(name); signal.removeEventListener('abort', abort); };
    const listener: Waiting = { resolve: (data) => { remove(); resolve(data as Value); }, reject: (error) => { remove(); reject(error); } };
    const abort = () => listener.reject(new DOMException('The view was closed', 'AbortError'));
    if (signal.aborted) { reject(new DOMException('The view was closed', 'AbortError')); return; }
    listeners.add(listener); waiting.set(name, listeners); signal.addEventListener('abort', abort, { once: true });
  });
}

export function deliverLiveValue(key: QueryKey, data: unknown): void {
  for (const listener of [...waiting.get(JSON.stringify(key)) ?? []]) listener.resolve(data);
}

export function refuseLiveValue(key: QueryKey, error: Error): void {
  for (const listener of [...waiting.get(JSON.stringify(key)) ?? []]) listener.reject(error);
}
