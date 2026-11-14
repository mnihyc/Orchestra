/** Opening or sharing a discovered group never joins it; Join is the native explicit act. */
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Chats from '@/api/chats';
import type * as ChatStore from '@/realtime/stores/chat';
import type * as Access from '@/features/access/AccessDialog';
import type * as Toast from '@/ui/Toast';
import type { Chat } from '@/api/chat-contract';
import { client } from '@/api/client';
import { GroupHeading } from '@/features/chat/GroupHeading';
import { useGroupActions } from '@/features/chat/groupActions';
import { i18n, setLocale } from '@/i18n';
import { linkTo } from '@/lib/links';
import { chatWriter } from '@/realtime/stores/chat';
import { Button } from '@/ui/Button';
import { ContextMenu } from '@/ui/ContextMenu';
import { Menu } from '@/ui/Menu';
import { ADA, chat } from '../doubles/chat-rows';
import { show } from './support';

const calls = vi.hoisted(() => ({ join: vi.fn<typeof Chats.joinGroup>(), refresh: vi.fn<typeof ChatStore.refreshChat>(), open: vi.fn<(href: string) => void>(), share: vi.fn<typeof Access.openAccess>(), toast: vi.fn<typeof Toast.toast>() }));
vi.mock('@/api/chats', async (original) => ({ ...await original<typeof Chats>(), joinGroup: calls.join }));
vi.mock('@/realtime/stores/chat', async (original) => ({ ...await original<typeof ChatStore>(), refreshChat: calls.refresh }));
vi.mock('@/features/access/AccessDialog', async (loadOriginal) => ({ ...await loadOriginal<object>(), openAccess: calls.share }));
vi.mock('@/ui/Toast', async (original) => ({ ...await original<typeof Toast>(), toast: calls.toast }));
function Actions({ row }: { row: Chat }) {
  const group = useGroupActions(row, calls.open);
  return <ContextMenu label="Row actions" entries={group.entries}><div data-testid="group-row">
    <Menu label="Row actions" entries={group.entries} trigger={<Button>Row actions</Button>} />
  </div></ContextMenu>;
}
const group = (more: Partial<Chat> = {}): Chat => chat({ kind: 'group', node: null, name: 'Review', home: { id: 'topic', kind: 'topic', name: 'Release', redacted: false },
  visibility: 'public', members: [], managers: [], can: { ...chat().can, participating: false, join: true }, ...more });
const draw = (row = group()) => show(<><GroupHeading chat={row} /><Actions row={row} /></>);
const session = client.session.getState();
beforeEach(() => { vi.clearAllMocks(); calls.join.mockReset(); calls.join.mockResolvedValue(undefined); client.session.setState({ userId: ADA.id, readerVersion: 601 }); });
afterEach(async () => { cleanup(); vi.restoreAllMocks(); client.session.setState(session); await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('opens and shares the same group from its menu and context menu without joining in %s', async (locale) => {
  await setLocale(locale); const row = group(), user = userEvent.setup(); draw(row);
  expect(screen.getByRole('link', { name: 'Release' })).toBeVisible();
  expect(screen.getByText(i18n.t('chat.group.publicIn'))).toBeVisible();
  expect(calls.join).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Row actions' }));
  await user.click(screen.getByRole('menuitem', { name: i18n.t('chat.group.message') }));
  expect(calls.open).toHaveBeenCalledExactlyOnceWith(linkTo({ kind: 'chat', id: row.id }));
  fireEvent.contextMenu(screen.getByTestId('group-row'));
  await user.click(await screen.findByRole('menuitem', { name: i18n.t('everything.share') }));
  expect(calls.share).toHaveBeenCalledExactlyOnceWith({ kind: 'chat', id: row.id, name: 'Review' });
  expect(calls.join).not.toHaveBeenCalled();
});

it('offers no Join when native eligibility is absent even for a public group', async () => {
  draw(group({ can: { ...chat().can, participating: false } }));
  expect(screen.queryByRole('button', { name: i18n.t('chat.group.join') })).toBeNull();
  await userEvent.click(screen.getByRole('button', { name: 'Row actions' }));
  expect(screen.queryByRole('menuitem', { name: i18n.t('chat.group.join') })).toBeNull();
  expect(calls.join).not.toHaveBeenCalled();
});

it('shares pending Join between header and row, then refreshes native facts without a made-up Chat', async () => {
  let finish: (() => void) | undefined;
  calls.join.mockImplementation(() => new Promise<void>((resolve) => { finish = resolve; }));
  const apply = vi.spyOn(chatWriter, 'apply'), row = group(), user = userEvent.setup(); draw(row);
  await user.click(screen.getByRole('button', { name: 'Row actions' }));
  await user.click(screen.getByRole('menuitem', { name: i18n.t('chat.group.join') }));
  await waitFor(() => expect(calls.join).toHaveBeenCalledExactlyOnceWith(row.id));
  const headerJoin = screen.getByRole('button', { name: i18n.t('chat.group.join') });
  await waitFor(() => expect(headerJoin).toHaveAttribute('aria-disabled', 'true'));
  await user.click(headerJoin); expect(calls.join).toHaveBeenCalledOnce();
  const settle = finish; if (settle === undefined) throw new Error('Join did not start');
  await act(async () => { settle(); await Promise.resolve(); });
  await waitFor(() => expect(calls.refresh).toHaveBeenCalledExactlyOnceWith(row.id));
  expect(apply).not.toHaveBeenCalled();
});

it('keeps a native join refusal retryable and retires a response from the previous reader', async () => {
  calls.join.mockRejectedValueOnce(new Error('Native membership refusal'));
  const user = userEvent.setup(); draw();
  await user.click(screen.getByRole('button', { name: i18n.t('chat.group.join') }));
  expect(await screen.findByRole('alert')).toHaveTextContent(i18n.t('errors.page'));
  expect(calls.refresh).not.toHaveBeenCalled();
  let finish: (() => void) | undefined;
  calls.join.mockImplementation(() => new Promise<void>((resolve) => { finish = resolve; }));
  await user.click(screen.getByRole('button', { name: i18n.t('chat.group.join') }));
  await waitFor(() => expect(calls.join).toHaveBeenCalledTimes(2));
  const settle = finish; if (settle === undefined) throw new Error('Join retry did not start');
  act(() => client.session.setState({ readerVersion: 602 }));
  await act(async () => { settle(); await Promise.resolve(); });
  expect(calls.refresh).not.toHaveBeenCalled(); expect(calls.toast).not.toHaveBeenCalled();
});
