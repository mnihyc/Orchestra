/** A committed acknowledgement completes the actual dialog without inventing a full Forward reply. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import type * as Client from '@/api/client';
import type * as Chats from '@/api/chats';
import type * as Access from '@/api/access';
import type * as Search from '@/api/search';
import type * as Recents from '@/api/pickerRecents';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { orgChatsQuery } from '@/api/chats';
import { ForwardDialog, type ForwardDialogProps, type ForwardSource } from '@/features/forward/ForwardDialog';
import type { ChatPage, NameSearchPage, UserSearchReply } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ADA, GRACE, chat } from '../doubles/chat-rows';
import { show } from './support';

const post = vi.hoisted(() => vi.fn());
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  return { ...actual, api: { ...actual.api, POST: post } };
});
const reads = vi.hoisted(() => { const recents: Record<string, string[]> = {}; return { chats: vi.fn<(org: string, filter: Chats.ChatListFilter, cursor: string | null) => Promise<ChatPage>>(), people: vi.fn<(q: string, org: string | null, cursor: string | null) => Promise<UserSearchReply>>(), names: vi.fn<(search: Search.NameSearch, cursor: string | null) => Promise<NameSearchPage>>(), recents, record: vi.fn<typeof Recents.recordPickerChoice>() }; });
vi.mock('@/api/chats', async (load) => {
  const actual = await load<typeof Chats>();
  return { ...actual, orgChatsQuery: (org: string, filter: Chats.ChatListFilter & { ids?: readonly string[] }) => ({
    ...actual.orgChatsQuery(org, filter), queryFn: ({ pageParam }: { pageParam: string | null }) => reads.chats(org, filter, pageParam),
  }) };
});
vi.mock('@/api/access', async (load) => {
  const actual = await load<typeof Access>();
  return { ...actual, personPickerQuery: (q: string, org: string | null) => ({ ...actual.personPickerQuery(q, org),
    queryFn: ({ pageParam }: { pageParam: string | null }) => reads.people(q, org, pageParam),
  }) };
});
vi.mock('@/api/search', async (load) => {
  const actual = await load<typeof Search>();
  return { ...actual, pickerNamesQuery: (search: Search.NameSearch) => ({ ...actual.pickerNamesQuery(search),
    queryFn: ({ pageParam }: { pageParam: string | null }) => reads.names(search, pageParam),
  }) };
});
vi.mock('@/api/pickerRecents', async (load) => {
  const actual = await load<typeof Recents>();
  return { ...actual, pickerRecentsQuery: (person: string) => ({ ...actual.pickerRecentsQuery(person), queryFn: () => Promise.resolve({ count: 5, choices: reads.recents }) }), recordPickerChoice: reads.record };
});
beforeEach(() => {
  reads.recents = {}; reads.chats.mockReset().mockResolvedValue({ items: [], next_cursor: null });
  reads.people.mockReset().mockResolvedValue({ items: [], next_cursor: null });
  reads.names.mockReset().mockResolvedValue({ items: [], partial: false, next_cursor: null });
  reads.record.mockReset().mockResolvedValue({ count: 5, remembered: true });
});
const session = client.session.getState();
afterEach(async () => { cleanup(); client.session.setState(session); post.mockReset(); await setLocale('en'); });

it('closes on a real accepted receipt, reports success once and never calls the full-result callback or resubmits', async () => {
  client.session.setState({ status: 'signed_in', userId: 'sender' });
  // The caller handles the existing common receipt signal, never a fabricated full delivery projection.
  post.mockResolvedValue({ data: { projection: 'receipt' }, response: new Response(null, { status: 200, headers: { 'x-request-id': 'forward-request' } }) });
  const closed = vi.fn(), forwarded = vi.fn<NonNullable<ForwardDialogProps['onForwarded']>>();
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  const empty: ChatPage = { items: [], next_cursor: null };
  cache.setQueryData(orgChatsQuery('org', { scope: 'mine', of: null }).queryKey, { pages: [empty], pageParams: [null] });
  const source: ForwardSource = { kind: 'thread', threadId: 'source-thread', orgId: 'org', label: 'Source thread', threadChat: { chatId: 'source-chat', label: 'Source chat' }, itemIds: ['source-item'], preview: [{ id: 'source-item', author: 'Sender', at: '2026-10-01T12:00:00Z', text: 'Owned fixture text' }] };
  function Host() {
    const [open, setOpen] = useState(true);
    return <ForwardDialog source={open ? source : null} initialDestinations={[{ key: 'chat:destination', kind: 'group', chatId: 'destination', label: 'Destination', detail: null }]}
      onClose={() => { closed(); setOpen(false); }} onForwarded={forwarded} />;
  }
  const shown = show(<QueryClientProvider client={cache}><Host /></QueryClientProvider>);
  const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: i18n.t('forward.button', { count: 1 }) }));
  expect(await screen.findByText(i18n.t('forward.done', { count: 1 }))).toBeVisible();
  await waitFor(() => expect(screen.queryByRole('dialog', { name: i18n.t('forward.title') })).not.toBeInTheDocument());
  expect(closed).toHaveBeenCalledOnce(); expect(forwarded).not.toHaveBeenCalled();
  expect(post).toHaveBeenCalledOnce();
  const anyClientId: unknown = expect.any(String);
  expect(post).toHaveBeenCalledWith('/forward', expect.objectContaining({ body: { from: { thread_id: 'source-thread', item_ids: ['source-item'] }, to: [{ chat_id: 'destination' }], client_id: anyClientId } }));
  await user.keyboard('{Enter}');
  expect(post).toHaveBeenCalledOnce();
  expect(screen.queryByText(i18n.t('forward.refused'))).not.toBeInTheDocument();
  shown.unmount(); cache.clear();
});

it.each(['en', 'zh-CN'] as const)('finds every native destination from one key and resolves a late recent chat in one batch in %s', async (locale) => {
  await setLocale(locale); client.session.setState({ status: 'signed_in', userId: ADA.id, readerVersion: 611 });
  const late = chat({ id: 'late-chat', kind: 'group', node: null, name: 'Late saved group', members: [ADA, GRACE] });
  const first = chat({ id: 'first-chat', kind: 'group', node: null, name: 'First group', members: [ADA, GRACE] });
  reads.recents = { chat: [late.id] };
  reads.chats.mockImplementation((_org: string, filter: Chats.ChatListFilter & { ids?: readonly string[] }, cursor: string | null) => Promise.resolve({
    items: filter.scope !== 'mine' ? [] : filter.ids !== undefined ? [late] : cursor === null ? [first] : [late],
    next_cursor: filter.scope === 'mine' && filter.ids === undefined && cursor === null ? 'more' : null,
  }));
  const source: ForwardSource = { kind: 'chat', chatId: 'source', orgId: 'org', label: 'Source', messages: [] };
  show(<ForwardDialog source={source} onClose={() => undefined} />);
  const user = userEvent.setup(), input = screen.getByRole('combobox', { name: i18n.t('forward.search') });
  await user.click(input);
  expect(await screen.findByRole('option', { name: late.name ?? '' })).toBeVisible();
  expect(screen.getAllByRole('option')[0]).toHaveAccessibleName(late.name ?? '');
  const batches = reads.chats.mock.calls.filter(([, filter]) => (filter as { ids?: readonly string[] }).ids !== undefined);
  expect(batches).toHaveLength(2); expect(batches.every(([, filter]) => (filter as { ids?: readonly string[] }).ids?.join(',') === late.id)).toBe(true);
  await user.keyboard('{Home}{Enter}');
  await waitFor(() => expect(reads.record).toHaveBeenCalledWith({ kind: 'chat', key: late.id }));
  expect(screen.getByRole('button', { name: i18n.t('forward.remove', { name: late.name ?? '' }) })).toBeVisible();
  await user.click(input); await user.type(input, 'f');
  await waitFor(() => expect(reads.chats).toHaveBeenCalledWith('org', expect.objectContaining({ scope: 'mine', q: 'f' }), null));
  expect(reads.people).toHaveBeenCalledWith('f', 'org', null);
  expect(reads.names).toHaveBeenCalledWith(expect.objectContaining({ q: 'f', kinds: ['thread'], orgId: 'org' }), null);
  expect(screen.getByRole('button', { name: i18n.t('forward.remove', { name: late.name ?? '' }) })).toBeVisible();
  expect(reads.chats.mock.calls.some(([, , cursor]) => cursor === 'more')).toBe(false);
  await user.click(screen.getByRole('button', { name: i18n.t('forward.more', { kind: i18n.t('forward.kind.group') }) }));
  await waitFor(() => expect(reads.chats.mock.calls.some(([, , cursor]) => cursor === 'more')).toBe(true));
});
