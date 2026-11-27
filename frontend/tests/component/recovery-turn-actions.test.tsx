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
import type * as ThreadsApi from '@/api/threads';
import { show } from './support';

vi.mock('@/features/source/Restore', () => ({ RestoreFromTurn: () => null }));
const calls = vi.hoisted(() => ({ retry: vi.fn() }));
vi.mock('@/api/threads', async (load) => ({ ...await load<typeof ThreadsApi>(), retryTurn: calls.retry }));
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

describe.each([
  { locale: 'en', ended: '3 commands ended', running: '1 command still running', unknown: 'End unconfirmed for 2 commands' },
  { locale: 'zh-CN', ended: '3 条命令已结束', running: '1 条命令仍在运行', unknown: '2 条命令尚未确认结束' },
] as const)('command cleanup in $locale', ({ locale, ended, running, unknown }) => {
  it.each(['normal', 'verbose', 'diagnostics'] as const)('shows only relevant positive counts as a stopped turn’s report changes in %s', async (detail) => {
    await setLocale(locale);
    const stopped = turn('stopped', { ordinal: 1, inputId: 'input', state: 'interrupted', completedAt: '2026-11-19T00:00:00Z' });
    threadWriter.reset(THREAD, { turns: [stopped] }, false);
    const view = show(<TimelineContextValue.Provider value={{ ...context, detail }}><TurnFooter turnId={stopped.id} lined answered /></TimelineContextValue.Provider>);
    const facts = () => view.container.querySelector('[data-turn-facts]');
    let seq = stopped.seq_updated;
    const report = (ended: number, still_running: number, unknown: number) => act(() => {
      threadWriter.apply(THREAD, { turns: [{ ...stopped, seq_updated: ++seq, command_cleanup: { ended, still_running, unknown } }] });
    });

    expect(facts()).toBeNull();
    report(0, 0, 0);
    expect(facts()).toBeNull();
    report(0, 1, 0);
    expect(facts()?.textContent).toBe(running);
    report(3, 1, 2);
    expect(facts()?.textContent).toBe((detail === 'diagnostics' ? [ended, running, unknown] : [running, unknown]).join(' · '));
    report(0, 0, 2);
    expect(facts()?.textContent).toBe(unknown);
    report(3, 0, 0);
    if (detail === 'diagnostics') expect(facts()?.textContent).toBe(ended);
    else expect(facts()).toBeNull();
  });
});

it.each(['en', 'zh-CN'] as const)('states a failed agent program once, with its own words and one way to try again, in %s', async (locale) => {
  await setLocale(locale);
  const ended = { ...turn('program-failed', { ordinal: 1, inputId: 'asked', state: 'failed' }),
    error: { code: 'harness_error', message: i18n.t('harnessFailures.harness_error'), retryable: false, incomplete: false, window: null, said: null, resets_at: null } };
  const words = 'line 3: "footer.tsx" is not a directive';
  const reported = errorItem('program-error', words, { turn: ended.id, code: 'fake.script' });
  threadWriter.reset(THREAD, { thread: { id: THREAD, state: 'idle', current_turn_id: null, seq: 2, lock: null, blocked: null } as Thread, turns: [ended], items: [reported] }, false);
  calls.retry.mockReset().mockResolvedValue({ input: { id: 'again', thread_id: THREAD } });
  const view = show(<TimelineContextValue.Provider value={context}><TurnFooter turnId={ended.id} /></TimelineContextValue.Provider>);
  const alerts = screen.getAllByRole('alert');
  expect(alerts).toHaveLength(1);
  expect(alerts[0]).toHaveTextContent(i18n.t('harnessFailures.harness_error'));
  expect(screen.getByRole('note', { name: i18n.t('harnessFailures.programMessage') })).toHaveTextContent(words);
  expect(view.container).not.toHaveTextContent(i18n.t('ui.error.operationFailed'));
  expect(screen.queryByText(i18n.t('timeline.footer.failed'), { exact: true })).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
  expect(calls.retry).toHaveBeenCalledOnce();
  expect(calls.retry.mock.calls[0]?.[1]).toBe(ended.id);
});
