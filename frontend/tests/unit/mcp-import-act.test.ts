// @vitest-environment node
/** A nested MCP creation updates its real child and only its currently loaded parent projection. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { applyChanges, findAct, threadMcpChanges } from '@/api/acts';
import { client } from '@/api/client';
import type { EnvironmentMcpCreateReceipt } from '@/generated/api-types';
import { detailMcp } from '../component/resource-detail-fixtures';
import { snapshot } from './continuation-fixtures';
import { AT, marker } from '../doubles/thread-rows';

for (const operation of ['imports', 'servers'] as const) it(`projects the nested ${operation} server without replacing its parent`, async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const parentKey = ['environment-mcp', 'parent'];
  const otherKey = ['environment-mcp', 'other'];
  const scopedKey = ['environment-mcp', 'parent', 'thread'];
  const before = { servers: [], found: ['old discovery'] };
  const after = { servers: [detailMcp.id], found: [] };
  const parentRead = vi.fn(() => Promise.resolve(after));
  const otherRead = vi.fn(() => Promise.resolve(before));
  cache.setQueryData(parentKey, before); cache.setQueryData(otherKey, before); cache.setQueryData(scopedKey, before);
  const stopParent = new QueryObserver(cache, { queryKey: parentKey, queryFn: parentRead, staleTime: Infinity }).subscribe(() => undefined);
  const scopedRead = vi.fn(() => Promise.resolve(after));
  const stopScoped = new QueryObserver(cache, { queryKey: scopedKey, queryFn: scopedRead, staleTime: Infinity }).subscribe(() => undefined);
  const stopOther = new QueryObserver(cache, { queryKey: otherKey, queryFn: otherRead, staleTime: Infinity }).subscribe(() => undefined);
  const session = client.session.getState();
  client.session.setState({ userId: 'reader' });
  try {
    const act = findAct('POST', `/environments/parent/mcp/${operation}`, { answer: detailMcp });
    expect(act).not.toBeNull();
    expect(act?.changes).toContainEqual({ row: { kind: 'mcp_server', id: detailMcp.id, value: detailMcp, insert: true } });
    expect(act?.changes.some((change) => 'row' in change && change.row.kind === 'environment')).toBe(false);
    await applyChanges(act?.changes ?? [], cache);
    expect(cache.getQueryData(['mcp-server', detailMcp.id])).toEqual(detailMcp);
    expect(cache.getQueryData(parentKey)).toEqual(after);
    expect(parentRead).toHaveBeenCalledOnce();
    expect(cache.getQueryData(scopedKey)).toEqual(after);
    expect(scopedRead).toHaveBeenCalledOnce();
    expect(cache.getQueryData(otherKey)).toEqual(before);
    expect(otherRead).not.toHaveBeenCalled();
  } finally { stopParent(); stopOther(); stopScoped(); cache.clear(); client.session.setState(session); }
});

it('keeps a restricted receipt distinct from the independently readable server', () => {
  const receipt: EnvironmentMcpCreateReceipt = { projection: 'receipt', server_id: 'new-server', environment_id: 'parent' };
  const act = findAct('POST', '/environments/parent/mcp/imports', { answer: receipt });
  expect(act).not.toBeNull();
  expect(act?.changes.some((change) => 'row' in change)).toBe(false);
  expect(act?.changes).toEqual([{ environmentMcp: 'parent' }]);
});

it('refreshes only the owning current thread connection after a native report or a source switch', async () => {
  const base = snapshot().thread;
  if (base.environment === null) throw new Error('This world has a native thread in its environment');
  const current = { ...base, environment: base.environment, current_segment_id: 'current-native' };
  const report = { ...marker('connection-ready', { marker: 'harness_changed', data: {
    program: 'Codex', change: 'mcp_startup', subject: 'docs', previous: { state: 'unsupported' },
    value: { status: 'ready', definition_source: 'native' }, expected: { state: 'unsupported' },
    changed: false, diverged: false, next_turn: false, turn_id: null, reported_at: AT,
  } }), thread_id: current.id, segment_id: current.current_segment_id };
  const expected = [{ missing: [{ key: ['environment-mcp', current.environment.id, current.id], exact: true }], cancelPending: true }];
  expect(threadMcpChanges(current, current, [report])).toEqual(expected);
  expect(threadMcpChanges(current, current, [{ ...report, stream: 'child' }])).toEqual([]);
  expect(threadMcpChanges(current, current, [{ ...report, segment_id: 'older-native' }])).toEqual([]);
  expect(threadMcpChanges(current, current, [{ ...report, thread_id: 'another-thread' }])).toEqual([]);
  expect(threadMcpChanges(current, current, [{ ...report, before_orch: true }])).toEqual([]);
  expect(threadMcpChanges(current, { ...current, current_segment_id: 'older-native' }, [])).toEqual(expected);
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const keys = [['environment-mcp', current.environment.id, current.id], ['environment-mcp', current.environment.id, 'another-thread'], ['environment-mcp', current.environment.id]];
  const reads = keys.map(() => vi.fn(() => Promise.resolve('current')));
  const stops = keys.map((queryKey, index) => {
    cache.setQueryData(queryKey, 'held');
    return new QueryObserver(cache, { queryKey, queryFn: reads[index], staleTime: Infinity }).subscribe(() => undefined);
  });
  try {
    await applyChanges(threadMcpChanges(current, current, [report]), cache);
    expect(reads.map((read) => read.mock.calls.length)).toEqual([1, 0, 0]);
  } finally { stops.forEach((stop) => stop()); cache.clear(); }
});
