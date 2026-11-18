/** A tab's body waits as the shape of its own page: a conversation, a list or a page; and says once that it is loading. */
import { screen } from '@testing-library/react';
import { expect, it } from 'vitest';

import { shapeOfRoute, shapeOfTab, TabSkeleton } from '@/features/workbench/TabSkeleton';
import { makeTab } from '@/features/workbench/layout';
import { i18n } from '@/i18n';
import { show } from './support';

it('takes the shape of the page a route draws', () => {
  expect(shapeOfRoute('/thread/$threadId/')).toBe('conversation');
  expect(shapeOfRoute('/chat/$chatId/')).toBe('conversation');
  expect(shapeOfRoute('/resources/providers/')).toBe('list');
  expect(shapeOfRoute('/resources/providers/$id/')).toBe('page');
  expect(shapeOfRoute(null)).toBe('page');
});

it('takes the shape of a tab from the address it was opened for', () => {
  expect(shapeOfTab(makeTab('page', { href: '/thread/01950000-0000-7000-8000-0000000000bb' }))).toBe('conversation');
  expect(shapeOfTab(makeTab('page', { href: '/resources/providers' }))).toBe('list');
  expect(shapeOfTab(makeTab('file', { env: 'e', thread: '', path: 'a.rs' }))).toBe('page');
});

it.each(['conversation', 'list', 'page'] as const)('says once that the %s is loading', (shape) => {
  show(<TabSkeleton shape={shape} />);
  const waiting = screen.getAllByRole('status');
  expect(waiting).toHaveLength(1);
  expect(waiting[0]).toHaveTextContent(i18n.t('common.loading'));
});
