/** A side question uses its loaded thread's verified mode and never promises unsupported actions. */
import { act, cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type * as ForksApi from '@/api/forks';
import type * as NodesApi from '@/api/nodes';
import { SideAskHost } from '@/features/side-threads/AskOnSide';
import { openSideAsk, useSideAsk } from '@/features/side-threads/ask';
import type { SideReading, Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { threadWriter } from '@/realtime/stores/thread';
import { show } from './support';
import { threadInput } from '../doubles/thread-rows';

const calls = vi.hoisted(() => ({ start: vi.fn<typeof ForksApi.startSide>(), read: vi.fn<() => Promise<Thread>>() }));
vi.mock('@/api/forks', async (original) => ({ ...await original<typeof ForksApi>(), startSide: calls.start }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof NodesApi>(), nodeQuery: (id: string) => ({ queryKey: ['side-test-node', id], queryFn: calls.read }) }));
const THREAD = 'side-parent';

function parent(mode: SideReading | null, more: Partial<Thread> = {}): Thread {
  return { id: THREAD, kind: 'thread', name: 'Repair login', seq: 1, archived: false, side_of: null, lock: null,
    access: { permissions: ['thread.view', 'thread.interact'] }, sides: { allowed: true, max: 4, open: 0, mode, reason: null }, ...more } as Thread;
}
function open(thread: Thread | null) {
  if (thread !== null) threadWriter.reset(THREAD, { thread }, false);
  useSideAsk.setState({ current: { threadId: THREAD, threadName: 'Repair login', toTurn: null, text: ' Which test covers this? ' }, open: true });
  show(<SideAskHost />);
  return screen.getByRole('dialog', { name: i18n.t('side.ask.title') });
}

beforeEach(() => { calls.start.mockReset(); calls.read.mockReset(); threadWriter.evict(THREAD); useSideAsk.getState().forget(); });
afterEach(async () => { cleanup(); threadWriter.evict(THREAD); threadWriter.evict('accepted-side'); useSideAsk.getState().forget(); await setLocale('en'); });

describe('asking on the side', () => {
  it.each(['en', 'zh-CN'] as const)('keeps an unanswered draft across Escape and reopens quietly in %s', async (locale) => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale), user = userEvent.setup();
    const dialog = open(parent('read_only'));
    await user.clear(within(dialog).getByRole('textbox'));
    await user.type(within(dialog).getByRole('textbox'), 'Keep this question while I look elsewhere.');
    await user.keyboard('{Escape}');
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(calls.read).not.toHaveBeenCalled(); expect(calls.start).not.toHaveBeenCalled();
    act(() => { openSideAsk({ threadId: THREAD, threadName: 'Repair login', toTurn: null, text: '' }); });
    const reopened = screen.getByRole('dialog', { name: t('side.ask.title') });
    expect(within(reopened).getByRole('textbox')).toHaveValue('Keep this question while I look elsewhere.');
    expect(calls.read).not.toHaveBeenCalled();
  });

  it('retries an unconfirmed send with the same frozen words and client identity even after closing', async () => {
    calls.start.mockRejectedValueOnce(new Error('The connection ended before the answer arrived.'));
    calls.start.mockImplementation(() => new Promise(() => undefined));
    const user = userEvent.setup(), dialog = open(parent('read_only'));
    await user.click(within(dialog).getByRole('button', { name: 'Ask' }));
    await waitFor(() => expect(within(dialog).getByRole('button', { name: 'Try again' })).toBeVisible());
    const first = calls.start.mock.calls[0];
    expect(within(dialog).getByRole('textbox')).toBeDisabled();
    await user.keyboard('{Escape}');
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    act(() => { openSideAsk({ threadId: THREAD, threadName: 'Repair login', toTurn: null, text: '' }); });
    const again = screen.getByRole('dialog', { name: 'Ask on the side' });
    expect(within(again).getByRole('textbox')).toBeDisabled();
    await user.click(within(again).getByRole('button', { name: 'Try again' }));
    await waitFor(() => expect(calls.start).toHaveBeenCalledTimes(2));
    expect(calls.start.mock.calls[1]).toEqual(first);
    const retrying = within(again).getByRole('button', { name: 'Try again' });
    expect(retrying).toHaveAttribute('aria-busy', 'true');
    expect(retrying).toHaveFocus();
    expect(within(again).getByRole('textbox')).toBeDisabled();
    expect(calls.read).not.toHaveBeenCalled();
  });

  it('recovers the frozen attempt when the form is actually unmounted and recreated', async () => {
    calls.start.mockRejectedValueOnce(new Error('The reply was lost.'));
    calls.start.mockImplementation(() => new Promise(() => undefined));
    const user = userEvent.setup(), dialog = open(parent('read_only'));
    await user.click(within(dialog).getByRole('button', { name: 'Ask' }));
    await waitFor(() => expect(within(dialog).getByRole('button', { name: 'Try again' })).toBeVisible());
    const original = calls.start.mock.calls[0];
    cleanup();
    act(() => { openSideAsk({ threadId: THREAD, threadName: 'Repair login', toTurn: null, text: '' }); });
    show(<SideAskHost />);
    const reopened = screen.getByRole('dialog', { name: 'Ask on the side' });
    expect(within(reopened).getByRole('textbox')).toHaveValue('Which test covers this?');
    expect(within(reopened).getByRole('textbox')).toBeDisabled();
    await user.click(within(reopened).getByRole('button', { name: 'Try again' }));
    await waitFor(() => expect(calls.start).toHaveBeenCalledTimes(2));
    expect(calls.start.mock.calls[1]).toEqual(original);
  });

  it.each(['en', 'zh-CN'] as const)('sends on Enter, leaves Shift+Enter and IME to the question, in %s', async (locale) => {
    await setLocale(locale);
    calls.start.mockImplementation(() => new Promise(() => undefined));
    const dialog = open(parent('read_only'));
    const editor = within(dialog).getByRole('textbox');
    editor.focus();
    fireEvent.keyDown(editor, { key: 'Enter', isComposing: true });
    fireEvent.keyDown(editor, { key: 'Enter', ctrlKey: true, keyCode: 229 });
    expect(calls.start).not.toHaveBeenCalled();
    await userEvent.keyboard('{Shift>}{Enter}{/Shift}');
    expect(calls.start).not.toHaveBeenCalled();
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(calls.start).toHaveBeenCalledTimes(1));
    expect(within(dialog).getByRole('textbox')).toBeDisabled();
  });

  it('clears the question only after the server confirms it was accepted', async () => {
    let accept: (value: Awaited<ReturnType<typeof ForksApi.startSide>>) => void = () => undefined;
    calls.start.mockImplementation(() => new Promise((resolve) => { accept = resolve; }));
    const dialog = open(parent('read_only'));
    await userEvent.click(within(dialog).getByRole('button', { name: 'Ask' }));
    expect(within(dialog).getByRole('textbox')).toBeDisabled();
    const side = { ...parent('read_only'), id: 'accepted-side', side_of: { id: THREAD, kind: 'thread' as const, name: 'Repair login', redacted: false } };
    act(() => { accept({ thread: side, input: { ...threadInput('accepted', 'Which test covers this?', { author: null, position: '1' }), thread_id: side.id }, seq: 1 }); });
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    act(() => { openSideAsk({ threadId: THREAD, threadName: 'Repair login', toTurn: null, text: '' }); });
    expect(screen.getByRole('textbox')).toHaveValue('');
  });

  for (const locale of ['en', 'zh-CN'] as const) it.each(['read_only', 'approval_gated', 'no_tools'] as const)('shows the verified %s mode and sends once in ' + locale, async (mode) => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale);
    // Keep the native response in flight so this proves a repeated click cannot start another aside.
    calls.start.mockImplementation(() => new Promise(() => undefined));
    const dialog = open(parent(mode));
    expect(within(dialog).getByText(t('side.ask.mode', { mode: t(`side.reading.${mode}`) }))).toBeVisible();
    expect(calls.read).not.toHaveBeenCalled();
    const ask = within(dialog).getByRole('button', { name: t('side.ask.send') });
    await userEvent.click(ask); await userEvent.click(ask);
    await waitFor(() => expect(calls.start).toHaveBeenCalledTimes(1));
    const sent = calls.start.mock.calls[0];
    expect(sent?.[0]).toBe(THREAD);
    expect(sent?.[1].content).toEqual([{ type: 'text', text: 'Which test covers this?' }]);
    expect(typeof sent?.[1].client_id).toBe('string');
    expect(calls.read).not.toHaveBeenCalled();
  });

  it.each(['en', 'zh-CN'] as const)('keeps the draft and refuses an undeclared mode before any request in %s', async (locale) => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale);
    const dialog = open(parent(null));
    expect(within(dialog).getByRole('status')).toHaveTextContent(t('side.unavailable.readOnly'));
    expect(within(dialog).getByRole('button', { name: t('side.ask.send') })).toBeDisabled();
    expect(within(dialog).getByRole('textbox')).toHaveValue(' Which test covers this? ');
    await userEvent.click(within(dialog).getByRole('button', { name: t('side.ask.send') }));
    expect(calls.start).not.toHaveBeenCalled(); expect(calls.read).not.toHaveBeenCalled();
  });

  it('describes an earlier adapter refusal as the agent program’s factual limit and never starts an unsafe aside', () => {
    const dialog = open(parent(null, { sides: { allowed: true, open: 0, max: 4, mode: null, reason: 'This adapter cannot enforce read-only files.' } }));
    expect(within(dialog).getByRole('status')).toHaveTextContent(i18n.t('side.unavailable.readOnly'));
    expect(within(dialog).queryByText('This adapter cannot enforce read-only files.')).not.toBeInTheDocument();
    expect(within(dialog).getByRole('button', { name: 'Ask' })).toBeDisabled();
    expect(calls.start).not.toHaveBeenCalled();
  });

  it.each(['en', 'zh-CN'] as const)('keeps an existing draft when verified reading support is withdrawn in %s', async (locale) => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale), user = userEvent.setup();
    const original = parent('read_only');
    const dialog = open(original);
    const editor = within(dialog).getByRole('textbox');
    await user.clear(editor);
    await user.type(editor, 'Keep the redirect question.');
    act(() => threadWriter.apply(THREAD, { thread: { ...original, seq: 2, sides: { allowed: true, open: 0, max: 4, mode: null, reason: 'This agent cannot answer without being able to change things.' } } }));
    expect(within(dialog).getByRole('status')).toHaveTextContent(t('side.unavailable.readOnly'));
    expect(editor).toHaveValue('Keep the redirect question.');
    const send = within(dialog).getByRole('button', { name: t('side.ask.send') });
    expect(send).toBeDisabled();
    await user.click(send);
    expect(calls.start).not.toHaveBeenCalled();
    expect(calls.read).not.toHaveBeenCalled();
  });

  it('reads the native thread once only when it is absent, and enables asking after its mode arrives', async () => {
    calls.read.mockResolvedValue(parent('read_only'));
    const dialog = open(null);
    await waitFor(() => expect(within(dialog).getByRole('button', { name: 'Ask' })).toBeEnabled());
    expect(calls.read).toHaveBeenCalledTimes(1);
    expect(within(dialog).getByText('Side mode: Enforced read-only files')).toBeVisible();
    expect(calls.start).not.toHaveBeenCalled();
  });

  it('does not echo an old private thread name before the current reader’s ordinary read succeeds', async () => {
    let answer: (thread: Thread) => void = () => undefined;
    calls.read.mockImplementation(() => new Promise((resolve) => { answer = resolve; }));
    const dialog = open(null);
    expect(within(dialog).queryByText(/Repair login/)).not.toBeInTheDocument();
    await waitFor(() => expect(calls.read).toHaveBeenCalledTimes(1));
    act(() => { answer(parent('read_only', { name: 'Renamed thread' })); });
    expect(await within(dialog).findByText(/Starts from Renamed thread's conversation/)).toBeVisible();
    expect(within(dialog).queryByText(/Repair login/)).not.toBeInTheDocument();
  });

  it('does not mistake a policy refusal for verified harness support', () => {
    const dialog = open(parent('read_only', { sides: { allowed: false, open: 0, max: 4, mode: 'read_only', reason: null } }));
    expect(within(dialog).getByRole('status')).toHaveTextContent('Side threads are switched off here. Use Fork instead.');
    expect(within(dialog).getByRole('button', { name: 'Ask' })).toBeDisabled();
    expect(calls.start).not.toHaveBeenCalled();
  });
});
