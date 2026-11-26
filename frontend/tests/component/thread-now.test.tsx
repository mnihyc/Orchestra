/** The status line of the dock observes actual admitted rows: what is being done, one clock, Stop through the native action, and the one live region of the page. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Live from '@/realtime/live';
import type * as Threads from '@/api/threads';
import type * as Calls from '@/api/calls';
import { approvalKey } from '@/api/approvals';
import { threadScreenFactsQuery } from '@/api/threads';
import { activityWords, ThreadNowProvider } from '@/features/thread/NowLine';
import { StateFact } from '@/features/thread/Header';
import { ChangesButton } from '@/features/thread/ChangesButton';
import { PinnedApprovals } from '@/features/thread/PinnedApprovals';
import { Profiler } from 'react';
import { StatusLine } from '@/features/timeline/StatusLine';
import { BlockedBanner } from '@/features/thread/BlockedBanner';
import { waitingWords } from '@/features/timeline/waiting';
import { ViewCompactContext } from '@/features/workbench/Narrow';
import { useAppearance } from '@/lib/appearance';
import type { CallRecovery, Item, Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { formatSpan } from '@/lib/time';
import { flushDeltas, threadViewOf, threadWriter } from '@/realtime/stores/thread';
import { defaultThreadRead } from '@/realtime/threadScope';
import { ALLOW_DENY, agentMessage, approvalItem, command, harnessApproval, reasoning, threadInput, toolCall, turn } from '../doubles/thread-rows';
import { me, userRef } from '../doubles/fixtures';
import { snapshot } from '../unit/continuation-fixtures';
import { executionDisplay } from '../doubles/thread-display';
import { show } from './support';

const wire = vi.hoisted(() => ({ interrupt: vi.fn<typeof Threads.interruptTurn>() }));
/** What the server says of the thread's model-call recovery, read by the banner and the live line alike. */
const recoveries = vi.hoisted(() => ({ held: null as CallRecovery | null }));
const ADA = userRef(me(1, 'ada', 'Ada'));
const SILENT_AFTER_MS = 120_000;
vi.mock('@/api/threads', async (original) => ({ ...await original<typeof Threads>(), interruptTurn: wire.interrupt }));
vi.mock('@/api/calls', async (original) => ({ ...await original<typeof Calls>(),
  callRecoveryQuery: (id: string) => ({ queryKey: ['call-recovery', id], queryFn: () => Promise.resolve({ recovery: recoveries.held }), staleTime: Infinity }) }));
vi.mock('@/realtime/live', async (original) => ({ ...await original<typeof Live>(), useSocketStatus: () => ({ phase: 'ready' }) }));
const ids: string[] = [];
const appearance = useAppearance.getState();
const oldTheme = document.documentElement.dataset.theme;
afterEach(async () => {
  cleanup(); vi.useRealTimers(); for (const id of ids.splice(0)) threadWriter.evict(id); vi.clearAllMocks(); await setLocale('en');
  useAppearance.setState(appearance);
  if (oldTheme === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = oldTheme;
});

/** A thread whose turn runs, held by the store; its steps are the given rows. */
function running(rows: (turnId: string) => Item[], more: { startedAt?: string } = {}) {
  const source = snapshot(null); const id = source.thread.id; ids.push(id);
  const current = { ...turn('now-turn', { state: 'running', ordinal: 2, inputId: 'input', startedAt: more.startedAt ?? new Date(Date.now() - 90_000).toISOString() }), thread_id: id };
  const thread = { ...source.thread, display: executionDisplay('running'), state: 'running' as const, current_turn_id: current.id };
  threadWriter.reset(id, { ...source, thread, turns: [current], items: rows(current.id) }, false);
  return { id, source, current, thread };
}
const line = () => screen.getByRole('status');

it('streams the answer without redrawing the approval state and file counts', () => {
  vi.useFakeTimers();
  const { id, current, thread } = running((turnId) => [agentMessage('answer', '', { turn: turnId, status: 'in_progress' })]);
  const commits = vi.fn();
  show(<Profiler id="turn-facts" onRender={commits}><StateFact thread={thread} compact />
    <ChangesButton threadId={id} turnId={current.id} onOpen={() => undefined} /><PinnedApprovals threadId={id} /></Profiler>);
  commits.mockClear();
  act(() => {
    threadWriter.delta(id, { item_id: 'answer', field: 'text', offset: 0, len: 4, data: 'text' });
    flushDeltas();
  });
  expect(threadViewOf(id).items.get('answer')?.payload).toMatchObject({ text: 'text' });
  expect(commits).not.toHaveBeenCalled();
});

it.each(['en', 'zh-CN'] as const)('keeps the admitted start deadline on the current stage and never borrows one for a missing receipt in %s', async (locale) => {
  await setLocale(locale);
  const source = snapshot(null), id = source.thread.id; ids.push(id);
  const deadline = '2026-11-15T12:30:00Z';
  const current = { ...turn('waiting-start', { ordinal: 1, state: 'starting', inputId: 'waiting-input' }), thread_id: id, start_wait: {
    stage: 'acceptance' as const, began_at: '2026-11-15T12:28:00Z', deadline, after_millis: 120_000, admitted_default_source: 'default' as const, closed_at: null,
  } };
  threadWriter.reset(id, { ...source, thread: { ...source.thread, state: 'starting', display: executionDisplay('starting'), current_turn_id: current.id, starting_step: 'turn' }, turns: [current] }, false);
  const page = show(<StatusLine threadId={id} />);
  expect(line()).toHaveTextContent(locale === 'en' ? 'Waiting for acceptance · Until' : '等待接收 · 截止');
  expect(page.container.querySelector('[data-start-wait] time')).toHaveAttribute('dateTime', deadline);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.details') }));
  expect(await screen.findByRole('dialog', { name: i18n.t('common.details') })).toHaveTextContent(locale === 'en' ? 'Stage began' : '阶段开始时间');
  await userEvent.keyboard('{Escape}');
  act(() => { threadWriter.apply(id, { turns: [{ ...current, start_wait: undefined, seq_updated: source.seq + 5 }] }); });
  expect(screen.queryByText(locale === 'en' ? 'Waiting for acceptance · Until' : '等待接收 · 截止')).toBeNull();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.details') }));
  expect(await screen.findByRole('dialog', { name: i18n.t('common.details') })).toHaveTextContent(i18n.t('factDisclosure.notReported'));
});

it.each(['en', 'zh-CN'] as const)('names the current step with the turn’s one clock, and stops through the native action where it carries the Stop in %s', async (locale) => {
  await setLocale(locale);
  const { id, source, current } = running((turnId) => [
    command('old-running', { turn: 'old', command: 'Old command' }),
    command('current-command', { turn: turnId, command: 'ls', at: new Date(Date.now() - 3000).toISOString() }),
  ]);
  wire.interrupt.mockResolvedValue({ state: 'stopped', requested_at: new Date().toISOString(), turn: { ...current, seq_updated: source.seq + 1, state: 'interrupted', completed_at: new Date().toISOString() } });
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const view = show(<QueryClientProvider client={cache}><StatusLine threadId={id} withStop /></QueryClientProvider>);
  expect(line()).toHaveTextContent(i18n.t('thread.now.running', { command: 'ls' }));
  expect(screen.queryByText('Old command')).not.toBeInTheDocument();
  // One clock: the turn's elapsed time (a minute and a half), and not the step's own three seconds.
  const clocks = view.container.querySelectorAll('[data-status-item="elapsed"]');
  expect(clocks).toHaveLength(1);
  expect(clocks[0]?.textContent).toMatch(/1\D+3\d/);
  // A thought that is being written without any summary is named "Thinking…"; once it has one, the line names it by the summary's first line.
  act(() => threadWriter.apply(id, { items: [{ ...command('current-command', { turn: current.id, command: 'ls', exit: 0 }), seq_updated: source.seq + 1 }, { ...reasoning('thinking', '', { turn: current.id, status: 'in_progress', at: new Date().toISOString() }), seq_created: source.seq + 1, seq_updated: source.seq + 1 }] }));
  expect(line()).toHaveTextContent(i18n.t('thread.now.thinking'));
  act(() => threadWriter.apply(id, { items: [{ ...reasoning('thinking', '**Weighing the options**\n\nThe second one is smaller.', { turn: current.id, status: 'in_progress', at: new Date().toISOString() }), seq_created: source.seq + 1, seq_updated: source.seq + 2 }] }));
  expect(line()).toHaveTextContent('Weighing the options');
  expect(line()).not.toHaveTextContent('The second one is smaller.');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('composer.stop') }));
  await waitFor(() => expect(wire.interrupt).toHaveBeenCalledExactlyOnceWith(defaultThreadRead(id), current.id));
  await waitFor(() => expect(view.container.querySelector('[data-status-line]')).toBeNull());
  cache.clear();
});

it.each(['en', 'zh-CN'] as const)('names a pending native request as needing the person’s input, and not as a clock of its own in %s', async (locale) => {
  await setLocale(locale); const source = snapshot(null); const id = source.thread.id; ids.push(id);
  const current = turn('approval-turn', { state: 'running', ordinal: 1, inputId: 'input' });
  const approval = harnessApproval('now-request', { threadId: id, itemId: 'ask', requester: ADA, title: 'Run cleanup', request: { request_kind: 'command', options: ALLOW_DENY, detail: { command: 'clean output', cwd: '/work', reason: null } } });
  const item: Item = { ...approvalItem('ask', approval.id, { turn: current.id }), kind: 'approval', payload: { approval_id: approval.id, approval } };
  threadWriter.reset(id, { ...source, thread: { ...source.thread, display: executionDisplay('waiting_approval'), state: 'waiting_approval', current_turn_id: current.id }, turns: [current], items: [item] }, false);
  const cache = new QueryClient(); const view = show(<QueryClientProvider client={cache}><StatusLine threadId={id} /></QueryClientProvider>);
  expect(line()).toHaveTextContent(i18n.t('thread.now.needsInput'));
  await act(async () => { cache.setQueryData(approvalKey(approval.id), { ...approval, state: 'approved' }); await Promise.resolve(); });
  act(() => threadWriter.apply(id, { thread: { ...source.thread, seq: source.seq + 1, state: 'idle', current_turn_id: null } }));
  expect(view.container.querySelector('[data-status-line]')).toBeNull();
  cache.clear();
});

it('says that nothing new came after two minutes while no tool runs, counting up, and stops saying it when something arrives', async () => {
  vi.useFakeTimers();
  const { id, source, current } = running((turnId) => [command('finished', { turn: turnId, command: 'ls', exit: 0 })], { startedAt: new Date(Date.now() - 10_000).toISOString() });
  const name = snapshot(null).thread.binding?.agent.name ?? snapshot(null).thread.name;
  const [before = ''] = i18n.t('timeline.status.silent', { name, time: '@@' }).split('@@');
  const cache = new QueryClient();
  cache.setQueryData(threadScreenFactsQuery(id).queryKey, { silent_after_secs: SILENT_AFTER_MS / 1_000, plan_answered_by: 'writers' });
  show(<QueryClientProvider client={cache}><StatusLine threadId={id} /></QueryClientProvider>);
  expect(line()).not.toHaveTextContent(before.trim());
  // Two minutes and a little pass at once, and the line is told by the next second.
  vi.setSystemTime(Date.now() + SILENT_AFTER_MS + 5_000);
  await act(async () => { await vi.advanceTimersByTimeAsync(1_000); });
  expect(line()).toHaveTextContent(before.trim());
  const first = line().textContent;
  vi.setSystemTime(Date.now() + 10_000);
  await act(async () => { await vi.advanceTimersByTimeAsync(1_000); });
  expect(line().textContent).not.toBe(first);
  // A row that arrives is something new from the agent.
  act(() => threadWriter.apply(id, { thread: { ...source.thread, seq: source.seq + 1, state: 'running', current_turn_id: current.id }, items: [{ ...command('next', { turn: current.id, command: 'pwd', exit: 0 }), seq_created: source.seq + 1, seq_updated: source.seq + 1 }] }));
  await act(async () => { await vi.advanceTimersByTimeAsync(1_000); });
  expect(line()).not.toHaveTextContent(before.trim());
  cache.clear();
});

it('says that nothing new came after the seconds the thread’s setting names, however few', async () => {
  vi.useFakeTimers();
  const { id } = running((turnId) => [command('finished', { turn: turnId, command: 'ls', exit: 0 })], { startedAt: new Date(Date.now() - 5_000).toISOString() });
  const name = snapshot(null).thread.binding?.agent.name ?? snapshot(null).thread.name;
  const [before = ''] = i18n.t('timeline.status.silent', { name, time: '@@' }).split('@@');
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  cache.setQueryData(threadScreenFactsQuery(id).queryKey, { silent_after_secs: 20, plan_answered_by: 'writers' });
  show(<QueryClientProvider client={cache}><StatusLine threadId={id} /></QueryClientProvider>);
  vi.setSystemTime(Date.now() + 10_000);
  await act(async () => { await vi.advanceTimersByTimeAsync(1_000); });
  expect(line()).not.toHaveTextContent(before.trim());
  vi.setSystemTime(Date.now() + 15_000);
  await act(async () => { await vi.advanceTimersByTimeAsync(1_000); });
  // Well before the two minutes that nothing set would have waited for.
  expect(line()).toHaveTextContent(before.trim());
  cache.clear();
});

it.each([
  ['from when its step began, where the row says it', { starting_since: '2026-10-01T10:03:30.000Z' }, 90_000],
  ['from the message that waits, where the row does not say it', {}, 60_000],
])('counts the wait of a thread that starts %s', (_name, more, expected) => {
  vi.useFakeTimers(); vi.setSystemTime(Date.parse('2026-10-01T10:05:00.000Z'));
  const source = snapshot(null); const id = source.thread.id; ids.push(id);
  const thread = { ...source.thread, display: executionDisplay('starting'), state: 'starting', starting_step: 'session', current_turn_id: null, updated_at: '2026-10-01T10:04:00.000Z', ...more } as Thread;
  threadWriter.reset(id, { ...source, thread, turns: [], items: [] }, false);
  const view = show(<StatusLine threadId={id} />);
  expect(view.container.querySelector('[data-status-line]')).toHaveTextContent(formatSpan(expected, { locale: 'en', timeZone: 'UTC' }));
});

it('keeps one clock across the steps of a start, also once its turn exists, and counts afresh for the next message', () => {
  vi.useFakeTimers(); vi.setSystemTime(Date.parse('2026-10-01T10:05:00.000Z'));
  const source = snapshot(null); const id = source.thread.id; ids.push(id);
  const asked = { ...threadInput('asked', 'Start two sub-agents', { author: ADA, position: '1', state: 'delivering' }), created_at: '2026-10-01T10:03:00.000Z' };
  const thread = { ...source.thread, display: executionDisplay('starting'), state: 'starting', starting_step: 'files', starting_since: '2026-10-01T10:03:30.000Z',
    current_turn_id: null, updated_at: '2026-10-01T10:04:00.000Z' } as Thread;
  threadWriter.reset(id, { ...source, thread, turns: [], items: [], inputs: [asked] }, false);
  const view = show(<StatusLine threadId={id} />);
  const clock = () => view.container.querySelector('[data-status-item="waited"]');
  expect(clock()).toHaveTextContent(formatSpan(90_000, { locale: 'en', timeZone: 'UTC' }));
  // The next step says only when it began itself; the clock goes on from the first.
  const starting = { ...turn('start-turn', { state: 'starting', ordinal: 1, inputId: 'asked' }), thread_id: id, started_at: null };
  act(() => threadWriter.apply(id, { thread: { ...thread, starting_step: 'session', starting_since: '2026-10-01T10:04:50.000Z', current_turn_id: starting.id, seq: thread.seq + 1 }, turns: [starting] }));
  expect(clock()).toHaveTextContent(formatSpan(90_000, { locale: 'en', timeZone: 'UTC' }));
  // Another message's start is another clock.
  const next = { ...threadInput('next', 'Then the tests', { author: ADA, position: '2', state: 'delivering' }), created_at: '2026-10-01T10:04:55.000Z' };
  act(() => threadWriter.apply(id, { thread: { ...thread, starting_step: 'files', starting_since: '2026-10-01T10:04:58.000Z', current_turn_id: null, seq: thread.seq + 2 },
    inputs: [{ ...asked, state: 'delivered', seq_updated: 1 }, next] }));
  expect(clock()).toHaveTextContent(formatSpan(2_000, { locale: 'en', timeZone: 'UTC' }));
});

it.each(['en', 'zh-CN'] as const)('keeps the start command, what holds it up and its step clock together in the dock in %s', async (locale) => {
  await setLocale(locale);
  vi.useFakeTimers(); vi.setSystemTime(Date.parse('2026-10-01T10:05:00.000Z'));
  const source = snapshot(null), id = source.thread.id; ids.push(id);
  const thread: Thread = { ...source.thread, display: executionDisplay('starting'), state: 'starting', starting_step: 'files',
    starting_since: '2026-10-01T10:03:30.000Z', starting_work: { command: 'fs.mkdir', behind: 'fs.record' },
    current_turn_id: null, updated_at: '2026-10-01T10:04:00.000Z' };
  threadWriter.reset(id, { ...source, thread, turns: [], items: [] }, false);
  const view = show(<StatusLine threadId={id} />);
  expect(line()).toHaveTextContent(i18n.t('thread.starting.behind', { what: i18n.t('thread.starting.work.fsMkdir'), other: i18n.t('thread.starting.other.fsRecord') }));
  expect(view.container.querySelector('[data-status-item="waited"]')).toHaveTextContent(/1\D+30/);
  expect(screen.getAllByRole('status')).toHaveLength(1);
});

it.each(['en', 'zh-CN'] as const)('says how many sub-agents work when several were started together, and the one by its name when it works alone, in %s', async (locale) => {
  await setLocale(locale);
  const child = (id: string, name: string, status: 'in_progress' | 'completed') => (turnId: string) => toolCall(id, { kind: 'subagent', child_stream: `sub:${id}`, name, role: 'Explore', prompt: null,
    state: status === 'in_progress' ? 'running' : 'completed' }, { turn: turnId, title: name, status });
  const { id } = running((turnId) => [child('one', 'Find the login flow', 'in_progress')(turnId), child('two', 'Find the tests', 'in_progress')(turnId)]);
  show(<StatusLine threadId={id} />);
  expect(line()).toHaveTextContent(i18n.t('thread.now.subagents', { count: 2 }));
  act(() => threadWriter.apply(id, { items: [{ ...child('one', 'Find the login flow', 'completed')('now-turn'), seq_updated: 50 }] }));
  expect(line()).toHaveTextContent(i18n.t('timeline.step.subagent', { name: 'Find the tests' }));
});

it.each(['en', 'zh-CN'] as const)('keeps a clock on a step of a start once its turn exists and says when the step has made no progress for the silence setting, in %s', async (locale) => {
  await setLocale(locale);
  vi.useFakeTimers(); vi.setSystemTime(Date.parse('2026-10-01T10:05:00.000Z'));
  const source = snapshot(null), id = source.thread.id; ids.push(id);
  const current = { ...turn('credential-turn', { state: 'starting', ordinal: 2, inputId: 'later-input' }), thread_id: id, started_at: null };
  const thread: Thread = { ...source.thread, display: executionDisplay('starting'), state: 'starting', starting_step: 'credential',
    starting_since: '2026-10-01T10:04:30.000Z', current_turn_id: current.id, updated_at: '2026-10-01T10:04:30.000Z' };
  threadWriter.reset(id, { ...source, thread, turns: [current], items: [] }, false);
  const cache = new QueryClient();
  cache.setQueryData(threadScreenFactsQuery(id).queryKey, { silent_after_secs: SILENT_AFTER_MS / 1_000, plan_answered_by: 'writers' });
  const view = show(<QueryClientProvider client={cache}><StatusLine threadId={id} /></QueryClientProvider>);
  const step = i18n.t('thread.starting.credential', { provider: thread.binding?.provider.name ?? '' });
  const stalled = i18n.t('timeline.status.startSilent', { step });
  const waited = () => view.container.querySelector('[data-status-item="waited"]');
  expect(line()).toHaveTextContent(step);
  expect(line()).not.toHaveTextContent(stalled);
  expect(waited()).toHaveTextContent(formatSpan(30_000, { locale, timeZone: 'UTC' }));
  // The step stands for longer than the silence setting: the line says so, beside the one clock that goes on.
  vi.setSystemTime(Date.parse('2026-10-01T10:06:35.000Z'));
  await act(async () => { await vi.advanceTimersByTimeAsync(1_000); });
  expect(line()).toHaveTextContent(stalled);
  expect(waited()).toHaveTextContent(formatSpan(126_000, { locale, timeZone: 'UTC' }));
  expect(screen.getAllByRole('status')).toHaveLength(1);
  cache.clear();
});

it('does not call it silence while a tool runs', async () => {
  vi.useFakeTimers();
  const { id } = running((turnId) => [command('long', { turn: turnId, command: 'sleep 600', at: new Date(Date.now() - 1_000).toISOString() })]);
  show(<StatusLine threadId={id} />);
  // Six minutes pass at once: were the time a tool runs counted as silence, the first second after them would say so.
  vi.setSystemTime(Date.now() + SILENT_AFTER_MS * 3);
  await act(async () => { await vi.advanceTimersByTimeAsync(1_000); });
  expect(line()).toHaveTextContent(i18n.t('thread.now.running', { command: 'sleep 600' }));
  expect(line().textContent).not.toMatch(/Nothing new/);
});

it('names a starting step before any item has arrived', () => {
  const current = turn('starting', { state: 'starting', ordinal: 1, inputId: 'input' });
  current.started_at = null;
  expect(activityWords(undefined, current, i18n.t)).toBe(i18n.t('thread.now.starting'));
});

for (const locale of ['en', 'zh-CN'] as const) for (const hasTurn of [false, true]) {
  it(`shows Stopping with one advancing stop clock, including before a turn exists (${locale}, turn=${hasTurn})`, async () => {
    await setLocale(locale); useAppearance.setState({ locale }); vi.useFakeTimers();
    const source = snapshot(null), id = source.thread.id; ids.push(id);
    const current = turn('stopping-turn', { state: 'running', ordinal: 1, inputId: 'input', startedAt: new Date(Date.now() - 90_000).toISOString() });
    threadWriter.reset(id, { ...source, thread: { ...source.thread, state: hasTurn ? 'running' : 'starting',
      current_turn_id: hasTurn ? current.id : null, stopping: { version: 1, requested_at: new Date(Date.now() - 5_000).toISOString(), pending: true, thread_seq: source.seq, state_seq: source.seq } },
    turns: hasTurn ? [current] : [] }, false);
    const view = show(<StatusLine threadId={id} />);
    expect(line()).toHaveTextContent(i18n.t('thread.stopping.line'));
    expect(view.container.querySelector('[data-status-item="elapsed"]')).toBeNull();
    expect(view.container.querySelectorAll('[data-status-item="waited"]')).toHaveLength(1);
    expect(view.container.querySelector('[data-status-item="waited"]')).toHaveTextContent(formatSpan(5_000, { locale }));
    const before = line().textContent;
    await act(async () => { await vi.advanceTimersByTimeAsync(1_000); });
    expect(line().textContent).not.toBe(before);
    expect(screen.getAllByRole('status')).toHaveLength(1);
  });
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  it(`keeps one live region, whatever the width, and no Stop of its own while the composer has it, at ${locale}/${theme}/${width}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme;
    const { id, source } = running((turnId) => [command('compact-command', { turn: turnId, command: 'ls', at: new Date().toISOString() })]);
    const view = show(<ViewCompactContext.Provider value={width === 400}><div style={{ width }}><ThreadNowProvider threadId={id}><StateFact thread={{ ...source.thread, display: executionDisplay('running'), state: 'running' }} compact /><StatusLine threadId={id} /></ThreadNowProvider></div></ViewCompactContext.Provider>);
    expect(screen.getAllByRole('status')).toHaveLength(1);
    expect(line()).toHaveTextContent(i18n.t('thread.now.running', { command: 'ls' }));
    // The chip of the header no longer carries the activity: the one line does, at every width.
    expect(view.container.querySelector('[data-thread-now-chip]')).toBeNull();
    expect(view.container.querySelector('[data-thread-now]')).toBeNull();
    expect(screen.queryAllByRole('button', { name: i18n.t('composer.stop') })).toHaveLength(0);
  });
}

it('carries the one Stop of the turn where the composer is not drawn', async () => {
  const { id, source, current } = running((turnId) => [command('compact-command', { turn: turnId, command: 'ls', at: new Date().toISOString() })]);
  wire.interrupt.mockResolvedValue({ state: 'stopped', requested_at: new Date().toISOString(), turn: { ...current, state: 'interrupted', seq_updated: source.seq + 1 } });
  show(<StatusLine threadId={id} withStop />);
  expect(screen.getAllByRole('button', { name: i18n.t('composer.stop') })).toHaveLength(1);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('composer.stop') }));
  await waitFor(() => expect(wire.interrupt).toHaveBeenCalledExactlyOnceWith(defaultThreadRead(id), current.id));
});

it.each(['en', 'zh-CN'] as const)('keeps Stop available while an active thread reloads after a stale revision in %s', async (locale) => {
  await setLocale(locale); const source = snapshot(null); const id = source.thread.id; ids.push(id);
  const current = { ...turn('reload-turn', { state: 'running', ordinal: 1, inputId: 'input' }), thread_id: id };
  threadWriter.capture(id, undefined);
  threadWriter.reset(id, { ...source, thread: { ...source.thread, state: 'running', current_turn_id: current.id }, turns: [current] }, false);
  wire.interrupt.mockResolvedValue({ state: 'stopping', requested_at: new Date().toISOString(), stopping: { version: 1, requested_at: new Date().toISOString(), pending: true, thread_seq: source.seq, state_seq: source.seq }, turn: current });
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  show(<QueryClientProvider client={cache}><StatusLine threadId={id} withStop /></QueryClientProvider>);
  act(() => threadWriter.invalidate(id, false));
  const stop = screen.getByRole('button', { name: i18n.t('composer.stop') });
  expect(stop).toBeEnabled();
  await userEvent.click(stop);
  await waitFor(() => expect(wire.interrupt).toHaveBeenCalledExactlyOnceWith(defaultThreadRead(id), current.id));
  cache.clear();
});

for(const locale of ['en','zh-CN'] as const)for(const theme of ['light','dark'] as const)for(const width of [400,1440])it(`keeps a blocked reason with its recovery once above the composer (${locale}/${theme}/${width})`,async()=>{
  await setLocale(locale);document.documentElement.dataset.theme=theme;
  const source=snapshot(null),id=source.thread.id;ids.push(id);
  const thread:Thread={...source.thread,state:'blocked',lock:null,current_turn_id:null,blocked:{reason:'model_not_set',detail:null,sandbox:null}};
  threadWriter.reset(id,{...source,thread,turns:[],items:[]},false);
  const reason=waitingWords(thread,i18n.t);
  if(reason===null)throw new Error('The native blocked state has its reason');
  const view=show(<ViewCompactContext.Provider value={width===400}><div style={{width}}><BlockedBanner thread={thread} onProceed={()=>undefined}/><StatusLine threadId={id} blockedReasonShown/></div></ViewCompactContext.Provider>);
  expect(screen.getAllByText(reason,{exact:true})).toHaveLength(1);
  expect(view.container.querySelector('[data-status-line]')).toBeNull();
});

it.each(['en', 'zh-CN'] as const)('keeps a scheduled call recovery on the live line with Retry now and Stop, and a stopped one in the banner alone, in %s', async (locale) => {
  await setLocale(locale);
  const source = snapshot(null), id = source.thread.id; ids.push(id);
  const thread: Thread = { ...source.thread, state: 'blocked', lock: null, current_turn_id: null, blocked: { reason: 'error', detail: 'call_recovery', sandbox: null } };
  threadWriter.reset(id, { ...source, thread, turns: [], items: [] }, false);
  const scheduled: CallRecovery = { state: 'scheduled', failure: { code: 'provider_busy', message: 'Synthetic capacity', retryable: true, window: null, said: null, resets_at: null, incomplete: false },
    rule_id: 'busy', step: { type: 'retry', delay_secs: 60, times: 3, backoff: 2 }, attempt: 2, due_at: new Date(Date.now() + 60_000).toISOString(), detail: null, count: null, window_secs: null };
  // The thread screen's dock: the blocked banner above, the live line below, both shown to someone who drives the thread.
  const dock = () => <><BlockedBanner thread={thread} onProceed={() => undefined} /><StatusLine threadId={id} blockedReasonShown /></>;
  recoveries.held = scheduled;
  const waiting = show(dock());
  expect(await screen.findByRole('button', { name: i18n.t('calls.retryNow') })).toBeVisible();
  expect(screen.getAllByRole('button', { name: i18n.t('calls.stop') })).toHaveLength(1);
  expect(screen.getByRole('status', { name: i18n.t('calls.status') })).toBeVisible();
  waiting.unmount();
  recoveries.held = { ...scheduled, state: 'stopped', step: null, due_at: null, detail: 'repeat_limit', count: 4, window_secs: 300 };
  const stopped = show(dock());
  expect(await screen.findByRole('button', { name: i18n.t('calls.continue') })).toBeVisible();
  expect(screen.getAllByRole('button', { name: i18n.t('calls.retry') })).toHaveLength(1);
  expect(screen.queryByRole('button', { name: i18n.t('calls.retryNow') })).toBeNull();
  expect(screen.queryByRole('status', { name: i18n.t('calls.status') })).toBeNull();
  // Nothing is left on the live line for the banner's recovery: no empty place to tab into.
  expect(stopped.container.querySelector('[data-status-place="live"]')).toBeNull();
});

it.each(['en','zh-CN'] as const)('retains a blocked reason where a read-only dock has no recovery banner in %s',async(locale)=>{
  await setLocale(locale);
  const source=snapshot(null),id=source.thread.id;ids.push(id);
  const thread:Thread={...source.thread,state:'blocked',lock:null,current_turn_id:null,blocked:{reason:'model_not_set',detail:null,sandbox:null}};
  threadWriter.reset(id,{...source,thread,turns:[],items:[]},false);
  show(<StatusLine threadId={id}/>);
  const reason=waitingWords(thread,i18n.t);
  if(reason===null)throw new Error('The native blocked state has its reason');
  expect(screen.getByRole('status')).toHaveTextContent(reason);
});
