// @vitest-environment node
/** A nested MCP creation updates its real child and only its currently loaded parent projection. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { applyChanges, findAct } from '@/api/acts';
import { client } from '@/api/client';
import type { EnvironmentMcpCreateReceipt } from '@/generated/api-types';
import { detailMcp } from '../component/resource-detail-fixtures';

for (const operation of ['imports', 'servers'] as const) it(`projects the nested ${operation} server without replacing its parent`, async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const parentKey = ['environment-mcp', 'parent'];
  const otherKey = ['environment-mcp', 'other'];
  const before = { servers: [], found: ['old discovery'] };
  const after = { servers: [detailMcp.id], found: [] };
  const parentRead = vi.fn(() => Promise.resolve(after));
  const otherRead = vi.fn(() => Promise.resolve(before));
  cache.setQueryData(parentKey, before); cache.setQueryData(otherKey, before);
  const stopParent = new QueryObserver(cache, { queryKey: parentKey, queryFn: parentRead, staleTime: Infinity }).subscribe(() => undefined);
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
    expect(cache.getQueryData(otherKey)).toEqual(before);
    expect(otherRead).not.toHaveBeenCalled();
  } finally { stopParent(); stopOther(); cache.clear(); client.session.setState(session); }
});

it('keeps a restricted receipt distinct from the independently readable server', () => {
  const receipt: EnvironmentMcpCreateReceipt = { projection: 'receipt', server_id: 'new-server', environment_id: 'parent' };
  const act = findAct('POST', '/environments/parent/mcp/imports', { answer: receipt });
  expect(act).not.toBeNull();
  expect(act?.changes.some((change) => 'row' in change)).toBe(false);
  expect(act?.changes).toEqual([{ missing: [{ key: ['environment-mcp', 'parent'], exact: true }], cancelPending: true }]);
});
