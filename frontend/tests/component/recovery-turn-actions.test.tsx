/** A failed turn offers manual recovery only while it still owns unfinished work. */
import { act, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { TurnFooter } from '@/features/timeline/TurnFooter';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import type { Thread } from '@/generated/api-types';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { threadWriter } from '@/realtime/stores/thread';
import { errorItem, turn } from '../doubles/thread-rows';
import { show } from './support';

vi.mock('@/features/source/Restore', () => ({ RestoreFromTurn: () => null }));
const THREAD = 'review-recovery-actions';
const context: TimelineContext = { threadId: THREAD, threadName: 'Recover work', detail: 'steps', recent: new Set(), layout: 'one', setLayout: () => undefined,
  meId: null, canInteract: true, canManage: true, live: true, lit: null, selection: NOT_PICKING };
const originalLocale = currentLocale();
afterEach(async () => { threadWriter.evict(THREAD); await setLocale(originalLocale); });

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
});
