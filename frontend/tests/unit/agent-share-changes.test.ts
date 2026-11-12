// @vitest-environment node
/** Share answers own the source list; recipient assessments and limited receipts retain their authorized suppliers. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { applyChanges, findAct } from '@/api/acts';

const of = ['agent-shares', 'nodes', 'source', 'of'];
const to = ['agent-shares', 'nodes', 'target', 'to'];
const other = ['agent-shares', 'nodes', 'other', 'of'];
const share = { id: 'loan', what: { id: 'source', kind: 'topic', name: 'Source', redacted: false },
  to: { id: 'target', kind: 'project', name: 'Target', redacted: false }, mode: 'live', inside: true, read_first: false,
  download: true, parts: { documents: 'read' }, entries: [], input: null, snapshot: null, by: null, at: '2026-01-01T00:00:00Z',
  current: { state: 'unassessed', recipient: null, parts: null, reason: 'recipient_context_required' } };
const create = (answer: unknown = { shares: [share] }) => findAct('POST', '/share/agents', { input: { what: { node_id: 'source' } }, answer })?.changes ?? [];
function setup() {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  cache.setQueryData(of, []); cache.setQueryData(other, []);
  return cache;
}

it('applies complete accepted rows to one source idempotently and never seeds an Agent or an unrelated list', async () => {
  const cache = setup();
  try {
    await applyChanges(create(), cache); await applyChanges(create(), cache);
    expect(cache.getQueryData(of)).toEqual([share]); expect(cache.getQueryData(other)).toEqual([]);
    expect(cache.getQueryData(['agent', 'loan'])).toBeUndefined();
    expect(cache.getQueryData(to)).toBeUndefined();
  } finally { cache.clear(); }
});

it('retakes a recipient assessment independently without copying the source assessment', async () => {
  const cache = setup(); cache.setQueryData(to, []);
  const assessed = { ...share, current: { state: 'reduced', recipient: 'target', parts: { documents: 'none' }, reason: 'current_limits' } };
  const read = vi.fn(() => Promise.resolve([assessed]));
  const stop = new QueryObserver(cache, { queryKey: to, queryFn: read, staleTime: Infinity }).subscribe(() => undefined);
  try {
    await applyChanges(create(), cache);
    expect(read).toHaveBeenCalledOnce(); expect(cache.getQueryData(to)).toEqual([assessed]);
    expect(cache.getQueryData(of)).toEqual([share]);
  } finally { stop(); cache.clear(); }
});

it('keeps an update on its source and expires a hidden recipient projection without reading it', async () => {
  const cache = setup(); cache.setQueryData(of, [share]); cache.setQueryData(to, [share]);
  const updated = { ...share, read_first: true };
  try {
    await applyChanges(findAct('PATCH', '/share/agents/loan', { answer: updated })?.changes ?? [], cache);
    expect(cache.getQueryData(of)).toEqual([updated]); expect(cache.getQueryData(to)).toBeUndefined();
    expect(cache.getQueryData(other)).toEqual([]);
  } finally { cache.clear(); }
});

it('uses an exact source read for a restricted receipt instead of treating the receipt as metadata', async () => {
  const cache = setup();
  const read = vi.fn(() => Promise.resolve([share]));
  const stop = new QueryObserver(cache, { queryKey: of, queryFn: read, staleTime: Infinity }).subscribe(() => undefined);
  try {
    await applyChanges(create({ projection: 'receipt', shares: [{ id: 'loan', mode: 'live', created: true, input_id: null, to: { id: 'target', kind: 'project' } }] }), cache);
    expect(read).toHaveBeenCalledOnce(); expect(cache.getQueryData(of)).toEqual([share]);
    expect(cache.getQueryData(other)).toEqual([]);
  } finally { stop(); cache.clear(); }
});

it('uses held native ancestry for an inherited recipient and leaves other knowledge untouched', async () => {
  const cache = setup(); const key = ['thread-shared', 'child']; const unrelated = ['thread-shared', 'unrelated'];
  cache.setQueryData(['node', 'child'], { path: [{ id: 'target' }] });
  cache.setQueryData(['node', 'unrelated'], { path: [{ id: 'elsewhere' }] });
  cache.setQueryData(key, { shares: [] }); cache.setQueryData(unrelated, { shares: [] });
  const read = vi.fn(() => Promise.resolve({ shares: [share], assessed_for: 'child' }));
  const otherRead = vi.fn(() => Promise.resolve({ shares: [] }));
  const stop = new QueryObserver(cache, { queryKey: key, queryFn: read, staleTime: Infinity }).subscribe(() => undefined);
  const stopOther = new QueryObserver(cache, { queryKey: unrelated, queryFn: otherRead, staleTime: Infinity }).subscribe(() => undefined);
  try {
    await applyChanges(create(), cache);
    expect(read).toHaveBeenCalledOnce(); expect(otherRead).not.toHaveBeenCalled();
    expect(cache.getQueryData(key)).toEqual({ shares: [share], assessed_for: 'child' });
  } finally { stop(); stopOther(); cache.clear(); }
});

it('removes a confirmed revocation from its source and retakes every shown recipient holding that share', async () => {
  const cache = setup(); cache.setQueryData(of, [share]); cache.setQueryData(to, [share]);
  const read = vi.fn(() => Promise.resolve([]));
  const stop = new QueryObserver(cache, { queryKey: to, queryFn: read, staleTime: Infinity }).subscribe(() => undefined);
  try {
    await applyChanges(findAct('DELETE', '/share/agents/loan')?.changes ?? [], cache);
    expect(cache.getQueryData(of)).toEqual([]); expect(read).toHaveBeenCalledOnce(); expect(cache.getQueryData(to)).toEqual([]);
    expect(cache.getQueryData(other)).toEqual([]);
  } finally { stop(); cache.clear(); }
});


it('does not invent an unread collection from its one accepted row and retires the earlier read', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  let finish!: (value: unknown[]) => void;
  const earlier = new Promise<unknown[]>((resolve) => { finish = resolve; });
  const existing = { ...share, id: 'existing' };
  const read = vi.fn<() => Promise<unknown[]>>().mockReturnValueOnce(earlier).mockResolvedValue([existing, share]);
  const stop = new QueryObserver(cache, { queryKey: of, queryFn: read, staleTime: Infinity }).subscribe(() => undefined);
  try {
    await applyChanges(create(), cache);
    expect(read).toHaveBeenCalledTimes(2); expect(cache.getQueryData(of)).toEqual([existing, share]);
    finish([]); await Promise.resolve();
    expect(cache.getQueryData(of)).toEqual([existing, share]);
  } finally { finish([]); stop(); cache.clear(); }
});


it.each([
  { object: 'documents', input: { document_id: 'material' }, what: { id: 'material', title: 'Material' } },
  { object: 'chats', input: { chat_id: 'material' }, what: { id: 'material', kind: 'group', name: 'Material' } },
])('keeps create, update and removal on the $object source', async ({ object, input, what }) => {
  const cache = setup(); const key = ['agent-shares', object, 'material', 'of'];
  cache.setQueryData(key, []); const row = { ...share, what };
  try {
    await applyChanges(findAct('POST', '/share/agents', { input: { what: input }, answer: { shares: [row] } })?.changes ?? [], cache);
    expect(cache.getQueryData(key)).toEqual([row]); expect(cache.getQueryData(of)).toEqual([]);
    const updated = { ...row, read_first: true };
    await applyChanges(findAct('PATCH', '/share/agents/loan', { answer: updated })?.changes ?? [], cache);
    expect(cache.getQueryData(key)).toEqual([updated]);
    await applyChanges(findAct('DELETE', '/share/agents/loan')?.changes ?? [], cache);
    expect(cache.getQueryData(key)).toEqual([]); expect(cache.getQueryData(of)).toEqual([]);
  } finally { cache.clear(); }
});
