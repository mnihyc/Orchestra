/** Group Share uses actual manager facts and never borrows the generic permission page. */
import { queryOptions } from '@tanstack/react-query';
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type * as Chats from '@/api/chats';
import type * as ChatStore from '@/realtime/stores/chat';
import { client, ReceiptAcknowledged } from '@/api/client';
import { ChatAccessPanel, GroupAccessSummary } from '@/features/access/GroupAccess';
import type { Chat } from '@/api/chat-contract';
import type { UserRef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import type { PlacePickerProps } from '@/features/settings/PlacePicker';
import { chatWriter } from '@/realtime/stores/chat';
import { ADA, DORA, GRACE, chat } from '../doubles/chat-rows';
import { show } from './support';

const requests = vi.hoisted(() => ({ get: vi.fn<() => Promise<Chat>>(), update: vi.fn<typeof Chats.updateGroup>(), manager: vi.fn<typeof Chats.setGroupManager>(), add: vi.fn<typeof Chats.addMember>(), remove: vi.fn<typeof Chats.removeMember>(), refresh: vi.fn<typeof ChatStore.refreshChat>() }));
vi.mock('@/api/chats', async (load) => ({ ...await load<typeof Chats>(), chatQuery: (id: string) => queryOptions({ queryKey: ['chat', id], queryFn: requests.get }), updateGroup: requests.update, setGroupManager: requests.manager, addMember: requests.add, removeMember: requests.remove }));
vi.mock('@/realtime/stores/chat', async (load) => ({ ...await load<typeof ChatStore>(), refreshChat: requests.refresh }));
vi.mock('@/features/chat/NewChat', () => ({ PeoplePicker: ({ chosen, onToggle }: { chosen: UserRef[]; onToggle: (user: UserRef) => void }) => <button aria-pressed={chosen.some((person) => person.id === DORA.id)} onClick={() => onToggle(DORA)}>{DORA.display_name}</button> }));
vi.mock('@/features/settings/PlacePicker', () => ({ PlacePicker: ({ onChange, rootOrgId, includeThreads, disabled }: PlacePickerProps) => <button disabled={disabled} onClick={() => onChange({ id: 'review-thread', kind: 'thread', color: null, folder: false, path: 'Example / Review' })}>{`Choose thread:${rootOrgId}:${includeThreads === true}`}</button> }));
const appearance = useAppearance.getState();
const session = client.session.getState();
const oldTheme = document.documentElement.dataset.theme;
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
function group(more: Partial<Chat> = {}): Chat {
  return chat({ kind: 'group', node: null, name: 'Release group', home: { id: '00000000-0000-4000-8002-000000000064', kind: 'org', name: 'Example', redacted: false }, visibility: 'private', members: [ADA, GRACE], managers: [ADA.id], can: { ...chat().can, moderate: true, manage: true }, ...more });
}
function open(row: Chat) { requests.get.mockResolvedValue(row); return show(<ChatAccessPanel id={row.id} fallback={<p>Generic permission page</p>} />); }
beforeEach(() => { vi.clearAllMocks(); requests.get.mockReset(); requests.update.mockReset(); requests.manager.mockReset(); requests.add.mockReset(); requests.remove.mockReset(); client.session.setState({ userId: ADA.id }); });
afterEach(async () => { cleanup(); vi.restoreAllMocks(); client.session.setState(session); useAppearance.setState(appearance); document.documentElement.classList.remove('dark'); if (oldTheme === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = oldTheme; await setLocale('en'); });

describe.each(modes)('group access in %s/%s', (locale, theme) => {
  beforeEach(async () => { await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark'); });
  it('shows the real roster and manager guard, with one visibility choice and a real chat link', async () => {
    const row = group({ guests: [GRACE.id], membership_expiries: { [GRACE.id]: '2030-01-01T12:00:00Z' } }); open(row);
    expect(await screen.findByRole('table', { name: i18n.t('chat.group.people') })).toBeVisible();
    expect(screen.queryByText('Generic permission page')).not.toBeInTheDocument();
    expect(screen.getByText(i18n.t('chat.people.guestChip'))).toBeVisible();
    expect(document.querySelector('time[datetime="2030-01-01T12:00:00Z"]')).toBeVisible();
    expect(screen.getByRole('combobox', { name: i18n.t('chat.group.visibility') })).toHaveTextContent(i18n.t('chat.group.private'));
    expect(screen.getByRole('button', { name: i18n.t('places.menu.copyLink') })).toBeVisible();
    expect(screen.getByRole('button', { name: i18n.t('chat.group.removePerson', { name: ADA.display_name }) })).toHaveAttribute('aria-disabled', 'true');
    expect(screen.getByRole('combobox', { name: i18n.t('chat.group.memberRole', { name: ADA.display_name }) })).toBeDisabled();
    const user = userEvent.setup();
    requests.manager.mockResolvedValueOnce(group({ ...row, seq: 2, managers: [ADA.id, GRACE.id] }));
    await user.click(screen.getByRole('combobox', { name: i18n.t('chat.group.memberRole', { name: GRACE.display_name }) }));
    await user.click(screen.getByRole('option', { name: i18n.t('chat.group.manager') }));
    await waitFor(() => expect(requests.manager).toHaveBeenCalledExactlyOnceWith(row.id, GRACE.id, true));
  });
  it('does not let a historic creator without current manager authority change visibility or membership', async () => {
    const row = group({ managers: [GRACE.id], can: { ...chat().can, manage: false } }); open(row);
    await screen.findByRole('table');
    expect(screen.getByRole('combobox', { name: i18n.t('chat.group.visibility') })).toBeDisabled();
    expect(screen.getByRole('button', { name: i18n.t('chat.group.add') })).toHaveAttribute('aria-disabled', 'true');
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: i18n.t('chat.group.add') }));
    expect(screen.queryByRole('button', { name: DORA.display_name })).not.toBeInTheDocument();
    expect(requests.add).not.toHaveBeenCalled(); expect(requests.update).not.toHaveBeenCalled();
  });
  it('keeps a refusal beside its unchanged choice and handles an accepted receipt without fabricating a Chat', async () => {
    const row = group(); open(row); await screen.findByRole('table'); const user = userEvent.setup();
    requests.update.mockRejectedValueOnce(new Error('Native visibility refusal'));
    await user.click(screen.getByRole('combobox', { name: i18n.t('chat.group.visibility') }));
    await user.click(screen.getByRole('option', { name: i18n.t('chat.group.public') }));
    expect(await screen.findByRole('alert')).toBeVisible();
    expect(screen.getByRole('combobox', { name: i18n.t('chat.group.visibility') })).toHaveTextContent(i18n.t('chat.group.private'));
    expect(requests.update).toHaveBeenCalledExactlyOnceWith(row.id, { visibility: 'public' });
    requests.update.mockRejectedValueOnce(new ReceiptAcknowledged({ projection: 'receipt', chat_id: row.id }, 200, 'group-visible'));
    await user.click(screen.getByRole('combobox', { name: i18n.t('chat.group.visibility') }));
    await user.click(screen.getByRole('option', { name: i18n.t('chat.group.public') }));
    await waitFor(() => expect(requests.refresh).toHaveBeenCalledWith(row.id));
    expect(requests.update).toHaveBeenCalledTimes(2);
    expect(screen.getByRole('combobox', { name: i18n.t('chat.group.visibility') })).toHaveTextContent(i18n.t('chat.group.private'));
  }, 10_000);
  it('adds real selected members and states the public home with the existing reference chip', async () => {
    const row = group(); open(row); await screen.findByRole('table'); const user = userEvent.setup();
    requests.add.mockResolvedValueOnce(group({ members: [ADA, GRACE, DORA], seq: 3 }));
    await user.click(screen.getByRole('button', { name: i18n.t('chat.group.add') }));
    await user.click(screen.getByRole('button', { name: DORA.display_name }));
    await user.click(screen.getByRole('button', { name: i18n.t('common.add') }));
    await waitFor(() => expect(requests.add).toHaveBeenCalledExactlyOnceWith(row.id, DORA.id));
    cleanup(); show(<GroupAccessSummary chat={group({ visibility: 'public' })} me={GRACE.id} />);
    expect(screen.getByText(i18n.t('chat.group.member'))).toBeVisible();
    expect(screen.getByText(i18n.t('chat.group.publicIn'))).toBeVisible();
    expect(screen.getByRole('link', { name: 'Example' })).toBeVisible();
  });
  it('moves to a thread of the actual root, retaining the selected home on refusal', async () => {
    const row = group(); open(row); await screen.findByRole('table'); const user = userEvent.setup();
    requests.update.mockRejectedValueOnce(new Error('Native move refusal'));
    await user.click(screen.getByRole('button', { name: i18n.t('chat.group.moveHome') }));
    const picker = screen.getByRole('button', { name: `Choose thread:${row.org_id}:true` });
    await user.click(picker);
    await user.click(screen.getByRole('button', { name: i18n.t('chat.group.move') }));
    expect(await screen.findByRole('alert')).toBeVisible();
    expect(requests.update).toHaveBeenCalledExactlyOnceWith(row.id, { home_id: 'review-thread' });
    expect(picker).toBeVisible();
    requests.update.mockRejectedValueOnce(new ReceiptAcknowledged({ projection: 'receipt', chat_id: row.id }, 200, 'move-accepted'));
    await user.click(screen.getByRole('button', { name: i18n.t('chat.group.move') }));
    await waitFor(() => expect(picker).not.toBeInTheDocument());
    expect(requests.update).toHaveBeenCalledTimes(2); expect(requests.refresh).toHaveBeenCalledWith(row.id);
  });
});

it('does not publish a home change after the reader changes', async () => {
  let finish: ((row: Chat) => void) | undefined;
  requests.update.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
  const apply = vi.spyOn(chatWriter, 'apply'), row = group(), user = userEvent.setup();
  open(row); await screen.findByRole('table');
  await user.click(screen.getByRole('button', { name: i18n.t('chat.group.moveHome') }));
  await user.click(screen.getByRole('button', { name: `Choose thread:${row.org_id}:true` }));
  await user.click(screen.getByRole('button', { name: i18n.t('chat.group.move') }));
  await waitFor(() => expect(requests.update).toHaveBeenCalledOnce());
  const settle = finish; if (settle === undefined) throw new Error('Move did not start');
  act(() => client.session.setState({ readerVersion: (client.session.getState().readerVersion ?? 0) + 1 }));
  await act(async () => { settle(group({ home: { id: 'review-thread', kind: 'thread', name: 'Review', redacted: false }, seq: 9 })); await Promise.resolve(); });
  expect(apply).not.toHaveBeenCalled(); expect(requests.refresh).not.toHaveBeenCalled();
  expect(screen.queryByRole('button', { name: i18n.t('chat.group.move') })).toBeNull();
});

it('keeps the native redacted home protected', () => {
  show(<GroupAccessSummary chat={group({ home: { id: 'private-home', kind: 'topic', name: 'Do not disclose this name', redacted: true } })} me={ADA.id} />);
  expect(screen.getByText(i18n.t('people.privateReference'))).toBeVisible();
  expect(screen.queryByText('Do not disclose this name')).toBeNull();
});
