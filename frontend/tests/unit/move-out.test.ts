/** One atomic native move follows its exact deletion footprint; no browser Delete follows Copy. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { beforeEach, expect, it, vi } from 'vitest';
import { executeMoveOut, moveOutPath, previewMoveOut, supportsMoveOut, type MoveOutDeletePlan } from '@/api/moveOut';
import { i18n } from '@/i18n';
import { homeMoveEntries } from '@/features/duplicate/move';
import { useDuplicateHost } from '@/features/duplicate/target';
import { applyChanges, findAct } from '@/api/acts';
import type { HomeMoveTarget } from '@/features/duplicate/move';
const calls = vi.hoisted(() => ({ json: vi.fn<(path: string, options: unknown) => Promise<unknown>>() }));
vi.mock('@/api/trace', () => ({ traceJson: calls.json }));
beforeEach(() => { vi.clearAllMocks(); useDuplicateHost.getState().close(); });
const kinds = ['host', 'machine', 'agent', 'skill', 'mount', 'template', 'provider'] as const;
const target = (kind: HomeMoveTarget['kind']): HomeMoveTarget => ({ kind, id: 'source', name: 'Source', rootOrgId: 'source-org', revision: 7 });
const copy = { destination: { kind: 'node', id: 'other-org' } as const, expected_revision: 7 };
const plan = (kind: HomeMoveTarget['kind']): MoveOutDeletePlan => ({ subject: { kind, id: 'source' }, destination: copy.destination, revision: 7, stamp: 'native-footprint', effects: kind === 'host' || kind === 'machine' ? ['keep_registration'] : ['remove_source'] });
it.each(kinds)('previews then executes %s through one actual native move route', async (kind) => {
  const native = target(kind); calls.json.mockResolvedValueOnce(plan(kind));
  expect(await previewMoveOut(native, copy)).toEqual(plan(kind));
  calls.json.mockResolvedValueOnce(kind === 'host' || kind === 'machine' ? { projection: 'receipt', kind, object_id: 'source', home_id: 'other-org', changed: true } : { projection: 'receipt', kind, id: 'new-copy' });
  expect(await executeMoveOut(native, { copy, delete_plan: { revision: 7, stamp: 'native-footprint' } }, 'one-act')).toEqual({ kind, id: kind === 'host' || kind === 'machine' ? 'source' : 'new-copy' });
  const base = moveOutPath(native);
  expect(calls.json.mock.calls.map((call) => call[0])).toEqual([`${base}/preview`, base]);
  expect(calls.json.mock.calls[1]?.[1]).toEqual({ method: 'POST', body: { copy, delete_plan: { revision: 7, stamp: 'native-footprint' } }, key: 'one-act' });
  expect(findAct('POST', `${base}/preview`)?.changes).toEqual([]); expect(findAct('POST', base)).not.toBeNull();
});
it('keeps a skill draft in its own native reader and writer variant', () => {
  expect(moveOutPath({ ...target('skill'), draft: true })).toBe('/skills/source/draft/move-out');
  expect(findAct('POST', '/skills/source/draft/move-out')).not.toBeNull();
});
it.each(['subject', 'destination'] as const)('refuses an unrelated %s preview before execution', async (changed) => {
  const native = plan('agent'); calls.json.mockResolvedValue(changed === 'subject' ? { ...native, subject: { kind: 'agent', id: 'foreign' } } : { ...native, destination: { kind: 'node', id: 'foreign' } });
  await expect(previewMoveOut(target('agent'), copy)).rejects.toThrow('did not match'); expect(calls.json).toHaveBeenCalledOnce();
});
it('a registration receipt cannot invent a second machine id', async () => {
  calls.json.mockResolvedValue({ projection: 'receipt', kind: 'machine', object_id: 'different-machine', home_id: 'other-org', changed: true });
  await expect(executeMoveOut(target('machine'), { copy, delete_plan: { revision: 7, stamp: 'native-footprint' } }, 'one-act')).rejects.toThrow('did not identify');
});
it('keeps unported families unavailable without guessing a two-write implementation', () => {
  for (const kind of ['environment'] as const) {
    expect(supportsMoveOut(kind)).toBe(false); expect(() => moveOutPath(target(kind))).toThrow('no published native move route');
  }
});

it('a registration acknowledgement must name the reviewed destination', async () => {
  calls.json.mockResolvedValue({ projection: 'receipt', kind: 'host', object_id: 'source', home_id: 'foreign', changed: true });
  await expect(executeMoveOut(target('host'), { copy, delete_plan: { revision: 7, stamp: 'native-footprint' } }, 'one-act')).rejects.toThrow('accepted destination');
});

it('an MCP native move carries the chosen physical custodian through preview and the one deletion-confirmed act', async () => {
  const native = target('mcp_server'), intent = { ...copy, machine_id: 'chosen-machine' };
  calls.json.mockResolvedValueOnce(plan('mcp_server'));
  await previewMoveOut(native, intent);
  calls.json.mockResolvedValueOnce({ projection: 'receipt', kind: 'mcp_server', id: 'made-mcp' });
  await executeMoveOut(native, { copy: intent, delete_plan: { revision: 7, stamp: 'native-footprint' } }, 'move-key');
  expect(calls.json.mock.calls[0]).toEqual(['/mcp-servers/source/move-out/preview', { method: 'POST', body: { ...intent, name: null } }]);
  expect(calls.json.mock.calls[1]).toEqual(['/mcp-servers/source/move-out', { method: 'POST', body: { copy: { ...intent, name: null }, delete_plan: { revision: 7, stamp: 'native-footprint' } }, key: 'move-key' }]);
});
it('a group move keeps its native ending and notice preview together and binds only that review to notification', async () => {
  const native = target('chat'), group = { delete_plan: { ...plan('chat'), effects: ['end_group_and_history'] }, notification: { people: 3, notify_limit: 200, preview_id: 'native-member-review' } };
  calls.json.mockResolvedValueOnce(group); expect(await previewMoveOut(native, copy)).toEqual(group);
  calls.json.mockResolvedValueOnce({ projection: 'receipt', kind: 'chat', id: 'new-group' });
  await executeMoveOut(native, { copy, delete_plan: { revision: 7, stamp: 'native-footprint' }, notify: true, notify_preview_id: 'native-member-review' }, 'one-move');
  expect(calls.json.mock.calls[1]).toEqual(['/chats/source/move-out', { method: 'POST', body: { copy, delete_plan: { revision: 7, stamp: 'native-footprint' }, notify: true, notify_preview_id: 'native-member-review' }, key: 'one-move' }]);
});
it('a group move cannot borrow an unwrapped plan from a resource preview', async () => {
  calls.json.mockResolvedValue(plan('chat')); await expect(previewMoveOut(target('chat'), copy)).rejects.toThrow('another native family');
});

it('a native group move receipt repairs the actual source and visible inventory as distinct readers', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const source = ['chat', 'source'] as const, list = ['everything', { kind: 'chat' }] as const;
  const other = ['chat', 'unrelated'] as const, untouched = { id: 'unrelated', ended: false };
  const before = { id: 'source', ended: false }, after = { id: 'source', ended: true }, inventory = { items: [{ id: 'new-chat' }] };
  cache.setQueryData(source, before); cache.setQueryData(list, { items: [] }); cache.setQueryData(other, untouched);
  const readSource = vi.fn(() => Promise.resolve(after)), readList = vi.fn(() => Promise.resolve(inventory)), readOther = vi.fn(() => Promise.resolve(untouched));
  const stops = [new QueryObserver(cache, { queryKey: source, queryFn: readSource, staleTime: Infinity }).subscribe(() => undefined), new QueryObserver(cache, { queryKey: list, queryFn: readList, staleTime: Infinity }).subscribe(() => undefined), new QueryObserver(cache, { queryKey: other, queryFn: readOther, staleTime: Infinity }).subscribe(() => undefined)];
  try { await applyChanges(findAct('POST', '/chats/source/move-out', { answer: { projection: 'receipt', kind: 'chat', id: 'new-chat' } })?.changes ?? [], cache);
    expect(cache.getQueryData(source)).toEqual(after); expect(cache.getQueryData(list)).toEqual(inventory); expect(readSource).toHaveBeenCalledOnce(); expect(readList).toHaveBeenCalledOnce();
    expect(cache.getQueryData(other)).toEqual(untouched); expect(readOther).not.toHaveBeenCalled();
  } finally { stops.forEach((stop) => stop()); cache.clear(); }
});
it('a registration-preserving move repairs its native machine projection without manufacturing a copy', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } }), key = ['machine', 'source'] as const;
  const list = ['machines'] as const, other = ['machine', 'unrelated'] as const;
  const after = { id: 'source', home_id: 'destination' }, inventory = [after], untouched = { id: 'unrelated', home_id: 'original' };
  cache.setQueryData(key, { id: 'source', home_id: 'original' }); cache.setQueryData(list, []); cache.setQueryData(other, untouched);
  const read = vi.fn(() => Promise.resolve(after)), readList = vi.fn(() => Promise.resolve(inventory)), readOther = vi.fn(() => Promise.resolve(untouched));
  const stops = [new QueryObserver(cache, { queryKey: key, queryFn: read, staleTime: Infinity }).subscribe(() => undefined), new QueryObserver(cache, { queryKey: list, queryFn: readList, staleTime: Infinity }).subscribe(() => undefined), new QueryObserver(cache, { queryKey: other, queryFn: readOther, staleTime: Infinity }).subscribe(() => undefined)];
  try {
    await applyChanges(findAct('POST', '/machines/source/move-out', { answer: { projection: 'receipt', kind: 'machine', object_id: 'source', home_id: 'destination', changed: true } })?.changes ?? [], cache);
    expect(cache.getQueryData(key)).toEqual(after); expect(cache.getQueryData(list)).toEqual(inventory); expect(read).toHaveBeenCalledOnce(); expect(readList).toHaveBeenCalledOnce();
    expect(cache.getQueryData(other)).toEqual(untouched); expect(readOther).not.toHaveBeenCalled(); expect(cache.getQueryData(['machine', 'destination'])).toBeUndefined();
  } finally { stops.forEach((stop) => stop()); cache.clear(); }
});

it('the actual imported row fact reaches the shared Move entrance without deriving it from home', () => {
  const entry = homeMoveEntries(i18n.t, 'mcp_server', { id: 'source', name: 'Tools', org_id: 'source-org', imported: { source: 'codex' }, access: { administers: false, permissions: ['mcp.manage'], through: [] } })[0];
  if (entry === undefined || !('onSelect' in entry)) throw new Error('The real Move entry must be actionable');
  entry.onSelect(); expect(useDuplicateHost.getState().current).toMatchObject({ mode: 'move', target: { kind: 'mcp_server', id: 'source', imported: true } }); useDuplicateHost.getState().close();
});
