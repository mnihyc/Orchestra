/** A failed turn offers manual recovery only while it still owns unfinished work. */
import { act, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import userEvent from '@testing-library/user-event';
import { TurnFooter } from '@/features/timeline/TurnFooter';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import type { Thread } from '@/generated/api-types';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { threadWriter } from '@/realtime/stores/thread';
import { errorItem, turn } from '../doubles/thread-rows';
import { show } from './support';

vi.mock('@/features/source/Restore', () => ({ RestoreFromTurn: () => null }));
const THREAD = 'review-recovery-actions';
const context: TimelineContext = { threadId: THREAD, threadName: 'Recover work', detail: 'normal', layout: 'one', setLayout: () => undefined,
  meId: null, canInteract: true, canManage: true, live: true, lit: null, selection: NOT_PICKING };
const originalLocale = currentLocale();
afterEach(async () => { threadWriter.evict(THREAD); await setLocale(originalLocale); });

describe.each(['en', 'zh-CN'] as const)('manual action ownership in %s', (locale) => {
  it('blocks Retry during automatic recovery and reserves cut-short Continue for the latest unfinished turn', async () => {
    await setLocale(locale);
    const ended = { ...turn('failed', { ordinal: 1, inputId: 'input', state: 'failed' }), error: { code: 'provider_busy', message: 'Synthetic capacity', retryable: true, incomplete: true, window: null, said: null, resets_at: null } };
    const thread = { id: THREAD, seq: 1, state: 'blocked', lock: null, blocked: { reason: 'error', detail: 'call_recovery', sandbox: null } } as Thread;
    const native = errorItem('native', ended.error.message, { turn: ended.id });
    if (native.kind !== 'error') throw new Error('A failed attempt has an error');
    native.payload = { ...native.payload, code: ended.error.code };
    threadWriter.reset(THREAD, { thread, turns: [ended], items: [native, { ...native, id: 'native-two', seq_created: 2, seq_updated: 2 }] }, false);
    const view = show(<TimelineContextValue.Provider value={context}><TurnFooter turnId={ended.id} /></TimelineContextValue.Provider>);
    expect(screen.getByRole('button', { name: i18n.t('timeline.footer.retry') })).toHaveAttribute('aria-disabled', 'true');
    await userEvent.hover(screen.getByRole('button', { name: i18n.t('timeline.footer.retry') }));
    expect(await screen.findByRole('tooltip')).toHaveTextContent(i18n.t('thread.turnActions.retryHandled'));
    await userEvent.unhover(screen.getByRole('button', { name: i18n.t('timeline.footer.retry') }));
    expect(screen.queryByText(i18n.t('timeline.failure.incomplete'))).not.toBeInTheDocument();
    expect(view.container).toHaveTextContent(i18n.t('calls.failureAttempts', { count: 2, attempts: '2' }));
    await act(() => threadWriter.reset(THREAD, { thread: { ...thread, state: 'idle', blocked: null }, turns: [ended] }, false));
    view.rerender(<TimelineContextValue.Provider value={context}><TurnFooter turnId={ended.id} /></TimelineContextValue.Provider>);
    expect(screen.getByRole('button', { name: i18n.t('timeline.footer.retry') })).not.toHaveAttribute('aria-disabled');
    expect(screen.getByRole('button', { name: i18n.t('timeline.failure.continue') })).toBeVisible();
    await act(() => threadWriter.reset(THREAD, { thread: { ...thread, state: 'running', blocked: null }, turns: [ended, turn('followed', { ordinal: 2, inputId: 'next', state: 'running' })] }, false));
    view.rerender(<TimelineContextValue.Provider value={context}><TurnFooter turnId={ended.id} /></TimelineContextValue.Provider>);
    expect(screen.getByRole('button', { name: i18n.t('timeline.footer.retry') })).not.toHaveAttribute('aria-disabled');
    expect(screen.queryByText(i18n.t('timeline.failure.incomplete'))).not.toBeInTheDocument();
  });
});

it.each(['en', 'zh-CN'] as const)('shows when the turn was lost and offers a deliberate next input in %s', async (locale) => {
  await setLocale(locale);
  const at = '2026-11-18T15:01:02Z';
  const ended = { ...turn('lost', { ordinal: 2, inputId: 'send-once', state: 'failed' }), completed_at: at,
    error: { code: 'turn_lost', message: 'The turn could not be reattached.', retryable: true, incomplete: true, window: null, said: null, resets_at: null } };
  threadWriter.reset(THREAD, { thread: { id: THREAD, state: 'idle', current_turn_id: null, seq: 2, lock: null, blocked: null } as Thread, turns: [ended] }, false);
  const view = show(<TimelineContextValue.Provider value={context}><TurnFooter turnId={ended.id} /></TimelineContextValue.Provider>);
  expect(screen.getByText(i18n.t('timeline.failure.turnLost'))).toBeVisible();
  expect(view.container.querySelector(`time[datetime="${at}"]`)).toBeVisible();
  expect(screen.getByRole('button', { name: i18n.t('timeline.failure.continue') })).toBeEnabled();
  expect(screen.queryByText(i18n.t('timeline.footer.failed'))).not.toBeInTheDocument();
});
