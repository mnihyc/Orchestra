/** A committed acknowledgement completes the actual dialog without inventing a full Forward reply. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import type * as Client from '@/api/client';
import { afterEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { orgChatsQuery } from '@/api/chats';
import { ForwardDialog, type ForwardDialogProps, type ForwardSource } from '@/features/forward/ForwardDialog';
import type { ChatPage } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { show } from './support';

const post = vi.hoisted(() => vi.fn());
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  return { ...actual, api: { ...actual.api, POST: post } };
});
const session = client.session.getState();
afterEach(() => { cleanup(); client.session.setState(session); post.mockReset(); });

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
