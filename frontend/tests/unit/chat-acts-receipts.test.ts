/** A committed chat receipt finishes the act without fabricating a row or rolling it back. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import type * as Chats from '@/api/chats';
import { ApiError, ReceiptAcknowledged } from '@/api/client';
import { edit, pin, remove, toggleReaction, useBusy } from '@/features/chat/acts';
import { i18n, setLocale } from '@/i18n';
import { chatWriter, useChatStore } from '@/realtime/stores/chat';
import { toast } from '@/ui/Toast';
import { ADA, DORA, at, chat, message } from '../doubles/chat-rows';

const requests = vi.hoisted(() => ({
  edit: vi.fn<typeof Chats.editMessage>(),
  remove: vi.fn<typeof Chats.deleteMessage>(),
  pin: vi.fn<typeof Chats.pin>(),
  react: vi.fn<typeof Chats.react>(),
}));
vi.mock('@/api/chats', async (load) => ({
  ...await load<typeof Chats>(), editMessage: requests.edit, deleteMessage: requests.remove, pin: requests.pin, react: requests.react,
}));
vi.mock('@/ui/Toast', () => ({ toast: vi.fn() }));

const row = () => message(1);
const held = () => useChatStore.getState().byChat.get(chat().id)?.messages.get(row().id);
const receipt = () => new ReceiptAcknowledged({
  projection: 'receipt', chat_id: chat().id, message_id: row().id, seq: 2,
}, 200, 'chat-receipt-request');
const refused = () => new ApiError({
  type: 'orch:error:authz.denied', code: 'authz.denied', title: 'Forbidden', status: 403,
  detail: 'The current writer permission is absent.', request_id: 'chat-refusal-request',
}, null);

function acceptedNotice(): void {
  expect(toast).toHaveBeenCalledExactlyOnceWith({
    tone: 'info', title: i18n.t('ui.receipt.title'), description: i18n.t('ui.receipt.accepted'),
  });
  expect(vi.mocked(toast).mock.calls.every(([notice]) => notice.action === undefined)).toBe(true);
}

beforeEach(async () => {
  await setLocale('en');
  requests.edit.mockReset(); requests.remove.mockReset(); requests.pin.mockReset(); requests.react.mockReset();
  vi.mocked(toast).mockClear();
  useBusy.setState({ ids: new Set() });
  useChatStore.setState({ byChat: new Map() });
  chatWriter.reset(chat().id, chat({ seq: 1 }), [row()], false, 1);
});
afterEach(() => { vi.restoreAllMocks(); });

it.each(['en', 'zh-CN'] as const)('accepts deletion without a fake tombstone or failure action (%s)', async (locale) => {
  await setLocale(locale);
  requests.remove.mockRejectedValueOnce(receipt());
  const own = vi.spyOn(chatWriter, 'own');
  const pending = remove(chat().id, row().id);
  expect(useBusy.getState().ids.has(row().id)).toBe(true);
  await expect(pending).resolves.toBe(true);
  expect(useBusy.getState().ids.has(row().id)).toBe(false);
  expect(requests.remove).toHaveBeenCalledExactlyOnceWith(chat().id, row().id);
  expect(own).not.toHaveBeenCalled();
  expect(held()).toEqual(row());
  acceptedNotice();
});

it.each([true, false])('accepts pin=%s without replacing the current row or offering replay', async (on) => {
  const original = row();
  if (!on) original.pinned = { at: at(1), by: DORA };
  chatWriter.reset(chat().id, chat({ seq: 1 }), [original], false, 1);
  requests.pin.mockRejectedValueOnce(receipt());
  const own = vi.spyOn(chatWriter, 'own');
  await pin(chat().id, original.id, on);
  expect(requests.pin).toHaveBeenCalledExactlyOnceWith(chat().id, original.id, on);
  expect(useBusy.getState().ids.has(original.id)).toBe(false);
  expect(own).not.toHaveBeenCalled();
  expect(held()).toEqual(original);
  acceptedNotice();
});

it.each([true, false])('keeps the accepted optimistic reaction on=%s until an authoritative row arrives', async (on) => {
  const original = row();
  if (!on) original.reactions = [{ emoji: '👍', users: [ADA.id] }];
  chatWriter.reset(chat().id, chat({ seq: 1 }), [original], false, 1);
  requests.react.mockRejectedValueOnce(receipt());
  const undo = vi.spyOn(chatWriter, 'unshown');
  const own = vi.spyOn(chatWriter, 'own');
  await toggleReaction(chat().id, original, '👍', ADA.id);
  expect(requests.react).toHaveBeenCalledExactlyOnceWith(chat().id, original.id, '👍', on);
  expect(undo).not.toHaveBeenCalled();
  expect(own).not.toHaveBeenCalled();
  expect(held()?.reactions).toEqual(on ? [{ emoji: '👍', users: [ADA.id] }] : []);
  expect(held()?.seq_updated).toBe(original.seq_updated);
  acceptedNotice();
  const authoritative = { ...original, seq_updated: 2, reactions: [{ emoji: '👍', users: [DORA.id] }] };
  chatWriter.apply(chat().id, { messages: [authoritative] }, 2);
  expect(held()).toEqual(authoritative);
});

it('keeps a genuine deletion refusal failed, clears busy state and leaves the row intact', async () => {
  requests.remove.mockRejectedValueOnce(refused());
  await expect(remove(chat().id, row().id)).resolves.toBe(false);
  expect(requests.remove).toHaveBeenCalledOnce();
  expect(useBusy.getState().ids.has(row().id)).toBe(false);
  expect(held()).toEqual(row());
  expect(toast).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ tone: 'danger', title: i18n.t('chat.failed.delete') }));
});

it.each([true, false])('keeps a genuine pin=%s refusal distinct from committed acceptance', async (on) => {
  requests.pin.mockRejectedValueOnce(refused());
  await pin(chat().id, row().id, on);
  expect(requests.pin).toHaveBeenCalledOnce();
  expect(useBusy.getState().ids.has(row().id)).toBe(false);
  expect(held()).toEqual(row());
  expect(toast).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ tone: 'danger', title: i18n.t(on ? 'chat.failed.pin' : 'chat.failed.unpin') }));
});

it('rolls back the optimistic reaction for a genuine refusal and reports that failure once', async () => {
  const original = held();
  if (original === undefined) throw new Error('the real chat store must contain its initial row');
  requests.react.mockRejectedValueOnce(refused());
  const undo = vi.spyOn(chatWriter, 'unshown');
  await toggleReaction(chat().id, original, '👍', ADA.id);
  expect(requests.react).toHaveBeenCalledExactlyOnceWith(chat().id, original.id, '👍', true);
  expect(undo).toHaveBeenCalledOnce();
  expect(held()).toBe(original);
  expect(toast).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ tone: 'danger', title: i18n.t('chat.failed.react') }));
});

it('still applies an actual full response as the authoritative message', async () => {
  const changed = { ...row(), deleted: true, parts: [], seq_updated: 2 };
  requests.remove.mockResolvedValueOnce(changed);
  await expect(remove(chat().id, row().id)).resolves.toBe(true);
  expect(held()).toEqual(changed);
  expect(toast).not.toHaveBeenCalled();
});


it.each(['en', 'zh-CN'] as const)('finishes a committed edit receipt without inventing a row or offering replay (%s)', async (locale) => {
  await setLocale(locale);
  const parts = [{ type: 'text' as const, text: 'Accepted edited words' }];
  requests.edit.mockRejectedValueOnce(receipt());
  const own = vi.spyOn(chatWriter, 'own');
  const pending = edit(chat().id, row().id, parts);
  expect(useBusy.getState().ids.has(row().id)).toBe(true);
  await expect(pending).resolves.toBe(true);
  expect(useBusy.getState().ids.has(row().id)).toBe(false);
  expect(requests.edit).toHaveBeenCalledExactlyOnceWith(chat().id, row().id, parts);
  expect(own).not.toHaveBeenCalled();
  expect(held()).toEqual(row());
  acceptedNotice();
  const authoritative = { ...row(), parts, seq_updated: 2 };
  chatWriter.apply(chat().id, { messages: [authoritative] }, 2);
  expect(held()).toEqual(authoritative);
  expect(requests.edit).toHaveBeenCalledOnce();
});

it('keeps a real edit refusal failed with the original row and one failure notice', async () => {
  const parts = [{ type: 'text' as const, text: 'Refused edited words' }];
  requests.edit.mockRejectedValueOnce(refused());
  await expect(edit(chat().id, row().id, parts)).resolves.toBe(false);
  expect(requests.edit).toHaveBeenCalledExactlyOnceWith(chat().id, row().id, parts);
  expect(useBusy.getState().ids.has(row().id)).toBe(false);
  expect(held()).toEqual(row());
  expect(toast).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ tone: 'danger', title: i18n.t('chat.failed.edit') }));
});

it('applies an actual full edit response through the existing authoritative writer', async () => {
  const parts = [{ type: 'text' as const, text: 'Authoritative edited words' }];
  const changed = { ...row(), parts, seq_updated: 2 };
  requests.edit.mockResolvedValueOnce(changed);
  await expect(edit(chat().id, row().id, parts)).resolves.toBe(true);
  expect(requests.edit).toHaveBeenCalledExactlyOnceWith(chat().id, row().id, parts);
  expect(held()).toEqual(changed);
  expect(toast).not.toHaveBeenCalled();
});
