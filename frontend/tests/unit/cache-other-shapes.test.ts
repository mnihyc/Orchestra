/** A live row is written into the projections of its own kind. An object of another shape that only carries the same
 * identifier (an entry of what an agent reaches, whose `path` is a line of text) stays as the server gave it. */
import { QueryClient } from '@tanstack/react-query';
import { describe, expect, it } from 'vitest';
import { applyRow, applyThread } from '@/api/cache';
import { sharedTreeQuery } from '@/api/share';
import type { SharedEntry, Thread, ThreadSharedTreeGetReply } from '@/generated/api-types';

const listed = { id: 't2', kind: 'thread', path: 'Project/Topic/Other thread', name: 'Other thread', description: null, unit: null, parts: 3, size: null, updated_at: null, children: null, readable: true, writable: false, manageable: false, deleted: false, via: null, shared_by: null, redacted: false } as unknown as SharedEntry;
const row = { id: 't2', kind: 'thread', name: 'Other thread', description: 'A thread', path: [{ id: 'p1', kind: 'project', name: 'Project' }, { id: 'tp', kind: 'topic', name: 'Topic' }], state: 'running', children: [], size: { turns: 3 } };

function seeded(): { cache: QueryClient; key: readonly unknown[] } {
  const cache = new QueryClient({ defaultOptions: { queries: { structuralSharing: false } } });
  const key = sharedTreeQuery('t1', '.').queryKey;
  cache.setQueryData(key, { path: '.', entries: [listed], truncated: false } satisfies ThreadSharedTreeGetReply);
  return { cache, key };
}

describe('a live row and an object of another shape with the same identifier', () => {
  it('leaves a listed entry of what an agent reaches as it was read when the thread it names changes', () => {
    const { cache, key } = seeded();
    try {
      applyThread(cache, row as unknown as Thread);
      const entry = cache.getQueryData<ThreadSharedTreeGetReply>(key)?.entries[0];
      expect(typeof entry?.path).toBe('string');
      expect(entry).toEqual(listed);
    } finally { cache.clear(); }
  });

  it('leaves it as it was read when a row of the thread arrives on the live connection', () => {
    for (const change of [
      { kind: 'thread', id: 't2', value: row },
      { kind: 'node', id: 't2', value: row, partial: true },
    ]) {
      const { cache, key } = seeded();
      try {
        applyRow(cache, change);
        const entry = cache.getQueryData<ThreadSharedTreeGetReply>(key)?.entries[0];
        expect(typeof entry?.path, change.kind).toBe('string');
        expect(entry, change.kind).toEqual(listed);
      } finally { cache.clear(); }
    }
  });
});
