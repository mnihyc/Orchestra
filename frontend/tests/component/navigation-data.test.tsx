/** An org avatar observes cached unread counts; only opening that org loads its chats. */
import { act, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { useUnreadChats } from '@/features/navigation/counts';
import { show } from './support';

const world = vi.hoisted(() => ({ read: vi.fn<(orgId: string) => void>(), states: new Map<string, never>(), latest: new Map<string, never>() }));
vi.mock('@/api/chats', () => ({ orgChatsQuery: (orgId: string) => ({
  queryKey: ['lazy-org-chats', orgId], initialPageParam: null, getNextPageParam: () => undefined,
  queryFn: () => { world.read(orgId); return Promise.resolve({ items: [], next_cursor: null }); },
}) }));
vi.mock('@/realtime/stores/chat', () => ({
  useChatStates: (select: (state: typeof world) => unknown) => select(world),
  isUnread: () => false,
}));
function Avatars() { return <p>Known counts: {useUnreadChats('acme')}, {useUnreadChats('beta')}</p>; }
function Navigation() { return <p>Opened org: {useUnreadChats('beta', true)}</p>; }

it('does not read any org just to draw its avatar, and reads the selected org once when opened', async () => {
  const view = show(<Avatars />);
  await act(async () => { await Promise.resolve(); });
  expect(world.read).not.toHaveBeenCalled();
  view.rerender(<><Avatars /><Navigation /></>);
  await screen.findByText('Opened org: 0');
  await act(async () => { await Promise.resolve(); });
  expect(world.read.mock.calls.map(([org]) => org)).toEqual(['beta']);
});
