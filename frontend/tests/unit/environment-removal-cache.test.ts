/** A completed removal leaves active lists but keeps the native authorized retained-data answer. */
import { afterEach, expect, it } from 'vitest';
import { actSucceeded } from '@/api/acts';
import { queryClient } from '@/api/query';
import type { Environment } from '@/generated/api-types';
import { detailEnvironment } from '../component/resource-detail-fixtures';

afterEach(() => queryClient.clear());
it('applies the delete answer to ordinary and inventory lists without discarding the retained detail', async () => {
  const first = { ...detailEnvironment, id: 'deleted', name: 'Disposable' };
  const other = { ...detailEnvironment, id: 'lasting', name: 'Keep' };
  const inventory = ['everything', 'team', 'resources', { kinds: ['environment'], resource: { details: true } }] as const;
  const wrap = (value: typeof first) => ({ id: value.id, kind: 'environment', resource: { kind: 'environment', value } });
  queryClient.setQueryData(['environments'], [first, other]);
  queryClient.setQueryData(inventory, { items: [wrap(first), wrap(other)], next_cursor: null });
  queryClient.setQueryData(['environment', first.id], first);
  const answer: Environment = { ...first, status: 'removed', remove_at: null, removal: {
    phase: 'kept', keep_days: 7, purge_at: '2026-10-23T12:00:00Z', deleted_at: '2026-10-16T12:00:00Z', deleted_by: first.owner,
    purged_at: null, bytes: null, size_at: null, reason: null, revision: 1, can_inspect: true, can_restore: true, can_purge: false, job: null,
  } };
  await actSucceeded({ method: 'DELETE', path: '/environments/deleted', answer });
  expect(queryClient.getQueryData(['environments'])).toEqual([other]);
  expect(queryClient.getQueryData(inventory)).toEqual({ items: [wrap(other)], next_cursor: null });
  expect(queryClient.getQueryData(['environment', first.id])).toEqual(answer);
});
