/** A kept team-chat view does not discover its node or ancestors before a visible slot opens it. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Chats from '@/api/chats';
import userEvent from '@testing-library/user-event';
import { ApiError, client } from '@/api/client';
import { clearChatFocus, requestChatFocus } from '@/features/chat/focus';
import { writeViewChoice } from '@/lib/viewChoices';
import type { ReactNode } from 'react';
import { ChatPanel } from '@/features/chat/ChatPanel';
import { makeTab } from '@/features/workbench/layout';
import { ViewOpeningContext } from '@/features/workbench/viewOpening';
import type { Node, NodeChatOpening } from '@/generated/api-types';
import { treeWriter } from '@/realtime/stores/tree';
import { ACME } from '../doubles/fixtures';
import { type Person, World } from '../doubles/world';
import { nodeChatOpening } from '../doubles/screen-openings';
import { show } from './support';

const reads = vi.hoisted(() => {
  const search: Record<string, string> = {};
  return { node: vi.fn<(id: string) => Promise<Node>>(), opening: vi.fn<(id: string, scope: string, chat: string | null) => Promise<NodeChatOpening>>(), href: '/topic/topic?tab=overview', search, availability: undefined as { ready: boolean; chat?: boolean } | undefined, replace: vi.fn() };
});
vi.mock('@/api/chats', async (load) => ({ ...await load<typeof Chats>(), nodeChatOpeningQuery: (id: string, scope: string, chat: string | null) => ({ queryKey: ['chat-panel-opening', id], queryFn: () => reads.opening(id, scope, chat), staleTime: Infinity }) }));
vi.mock('@/shell/screen', () => ({ useCurrentHeading: () => ({ nodeId: ACME.fixLogin, views: reads.availability }) }));
vi.mock('@tanstack/react-router', () => ({ useRouterState: () => reads.search, useRouter: () => ({ state: { location: { href: reads.href } }, history: { replace: reads.replace } }) }));
vi.mock('@/features/chat/ChatView', () => ({ ChatView: ({ chatId, scope }: { chatId: string; scope?: ReactNode }) => <>{scope}<output aria-label="Visible chat">{chatId}</output></> }));
afterEach(() => { reads.node.mockClear(); reads.opening.mockClear(); reads.search = {}; reads.availability = undefined; reads.replace.mockReset(); clearChatFocus(); treeWriter.clear(); client.session.setState({ userId: null }); });

function native(world: World, person: Person) {
  reads.node.mockImplementation(async (id) => (await world.requestFeeds.read(person, `/nodes/${id}`)).body as Node);
  reads.opening.mockImplementation((id, scope, chat) => nodeChatOpening(world, person, id, new URLSearchParams({ scope, ...(chat === null ? {} : { chat_id: chat }) })));
}

it('makes zero hidden node reads, shares the native shown reads and stays quiet after hiding and invalidation', async () => {
  treeWriter.clear();
  const world = new World(), person = world.people.get('ada');
  if (person === undefined) throw new Error('The panel needs its native reader');
  native(world, person);
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const tab = makeTab('chat');
  const draw = (shown: boolean) => <QueryClientProvider client={cache}><ViewOpeningContext.Provider value={shown ? 'team-chat' : null}><ChatPanel tab={tab} area="right" /></ViewOpeningContext.Provider></QueryClientProvider>;
  const view = show(draw(false));
  try {
    await act(async () => { await Promise.resolve(); });
    expect(reads.node).not.toHaveBeenCalled();
    expect(reads.opening).not.toHaveBeenCalled();
    view.rerender(draw(true));
    expect(await screen.findByLabelText('Visible chat')).toBeVisible();
    await waitFor(() => expect(reads.opening.mock.calls.map(([id]) => id)).toEqual([ACME.fixLogin]));
    expect(screen.getAllByRole('radio').map((radio) => radio.textContent)).toEqual(['Topic', 'Project']);
    view.rerender(draw(false));
    await act(async () => { await cache.invalidateQueries(); });
    expect(reads.opening).toHaveBeenCalledOnce();
    expect(reads.node).not.toHaveBeenCalled();
  } finally { view.unmount(); cache.clear(); }
});

it('lets a person leave a chat deep link, then selects the chat requested by an in-page shortcut', async () => {
  const world = new World(), person = world.people.get('ada');
  if (person === undefined) throw new Error('The panel needs its native reader');
  client.session.setState({ userId: 'chat-scope-reader' });
  writeViewChoice('chat-scope-reader', 'chat', 'scope', 'project');
  native(world, person);
  const topic = await reads.node(ACME.fixLogin), project = await reads.node(ACME.web);
  reads.search = { chat: topic.chat_id, message: 'linked-message', panel: 'chat', tab: 'overview' };
  reads.href = `/topic/${topic.id}?${new URLSearchParams(reads.search).toString()}`;
  reads.replace.mockImplementation((href: string) => { reads.href = href; reads.search = Object.fromEntries(new URL(href, window.location.origin).searchParams); });
  const tab = makeTab('chat');
  show(<ViewOpeningContext.Provider value="team-chat"><ChatPanel tab={tab} area="right" /></ViewOpeningContext.Provider>);
  expect(await screen.findByLabelText('Visible chat')).toHaveTextContent(topic.chat_id);
  await userEvent.click(screen.getByRole('radio', { name: 'Project' }));
  await waitFor(() => expect(screen.getByLabelText('Visible chat')).toHaveTextContent(project.chat_id));
  expect(reads.search).toEqual({ tab: 'overview' });
  act(() => requestChatFocus(topic.chat_id));
  await waitFor(() => expect(screen.getByLabelText('Visible chat')).toHaveTextContent(topic.chat_id));
  act(() => clearChatFocus());
  expect(screen.getByLabelText('Visible chat')).toHaveTextContent(topic.chat_id);
});


it('does not read a private document parent when the focused content has no chat grant', async () => {
  reads.availability = { ready: true, chat: false };
  const tab = makeTab('chat');
  show(<ViewOpeningContext.Provider value="shared-document"><ChatPanel tab={tab} area="right" /></ViewOpeningContext.Provider>);
  await act(async () => { await Promise.resolve(); });
  expect(reads.node).not.toHaveBeenCalled();
  expect(reads.opening).not.toHaveBeenCalled();
  expect(screen.getByText('No chat on this screen')).toBeVisible();
  expect(screen.queryByLabelText('Visible chat')).not.toBeInTheDocument();
});

it('retires following chat content when the current context withdraws its chat grant', async () => {
  const world = new World(), person = world.people.get('ada');
  if (person === undefined) throw new Error('The panel needs its native reader');
  native(world, person);
  reads.availability = { ready: true, chat: true };
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const tab = makeTab('chat');
  const draw = () => <QueryClientProvider client={cache}><ViewOpeningContext.Provider value="changing-grant"><ChatPanel tab={tab} area="right" /></ViewOpeningContext.Provider></QueryClientProvider>;
  const view = show(draw());
  try {
    expect(await screen.findByLabelText('Visible chat')).toBeVisible();
    await waitFor(() => expect(reads.opening).toHaveBeenCalledOnce());
    reads.availability = { ready: true, chat: false };
    view.rerender(draw());
    expect(screen.queryByLabelText('Visible chat')).not.toBeInTheDocument();
    await act(async () => { await cache.invalidateQueries(); });
    expect(reads.opening).toHaveBeenCalledOnce();
    expect(reads.node).not.toHaveBeenCalled();
    expect(screen.getByText('No chat on this screen')).toBeVisible();
  } finally { view.unmount(); cache.clear(); }
});

it('keeps an explicitly addressed chat independent of a focused document without a chat grant', async () => {
  const world = new World(), person = world.people.get('ada');
  if (person === undefined) throw new Error('The panel needs its native reader');
  native(world, person);
  reads.availability = { ready: true, chat: false };
  const tab = makeTab('chat', { node: ACME.web });
  show(<ViewOpeningContext.Provider value="fixed-chat"><ChatPanel tab={tab} area="right" /></ViewOpeningContext.Provider>);
  expect(await screen.findByLabelText('Visible chat')).toBeVisible();
  expect(reads.opening.mock.calls.map(([id]) => id)).toEqual([ACME.web]);
  expect(reads.node).not.toHaveBeenCalled();
});

it('clears previously readable scopes when a fresh native admission is refused', async () => {
  const world = new World(), person = world.people.get('ada');
  if (person === undefined) throw new Error('The panel needs its native reader');
  native(world, person);
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const tab = makeTab('chat');
  const view = show(<QueryClientProvider client={cache}><ViewOpeningContext.Provider value="revoked-chat"><ChatPanel tab={tab} area="right" /></ViewOpeningContext.Provider></QueryClientProvider>);
  try {
    expect(await screen.findByLabelText('Visible chat')).toBeVisible();
    reads.opening.mockRejectedValue(new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', status: 403,
      title: 'Not allowed', detail: 'This place is no longer readable', request_id: 'revoked' }, null));
    await act(async () => { await cache.refetchQueries({ queryKey: ['chat-panel-opening', ACME.fixLogin], exact: true }); });
    await waitFor(() => expect(screen.queryByLabelText('Visible chat')).not.toBeInTheDocument());
    expect(screen.queryByRole('radiogroup')).not.toBeInTheDocument();
    expect(reads.opening).toHaveBeenCalledTimes(2);
  } finally { view.unmount(); cache.clear(); }
});
