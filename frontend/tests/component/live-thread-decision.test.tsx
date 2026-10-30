import { display } from '../doubles/thread-display';
/** Committed decisions settle a watching page and its working clocks without a reload or another approval read. */
import { act, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type * as ApprovalsApi from '@/api/approvals';
import { ApprovalCard } from '@/features/timeline/items/Approval';
import { StateFact } from '@/features/thread/Header';
import { StatusLine } from '@/features/timeline/StatusLine';
import { WorkFold } from '@/features/timeline/Steps';
import { TurnFooter } from '@/features/timeline/TurnFooter';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import type { Approval, Item, Thread } from '@/generated/api-types';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { formatSpan } from '@/lib/time';
import { threadWriter, useThreadPart } from '@/realtime/stores/thread';
import { ALLOW_DENY, approvalItem, command, harnessApproval, turn } from '../doubles/thread-rows';
import { me, userRef } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn<() => Promise<Approval>>(), decide: vi.fn() }));
vi.mock('@/api/approvals', async (load) => ({ ...await load<typeof ApprovalsApi>(), approvalQuery: (id: string) => ({ queryKey: ['approval', id], queryFn: calls.read }), decideApproval: calls.decide }));
vi.mock('@/features/source/Restore', () => ({ RestoreFromTurn: () => null }));

const THREAD = 'live-decision';
const START = Date.parse('2026-10-15T10:00:00Z');
const at = (ms: number) => new Date(START + ms).toISOString();
function liveElapsed() {
  const line = document.querySelector<HTMLElement>('[data-status-line]');
  if (line === null) throw new Error('A working turn must have its live line');
  expect(line).toHaveTextContent(i18n.t('statusItems.generating'));
  return within(line).getByRole('button', { name: i18n.t('statusItems.items.elapsed') });
}
const context: TimelineContext = { threadId: THREAD, threadName: 'Check the redirect', detail: 'summary', recent: new Set(), layout: 'one', setLayout: () => undefined,
  meId: null, canInteract: true, canManage: true, live: true, lit: null, selection: NOT_PICKING };
const owner = userRef(me(1, 'ada', 'Ada'));
const decider = userRef(me(2, 'grace', 'Grace'));
const thread = { id: THREAD, name: context.threadName, seq: 3, revision: 1, current_turn_id: 'turn-1', state: 'waiting_approval', display: display('waiting_approval', { seq: 3 }), archived: false, lock: null, blocked: null,
  side_of: null, binding: null, owner, pending_approvals: 1, access: { permissions: ['thread.view', 'thread.interact', 'thread.manage'] } } as Thread;

function question(input: boolean): Approval {
  return harnessApproval('ask-1', { threadId: THREAD, itemId: 'request-item', requester: owner, title: input ? 'Choose scope' : 'Run checks',
    request: input ? { request_kind: 'user_input', options: [], detail: { questions: [{ id: 'scope', header: '', question: 'Which part should change?', multiple: false, options: null, allow_other: true }] } }
      : { request_kind: 'command', options: ALLOW_DENY, detail: { command: 'check redirect', cwd: '/workspace', reason: null } },
  });
}
function item(approval: Approval, status: Item['status'] = 'pending'): Extract<Item, { kind: 'approval' }> {
  const made = approvalItem('request-item', approval.id, { turn: 'turn-1' });
  if (made.kind !== 'approval') throw new Error('The fixture is a request');
  return { ...made, thread_id: THREAD, seq_created: 3, seq_updated: status === 'pending' ? 3 : 4, status, payload: { approval_id: approval.id, approval } };
}
function View() {
  const current = useThreadPart(THREAD, (view) => view.thread);
  const request = useThreadPart(THREAD, (view) => view.items.get('request-item'));
  if (current === null || request?.kind !== 'approval') return null;
  return <TimelineContextValue.Provider value={context}>
    <header data-testid="thread-state"><StateFact thread={current} compact /></header>
    <aside data-testid="state-clock"><StateFact thread={current} /></aside>
    <ApprovalCard item={request} />
    <StatusLine />
    <WorkFold turnId="turn-1" ids={['check-1']} open={false} />
    <TurnFooter turnId="turn-1" />
  </TimelineContextValue.Provider>;
}
let locale = currentLocale();
let appearance = useAppearance.getState();
let theme: string | null = null;
beforeEach(() => {
  locale = currentLocale(); appearance = useAppearance.getState(); theme = document.documentElement.getAttribute('data-theme');
  calls.read.mockReset(); calls.decide.mockReset(); threadWriter.evict(THREAD);
});
afterEach(async () => {
  threadWriter.evict(THREAD); vi.useRealTimers(); useAppearance.setState(appearance); await setLocale(locale);
  if (theme === null) document.documentElement.removeAttribute('data-theme'); else document.documentElement.setAttribute('data-theme', theme);
});

describe.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)('live decisions in %s / %s', (language, look) => {
  beforeEach(async () => { await setLocale(language); useAppearance.setState({ locale: language, timeZone: 'UTC', theme: look, resolvedTheme: look }); document.documentElement.setAttribute('data-theme', look); });
  it.each([false, true])('takes another page’s committed decision and completion, keeping the human wait out (question: %s)', async (input) => {
    vi.useFakeTimers(); vi.setSystemTime(START + 607_000);
    const pending = question(input);
    const started = turn('turn-1', { ordinal: 1, state: 'running', inputId: 'input-1', startedAt: at(0), waitingSince: at(7_000) });
    threadWriter.reset(THREAD, { thread, turns: [started], items: [item(pending), command('check-1', { command: 'check redirect', exit: 0, turn: 'turn-1' })] }, false);
    show(<View />);
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    const clock = (ms: number) => formatSpan(ms, { locale: language, timeZone: 'UTC' });
    expect(within(screen.getByTestId('thread-state')).getByText(i18n.t('composer.row.waitingForYou'))).toBeVisible();
    expect(document.querySelector('[data-status-line]')).toHaveTextContent(i18n.t('timeline.status.waitingApproval', { time: clock(600_000) }));
    expect(document.querySelector('[data-fold="turn-1"]')).toHaveTextContent(clock(7_000));
    expect(document.querySelector('[data-fold="turn-1"] .animate-spin')).toBeNull();
    await act(async () => { await vi.advanceTimersByTimeAsync(60_000); });
    expect(document.querySelector('[data-fold="turn-1"]')).toHaveTextContent(clock(7_000));
    const resolved: Approval = { ...pending, state: 'approved', can_decide: false, decided_by: decider, decided_at: at(667_000),
      decision: input ? { answers: { scope: ['Login'] } } : { option_id: 'allow_once' } };
    // This is the receiving page: it performs no POST and changes only from the ordinary live rows.
    act(() => threadWriter.apply(THREAD, { thread: { ...thread, seq: 4, state: 'running', display: display('running', { seq: 4 }), pending_approvals: 0 },
      turns: [{ ...started, waiting_since: null, waited_ms: 660_000, seq_updated: 4 }], items: [item(resolved, 'approved')] }));
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    const outcome = input ? i18n.t('approvals.answered') : i18n.t('approvals.allowOnce');
    expect(screen.getByRole('button', { name: new RegExp(pending.title) })).toHaveTextContent(outcome);
    expect(screen.getByRole('button', { name: new RegExp(pending.title) })).toHaveTextContent('Grace');
    expect(within(screen.getByTestId('thread-state')).getByText(i18n.t('state.running'))).toBeVisible();
    expect(liveElapsed().textContent).toBe(clock(7_000));
    act(() => threadWriter.apply(THREAD, { thread, turns: [started], items: [item(pending)] }));
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    expect(within(screen.getByTestId('thread-state')).getByText(i18n.t('state.running'))).toBeVisible();
    await act(async () => { await vi.advanceTimersByTimeAsync(5_000); });
    expect(screen.getByTestId('state-clock')).toHaveTextContent('0:12');
    expect(liveElapsed().textContent).toBe(clock(12_000));
    act(() => threadWriter.apply(THREAD, { thread: { ...thread, seq: 5, state: 'idle', display: display('idle', { seq: 5, read_seq: 5 }), pending_approvals: 0 },
      turns: [{ ...started, state: 'completed', completed_at: at(672_000), waiting_since: null, waited_ms: 660_000, seq_updated: 5 }] }));
    expect(within(screen.getByTestId('thread-state')).getByText(i18n.t('state.idle'))).toBeVisible();
    expect(document.querySelector('[data-status-line]')).toBeNull();
    expect(document.querySelector('[data-footer="turn-1"]')).toHaveTextContent(clock(12_000));
    expect(document.querySelector('[data-fold="turn-1"]')).toHaveTextContent(clock(12_000));
    expect(calls.read).not.toHaveBeenCalled(); expect(calls.decide).not.toHaveBeenCalled();
  });
});
