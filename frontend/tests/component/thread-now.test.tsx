/** The status line of the dock observes actual admitted rows: what is being done, one clock, Stop through the native action, and the one live region of the page. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Live from '@/realtime/live';
import type * as Threads from '@/api/threads';
import { approvalKey } from '@/api/approvals';
import { settingsKey } from '@/api/settings';
import { activityWords, ThreadNowProvider } from '@/features/thread/NowLine';
import { StateFact } from '@/features/thread/Header';
import { StatusLine } from '@/features/timeline/StatusLine';
import { DEFAULT_SILENT_AFTER_MS } from '@/features/timeline/silence';
import { ViewCompactContext } from '@/features/workbench/Narrow';
import { useAppearance } from '@/lib/appearance';
import type { Item, Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { formatSpan } from '@/lib/time';
import { threadWriter } from '@/realtime/stores/thread';
import { defaultThreadRead } from '@/realtime/threadScope';
import { ALLOW_DENY, approvalItem, command, harnessApproval, reasoning, turn } from '../doubles/thread-rows';
import { me, userRef } from '../doubles/fixtures';
import { snapshot } from '../unit/continuation-fixtures';
import { executionDisplay } from '../doubles/thread-display';
import { show } from './support';

const wire = vi.hoisted(() => ({ interrupt: vi.fn<typeof Threads.interruptTurn>() }));
const ADA = userRef(me(1, 'ada', 'Ada'));
vi.mock('@/api/threads', async (original) => ({ ...await original<typeof Threads>(), interruptTurn: wire.interrupt }));
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

it.each(['en', 'zh-CN'] as const)('names the current step with the turn’s one clock, and stops through the native action where it carries the Stop in %s', async (locale) => {
  await setLocale(locale);
  const { id, source, current } = running((turnId) => [
    command('old-running', { turn: 'old', command: 'Old command' }),
    command('current-command', { turn: turnId, command: 'ls', at: new Date(Date.now() - 3000).toISOString() }),
  ]);
  wire.interrupt.mockResolvedValue({ ...current, seq_updated: source.seq + 1, state: 'interrupted', completed_at: new Date().toISOString() });
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
  show(<StatusLine threadId={id} />);
  expect(line()).not.toHaveTextContent(before.trim());
  // Two minutes and a little pass at once, and the line is told by the next second.
  vi.setSystemTime(Date.now() + DEFAULT_SILENT_AFTER_MS + 5_000);
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
});

it('says that nothing new came after the seconds the thread’s setting names, however few', async () => {
  vi.useFakeTimers();
  const { id } = running((turnId) => [command('finished', { turn: turnId, command: 'ls', exit: 0 })], { startedAt: new Date(Date.now() - 5_000).toISOString() });
  const name = snapshot(null).thread.binding?.agent.name ?? snapshot(null).thread.name;
  const [before = ''] = i18n.t('timeline.status.silent', { name, time: '@@' }).split('@@');
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  cache.setQueryData(settingsKey(id), [{ key: 'thread.silent_after_secs', value: 20 }]);
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

it('does not call it silence while a tool runs', async () => {
  vi.useFakeTimers();
  const { id } = running((turnId) => [command('long', { turn: turnId, command: 'sleep 600', at: new Date(Date.now() - 1_000).toISOString() })]);
  show(<StatusLine threadId={id} />);
  // Six minutes pass at once: were the time a tool runs counted as silence, the first second after them would say so.
  vi.setSystemTime(Date.now() + DEFAULT_SILENT_AFTER_MS * 3);
  await act(async () => { await vi.advanceTimersByTimeAsync(1_000); });
  expect(line()).toHaveTextContent(i18n.t('thread.now.running', { command: 'sleep 600' }));
  expect(line().textContent).not.toMatch(/Nothing new/);
});

it('names a starting step before any item has arrived', () => {
  const current = turn('starting', { state: 'starting', ordinal: 1, inputId: 'input' });
  current.started_at = null;
  expect(activityWords(undefined, current, i18n.t)).toBe(i18n.t('thread.now.starting'));
});

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
  wire.interrupt.mockResolvedValue({ ...current, state: 'interrupted', seq_updated: source.seq + 1 });
  show(<StatusLine threadId={id} withStop />);
  expect(screen.getAllByRole('button', { name: i18n.t('composer.stop') })).toHaveLength(1);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('composer.stop') }));
  await waitFor(() => expect(wire.interrupt).toHaveBeenCalledExactlyOnceWith(defaultThreadRead(id), current.id));
});
