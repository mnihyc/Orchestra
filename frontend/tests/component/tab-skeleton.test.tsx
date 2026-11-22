/** A tab's body waits as the shape of its own page: a conversation, a list or a page; and says once that it is loading. */
import { act, fireEvent, screen, within } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { useQuery } from '@tanstack/react-query';
import { LiveQueryClient } from '@/api/query';
import { client } from '@/api/client';
import { NetworkError } from '@/lib/requests';
import { SkeletonRows } from '@/ui/Skeleton';

import { shapeOfRoute, shapeOfTab, TabSkeleton } from '@/features/workbench/TabSkeleton';
import { makeTab } from '@/features/workbench/layout';
import { i18n, setLocale } from '@/i18n';
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

it.each(['en', 'zh-CN'] as const)('shows the actual admitted read retry and keeps its deadline when defaults change in %s', async (locale) => {
  await setLocale(locale); vi.useFakeTimers(); const session = client.session.getState();
  client.session.setState({ userId: 'retry-facts-reader', readerVersion: 77 });
  const at = Date.now(), cache = new LiveQueryClient({ defaultOptions: { queries: { retry: 1, retryDelay: 1_000 } } });
  let finish!: (value: string) => void;
  const pending = new Promise<string>((resolve) => { finish = resolve; });
  const read = vi.fn<() => Promise<string>>().mockRejectedValueOnce(new NetworkError('failed')).mockImplementationOnce(() => pending);
  function Body() { const query = useQuery({ queryKey: ['actual-retry-facts'], queryFn: read }); return query.data === undefined ? <SkeletonRows read={query} rows={1} /> : <p>{query.data}</p>; }
  const view = show(<Body />, cache);
  try {
    await act(async () => { await vi.advanceTimersByTimeAsync(0); }); expect(read).toHaveBeenCalledOnce();
    fireEvent.click(screen.getByRole('button', { name: i18n.t('common.details') }));
    const body = document.querySelector<HTMLElement>('[data-fact-disclosure-body]');
    if (body === null) throw new Error('Retry facts must use the shared disclosure');
    expect(within(body).getByText(locale === 'en' ? 'Read attempt' : '读取尝试').closest('div')).toHaveTextContent('1');
    expect(body.querySelector('time')).toHaveAttribute('datetime', new Date(at + 1_000).toISOString());
    cache.setDefaultOptions({ queries: { retry: 1, retryDelay: 9_000 } });
    expect(body.querySelector('time')).toHaveAttribute('datetime', new Date(at + 1_000).toISOString());
    expect(read).toHaveBeenCalledOnce();
    await act(async () => { await vi.advanceTimersByTimeAsync(1_000); }); expect(read).toHaveBeenCalledTimes(2);
    expect(within(body).getByText(locale === 'en' ? 'Read attempt' : '读取尝试').closest('div')).toHaveTextContent('2');
    await act(async () => { finish('Current data'); await vi.advanceTimersByTimeAsync(0); });
    expect(screen.getByText('Current data')).toBeVisible(); expect(document.querySelector('[data-fact-disclosure-body]')).toBeNull();
  } finally { view.unmount(); finish('Current data'); cache.clear(); client.session.setState(session); vi.useRealTimers(); await setLocale('en'); }
});

it('retires retry facts on cancellation and never lends a still-running former read to a replacement reader', async () => {
  vi.useFakeTimers(); const session = client.session.getState();
  const cache = new LiveQueryClient({ defaultOptions: { queries: { retry: 1, retryDelay: 1_000 } } });
  let finish!: (value: string) => void; const pending = new Promise<string>((resolve) => { finish = resolve; });
  const read = vi.fn<() => Promise<string>>().mockRejectedValueOnce(new NetworkError('failed')).mockImplementationOnce(() => pending);
  client.session.setState({ userId: 'former-retry-reader', readerVersion: 90 });
  function Body() { const query = useQuery({ queryKey: ['reader-owned-retry'], queryFn: read }); return <SkeletonRows read={query} rows={1} />; }
  const view = show(<Body />, cache);
  try {
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    expect(screen.getByRole('button', { name: i18n.t('common.details') })).toBeVisible();
    act(() => client.session.setState({ userId: 'replacement-retry-reader', readerVersion: 91 }));
    expect(screen.queryByRole('button', { name: i18n.t('common.details') })).not.toBeInTheDocument();
    await act(async () => { await vi.advanceTimersByTimeAsync(1_000); }); expect(read).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole('button', { name: i18n.t('common.details') })).not.toBeInTheDocument();
    await act(async () => { await cache.cancelQueries({ queryKey: ['reader-owned-retry'], exact: true }); });
    await act(async () => { await vi.advanceTimersByTimeAsync(5_000); }); expect(read).toHaveBeenCalledTimes(2);
  } finally { view.unmount(); finish('Retired data'); cache.clear(); client.session.setState(session); vi.useRealTimers(); }
});
