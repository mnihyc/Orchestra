/** Full share answers update their source; write-only receipts never become content rows. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { applyChanges, findAct } from '@/api/acts';
import type { AgentShare } from '@/generated/api-types';
import { ADA } from './general-access-fixtures';

const share: AgentShare = {
  id: 'share', what: { id: 'source', kind: 'topic', name: 'Source', redacted: false },
  to: { id: 'recipient', kind: 'thread', name: 'Recipient', redacted: false },
  mode: 'live', inside: true, download: true, parts: { documents: 'edit' },
  read_first: false, viewer_entries: [], current: null, entries: [], snapshot: null, input: null,
  by: ADA, at: '2026-11-03T08:00:00Z',
};
const key = ['agent-shares', 'nodes', 'source', 'of'];
const other = ['agent-shares', 'nodes', 'other', 'of'];

for (const method of ['POST', 'PATCH'] as const) it(`installs the full ${method} answer without rereading a loaded share list`, async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
  cache.setQueryData(key, method === 'POST' ? [] : [{ ...share, parts: { documents: 'read' } }]);
  cache.setQueryData(other, []);
  const read = vi.fn(() => Promise.resolve([])), otherRead = vi.fn(() => Promise.resolve([]));
  const off = new QueryObserver(cache, { queryKey: key, queryFn: read }).subscribe(() => undefined);
  const offOther = new QueryObserver(cache, { queryKey: other, queryFn: otherRead }).subscribe(() => undefined);
  try {
    await applyChanges(findAct(method, method === 'POST' ? '/share/agents' : '/share/agents/share', {
      answer: method === 'POST' ? { shares: [share] } : share,
    })!.changes, cache);
    expect(cache.getQueryData(key)).toEqual([share]);
    expect(cache.getQueryData(other)).toEqual([]);
    expect(read).not.toHaveBeenCalled();
    expect(otherRead).not.toHaveBeenCalled();
  } finally { off(); offOther(); cache.clear(); }
});

for (const shown of [true, false]) it(`repairs only the ${shown ? 'shown' : 'hidden'} owner containing a revoked share`, async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
  cache.setQueryData(key, [share]); cache.setQueryData(other, []);
  const page = ['page', 'nodes', 'source'];
  cache.setQueryData(page, { revision: 1, agents: ['recipient'] });
  const read = vi.fn(() => Promise.resolve([])), otherRead = vi.fn(() => Promise.resolve([]));
  const pageRead = vi.fn(() => Promise.resolve({ revision: 2, agents: [] }));
  const off = shown ? new QueryObserver(cache, { queryKey: key, queryFn: read }).subscribe(() => undefined) : () => undefined;
  const offPage = shown ? new QueryObserver(cache, { queryKey: page, queryFn: pageRead }).subscribe(() => undefined) : () => undefined;
  const offOther = new QueryObserver(cache, { queryKey: other, queryFn: otherRead }).subscribe(() => undefined);
  try {
    await applyChanges(findAct('DELETE', '/share/agents/share')!.changes, cache);
    expect(cache.getQueryData(key)).toEqual([]);
    expect(cache.getQueryData(other)).toEqual([]);
    expect(read).not.toHaveBeenCalled();
    expect(pageRead).toHaveBeenCalledTimes(shown ? 1 : 0);
    expect(cache.getQueryData(page)).toEqual(shown ? { revision: 2, agents: [] } : undefined);
    expect(otherRead).not.toHaveBeenCalled();
  } finally { off(); offPage(); offOther(); cache.clear(); }
});

it('keeps a write-only receipt out of shown rows without reading unrelated owners', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
  cache.setQueryData(key, []); cache.setQueryData(other, []);
  const read = vi.fn(() => Promise.resolve([])), otherRead = vi.fn(() => Promise.resolve([]));
  const observer = new QueryObserver(cache, { queryKey: key, queryFn: read });
  const off = observer.subscribe(() => undefined);
  const offOther = new QueryObserver(cache, { queryKey: other, queryFn: otherRead }).subscribe(() => undefined);
  try {
    await applyChanges(findAct('POST', '/share/agents', {
      input: { what: { node_id: 'source' } },
      answer: { projection: 'receipt', shares: [{ id: 'share', to: { kind: 'thread', id: 'recipient' }, mode: 'live', created: true, input_id: null }] },
    })!.changes, cache);
    expect(observer.getCurrentResult().data).toEqual([]);
    expect(cache.getQueryData(other)).toEqual([]);
    expect(cache.getQueryData(['node', 'share'])).toBeUndefined();
    expect(otherRead).not.toHaveBeenCalled();
  } finally { off(); offOther(); cache.clear(); }
});
