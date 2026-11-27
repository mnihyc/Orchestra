/** The existing composer banner shows only the current native turn's retained deadline. */
import { act, cleanup, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { BlockedBanner } from '@/features/thread/BlockedBanner';
import type { Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { formatDateTime } from '@/lib/time';
import { snapshot } from '../unit/continuation-fixtures';
import { executionDisplay } from '../doubles/thread-display';
import { show } from './support';
import userEvent from '@testing-library/user-event';
import { threadWriter } from '@/realtime/stores/thread';
import { turn as nativeTurn } from '../doubles/thread-rows';

vi.mock('@/features/change/target', async (loadOriginal) => ({ ...await loadOriginal<object>(), openChange: vi.fn() }));
afterEach(async () => { cleanup(); threadWriter.evict(snapshot().thread.id); useAppearance.setState({ locale: 'en' }); await setLocale('en'); });
const turn = '00000000-0000-0000-0000-000000000201';
const since = '2026-11-08T00:00:00Z';
const at = '2026-11-08T00:10:00Z';
let factRevision = 100;
function thread(): Thread {
  return { ...snapshot().thread, state: 'blocked', display: executionDisplay('blocked'), lock: null, current_turn_id: turn,
    blocked: { reason: 'agent_offline', detail: null, sandbox: null } };
}
function hold(indefinite = false, huge = false, closed = false) {
  const seq = ++factRevision;
  const row = Object.assign(nativeTurn(turn, { ordinal: 1, inputId: 'offline-input', state: 'running' }), { seq_updated: seq, offline_episode: {
    began_at: since, deadline: indefinite ? { kind: 'indefinite' } : { kind: 'finite', at: huge ? null : at, after_seconds: huge ? Number.MAX_SAFE_INTEGER + 1 : 600 },
    policy_source: 'default', episode_revision: 1, closed_at: closed ? at : null,
  } });
  threadWriter.reset(snapshot().thread.id, { ...snapshot(), seq, thread: { ...thread(), seq }, turns: [row] }, false);
}
for (const locale of ['en', 'zh-CN'] as const) {
  it(`shows the captured finite outage without another policy read (${locale})`, async () => {
    await setLocale(locale); useAppearance.setState({ locale }); hold();
    const view = show(<BlockedBanner thread={thread()} onProceed={vi.fn()} />);
    const line = view.container.querySelector('[data-offline-episode]');
    expect(line).toHaveTextContent(locale === 'en' ? 'Offline · Deadline' : '离线 · 截止');
    expect(line?.querySelector('time')).toHaveAttribute('dateTime', at);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.details') }));
    const body = view.baseElement.querySelector('[data-fact-disclosure-body]');
    expect(body).toHaveTextContent(locale === 'en' ? 'Captured policy' : '当时策略');
    expect(body).toHaveTextContent(locale === 'en' ? 'Finite deadline' : '有限截止时间');
  });
}
it.each(['en', 'zh-CN'] as const)('keeps captured indefinite waiting distinct from an old missing fact after another current cut in %s', async (locale) => {
  await setLocale(locale); hold(true);
  const view = show(<BlockedBanner thread={thread()} onProceed={vi.fn()} />);
  const words = locale === 'en' ? 'Offline · Waiting without a deadline' : '离线 · 无限期等待';
  expect(screen.getByText(words)).toBeVisible();
  act(() => threadWriter.patch(snapshot().thread.id, (row) => ({ ...row, revision: row.revision + 1 })));
  expect(screen.getByText(words)).toBeVisible();
  act(() => threadWriter.apply(snapshot().thread.id, { turns: [{ ...nativeTurn(turn, { ordinal: 1, inputId: 'offline-input', state: 'running' }), seq_updated: ++factRevision }] }));
  expect(screen.queryByText(words)).toBeNull();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.details') }));
  const body = view.baseElement.querySelector('[data-fact-disclosure-body]');
  if (!(body instanceof HTMLElement)) throw new Error('The current status owns its fact body');
  expect(within(body).getAllByText(locale === 'en' ? 'Not reported' : '尚未报告').length).toBeGreaterThan(0);
});
it('suppresses a stale or closed episode and keeps the state generic', () => {
  hold(true);
  const newer = { ...thread(), current_turn_id: '00000000-0000-0000-0000-000000000202' };
  const view = show(<BlockedBanner thread={newer} onProceed={vi.fn()} />);
  expect(view.container.querySelector('[data-offline-episode]')).toBeNull();
  act(() => hold(true, false, true));
  view.rerender(<BlockedBanner thread={thread()} onProceed={vi.fn()} />);
  expect(view.container.querySelector('[data-offline-episode]')).toBeNull();
});
it('describes a finite deadline beyond the calendar without calling it indefinite', () => {
  hold(false, true);
  const context = { locale: 'en', timeZone: useAppearance.getState().timeZone };
  show(<BlockedBanner thread={thread()} onProceed={vi.fn()} />);
  expect(screen.getByText(i18n.t('thread.blocked.offlineDeadlineBeyondCalendar', { since: formatDateTime(since, context) }))).toBeVisible();
  expect(screen.queryByText('Offline · Waiting without a deadline')).toBeNull();
});

it.each(['en', 'zh-CN'] as const)('says queued work waits for native end confirmation after a forced Stop in %s', async (locale) => {
  await setLocale(locale);
  const native: Thread = { ...thread(), current_turn_id: null, pending_inputs: 1,
    blocked: { reason: 'error', sandbox: null, detail: "native_turn_still_running: the native turn's end is not known; fresh work waits for its actual completion" } };
  const view = show(<BlockedBanner thread={native} onProceed={vi.fn()} />);
  expect(screen.getByText(locale === 'en' ? 'Native turn end unconfirmed · queued work waits' : '原生轮次尚未确认结束 · 排队消息继续等待')).toBeVisible();
  expect(view.container.querySelector('[data-blocked]')).not.toHaveTextContent('native_turn_still_running');
  expect(view.container.querySelector('[data-blocked]')).not.toHaveTextContent(i18n.t('thread.blocked.error', { detail: '' }));
});

it.each(['en', 'zh-CN'] as const)('names a failure the conversation shows at its turn in one line, without saying the program’s text again, in %s', async (locale) => {
  await setLocale(locale);
  const said = 'exited with status 1: ⚠ connectors are disabled | | Error: sandbox required but unavailable |';
  const failed = Object.assign(nativeTurn(turn, { ordinal: 1, inputId: 'failed-input', state: 'failed' }), { seq_updated: ++factRevision,
    error: { code: 'harness_error', message: 'The agent program could not finish its work.', retryable: false, window: null, said: null, resets_at: null, incomplete: false } });
  const blocked: Thread = { ...thread(), current_turn_id: null, blocked: { reason: 'error', sandbox: null, detail: said } };
  threadWriter.reset(snapshot().thread.id, { ...snapshot(), seq: failed.seq_updated, thread: { ...blocked, seq: failed.seq_updated }, turns: [failed] }, false);
  const view = show(<BlockedBanner thread={blocked} onProceed={vi.fn()} />);
  expect(view.container.querySelector('[data-blocked]')).toHaveTextContent(i18n.t('thread.blocked.lastTurnFailed'));
  expect(view.container.querySelector('[data-blocked]')).not.toHaveTextContent('sandbox required');
});
