/** A permitted nonmember chat names its page from the same authorized opening as its visible body. */
import { act, cleanup, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Chats from '@/api/chats';
import type * as Nodes from '@/api/nodes';
import type * as ChatView from '@/features/chat/ChatView';
import { Messages } from '@/features/chat/Messages';
import { chatWriter } from '@/realtime/stores/chat';
import { i18n, setLocale } from '@/i18n';
import { chat } from '../doubles/chat-rows';
import { org, userRef, me } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ heading: vi.fn(), standalone: vi.fn() }));
const root = org('heading-org', 'acme', 'Acme', userRef(me(1, 'ada', 'Ada')));
const inspected = chat({ id: 'heading-inspected-chat', org_id: root.id, kind: 'group', node: null, name: 'Release review',
  can: { admin_only: [], participating: false, send: false, moderate: false, export: false, reason: 'inspecting' } });
vi.mock('@/api/nodes', async (load) => ({ ...await load<typeof Nodes>(),
  orgBySlugQuery: () => ({ queryKey: ['heading-org'], queryFn: () => Promise.resolve(root) }),
}));
vi.mock('@/api/chats', async (load) => ({ ...await load<typeof Chats>(),
  chatQuery: (id: string) => ({ queryKey: ['heading-standalone', id], queryFn: calls.standalone }),
  orgChatsQuery: () => ({ queryKey: ['heading-own-chats'], initialPageParam: null,
    queryFn: () => Promise.resolve({ items: [], next_cursor: null }), getNextPageParam: () => undefined }),
}));
vi.mock('@/shell/screen', () => ({ useScreenHeading: calls.heading }));
vi.mock('@/shell/org', () => ({ selectOrg: vi.fn() }));
vi.mock('@/realtime/live', async (load) => ({ ...await load<object>(), useOrgTreeFeed: vi.fn() }));
vi.mock('@/features/chat/ChatView', async (load) => ({ ...await load<typeof ChatView>(), ChatView: () => null }));
vi.mock('@/features/chat/AllChats', () => ({ AllChats: () => null }));
vi.mock('@/features/chat/NewChat', () => ({ NewChat: () => null }));

afterEach(async () => { cleanup(); chatWriter.evict(inspected.id); vi.clearAllMocks(); await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('follows the authorized nonmember name and retires it on revocation without another read in %s', async (locale) => {
  await setLocale(locale);
  calls.standalone.mockRejectedValue(new Error('An unrelated standalone read must not supply this heading'));
  show(<Messages slug="acme" search={{ chat: inspected.id }} onSearch={vi.fn()} />);
  await waitFor(() => expect(calls.heading).toHaveBeenLastCalledWith(expect.objectContaining({ title: i18n.t('screens.messages.title') })));
  act(() => chatWriter.reset(inspected.id, inspected, [], false, 1));
  await waitFor(() => expect(calls.heading).toHaveBeenLastCalledWith(expect.objectContaining({ title: i18n.t('chat.title.named', { name: inspected.name }) })));
  act(() => chatWriter.apply(inspected.id, { chat: { ...inspected, name: 'Updated review' } }, 2));
  await waitFor(() => expect(calls.heading).toHaveBeenLastCalledWith(expect.objectContaining({ title: i18n.t('chat.title.named', { name: 'Updated review' }) })));
  act(() => chatWriter.closed(inspected.id, 'revoked'));
  await waitFor(() => expect(calls.heading).toHaveBeenLastCalledWith(expect.objectContaining({ title: i18n.t('screens.messages.title') })));
  expect(calls.standalone).not.toHaveBeenCalled();
});
