/** Native system-scope answers update their exact open projection, preserving unrelated scopes. */
import { afterEach, expect, it } from 'vitest';
import { actSucceeded, findAct } from '@/api/acts';
import { queryClient } from '@/api/query';
import { pageFor } from './general-access-fixtures';
import { systemChannel } from '../component/admin-system-fixtures';

afterEach(() => queryClient.clear());

it('applies the returned system defaults page to its native cache key without treating it as a node', async () => {
  const before = pageFor('org');
  const accepted = { ...before, revision: before.revision + 1 };
  queryClient.setQueryData(['defaults', 'system'], before);
  queryClient.setQueryData(['defaults', 'another-place'], before);
  await actSucceeded({ method: 'PATCH', path: '/admin/defaults', answer: accepted });
  expect(queryClient.getQueryData(['defaults', 'system'])).toEqual(accepted);
  expect(queryClient.getQueryData(['defaults', 'another-place'])).toEqual(before);
  expect(queryClient.getQueryData(['node', 'system'])).toBeUndefined();
  expect(findAct('POST', '/admin/defaults/preview')).toMatchObject({ changes: [] });
});

it('adds and replaces an acknowledged system channel without inserting it in an org channel list', async () => {
  const original = systemChannel();
  queryClient.setQueryData(['channels', 'system'], []);
  queryClient.setQueryData(['channels', 'another-org'], []);
  await actSucceeded({ method: 'POST', path: '/admin/notification-channels', answer: original });
  expect(queryClient.getQueryData(['channels', 'system'])).toEqual([original]);
  expect(queryClient.getQueryData(['channels', 'another-org'])).toEqual([]);
  const accepted = { ...original, enabled: false };
  await actSucceeded({ method: 'PATCH', path: `/notification-channels/${original.id}`, answer: accepted });
  expect(queryClient.getQueryData(['channels', 'system'])).toEqual([accepted]);
  expect(queryClient.getQueryData(['channels', 'another-org'])).toEqual([]);
});
