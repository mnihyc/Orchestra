/** Current document answers enter only the loaded lists whose native scope and filters they satisfy. */
import { QueryClient } from '@tanstack/react-query';
import { expect, it } from 'vitest';
import { answerChanges } from '@/api/actAnswers';
import { applyChanges } from '@/api/acts';

const list = (node: string, under = false, deleted = false, participating = false) => ['documents', node, under, deleted, participating] as const;
const page = (items: unknown[]) => ({ pages: [{ items, next_cursor: null }], pageParams: [null] });
const doc = { id: 'document', title: 'Plan', node: { id: 'project-a' }, path: [{ id: 'org-a' }, { id: 'project-a' }],
  deleted: false, seq: 2, access: { permissions: ['doc.view', 'doc.manage'], participating: true } };
const items = (cache: QueryClient, key: readonly unknown[]) => cache.getQueryData<{ pages: { items: unknown[] }[] }>(key)?.pages[0]?.items;

it('a restore moves its returned row from Deleted into current and participating lists without any read', async () => {
  const cache = new QueryClient();
  cache.setQueryData(list('project-a'), page([])); cache.setQueryData(list('project-a', false, false, true), page([]));
  cache.setQueryData(list('project-a', false, true), page([{ ...doc, deleted: true, seq: 1 }]));
  cache.setQueryData(list('project-b'), page([]));
  const changes = answerChanges({ method: 'POST', path: '/documents/document/restore', answer: doc }, () => 'document', [{ key: ['documents'] }]);
  expect(changes.every((change) => 'row' in change)).toBe(true);
  await applyChanges(changes, cache);
  expect(items(cache, list('project-a'))).toEqual([doc]);
  expect(items(cache, list('project-a', false, false, true))).toEqual([doc]);
  expect(items(cache, list('project-a', false, true))).toEqual([]);
  expect(items(cache, list('project-b'))).toEqual([]); cache.clear();
});

it('a move uses the actual current path for descendant lists and never inserts an administrative-only row into participation', async () => {
  const cache = new QueryClient();
  cache.setQueryData(list('project-a'), page([doc])); cache.setQueryData(list('project-b'), page([]));
  cache.setQueryData(list('org-a', true), page([doc])); cache.setQueryData(list('unrelated-org', true), page([]));
  cache.setQueryData(list('project-b', false, false, true), page([]));
  const moved = { ...doc, node: { id: 'project-b' }, path: [{ id: 'org-a' }, { id: 'project-b' }], access: { ...doc.access, participating: false } };
  const changes = answerChanges({ method: 'POST', path: '/documents/document/move', answer: moved }, () => 'document', [{ key: ['documents'] }]);
  await applyChanges(changes, cache);
  expect(items(cache, list('project-a'))).toEqual([]); expect(items(cache, list('project-b'))).toEqual([moved]);
  expect(items(cache, list('org-a', true))).toEqual([moved]); expect(items(cache, list('unrelated-org', true))).toEqual([]);
  expect(items(cache, list('project-b', false, false, true))).toEqual([]); cache.clear();
});

it('a renamed reference leaves a loaded title search while keeping its continuation', async () => {
  const cache = new QueryClient();
  const key = [...list('project-a', true), 'references', 'handbook', null];
  cache.setQueryData(key, { items: [{ ...doc, title: 'Handbook' }], next_cursor: 'older-matches' });
  await applyChanges([{ row: { kind: 'document', id: doc.id, value: { ...doc, title: 'Other notes' } } }], cache);
  expect(cache.getQueryData(key)).toEqual({ items: [], next_cursor: 'older-matches' });
  cache.clear();
});
