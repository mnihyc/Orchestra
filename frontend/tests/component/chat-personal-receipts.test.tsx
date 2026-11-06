/** Accepted personal chat writes never become a failed save or a fabricated readable row. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { AriaAttributes, ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import type * as Chats from '@/api/chats';
import { draftKey } from '@/api/chats';
import { ApiError, client, ReceiptAcknowledged } from '@/api/client';
import { Bell } from '@/features/chat/Bell';
import { ChatRow } from '@/features/chat/ChatRow';
import { SAVE_AFTER_MS, useChatDraftGuard, useDraft } from '@/features/chat/draft';
import { i18n, setLocale } from '@/i18n';
import { statesWriter, useChatState, useChatStates } from '@/realtime/stores/chat';
import { toast } from '@/ui/Toast';
import { ADA, chat, state } from '../doubles/chat-rows';
import { show } from './support';

const requests = vi.hoisted(() => ({
  state: vi.fn<typeof Chats.setChatState>(), read: vi.fn<typeof Chats.fetchDraft>(),
  save: vi.fn<typeof Chats.saveDraft>(), clear: vi.fn<typeof Chats.discardDraft>(),
}));
vi.mock('@/api/chats', async (load) => ({
  ...await load<typeof Chats>(), setChatState: requests.state, fetchDraft: requests.read,
  saveDraft: requests.save, discardDraft: requests.clear,
}));
vi.mock('@/ui/Toast', async (load) => ({ ...await load<typeof import('@/ui/Toast')>(), toast: vi.fn() }));
vi.mock('@tanstack/react-router', async (load) => ({
  ...await load<object>(),
  Link: ({ children, className, 'aria-label': label, 'aria-current': current }: { children: ReactNode; className?: string; 'aria-label'?: string; 'aria-current'?: AriaAttributes['aria-current'] }) => <a href="/org/acme/messages" className={className} aria-label={label} aria-current={current}>{children}</a>,
}));

const room = chat();
const accepted = (id = room.id) => new ReceiptAcknowledged({ projection: 'receipt', chat_id: id }, 200, 'personal-chat-accepted');
const refused = () => new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', title: 'Forbidden', status: 403, detail: 'The current writer permission is absent.', request_id: 'personal-chat-refused' }, null);
function ChatBell() { return <Bell chat={room} state={useChatState(room.id)} />; }
function acknowledgement(): void {
  expect(toast).toHaveBeenCalledExactlyOnceWith({ tone: 'info', title: i18n.t('ui.receipt.title'), description: i18n.t('ui.receipt.accepted') });
}

async function clearDraftScopes(): Promise<void> {
  // Unsaved words correctly survive unmount. End this fixture's reader through
  // the existing session boundary rather than weakening the product's guard.
  client.session.setState({ status: 'signed_in', userId: 'personal-chat-fixture' });
  const logout = vi.spyOn(client.api, 'POST').mockResolvedValue({ data: undefined, response: new Response(null, { status: 204 }) });
  try { await client.signOut(); } finally { logout.mockRestore(); }
}

beforeEach(async () => {
  await clearDraftScopes();
  await setLocale('en');
  requests.state.mockReset(); requests.read.mockReset(); requests.save.mockReset(); requests.clear.mockReset();
  vi.mocked(toast).mockClear(); statesWriter.clear();
});
afterEach(async () => { cleanup(); vi.useRealTimers(); await clearDraftScopes(); vi.restoreAllMocks(); statesWriter.clear(); });

it('acknowledges a bell receipt while retaining the served state until its ordinary event arrives', async () => {
  const original = state(room.id, { level: 'off', effective_level: 'off', read_revision: 7 });
  statesWriter.put(original); requests.state.mockRejectedValueOnce(accepted());
  const put = vi.spyOn(statesWriter, 'put');
  show(<ChatBell />);
  const user = userEvent.setup(), bell = screen.getByRole('button', { name: i18n.t('actions.notificationSettings.label') });
  await user.click(bell); await user.click(screen.getByRole('menuitemradio', { name: i18n.t('choiceCatalog.subscriptionLevel.all.name') }));
  await waitFor(acknowledgement);
  expect(requests.state).toHaveBeenCalledExactlyOnceWith(room.id, { level: 'all' });
  expect(put).not.toHaveBeenCalled();
  expect(useChatStates.getState().states.get(room.id)).toEqual(original);
  expect(bell).not.toHaveAttribute('aria-busy');
  expect(bell).toHaveTextContent(i18n.t('notificationBell.level.off'));
  act(() => statesWriter.put(state(room.id, { level: 'all', effective_level: 'all', read_revision: 7 })));
  expect(bell).toHaveTextContent(i18n.t('notificationBell.level.all'));
});

it.each(['mute', 'hide'] as const)('acknowledges the row %s receipt without replacing state or offering replay', async (action) => {
  const people = chat({ kind: 'group', node: null, name: 'Release crew', members: [ADA] });
  const original = state(people.id); statesWriter.put(original);
  requests.state.mockRejectedValueOnce(accepted());
  const put = vi.spyOn(statesWriter, 'put');
  show(<ul><ChatRow chat={people} slug="acme" selected={false} me={ADA.id} search={{}} /></ul>);
  const user = userEvent.setup(); fireEvent.contextMenu(screen.getByRole('listitem'));
  await user.click(screen.getByRole('menuitem', { name: i18n.t(action === 'mute' ? 'chat.row.mute' : 'chat.row.hide') }));
  await waitFor(acknowledgement);
  expect(requests.state).toHaveBeenCalledExactlyOnceWith(people.id, action === 'mute' ? { level: 'off' } : { hidden: true });
  expect(put).not.toHaveBeenCalled(); expect(useChatStates.getState().states.get(people.id)).toEqual(original);
  expect(vi.mocked(toast).mock.calls[0]?.[0].action).toBeUndefined();
});

it('keeps a real bell refusal distinct from an accepted receipt', async () => {
  const original = state(room.id, { level: 'off', effective_level: 'off' }); statesWriter.put(original);
  requests.state.mockRejectedValueOnce(refused()); show(<ChatBell />);
  const user = userEvent.setup(); await user.click(screen.getByRole('button', { name: i18n.t('actions.notificationSettings.label') }));
  await user.click(screen.getByRole('menuitemradio', { name: i18n.t('choiceCatalog.subscriptionLevel.all.name') }));
  await waitFor(() => expect(toast).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ tone: 'danger', title: i18n.t('chat.failed.bell') })));
  expect(useChatStates.getState().states.get(room.id)).toEqual(original);
});

function draftFrame(id: string, cache: QueryClient) {
  function Editor() {
    const draft = useDraft(id, true);
    return <><input aria-label="Personal draft" value={draft.text} onChange={(event) => draft.setText(event.target.value)} /><output aria-label="Draft saving">{draft.saving}</output><button onClick={draft.retry}>Retry personal draft</button></>;
  }
  return function Frame({ placement }: { placement: string }) {
    useChatDraftGuard(); return <QueryClientProvider client={cache}><Editor key={placement} /></QueryClientProvider>;
  };
}
function warned(): boolean { const event = new Event('beforeunload', { cancelable: true }); window.dispatchEvent(event); return event.defaultPrevented; }

it('accepts saved words without manufacturing a readable draft, retaining a warning or resending after a move', async () => {
  const id = 'personal-draft-receipt', cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  cache.setQueryData(draftKey(id), null); requests.save.mockRejectedValue(accepted(id));
  const Frame = draftFrame(id, cache), view = show(<Frame placement="panel" />); vi.useFakeTimers();
  try {
    fireEvent.change(screen.getByLabelText('Personal draft'), { target: { value: 'Accepted private words' } });
    await act(async () => { await vi.advanceTimersByTimeAsync(SAVE_AFTER_MS); });
    expect(screen.getByLabelText('Draft saving')).toHaveTextContent('saved');
    expect(warned()).toBe(false);
    expect(cache.getQueryData(draftKey(id))).toBeNull();
    expect(requests.save).toHaveBeenCalledExactlyOnceWith(id, { parts: [{ type: 'text', text: 'Accepted private words' }], reply_to: null });
    view.rerender(<Frame placement="expanded" />);
    expect(screen.getByLabelText('Personal draft')).toHaveValue('Accepted private words');
    await act(async () => { await vi.advanceTimersByTimeAsync(SAVE_AFTER_MS * 2); fireEvent.click(screen.getByRole('button', { name: 'Retry personal draft' })); await Promise.resolve(); });
    expect(requests.save).toHaveBeenCalledTimes(1); expect(requests.read).not.toHaveBeenCalled();
  } finally { view.unmount(); cache.clear(); }
});

it('a receipt acknowledges only the submitted words while newer text keeps its unsaved warning', async () => {
  const id = 'personal-draft-newer-than-receipt', cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  cache.setQueryData(draftKey(id), null);
  let accept: (error: unknown) => void = () => undefined;
  requests.save.mockImplementationOnce(() => new Promise((_resolve, reject) => { accept = reject; })).mockRejectedValueOnce(refused());
  const Frame = draftFrame(id, cache), view = show(<Frame placement="panel" />); vi.useFakeTimers();
  try {
    fireEvent.change(screen.getByLabelText('Personal draft'), { target: { value: 'Submitted words' } });
    await act(async () => { await vi.advanceTimersByTimeAsync(SAVE_AFTER_MS); });
    fireEvent.change(screen.getByLabelText('Personal draft'), { target: { value: 'Newer unsaved words' } });
    await act(async () => { accept(accepted(id)); await Promise.resolve(); });
    expect(screen.getByLabelText('Personal draft')).toHaveValue('Newer unsaved words'); expect(warned()).toBe(true);
    expect(cache.getQueryData(draftKey(id))).toBeNull();
    await act(async () => { await vi.advanceTimersByTimeAsync(SAVE_AFTER_MS); });
    expect(requests.save.mock.calls.map(([, body]) => body.parts)).toEqual([[{ type: 'text', text: 'Submitted words' }], [{ type: 'text', text: 'Newer unsaved words' }]]);
    expect(screen.getByLabelText('Draft saving')).toHaveTextContent('failed'); expect(warned()).toBe(true);
  } finally { view.unmount(); cache.clear(); }
});

it('still uses a genuine full draft reply and preserves a real failed save for explicit retry', async () => {
  const id = 'personal-draft-full-and-refused', cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  cache.setQueryData(draftKey(id), null); requests.save.mockRejectedValueOnce(refused());
  const Frame = draftFrame(id, cache), view = show(<Frame placement="panel" />); vi.useFakeTimers();
  try {
    fireEvent.change(screen.getByLabelText('Personal draft'), { target: { value: 'Retry these private words' } });
    await act(async () => { await vi.advanceTimersByTimeAsync(SAVE_AFTER_MS); });
    expect(screen.getByLabelText('Draft saving')).toHaveTextContent('failed'); expect(warned()).toBe(true);
    const full = { parts: [{ type: 'text', text: 'Retry these private words' } as const], reply_to: null, mention_names: [], updated_at: '2026-11-05T00:00:00Z' };
    requests.save.mockResolvedValueOnce(full);
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Retry personal draft' })); await Promise.resolve(); });
    expect(screen.getByLabelText('Draft saving')).toHaveTextContent('saved'); expect(warned()).toBe(false);
    expect(cache.getQueryData(draftKey(id))).toEqual(full); expect(requests.save).toHaveBeenCalledTimes(2);
  } finally { view.unmount(); cache.clear(); }
});
