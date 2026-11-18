/** Automatic catch-up follows the current reader's threshold; an explicit request is independent. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient } from '@tanstack/react-query';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Threads from '@/api/threads';
import { client } from '@/api/client';
import { captureThreadRead } from '@/api/threadRead';
import { NewLine, RequestedCatchupCard } from '@/features/timeline/Catchup';
import { i18n } from '@/i18n';
import { threadWriter } from '@/realtime/stores/thread';
import { meta } from '../doubles/fixtures';
import { snapshot } from '../unit/continuation-fixtures';
import { show } from './support';

const reads = vi.hoisted(() => vi.fn());
vi.mock('@/api/threads', async(original) => ({ ...await original<typeof Threads>(), catchupQuery: () => ({
  queryKey: ['catchup-default'], enabled: true, staleTime: Infinity,
  queryFn: () => { reads(); return Promise.resolve({ items: 5, turns_completed: 2, commands: 1, files_changed: 1, approvals_pending: 0, turns_failed: 0, errors: 0, milestones: [] }); },
}) }));
const previous = client.session.getState();
let cache: QueryClient, id: string;
beforeEach(() => {
  reads.mockClear();
  client.session.setState({ status: 'signed_in', userId: 'catch-up-reader', readerVersion: 1 });
  cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  cache.setQueryData(['me-state'], {});
  const source = snapshot(); id = source.thread.id;
  threadWriter.reset(id, { ...source, thread: { ...source.thread, my_last_read_seq: 0, seq: 5 } }, false, captureThreadRead(id));
});
afterEach(() => { cleanup(); threadWriter.evict(id); cache.clear(); client.session.setState(previous); });
function defaults(count: number) {
  const metadata = meta();
  act(() => { cache.setQueryData(['meta'], { ...metadata, interface_defaults: { ...metadata.interface_defaults, catch_up_above_items: count, catch_up_above_items_source: { scope: 'system', node: null } } }); });
}

it('discloses the current catch-up count and its real source without starting another read', async() => {
  defaults(80); show(<NewLine threadId={id}/>, cache);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.details') }));
  expect(await screen.findByText(/Automatic catch-up after 80 unread items/u)).toBeVisible();
  expect(screen.getByRole('link', { name: /system/iu })).toHaveAttribute('href', '/admin/settings?setting=thread.catch_up_above_items');
  expect(reads).not.toHaveBeenCalled();
  act(() => { cache.setQueryData(['me-state'], { interface: { catch_up_above_items: 2 } }); });
  expect(await screen.findByText(/Automatic catch-up after 2 unread items/u)).toBeVisible();
  expect(screen.getByRole('link', { name: 'Your Interface choice' })).toHaveAttribute('href', '/settings/interface#interface-reading');
  await waitFor(() => expect(reads).toHaveBeenCalledOnce());
});

it('rechecks a cached catch-up against changed installation and personal thresholds', async() => {
  defaults(3); show(<NewLine threadId={id}/>, cache);
  expect(await screen.findByRole('region', { name: i18n.t('timeline.catchup.title') })).toBeVisible();
  defaults(80);
  await waitFor(() => expect(screen.queryByRole('region', { name: i18n.t('timeline.catchup.title') })).not.toBeInTheDocument());
  act(() => { cache.setQueryData(['me-state'], { interface: { catch_up_above_items: 0 } }); });
  expect(await screen.findByRole('region', { name: i18n.t('timeline.catchup.title') })).toBeVisible();
  expect(reads).toHaveBeenCalledOnce();
  act(() => { cache.setQueryData(['me-state'], {}); cache.removeQueries({ queryKey: ['meta'], exact: true }); });
  await waitFor(() => expect(screen.queryByRole('region', { name: i18n.t('timeline.catchup.title') })).not.toBeInTheDocument());
  expect(screen.getByText(i18n.t('timeline.newLine'))).toBeVisible();
  expect(reads).toHaveBeenCalledOnce();
});

it('keeps an explicit catch-up request available below the automatic threshold', async() => {
  defaults(80); const view = show(<NewLine threadId={id}/>, cache);
  expect(reads).not.toHaveBeenCalled();
  view.rerender(<RequestedCatchupCard threadId={id}/>);
  expect(await screen.findByRole('region', { name: i18n.t('timeline.catchup.title') })).toBeVisible();
  expect(reads).toHaveBeenCalledOnce();
});

it('waits for the current readers personal choice before starting automatic catch-up', async() => {
  defaults(3); cache.removeQueries({ queryKey: ['me-state'], exact: true });
  show(<NewLine threadId={id}/>, cache);
  expect(reads).not.toHaveBeenCalled();
  act(() => { cache.setQueryData(['me-state'], { interface: { catch_up_above_items: 80 } }); });
  expect(reads).not.toHaveBeenCalled();
  expect(screen.queryByRole('region', { name: i18n.t('timeline.catchup.title') })).not.toBeInTheDocument();
  act(() => { cache.setQueryData(['me-state'], { interface: { catch_up_above_items: 1 } }); });
  expect(await screen.findByRole('region', { name: i18n.t('timeline.catchup.title') })).toBeVisible();
  expect(reads).toHaveBeenCalledOnce();
});
