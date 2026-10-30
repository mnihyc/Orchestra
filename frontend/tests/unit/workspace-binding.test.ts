/** One canonical source change retires old observations without reacting to unrelated activity. */
import { QueryClient } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { applyRow } from '@/api/cache';
import { ApiError } from '@/api/client';
import { installWorkspaceBindings, workspaceBinding } from '@/api/workspaceBinding';
import { detailEnvironment } from '../component/resource-detail-fixtures';

it('retires once for an accepted physical replacement, preserving activity and home-only updates', () => {
  const cache = new QueryClient(), forget = vi.fn(), key = ['environment', detailEnvironment.id];
  cache.setQueryData(key, detailEnvironment);
  const stop = installWorkspaceBindings(cache, forget);
  const updated = { ...detailEnvironment, revision: 2, seq: 2, name: 'Renamed workspace',
    data_sources: { workspace: { kind: 'made_by_orch' as const }, home: { kind: 'made_by_orch' as const } } };
  // An explicit selection is distinct from an older unspecified source.
  applyRow(cache, { kind: 'environment', id: updated.id, value: updated });
  expect(forget).toHaveBeenCalledTimes(1); forget.mockClear();
  applyRow(cache, { kind: 'environment', id: updated.id, value: { ...updated, revision: 3, seq: 3, status: 'stopped',
    data_sources: { ...updated.data_sources, home: { kind: 'existing_directory', path: '/another-home' } } } });
  expect(forget).not.toHaveBeenCalled();
  const replaced = { ...updated, revision: 4, seq: 4, generation: 2, workspace_root: '/other-workspace' };
  applyRow(cache, { kind: 'environment', id: updated.id, value: replaced });
  applyRow(cache, { kind: 'environment', id: updated.id, value: replaced });
  applyRow(cache, { kind: 'environment', id: updated.id, value: updated });
  expect(forget).toHaveBeenCalledExactlyOnceWith(updated.id);
  expect(cache.getQueryData(key)).toMatchObject({ revision: 4, workspace_root: '/other-workspace' });
  stop(); cache.clear();
});

it('uses opaque incarnations for redacted readers and retires on exact authority refusal', () => {
  const cache = new QueryClient(), forget = vi.fn(), key = ['environment', detailEnvironment.id];
  const redacted = { ...detailEnvironment, generation: 0, workspace_root: '', machine: null, host: null, data_sources: null, workspace_incarnation: 'source-a' };
  cache.setQueryData(key, redacted); const stop = installWorkspaceBindings(cache, forget);
  const replaced = { ...redacted, workspace_incarnation: 'source-b' };
  expect(workspaceBinding(redacted)).not.toBe(workspaceBinding(replaced));
  cache.setQueryData(key, replaced); expect(forget).toHaveBeenCalledExactlyOnceWith(redacted.id); forget.mockClear();
  const error = new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', title: '', detail: '', status: 403, request_id: '' }, null);
  const query = cache.getQueryCache().find({ queryKey: key, exact: true });
  query?.setState({ data: undefined, error, status: 'error' });
  query?.setState({ error, status: 'error' });
  expect(forget).toHaveBeenCalledExactlyOnceWith(redacted.id);
  stop(); cache.clear();
});
