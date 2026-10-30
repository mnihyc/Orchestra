/** A failed turn offers manual recovery only while it still owns unfinished work. */
import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import type * as ThreadsApi from '@/api/threads';
import { TurnFooter } from '@/features/timeline/TurnFooter';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import type { Thread } from '@/generated/api-types';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { threadWriter } from '@/realtime/stores/thread';
import { errorItem, threadInput, turn } from '../doubles/thread-rows';
import { show } from './support';

vi.mock('@/features/source/Restore', () => ({ RestoreFromTurn: () => null }));
const retry = vi.hoisted(() => vi.fn<typeof ThreadsApi.retryTurn>());
vi.mock('@/api/threads', async (original) => ({ ...await original<typeof ThreadsApi>(), retryTurn: retry }));
const THREAD = 'review-recovery-actions';
const context: TimelineContext = { threadId: THREAD, threadName: 'Recover work', detail: 'steps', recent: new Set(), layout: 'one', setLayout: () => undefined,
  meId: null, canInteract: true, canManage: true, live: true, lit: null, selection: NOT_PICKING };
const originalLocale = currentLocale();
afterEach(async () => { retry.mockReset(); threadWriter.evict(THREAD); await setLocale(originalLocale); });

describe.each(['en', 'zh-CN'] as const)('manual action ownership in %s', (locale) => {
  it('hides stale retry and cut-short Continue during recovery and after a later turn', async () => {
    await setLocale(locale);
    const ended = { ...turn('failed', { ordinal: 1, inputId: 'input', state: 'failed' }), error: { code: 'provider_busy', message: 'Synthetic capacity', retryable: true, incomplete: true, window: null, said: null, resets_at: null } };
    const thread = { id: THREAD, seq: 1, state: 'blocked', lock: null, blocked: { reason: 'error', detail: 'call_recovery', sandbox: null } } as Thread;
    const native = errorItem('native', ended.error.message, { turn: ended.id });
    if (native.kind !== 'error') throw new Error('A failed attempt has an error');
    native.payload = { ...native.payload, code: ended.error.code };
    threadWriter.reset(THREAD, { thread, turns: [ended], items: [native, { ...native, id: 'native-two', seq_created: 2, seq_updated: 2 }] }, false);
    const view = show(<TimelineContextValue.Provider value={context}><TurnFooter turnId={ended.id} /></TimelineContextValue.Provider>);
    expect(screen.queryByRole('button', { name: i18n.t('timeline.footer.retry') })).not.toBeInTheDocument();
    expect(screen.queryByText(i18n.t('timeline.failure.incomplete'))).not.toBeInTheDocument();
    expect(view.container).toHaveTextContent(i18n.t('calls.failureAttempts', { count: 2, attempts: '2' }));
    await act(() => threadWriter.reset(THREAD, { thread: { ...thread, state: 'idle', blocked: null }, turns: [ended] }, false));
    view.rerender(<TimelineContextValue.Provider value={context}><TurnFooter turnId={ended.id} /></TimelineContextValue.Provider>);
    expect(screen.getByRole('button', { name: i18n.t('timeline.footer.retry') })).toBeVisible();
    expect(screen.getByRole('button', { name: i18n.t('timeline.failure.continue') })).toBeVisible();
    await act(() => threadWriter.reset(THREAD, { thread: { ...thread, state: 'running', blocked: null }, turns: [ended, turn('followed', { ordinal: 2, inputId: 'next', state: 'running' })] }, false));
    view.rerender(<TimelineContextValue.Provider value={context}><TurnFooter turnId={ended.id} /></TimelineContextValue.Provider>);
    expect(screen.queryByRole('button', { name: i18n.t('timeline.footer.retry') })).not.toBeInTheDocument();
    expect(screen.queryByText(i18n.t('timeline.failure.incomplete'))).not.toBeInTheDocument();
  });

  it('names the queue or delivery while the original input is still in flight', async () => {
    await setLocale(locale);
    const ended = turn('failed', { ordinal: 1, inputId: 'input', state: 'failed' });
    const thread = { id: THREAD, seq: 1, state: 'idle', current_turn_id: null, lock: null, blocked: null } as Thread;
    const pending = { ...threadInput('input', 'Keep this', { author: null, position: '1' }), thread_id: THREAD, seq_updated: 1 };
    threadWriter.reset(THREAD, { thread, turns: [ended], inputs: [pending] }, false);
    show(<TimelineContextValue.Provider value={context}><TurnFooter turnId={ended.id} /></TimelineContextValue.Provider>);
    expect(screen.getByText(i18n.t('timeline.footer.inputWaiting'))).toBeVisible();
    expect(screen.queryByRole('button', { name: i18n.t('timeline.footer.retry') })).not.toBeInTheDocument();
    act(() => threadWriter.apply(THREAD, { inputs: [{ ...pending, state: 'delivering', seq_updated: 2 }] }));
    expect(screen.getByText(i18n.t('timeline.footer.inputDelivering'))).toBeVisible();
    act(() => threadWriter.apply(THREAD, { inputs: [{ ...pending, state: 'delivered', seq_updated: 3 }] }));
    expect(screen.getByRole('button', { name: i18n.t('timeline.footer.retry') })).toBeVisible();
    expect(retry).not.toHaveBeenCalled();
  });

  it.each([
    'The original input is waiting in the message queue or is being delivered',
    'The original input is already waiting or being delivered',
  ])('explains a stale Retry refusal without hiding other errors: %s', async (detail) => {
    await setLocale(locale);
    const ended = turn('failed', { ordinal: 1, inputId: 'input', state: 'failed' });
    const thread = { id: THREAD, seq: 1, state: 'idle', current_turn_id: null, lock: null, blocked: null } as Thread;
    threadWriter.reset(THREAD, { thread, turns: [ended] }, false);
    retry.mockRejectedValueOnce(new ApiError({ type: 'orch:error:request.invalid', code: 'request.invalid', title: 'Invalid request',
      status: 400, detail, request_id: 'retry' }, null));
    show(<TimelineContextValue.Provider value={context}><TurnFooter turnId={ended.id} /></TimelineContextValue.Provider>);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('timeline.footer.retry') }));
    await waitFor(() => expect(screen.getByText(i18n.t('timeline.footer.inputWaitingOrDelivering'))).toBeVisible());
    expect(retry).toHaveBeenCalledTimes(1);
    const pending = { ...threadInput('input', 'Keep this', { author: null, position: '1' }), thread_id: THREAD, seq_updated: 2 };
    act(() => threadWriter.apply(THREAD, { inputs: [pending] }));
    expect(screen.getByText(i18n.t('timeline.footer.inputWaiting'))).toBeVisible();
    expect(screen.queryByText(i18n.t('timeline.footer.inputWaitingOrDelivering'))).not.toBeInTheDocument();
    act(() => threadWriter.apply(THREAD, { inputs: [{ ...pending, state: 'delivered', seq_updated: 3 }] }));
    expect(screen.queryByText(i18n.t('timeline.footer.inputWaitingOrDelivering'))).not.toBeInTheDocument();
    retry.mockRejectedValueOnce(new ApiError({ type: 'orch:error:request.invalid', code: 'request.invalid', title: 'Invalid request',
      status: 400, detail: 'Another invalid request', request_id: 'other' }, null));
    await userEvent.click(screen.getByRole('button', { name: i18n.t('timeline.footer.retry') }));
    await waitFor(() => expect(screen.getByText(i18n.t('errors.request.invalid'))).toBeVisible());
    expect(screen.queryByText(i18n.t('timeline.footer.inputWaitingOrDelivering'))).not.toBeInTheDocument();
  });

  it('keeps the waiting explanation when a delivered input frame predates the refusal', async () => {
    await setLocale(locale);
    const ended = turn('failed', { ordinal: 1, inputId: 'input', state: 'failed' });
    const thread = { id: THREAD, seq: 1, state: 'idle', current_turn_id: null, lock: null, blocked: null } as Thread;
    const input = { ...threadInput('input', 'Keep this', { author: null, position: '1', state: 'delivered' }), thread_id: THREAD, seq_updated: 1 };
    threadWriter.reset(THREAD, { thread, turns: [ended], inputs: [input] }, false);
    let refuse: (error: ApiError) => void = () => { throw new Error('The Retry was not requested'); };
    retry.mockImplementationOnce(() => new Promise((_, reject) => { refuse = reject; }));
    show(<TimelineContextValue.Provider value={context}><TurnFooter turnId={ended.id} /></TimelineContextValue.Provider>);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('timeline.footer.retry') }));
    // Even a delivered frame that arrives while the request is in flight is older than the refusal.
    act(() => threadWriter.apply(THREAD, { inputs: [{ ...input, seq_updated: 2 }] }));
    await act(async () => refuse(new ApiError({ type: 'orch:error:request.invalid', code: 'request.invalid', title: 'Invalid request',
      status: 400, detail: 'The original input is waiting in the message queue or is being delivered', request_id: 'retry' }, null)));
    await waitFor(() => expect(screen.getByText(i18n.t('timeline.footer.inputWaitingOrDelivering'))).toBeVisible());
    act(() => threadWriter.apply(THREAD, { inputs: [{ ...input, state: 'pending', seq_updated: 3 }] }));
    expect(screen.getByText(i18n.t('timeline.footer.inputWaiting'))).toBeVisible();
    act(() => threadWriter.apply(THREAD, { inputs: [{ ...input, seq_updated: 4 }] }));
    expect(screen.queryByText(i18n.t('timeline.footer.inputWaitingOrDelivering'))).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: i18n.t('timeline.footer.retry') })).toBeVisible();
  });
});
