/** Group acknowledgements finish committed acts without fake rows or replaying earlier additions. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import type * as Chats from '@/api/chats';
import { ApiError, ReceiptAcknowledged } from '@/api/client';
import { GroupMenu } from '@/features/chat/GroupMenu';
import type { UserRef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { chatWriter, refreshChat, useChatStore } from '@/realtime/stores/chat';
import { toast } from '@/ui/Toast';
import { ADA, DORA, GRACE, chat } from '../doubles/chat-rows';
import { show } from './support';

const requests = vi.hoisted(() => ({ rename: vi.fn<typeof Chats.renameChat>(), add: vi.fn<typeof Chats.addMember>() }));
vi.mock('@/api/chats', async (load) => ({ ...await load<typeof Chats>(), renameChat: requests.rename, addMember: requests.add }));
vi.mock('@/features/export/ChatMenu', () => ({ useChatExportEntries: () => [] }));
vi.mock('@/features/chat/NewChat', () => ({
  PeoplePicker: ({ chosen, onToggle }: { chosen: UserRef[]; onToggle: (user: UserRef) => void }) => <>{[DORA, GRACE].map((person) => <button key={person.id} aria-pressed={chosen.some((one) => one.id === person.id)} onClick={() => onToggle(person)}>{person.display_name}</button>)}</>,
}));
vi.mock('@/realtime/stores/chat', async (load) => ({ ...await load<typeof import('@/realtime/stores/chat')>(), refreshChat: vi.fn() }));
vi.mock('@/ui/Toast', async (load) => ({ ...await load<typeof import('@/ui/Toast')>(), toast: vi.fn() }));

const row = () => chat({ kind: 'group', node: null, name: 'Current group', members: [ADA], created_by: ADA });
const receipt = () => new ReceiptAcknowledged({ projection: 'receipt', chat_id: row().id }, 200, 'group-accepted');
const refused = () => new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', title: 'Forbidden', status: 403, detail: 'The current membership rule refused this addition.', request_id: 'group-refused' }, null);
function acceptedNotice(): void { expect(toast).toHaveBeenCalledExactlyOnceWith({ tone: 'info', title: i18n.t('ui.receipt.title'), description: i18n.t('ui.receipt.accepted') }); }
async function open(which: 'rename' | 'add') {
  const user = userEvent.setup();
  show(<GroupMenu chat={row()} me={ADA.id} onLeft={() => undefined} />);
  await user.click(screen.getByRole('button', { name: i18n.t('chat.group.menu') }));
  await user.click(screen.getByRole('menuitem', { name: i18n.t(which === 'rename' ? 'chat.group.rename' : 'chat.group.add') }));
  return user;
}
beforeEach(async () => { await setLocale('en'); requests.rename.mockReset(); requests.add.mockReset(); vi.mocked(toast).mockClear(); vi.mocked(refreshChat).mockClear(); useChatStore.setState({ byChat: new Map() }); });
afterEach(() => { cleanup(); vi.restoreAllMocks(); useChatStore.setState({ byChat: new Map() }); });

it('closes an acknowledged rename with a neutral notice and no fabricated Chat', async () => {
  requests.rename.mockRejectedValueOnce(receipt());
  const apply = vi.spyOn(chatWriter, 'apply'), user = await open('rename');
  const dialog = screen.getByRole('dialog', { name: i18n.t('chat.group.renameTitle') });
  await user.clear(within(dialog).getByRole('textbox'));
  await user.type(within(dialog).getByRole('textbox'), 'Accepted group');
  await user.click(within(dialog).getByRole('button', { name: i18n.t('chat.group.renameButton') }));
  await waitFor(() => expect(dialog).not.toBeInTheDocument());
  expect(requests.rename).toHaveBeenCalledExactlyOnceWith(row().id, 'Accepted group');
  expect(apply).not.toHaveBeenCalled(); expect(refreshChat).toHaveBeenCalledExactlyOnceWith(row().id);
  acceptedNotice();
});

it('continues every selected addition after receipts and closes once without inventing members', async () => {
  requests.add.mockRejectedValue(receipt());
  const apply = vi.spyOn(chatWriter, 'apply'), user = await open('add');
  const dialog = screen.getByRole('dialog', { name: i18n.t('chat.group.addTitle') });
  await user.click(within(dialog).getByRole('button', { name: DORA.display_name }));
  await user.click(within(dialog).getByRole('button', { name: GRACE.display_name }));
  await user.click(within(dialog).getByRole('button', { name: i18n.t('chat.group.addButton', { count: 2 }) }));
  await waitFor(() => expect(requests.add).toHaveBeenCalledTimes(2));
  await waitFor(() => expect(dialog).not.toBeInTheDocument());
  expect(requests.add.mock.calls).toEqual([[row().id, DORA.id], [row().id, GRACE.id]]);
  expect(apply).not.toHaveBeenCalled(); acceptedNotice();
});

it('never replays an acknowledged addition when a later real refusal leaves one person to retry', async () => {
  requests.add.mockRejectedValueOnce(receipt()).mockRejectedValueOnce(refused());
  const user = await open('add');
  const dialog = screen.getByRole('dialog', { name: i18n.t('chat.group.addTitle') });
  await user.click(within(dialog).getByRole('button', { name: DORA.display_name }));
  await user.click(within(dialog).getByRole('button', { name: GRACE.display_name }));
  await user.click(within(dialog).getByRole('button', { name: i18n.t('chat.group.addButton', { count: 2 }) }));
  await waitFor(() => expect(requests.add).toHaveBeenCalledTimes(2));
  expect(dialog).toBeVisible();
  expect(within(dialog).getByRole('button', { name: DORA.display_name })).toHaveAttribute('aria-pressed', 'false');
  expect(within(dialog).getByRole('button', { name: GRACE.display_name })).toHaveAttribute('aria-pressed', 'true');
  expect(within(dialog).getByRole('alert')).toBeVisible(); acceptedNotice();
  requests.add.mockResolvedValueOnce(row());
  await user.click(within(dialog).getByRole('button', { name: i18n.t('chat.group.addButton', { count: 1 }) }));
  await waitFor(() => expect(dialog).not.toBeInTheDocument());
  expect(requests.add.mock.calls).toEqual([[row().id, DORA.id], [row().id, GRACE.id], [row().id, GRACE.id]]);
});

it('keeps a genuine rename refusal in the dialog and applies a real Full reply on retry', async () => {
  requests.rename.mockRejectedValueOnce(refused());
  const apply = vi.spyOn(chatWriter, 'apply'), user = await open('rename');
  const dialog = screen.getByRole('dialog', { name: i18n.t('chat.group.renameTitle') });
  const submit = within(dialog).getByRole('button', { name: i18n.t('chat.group.renameButton') });
  await user.click(submit);
  expect(await within(dialog).findByRole('alert')).toBeVisible();
  expect(apply).not.toHaveBeenCalled(); expect(refreshChat).not.toHaveBeenCalled(); expect(toast).not.toHaveBeenCalled();
  const full = chat({ ...row(), name: 'Current group', seq: 4 }); requests.rename.mockResolvedValueOnce(full);
  await user.click(submit);
  await waitFor(() => expect(dialog).not.toBeInTheDocument());
  expect(apply).toHaveBeenCalledExactlyOnceWith(row().id, { chat: full }, full.seq);
  expect(refreshChat).toHaveBeenCalledExactlyOnceWith(row().id); expect(requests.rename).toHaveBeenCalledTimes(2);
});
