/** The rail and Navigation count personal states without reading any org's chat list. */
import { act, screen } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import type * as ChatsApi from '@/api/chats';
import { useUnreadChats } from '@/features/navigation/counts';
import { statesWriter } from '@/realtime/stores/chat';
import { state } from '../doubles/chat-rows';
import { show } from './support';

const world = vi.hoisted(() => ({ read: vi.fn<(orgId: string) => void>() }));
vi.mock('@/api/chats', async (load) => ({ ...await load<typeof ChatsApi>(), orgChatsQuery: (orgId: string) => ({
  queryKey: ['lazy-org-chats', orgId], initialPageParam: null, getNextPageParam: () => undefined,
  queryFn: () => { world.read(orgId); return Promise.resolve({ items: [], next_cursor: null }); },
}) }));
function Avatars() { return <p>Known counts: {useUnreadChats('acme')}, {useUnreadChats('beta')}</p>; }
function Navigation() { return <p>Opened org: {useUnreadChats('beta')}</p>; }

beforeEach(() => { statesWriter.clear(); world.read.mockClear(); });

it('uses bootstrap states and never reads a list when an org avatar or its Navigation opens', async () => {
  statesWriter.loaded([state('a', { org_id: 'acme', last_message_seq: 2 }), state('b', { org_id: 'beta', last_message_seq: 3 })]);
  const view = show(<Avatars />);
  expect(screen.getByText('Known counts: 1, 1')).toBeVisible();
  await act(async () => { await Promise.resolve(); });
  expect(world.read).not.toHaveBeenCalled();
  view.rerender(<><Avatars /><Navigation /></>);
  await screen.findByText('Opened org: 1');
  await act(async () => { await Promise.resolve(); });
  expect(world.read).not.toHaveBeenCalled();
});

it('counts new live states, messages, reads in another browser and hiding through the same source', () => {
  show(<><Avatars /><Navigation /></>);
  expect(screen.getByText('Known counts: 0, 0')).toBeVisible();
  act(() => { statesWriter.put(state('new', { org_id: 'beta', last_message_seq: 1 })); });
  expect(screen.getByText('Known counts: 0, 1')).toBeVisible();
  expect(screen.getByText('Opened org: 1')).toBeVisible();
  act(() => { statesWriter.put(state('new', { org_id: 'beta', last_message_seq: 1, last_read_seq: 1 })); });
  expect(screen.getByText('Opened org: 0')).toBeVisible();
  act(() => { statesWriter.heard('new', 2, null); });
  expect(screen.getByText('Opened org: 1')).toBeVisible();
  act(() => { statesWriter.put(state('new', { org_id: 'beta', last_message_seq: 2, last_read_seq: 1, hidden: true })); });
  expect(screen.getByText('Known counts: 0, 0')).toBeVisible();
  expect(screen.getByText('Opened org: 0')).toBeVisible();
  expect(world.read).not.toHaveBeenCalled();
});
