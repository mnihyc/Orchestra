/** Giving and reported use share their existing query, including authorized Environment openings. */
import { queryOptions, type QueryClient } from '@tanstack/react-query';
import { api, client, ok } from '@/api/client';
import type { LibraryEntry, LibraryTarget, ResourceFacts } from '@/generated/api-types';

export const libraryKey = (targets: readonly LibraryTarget[]) => ['resource-library', targets] as const;
const environmentTarget = (id: string): LibraryTarget => ({ object: { kind: 'environment', id }, version_id: null });
const reader = () => { const state = client.session.getState(); return [state.userId, state.readerVersion] as const; };
export const libraryReader = reader;
export function sameLibraryReader(before: ReturnType<typeof reader>): boolean {
  const now = reader(); return now[0] === before[0] && now[1] === before[1];
}
const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const nullableText = (value: unknown) => value === null || typeof value === 'string';
function environmentFacts(value: unknown, id: string): value is ResourceFacts {
  if (!record(value) || !record(value.subject) || value.subject.kind !== 'resource' || !record(value.subject.object) ||
    value.subject.object.kind !== 'environment' || value.subject.object.id !== id || typeof value.as_of !== 'string' ||
    typeof value.dependents_complete !== 'boolean' || !Array.isArray(value.dependents)) return false;
  if (!value.dependents.every((group: unknown) => record(group) && typeof group.relation === 'string' && typeof group.count === 'number' &&
    typeof group.hidden === 'number' && nullableText(group.until) && Array.isArray(group.visible) && group.visible.every((row: unknown) =>
      record(row) && typeof row.name === 'string' && typeof row.relation === 'string' && nullableText(row.until) &&
      (row.object === null || record(row.object) && typeof row.object.kind === 'string' && typeof row.object.id === 'string')))) return false;
  if (value.uses === null) return true;
  const uses = value.uses;
  return record(uses) && ['turn', 'mcp_call', 'skill_invocation', 'environment_made', 'provider_made'].includes(String(uses.kind)) &&
    ['all_time', 'last_30_days', 'people', 'people_30_days'].every((key) => uses[key] === null || typeof uses[key] === 'number') &&
    nullableText(uses.last_used_at) && record(uses.coverage) &&
    (uses.coverage.state === 'unrecorded' || uses.coverage.state === 'complete' || uses.coverage.state === 'partial' && typeof uses.coverage.since === 'string');
}

/** Omission is unchanged. Null or a foreign/malformed projection retires only this exact target. */
export function seedEnvironmentLibrary(cache: QueryClient, id: string, value: unknown): void {
  if (value === undefined) return;
  const target = environmentTarget(id), key = libraryKey([target]);
  if (environmentFacts(value, id)) {
    cache.setQueryData<LibraryEntry[]>(key, [{ target, facts: value, threads: null }]);
    return;
  }
  const held = cache.getQueryCache().find({ queryKey: key, exact: true });
  if (held?.state.data === undefined) return;
  if (!held.isActive() && held.state.fetchStatus !== 'fetching') cache.removeQueries({ queryKey: key, exact: true });
  else {
    held.setState({ data: undefined, error: null, status: 'pending' });
    if (held.state.fetchStatus !== 'fetching') void cache.invalidateQueries({ queryKey: key, exact: true });
  }
}

/** Cancelling this consumer never cancels the already-admitted Environment opening. */
async function settleOpening(pending: Promise<unknown>, signal: AbortSignal): Promise<void> {
  signal.throwIfAborted();
  await new Promise<void>((resolve, reject) => {
    const aborted = () => {
      const reason: unknown = signal.reason;
      reject(reason instanceof Error ? reason : new DOMException('The read was cancelled.', 'AbortError'));
    };
    signal.addEventListener('abort', aborted, { once: true });
    void pending.then(() => resolve(), () => resolve()).finally(() => signal.removeEventListener('abort', aborted));
  });
  signal.throwIfAborted();
}

export const libraryQuery = (targets: readonly LibraryTarget[]) => queryOptions({
  queryKey: libraryKey(targets),
  queryFn: async ({ signal, client: cache }): Promise<LibraryEntry[]> => {
    const before = reader(), target = targets.length === 1 ? targets[0] : undefined;
    if (target?.object.kind === 'environment' && target.version_id === null) {
      const key = libraryKey(targets), held = cache.getQueryCache().find<LibraryEntry[]>({ queryKey: key, exact: true });
      const updates = held?.state.dataUpdateCount;
      // Match the existing Environment key without importing its producer back into this query.
      const opening = cache.getQueryCache().find({ queryKey: ['environment', target.object.id], exact: true });
      const pending = opening?.state.fetchStatus === 'fetching' ? opening.promise : undefined;
      if (pending !== undefined) {
        await settleOpening(pending, signal);
        if (!sameLibraryReader(before)) throw new DOMException('The requesting reader changed.', 'AbortError');
        const published = cache.getQueryCache().find<LibraryEntry[]>({ queryKey: key, exact: true });
        if (published !== undefined && (published !== held || published.state.dataUpdateCount !== updates)) {
          const options = cache.defaultQueryOptions<LibraryEntry[]>({ queryKey: key });
          const staleTime = typeof options.staleTime === 'function' ? options.staleTime(published) : options.staleTime;
          const data = published.state.data;
          if (!published.isStaleByTime(staleTime) && data?.length === 1 &&
            data[0]?.target.object.kind === 'environment' && data[0].target.object.id === target.object.id &&
            data[0].target.version_id === null && environmentFacts(data[0].facts, target.object.id)) return data;
        }
      }
    }
    signal.throwIfAborted();
    if (!sameLibraryReader(before)) throw new DOMException('The requesting reader changed.', 'AbortError');
    return ok(api.POST('/resources/library', { body: { targets: [...targets] }, signal }));
  },
});
