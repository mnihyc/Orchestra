/** Native tool facts page on intent and retire with their session or reader. */
import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as ThreadsApi from '@/api/threads';
import { client } from '@/api/client';
import { OrchToolsFacts } from '@/features/inspector/OrchToolsFacts';
import type { OrchToolsPage } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';
const server = vi.hoisted(() => ({ read: vi.fn<(cursor: string | null) => Promise<OrchToolsPage>>() }));
vi.mock('@/api/threads', async (load) => ({ ...await load<typeof ThreadsApi>(), orchToolsQuery: (id: string, revision: string | null, search: string, reader: string) => ({
  queryKey: ['accepted-tools-case', id, revision, search, reader], initialPageParam: null as string | null,
  queryFn: ({ pageParam }: { pageParam: string | null }) => server.read(pageParam), getNextPageParam: (page: OrchToolsPage) => page.tools.next_cursor ?? undefined,
}) }));
const summary = { session_revision: 'accepted-one', offered_count: 1, accepted_at: '2026-11-15T03:00:00Z' };
beforeEach(() => { server.read.mockReset(); client.session.setState({ status: 'signed_in', userId: 'reader' }); });
afterEach(async () => { act(() => client.session.setState({ status: 'signed_out', userId: null })); await setLocale('en'); });
it.each(['en', 'zh-CN'] as const)('follows native continuation and hides a former session and reader in %s', async (locale) => {
  await setLocale(locale);
  server.read.mockImplementation((cursor) => Promise.resolve({ summary, tools: cursor === null ? { items: [{ key: 'read', offered: true, reason: null, source: null }], next_cursor: 'next' }
    : { items: [{ key: 'write', offered: false, reason: 'read_only', source: null }], next_cursor: null } }));
  const user = userEvent.setup();
  const page = show(<OrchToolsFacts thread="thread" summary={summary} />);
  expect(server.read).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: i18n.t('common.details') }));
  await waitFor(() => expect(server.read).toHaveBeenCalledOnce());
  await user.click(await screen.findByRole('button', { name: i18n.t('orchToolFacts.more') }));
  expect(await screen.findByText(i18n.t('orchToolFacts.reasons.readOnly'))).toBeVisible();
  page.rerender(<OrchToolsFacts thread="thread" summary={{ ...summary, session_revision: 'accepted-two' }} />);
  expect(screen.queryByText(i18n.t('orchToolFacts.reasons.readOnly'))).toBeNull();
  expect(server.read).toHaveBeenCalledTimes(2);
  act(() => client.session.setState({ status: 'signed_in', userId: 'another-reader' }));
  expect(screen.queryByRole('dialog')).toBeNull();
});

it.each(['en', 'zh-CN'] as const)('closes former setting-source facts as current authority narrows in %s', async (locale) => {
  await setLocale(locale);
  server.read.mockResolvedValue({ summary, tools: { items: [{ key: 'read', offered: true, reason: null, source: null }], next_cursor: null } });
  const user = userEvent.setup();
  const page = show(<OrchToolsFacts thread="thread" summary={summary} authority="may-read-source" />);
  await user.click(screen.getByRole('button', { name: i18n.t('common.details') }));
  expect(await screen.findByText(i18n.t('orchToolFacts.offered'))).toBeVisible();
  page.rerender(<OrchToolsFacts thread="thread" summary={summary} authority="thread-view-only" />);
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(server.read).toHaveBeenCalledOnce();
});
