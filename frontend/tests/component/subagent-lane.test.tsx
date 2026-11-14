/** A reported sub-agent opens its own live stream in the turn, using the same displays without a child composer. */
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type * as ThreadsApi from '@/api/threads';
import type { OutlineOptions } from '@/api/threads';
import type * as OpenView from '@/features/workbench/openView';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import { ToolCallRow } from '@/features/timeline/items/ToolCall';
import { AgentsFace, AgentsPanel } from '@/features/thread/AgentsPanel';
import { listAgents, useAgentCatalog } from '@/features/thread/agents';
import { AgentSwitchContext } from '@/features/thread/agentSwitch';
import { ViewOpeningContext } from '@/features/workbench/viewOpening';
import { SubagentLane, SubagentPanel } from '@/features/timeline/SubagentLane';
import { buildRows } from '@/features/timeline/rows';
import { timelineUi } from '@/features/timeline/state';
import type { Item, OutlineEntry, Thread, ThreadItemListReply, ThreadOutlinePage } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { defaultThreadRead } from '@/realtime/threadScope';
import { threadViewOf, threadWriter } from '@/realtime/stores/thread';
import { ACME } from '../doubles/fixtures';
import { backgroundWork } from '../doubles/background-wake';
import { World } from '../doubles/world';
import { agentMessage, change, command, fileChange, LOGIN_DIFF, marker, reasoning, toolCall, turn } from '../doubles/thread-rows';
import { show } from './support';
import { retiredCopy } from './retired-copy';

const calls = vi.hoisted(() => ({ read: vi.fn<typeof ThreadsApi.fetchSubagentItems>(), outline: vi.fn<(options: OutlineOptions, before: number | null) => Promise<ThreadOutlinePage>>(), panel: vi.fn() }));
vi.mock('@/api/threads', async (importOriginal) => {
  const actual = await importOriginal<typeof ThreadsApi>();
  return { ...actual, fetchSubagentItems: calls.read, outlineQuery: (target: Parameters<typeof actual.outlineQuery>[0], options: OutlineOptions = {}) => ({
    ...actual.outlineQuery(target, options), enabled: true,
    queryFn: ({ pageParam }: { pageParam: number | null }) => calls.outline(options, pageParam),
  }) };
});
vi.mock('@/features/workbench/openView', async (importOriginal) => ({ ...await importOriginal<typeof OpenView>(), revealView: calls.panel }));

const THREAD = 'thread-1';
const STREAM = 'sub:reviewer';
const context: TimelineContext = { threadId: THREAD, threadName: 'Repair login', detail: 'normal', layout: 'one', setLayout: () => undefined,
  meId: null, canInteract: false, canManage: false, live: true, lit: null, selection: NOT_PICKING };

function parent(state: 'running' | 'completed' = 'running', model?: string) {
  const item = toolCall('review-call', { kind: 'subagent', child_stream: STREAM, name: 'reviewer', role: 'review', prompt: 'Review the redirect.', state, ...(model === undefined ? {} : { model }) },
    { turn: 'turn-1', title: 'Review', status: state === 'running' ? 'in_progress' : 'completed' });
  if (item.kind !== 'tool_call') throw new Error('The tool fixture must return a tool call');
  return { ...item, thread_id: THREAD, segment_id: 'segment-1', seq_created: 1, seq_updated: 1 };
}

function numbered(item: Item, seq: number): Item { return { ...item, thread_id: THREAD, segment_id: 'segment-1', seq_created: seq, seq_updated: seq }; }
const child = numbered(agentMessage('review-result', 'The redirect is safe on the same origin.', { turn: 'turn-1', stream: STREAM }), 5);

function page(items: Item[] = [], older = false): ThreadItemListReply { return { turns: [], segments: [], items, has_older: older, has_newer: false }; }

beforeEach(() => {
  calls.read.mockReset(); calls.read.mockResolvedValue(page()); calls.panel.mockClear();
  calls.outline.mockReset().mockResolvedValue({ entries: [], next_before: null, total: 0, seq: 20 });
  timelineUi.forget(THREAD);
  threadWriter.evict(THREAD);
  const world = new World(), person = world.people.get('ada'), node = world.places.rowOf(ACME.mainThread);
  if (person === undefined || node === undefined) throw new Error('The lane needs its native thread fixture');
  const native = world.threads.thread(person, node);
  const thread: Thread = { ...native, id: THREAD, name: 'Repair login', seq: 20, revision: 1, state: 'running', side_of: null, lock: null, sides: null,
    current_segment_id: 'segment-1', cwd: '/workspace/web', directory: 'web', environment: null, environment_permissions: [],
    path: native.path.map((part) => part.id === native.id ? { ...part, id: THREAD, name: 'Repair login' } : part),
    access: { ...native.access, permissions: ['thread.view'] } };
  threadWriter.reset(THREAD, {
    thread,
    turns: [turn('turn-1', { ordinal: 1, state: 'running', inputId: 'input-1' })],
    items: [numbered(parent(), 1), child,
      numbered(command('review-test', { turn: 'turn-1', stream: STREAM, command: 'npm test login', output: 'The redirect test passes.', exit: 0 }), 6),
      numbered(agentMessage('other-child', 'Only the tester sees this stream.', { turn: 'turn-1', stream: 'sub:tester' }), 7),
      numbered(agentMessage('main-answer', 'This stays in the main conversation.', { turn: 'turn-1' }), 8)],
  }, false);
});
afterEach(async () => { cleanup(); threadWriter.evict(THREAD); timelineUi.forget(THREAD); vi.useRealTimers(); await setLocale('en'); document.documentElement.removeAttribute('data-theme'); });

function draw(open = true, reader = context) {
  timelineUi.setStep(THREAD, 'review-call', open);
  return show(<TimelineContextValue.Provider value={reader}><ToolCallRow threadId={THREAD} id="review-call" /></TimelineContextValue.Provider>);
}

describe('a sub-agent lane', () => {
  it.each(['normal', 'verbose', 'diagnostics'] as const)('uses the chosen %s turn shape with its own answer and file card while the parent still works', async (detail) => {
    const items = [numbered(parent('completed'), 1),
      numbered(reasoning('own-thought', 'Thinking about the redirect', { turn: 'turn-1', stream: STREAM }), 2),
      numbered(agentMessage('own-commentary', 'I will check it', { turn: 'turn-1', stream: STREAM, phase: 'commentary' }), 3),
      numbered(command('own-check', { turn: 'turn-1', stream: STREAM, command: 'check redirect', output: 'Only this child check', exit: 0 }), 4),
      numbered(fileChange('own-edit', { turn: 'turn-1', stream: STREAM, changes: [change('child.py', LOGIN_DIFF, { added: 4, removed: 1 })] }), 5),
      numbered(agentMessage('own-final', 'The child answer', { turn: 'turn-1', stream: STREAM, phase: 'final' }), 6),
      numbered(fileChange('parent-edit', { turn: 'turn-1', changes: [change('parent.py', LOGIN_DIFF, { added: 100, removed: 3 })] }), 7)];
    threadWriter.window(THREAD, items, false, false);
    const view = draw(true, { ...context, detail });
    const lane = screen.getByRole('region', { name: 'Sub-agent reviewer' });
    expect(within(lane).getByText('The child answer')).toBeVisible();
    expect(lane.querySelector('[data-turn-figures="turn-1"]')).not.toBeNull();
    const group = lane.querySelector('[data-work]');
    if (detail === 'normal') {
      expect(group).not.toBeNull();
      expect(within(group as HTMLElement).getByRole('button', { expanded: false })).toBeVisible();
      expect(lane).not.toHaveTextContent('I will check it');
      await userEvent.click(within(group as HTMLElement).getByRole('button', { expanded: false }));
      expect(lane).toHaveTextContent('I will check it');
    } else expect(group).toBeNull();
    const card = lane.querySelector<HTMLElement>('[data-turn-files]');
    expect(card).toHaveTextContent(i18n.t('timeline.footer.changed', { count: 1, added: 4, removed: 1 }));
    expect(card).not.toHaveTextContent('+104');
    await userEvent.click(within(card!).getByRole('button', { name: i18n.t('thread.turnFiles.review') }));
    const review = screen.getByRole('dialog', { name: i18n.t('views.changes.title') });
    expect(review).toHaveTextContent('child.py');
    expect(review).not.toHaveTextContent('parent.py');
    expect(view.container.querySelector('[data-footer="turn-1"]')).not.toHaveTextContent(i18n.t('timeline.footer.failed'));
  });

  it('enters a child’s own conversation from its starting row and keeps Open in panel beside it', async () => {
    const select = vi.fn();
    show(<AgentSwitchContext.Provider value={select}><TimelineContextValue.Provider value={context}><ToolCallRow threadId={THREAD} id="review-call" /></TimelineContextValue.Provider></AgentSwitchContext.Provider>);
    await userEvent.click(screen.getByRole('button', { name: /^Sub-agent reviewer/ }));
    expect(select).toHaveBeenCalledExactlyOnceWith('review-call');
    expect(screen.queryByRole('region', { name: 'Sub-agent reviewer' })).not.toBeInTheDocument();
    expect(calls.read).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('timeline.subagent.openPanel') }));
    expect(calls.panel).toHaveBeenCalledExactlyOnceWith('subagent', { thread: THREAD, item: 'review-call' });
  });

  it.each([false, true])('uses the main conversation classification with Diagnostics %s', (diagnostics) => {
    const reports = [
      ['session_status', { type: 'active', active_flags: [] }, 'Quiet session program'],
      ['mcp_startup', { status: 'starting' }, 'Starting service program'],
      ['mcp_startup', { status: 'ready' }, 'Ready service program'],
      ['mcp_startup', { status: 'failed', message: 'Credentials expired' }, 'Failed service program'],
    ] as const;
    act(() => threadWriter.apply(THREAD, { items: reports.map(([change, value, program], index) => numbered(marker(`child-report-${index}`, { marker: 'harness_changed', data: {
      program, change, subject: 'Source catalog', previous: { state: 'unsupported' }, value, expected: { state: 'unsupported' },
      changed: false, diverged: false, next_turn: false, turn_id: 'turn-1', reported_at: '2026-10-01T10:00:00.000Z',
    } }, { stream: STREAM, turn: 'turn-1' }), 22 + index)) }));
    draw(true, { ...context, detail: diagnostics ? 'diagnostics' : 'normal' });
    const lane = screen.getByRole('region', { name: 'Sub-agent reviewer' });
    expect(lane).toHaveTextContent('Failed service program');
    for (const program of ['Quiet session program', 'Starting service program', 'Ready service program']) {
      if (diagnostics) expect(lane).toHaveTextContent(program);
      else expect(lane).not.toHaveTextContent(program);
    }
  });

  it('keeps a running child collapsed without a history request until its row is opened', async () => {
    show(<TimelineContextValue.Provider value={context}><ToolCallRow threadId={THREAD} id="review-call" /></TimelineContextValue.Provider>);
    expect(screen.queryByRole('region', { name: 'Sub-agent reviewer' })).not.toBeInTheDocument();
    expect(calls.read).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: /^Sub-agent reviewer/, expanded: false }));
    expect(screen.getByRole('region', { name: 'Sub-agent reviewer' })).toHaveTextContent('The redirect is safe on the same origin.');
    await waitFor(() => expect(calls.read).toHaveBeenCalledTimes(1));
  });

  it('names the model its program says the sub-agent runs on, and nothing where it says none', () => {
    draw(true);
    const lane = screen.getByRole('region', { name: 'Sub-agent reviewer' });
    expect(lane.querySelector('[data-subagent-model]')).toBeNull();
    act(() => threadWriter.apply(THREAD, { items: [numbered(parent('running', 'deepseek/deepseek-v4'), 23)] }));
    expect(screen.getByRole('region', { name: 'Sub-agent reviewer' }).querySelector('[data-subagent-model]')).toHaveTextContent('deepseek/deepseek-v4');
  });

  it('collapses to the last child line with a single status and reopens the reported stream', async () => {
    act(() => threadWriter.apply(THREAD, { items: [numbered(parent('completed'), 22)] }));
    draw(false);
    const step = document.querySelector('[data-step="review-call"]');
    if (!(step instanceof HTMLElement)) throw new Error('The parent has its row');
    const toggle = within(step).getByRole('button', { expanded: false });
    expect(toggle.querySelector('[data-subagent-summary]')).toHaveTextContent('The redirect is safe on the same origin.');
    expect(within(toggle).getAllByRole('img', { name: 'Done' })).toHaveLength(1);
    expect(toggle.textContent).not.toMatch(/done/i);
    await userEvent.click(toggle);
    expect(screen.getByRole('region', { name: 'Sub-agent reviewer' })).toHaveTextContent('The redirect is safe on the same origin.');
  });

  it.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)('keeps the edit collapsed, its file and counts once, with Raw on its row and the actual difference on click in %s/%s', async (language, look) => {
    await setLocale(language); document.documentElement.setAttribute('data-theme', look);
    const item = numbered(fileChange('edit-1', { turn: 'turn-1', changes: [change('src/session.rs', LOGIN_DIFF, { added: 6, removed: 2 })] }), 22);
    act(() => threadWriter.apply(THREAD, { items: [item] }));
    show(<TimelineContextValue.Provider value={context}><ToolCallRow threadId={THREAD} id={item.id} /></TimelineContextValue.Provider>);
    expect(screen.getAllByText('+6 −2', { exact: true })).toHaveLength(1);
    const toggle = screen.getByRole('button', { expanded: false, name: new RegExp(i18n.t('timeline.step.edited', { path: 'src/session.rs' }).replaceAll('.', '\\.')) });
    expect(screen.queryByRole('table', { name: /src\/session.rs/ })).not.toBeInTheDocument();
    await userEvent.click(toggle);
    expect(screen.getByRole('table', { name: /src\/session.rs/ })).toBeVisible();
    expect(screen.getByRole('table', { name: /src\/session.rs/ })).toHaveTextContent('return redirect(next ?? "/");');
    expect(screen.getAllByText(/src\/session.rs/)).toHaveLength(1);
    expect(screen.getAllByText('+6 −2', { exact: true })).toHaveLength(1);
    expect(screen.queryByText(retiredCopy(language, 'timeline.step.raw'), { exact: true })).not.toBeInTheDocument();
    const raw = screen.getByRole('button', { name: i18n.t('timeline.step.showRaw') });
    expect(raw.textContent).toBe('');
    expect(raw.closest('[data-tool-row]')).toContainElement(toggle);
    expect(document.querySelector('[data-tool-duration]')).toBeNull();
    await userEvent.click(raw);
    expect(document.querySelector('[data-raw="edit-1"]')).not.toBeNull();
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    await userEvent.click(screen.getByRole('button', { name: i18n.t('timeline.step.showDisplay') }));
    expect(screen.getByRole('table', { name: /src\/session.rs/ })).toBeVisible();
    await userEvent.click(toggle);
    expect(screen.queryByRole('table')).not.toBeInTheDocument();
  });

  it.each(['normal', 'verbose', 'diagnostics'] as const)('starts recent edits, successful commands and failed commands as closed rows in %s reading', (detail) => {
    const items = [fileChange('latest-edit', { turn: 'turn-1', changes: [change('src/session.rs', LOGIN_DIFF)] }),
      command('ok-run', { turn: 'turn-1', command: 'check redirect', exit: 0, output: 'All checks pass' }),
      command('failed-run', { turn: 'turn-1', command: 'check expiry', exit: 1, output: 'The expiry check failed' })].map((item, index) => numbered(item, 22 + index));
    act(() => threadWriter.apply(THREAD, { items }));
    show(<TimelineContextValue.Provider value={{ ...context, detail }}>{items.map((item) => <ToolCallRow key={item.id} threadId={THREAD} id={item.id} />)}</TimelineContextValue.Provider>);
    const rows = document.querySelectorAll('[data-tool-row]');
    expect(rows).toHaveLength(3);
    for (const row of rows) {
      if (!(row instanceof HTMLElement)) throw new Error('Each step has its own tool row');
      expect(within(row).getByRole('button', { expanded: false })).toBeVisible();
      expect(within(row).queryByRole('button', { expanded: true })).not.toBeInTheDocument();
    }
    expect(screen.queryByRole('table')).not.toBeInTheDocument();
    expect(screen.queryByText('All checks pass')).not.toBeInTheDocument();
    expect(screen.queryByText('The expiry check failed')).not.toBeInTheDocument();
  });

  it.each([0, 1, 999, 1000, 1500])('shows a completed tool’s duration only from one second (%d ms)', (durationMs) => {
    const item = numbered(command('short-run', { turn: 'turn-1', command: 'check redirect', exit: 0, durationMs }), 22);
    act(() => threadWriter.apply(THREAD, { items: [item] }));
    show(<TimelineContextValue.Provider value={context}><ToolCallRow threadId={THREAD} id={item.id} /></TimelineContextValue.Provider>);
    const duration = document.querySelector('[data-tool-duration]');
    if (durationMs < 1000) expect(duration).toBeNull();
    else { expect(duration).not.toBeNull(); expect(duration?.textContent).not.toBe(''); }
    expect(screen.getByRole('button', { expanded: false, name: /check redirect/ })).toContainElement(screen.getByRole('img', { name: i18n.t('timeline.step.done') }));
  });

  it('keeps a live tool’s first sub-second interval quiet and begins the duration after its first second', async () => {
    vi.useFakeTimers(); const now = Date.now();
    const item = numbered(command('just-started', { turn: 'turn-1', command: 'check redirect', exit: null, at: new Date(now).toISOString() }), 22);
    act(() => threadWriter.apply(THREAD, { items: [item] }));
    show(<TimelineContextValue.Provider value={context}><ToolCallRow threadId={THREAD} id={item.id} /></TimelineContextValue.Provider>);
    expect(document.querySelector('[data-tool-duration]')).toBeNull();
    expect(document.querySelector('.animate-spin')).not.toBeNull();
    await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
    expect(document.querySelector('[data-tool-duration]')).not.toBeNull();
  });

  it('opens at its tool row with name, role and only that child stream at compact density', async () => {
    draw();
    const lane = screen.getByRole('region', { name: 'Sub-agent reviewer' });
    expect(lane).toHaveClass('border-l-2', 'pl-3');
    expect(lane).toHaveAttribute('data-density', 'compact');
    expect(lane).toHaveAttribute('data-conversation-lane', 'subagent');
    expect(screen.getByRole('button', { name: /Sub-agent reviewer/ })).toBeVisible();
    expect(within(lane).queryByText('reviewer', { exact: true })).not.toBeInTheDocument();
    expect(within(lane).getByText('Role: review')).toBeVisible();
    // The parent's row already supplies the name and clock; the inline lane must not repeat them.
    expect(lane.querySelector('[data-subagent-time]')).toBeNull();
    const log = within(lane).getByRole('log', { name: 'Activity of reviewer' });
    expect(within(log).getByText('The redirect is safe on the same origin.')).toBeVisible();
    expect(within(log).getByRole('button', { name: /npm test login/ })).toBeVisible();
    expect(log).not.toHaveTextContent('Only the tester');
    expect(log).not.toHaveTextContent('This stays in the main');
    expect(within(lane).queryByRole('textbox')).not.toBeInTheDocument();
    expect(within(lane).queryByRole('button', { name: /^(Send|Stop)$/ })).not.toBeInTheDocument();
    await waitFor(() => expect(calls.read).toHaveBeenCalledTimes(1));
    const asked = calls.read.mock.calls[0]!;
    expect(asked.slice(0, 3)).toEqual([defaultThreadRead(THREAD), STREAM, 21]);
    expect(asked[3]).toBeInstanceOf(AbortSignal);
    expect(asked[3]?.aborted).toBe(false);
  });

  it('shares a pending history read with its panel and cancels it only after the last view closes', async () => {
    let complete: (reply: ThreadItemListReply) => void = () => { throw new Error('The pending read has not been created'); };
    const pending = new Promise<ThreadItemListReply>((resolve) => { complete = resolve; });
    calls.read.mockReturnValue(pending);
    const item = threadViewOf(THREAD).items.get('review-call');
    if (item?.kind !== 'tool_call') throw new Error('The lane and panel need their actual parent row');
    timelineUi.setStep(THREAD, item.id, true);
    const user = userEvent.setup();
    const view = show(<TimelineContextValue.Provider value={context}>
      <section aria-label="Inline view"><ToolCallRow threadId={THREAD} id={item.id} /></section>
      <section aria-label="Panel view"><SubagentLane item={item} panel /></section>
    </TimelineContextValue.Provider>);
    try {
      await waitFor(() => expect(calls.read).toHaveBeenCalledTimes(1));
      const asked = calls.read.mock.calls[0]!;
      expect(asked.slice(0, 3)).toEqual([defaultThreadRead(THREAD), STREAM, 21]);
      const signal = asked[3];
      expect(signal).toBeInstanceOf(AbortSignal); expect(signal?.aborted).toBe(false);
      const inline = screen.getByRole('region', { name: 'Inline view' });
      await user.click(within(inline).getByRole('button', { name: /^Sub-agent reviewer/, expanded: true }));
      expect(within(inline).queryByRole('region', { name: 'Sub-agent reviewer' })).not.toBeInTheDocument();
      expect(within(screen.getByRole('region', { name: 'Panel view' })).getByRole('log', { name: 'Activity of reviewer' })).toBeVisible();
      expect(signal?.aborted).toBe(false); expect(calls.read).toHaveBeenCalledTimes(1);
      view.unmount();
      expect(signal?.aborted).toBe(true);
      const late = numbered(agentMessage('late-review', 'The closed reader must not take this late page.', { stream: STREAM, turn: 'turn-1' }), 3);
      await act(async () => { complete(page([late])); await pending; });
      expect(threadViewOf(THREAD).items.has(late.id)).toBe(false);
    } finally {
      view.unmount();
      await act(async () => { complete(page()); await pending; });
    }
  });

  it('folds and reopens the lane, preserving native child activity and the panel target', async () => {
    draw(); const user = userEvent.setup();
    const step = screen.getByRole('button', { name: /^Sub-agent reviewer/ });
    expect(step).toHaveAttribute('aria-expanded', 'true');
    await user.click(step);
    expect(screen.queryByRole('region', { name: 'Sub-agent reviewer' })).not.toBeInTheDocument();
    await user.click(step);
    const lane = screen.getByRole('region', { name: 'Sub-agent reviewer' });
    expect(within(lane).queryByRole('button', { name: 'Open in panel' })).not.toBeInTheDocument();
    const panel = screen.getByRole('button', { name: 'Open in panel' });
    const raw = screen.getByRole('button', { name: 'Show the raw call' });
    expect(panel.closest('[data-tool-row]')).toContainElement(step);
    expect(raw.closest('[data-tool-row]')).toContainElement(step);
    expect(panel.textContent).toBe(''); expect(raw.textContent).toBe('');
    await user.click(panel);
    expect(calls.panel).toHaveBeenCalledExactlyOnceWith('subagent', { thread: THREAD, item: 'review-call' });
    expect(within(lane).getByText('The redirect is safe on the same origin.')).toBeVisible();
  });

  it('updates a child message from live rows without copying it into the main stream', () => {
    draw();
    if (child.kind !== 'agent_message') throw new Error('The child fixture is an agent message');
    const updated = { ...child, seq_updated: 21, payload: { ...child.payload, text: 'The review has finished.' } };
    act(() => threadWriter.apply(THREAD, { items: [updated] }));
    const lane = screen.getByRole('region', { name: 'Sub-agent reviewer' });
    expect(within(lane).getByText('The review has finished.')).toBeVisible();
    const view = threadViewOf(THREAD);
    expect(view.order).not.toContain(child.id);
    expect(view.streams.get(STREAM)?.filter((id) => id === child.id)).toHaveLength(1);
  });

  it('reads earlier activity with the same stream and native sequence, without changing the main history flag', async () => {
    const older = numbered(agentMessage('older-review', 'Earlier in the review.', { stream: STREAM, turn: 'turn-1' }), 3);
    const first = numbered(agentMessage('first-review', 'The review began here.', { stream: STREAM, turn: 'turn-1' }), 2);
    calls.read.mockResolvedValueOnce(page([older], true)).mockResolvedValueOnce(page([first]));
    draw();
    await screen.findByText('Earlier in the review.');
    await userEvent.click(screen.getByRole('button', { name: 'Read earlier activity' }));
    await screen.findByText('The review began here.');
    expect(calls.read.mock.calls.map((args) => args.slice(0, 3))).toEqual([[defaultThreadRead(THREAD), STREAM, 21], [defaultThreadRead(THREAD), STREAM, 3]]);
    expect(threadViewOf(THREAD).hasOlder).toBe(false);
    expect(threadViewOf(THREAD).streams.get(STREAM)?.slice(0, 2)).toEqual(['first-review', 'older-review']);
  });

  it('puts the step that started a child among the work of its turn, and none of the child\u2019s own items in a row of the main conversation', () => {
    const rows = buildRows(threadViewOf(THREAD), { detail: 'normal', hideHistory: false, meId: null });
    expect(rows.some((row) => row.kind === 'work' && row.ids.includes('review-call'))).toBe(true);
    expect(rows.some((row) => row.kind === 'item' && row.id === child.id)).toBe(false);
  });

  it('lists the running child under Active with its line of activity, opens it from the panel, and keeps it under Done when it completes', async () => {
    const open = vi.fn();
    const entries = () => listAgents(threadViewOf(THREAD), [], i18n.t);
    const view = show(<AgentsPanel entries={entries()} current={null} onMain={vi.fn()} onOpen={open} onEnd={null} />);
    const active = screen.getByRole('region', { name: 'Active' });
    expect(within(active).getByText('reviewer')).toBeVisible();
    // What it is doing is the last thing its own stream says: its newest step.
    expect(within(active).getByText(/npm test login/)).toBeVisible();
    await userEvent.click(within(active).getByRole('button', { name: /reviewer/ }));
    expect(open).toHaveBeenCalledExactlyOnceWith('review-call');
    act(() => threadWriter.apply(THREAD, { items: [{ ...numbered(parent('completed'), 1), seq_updated: 22 }] }));
    view.rerender(<AgentsPanel entries={entries()} current={null} onMain={vi.fn()} onOpen={open} onEnd={null} />);
    expect(screen.queryByRole('region', { name: 'Active' })).not.toBeInTheDocument();
    const done = screen.getByRole('region', { name: 'Done' });
    expect(within(done).getByText('reviewer')).toBeVisible();
    // A finished sub-agent says what it came to.
    expect(within(done).getByText('The redirect is safe on the same origin.')).toBeVisible();
  });
});

it('keeps the named task on its existing child lane and separates native Stop from opening the conversation', async () => {
  const task = backgroundWork({ id: 'recorded-review', kind: 'subagent', item_id: 'review-call', turn_id: 'turn-1', label: 'Authentication reviewer' });
  const end = vi.fn(), open = vi.fn();
  show(<AgentsPanel entries={listAgents(threadViewOf(THREAD), [task], i18n.t)} current={null} onMain={vi.fn()} onOpen={open} onEnd={end} />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('backgroundWake.endNamed', { name: 'reviewer' }) }));
  expect(end).toHaveBeenCalledExactlyOnceWith(task);
  expect(open).not.toHaveBeenCalled();
});

it('offers no Stop for a sub-agent that cannot be stopped, or to a reader who may not, and never for a finished one', () => {
  const task = backgroundWork({ id: 'recorded-review', kind: 'subagent', item_id: 'review-call', turn_id: 'turn-1', label: 'Authentication reviewer', can_end: false });
  show(<AgentsPanel entries={listAgents(threadViewOf(THREAD), [task], i18n.t)} current={null} onMain={vi.fn()} onOpen={vi.fn()} onEnd={vi.fn()} />);
  expect(screen.queryByRole('button', { name: i18n.t('backgroundWake.endNamed', { name: 'reviewer' }) })).not.toBeInTheDocument();
  cleanup();
  show(<AgentsPanel entries={listAgents(threadViewOf(THREAD), [backgroundWork({ kind: 'subagent', item_id: 'review-call', turn_id: 'turn-1' })], i18n.t)} current={null} onMain={vi.fn()} onOpen={vi.fn()} onEnd={null} />);
  expect(screen.queryByRole('button', { name: /Stop|End/ })).not.toBeInTheDocument();
});

it('does not list a finished background child as running from a stale running transcript row', () => {
  const previous = threadViewOf(THREAD).turns.get('turn-1');
  if (previous === undefined) throw new Error('The child needs its retained source turn.');
  act(() => threadWriter.apply(THREAD, { turns: [{ ...turn('turn-1', { ordinal: 1, state: 'completed', inputId: 'input-1' }), seq_updated: previous.seq_updated + 1 }] }));
  show(<AgentsPanel entries={listAgents(threadViewOf(THREAD), [], i18n.t)} current={null} onMain={vi.fn()} onOpen={vi.fn()} onEnd={vi.fn()} />);
  expect(screen.queryByRole('region', { name: 'Active' })).not.toBeInTheDocument();
  expect(within(screen.getByRole('region', { name: 'Done' })).getByText(i18n.t('timeline.step.recordedRunning'))).toBeVisible();
});

it('shows a native lost child as lost instead of trusting its stale running transcript', () => {
  const task = backgroundWork({ kind: 'subagent', item_id: 'review-call', turn_id: 'turn-1', state: 'lost', can_end: false });
  show(<AgentsPanel entries={listAgents(threadViewOf(THREAD), [task], i18n.t)} current={null} onMain={vi.fn()} onOpen={vi.fn()} onEnd={vi.fn()} />);
  expect(screen.queryByRole('region', { name: 'Active' })).not.toBeInTheDocument();
  expect(within(screen.getByRole('region', { name: 'Done' })).getByText(i18n.t('backgroundWake.states.lost'))).toBeVisible();
  expect(screen.queryByRole('button', { name: /Stop|End/ })).not.toBeInTheDocument();
});

it('finds an agent by name in a long narrow list and opens the matching conversation', async () => {
  const tool = parent().payload.tool;
  if (tool.kind !== 'subagent') throw new Error('The source is a subagent');
  const children = Array.from({ length: 12 }, (_, index) => numbered(toolCall(`child-${index}`, { ...tool, name: `Reviewer ${index}`, child_stream: `sub:${index}`, state: 'completed' }, { turn: 'turn-1', title: `Reviewer ${index}`, status: 'completed' }), index + 30));
  act(() => threadWriter.apply(THREAD, { items: children }));
  const open = vi.fn();
  const view = show(<div style={{ width: 400 }}><AgentsPanel entries={listAgents(threadViewOf(THREAD), [], i18n.t)} current={null} onMain={vi.fn()} onOpen={open} onEnd={null} /></div>);
  await userEvent.type(screen.getByRole('textbox', { name: i18n.t('thread.agents.search') }), 'Reviewer 11');
  expect(screen.queryByRole('button', { name: /Reviewer 0\b/ })).not.toBeInTheDocument();
  const match = screen.getByRole('button', { name: /Reviewer 11\b/ });
  expect(match.closest('ul')).toHaveClass('overflow-y-auto');
  expect(view.container.querySelector('[data-agents-panel]')).toHaveClass('max-w-[calc(100vw-2rem)]');
  await userEvent.click(match);
  expect(open).toHaveBeenCalledExactlyOnceWith('child-11');
});

it('keeps a native running child active and stoppable after its parent turn has ended', async () => {
  const previous = threadViewOf(THREAD).turns.get('turn-1');
  if (previous === undefined) throw new Error('The child has its source turn');
  act(() => threadWriter.apply(THREAD, { turns: [{ ...previous, state: 'completed', seq_updated: previous.seq_updated + 1 }] }));
  const task = backgroundWork({ kind: 'subagent', item_id: 'review-call', turn_id: 'turn-1', state: 'running' });
  const end = vi.fn();
  show(<AgentsPanel entries={listAgents(threadViewOf(THREAD), [task], i18n.t)} current={null} onMain={vi.fn()} onOpen={vi.fn()} onEnd={end} />);
  expect(within(screen.getByRole('region', { name: 'Active' })).getByText('reviewer')).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('backgroundWake.endNamed', { name: 'reviewer' }) }));
  expect(end).toHaveBeenCalledExactlyOnceWith(task);
});

it('keeps roster-known work reachable after its starting row leaves the held history', () => {
  threadWriter.window(THREAD, [], true, true);
  const task = backgroundWork({ kind: 'subagent', item_id: 'old-review', turn_id: 'turn-1', label: 'Earlier reviewer', state: 'running' });
  const entries = listAgents(threadViewOf(THREAD), [task], i18n.t);
  expect(entries).toEqual([expect.objectContaining({ id: 'old-review', name: 'Earlier reviewer', state: 'running', task })]);
});

it('lists a finished older child from compact history facts without retaining its prompt body', () => {
  threadWriter.window(THREAD, [], true, false);
  const entries = listAgents(threadViewOf(THREAD), [], i18n.t, [{ item_id: 'old-review', seq: 1, at: '2026-10-01T10:00:00Z', title: 'Earlier reviewer', kind: 'subagent',
    agent: { stream: 'sub:older', turn_id: 'turn-1', name: 'Earlier reviewer', state: 'completed', started_at: '2026-10-01T10:00:00Z', duration_ms: 1200, tokens_out: 90, result: 'The earlier check passes', recorded: false, updated_seq: 10 } }]);
  show(<AgentsPanel entries={entries} current={null} onMain={vi.fn()} onOpen={vi.fn()} onEnd={null} />);
  expect(within(screen.getByRole('region', { name: 'Done' })).getByText('Earlier reviewer')).toBeVisible();
  expect(screen.getByText('The earlier check passes')).toBeVisible();
  expect(entries[0]?.stream).toBe('sub:older');
});

it('pages and searches the complete older child register without reading their conversation bodies', async () => {
  threadWriter.window(THREAD, [], true, true);
  const point = (id: string, seq: number): OutlineEntry => ({ item_id: id, seq, at: '2026-10-01T10:00:00Z', title: id, kind: 'subagent',
    agent: { stream: `sub:${id}`, turn_id: 'turn-1', name: id, state: 'completed', started_at: null, duration_ms: null, tokens_out: null, result: null, recorded: false, updated_seq: seq } });
  calls.outline.mockImplementation((options, before) => Promise.resolve({ entries: [point(options.q === 'hidden reviewer' ? 'Hidden reviewer' : before === null ? 'Recent reviewer' : 'Earlier reviewer', before === null ? 10 : 1)],
    next_before: options.q === '' && before === null ? 10 : null, total: 3, seq: 20 }));
  function Register() {
    const catalog = useAgentCatalog(THREAD);
    return <><AgentsFace entries={catalog.entries} count={catalog.count} /><AgentsPanel entries={catalog.entries} history={catalog.history} current={null} onMain={vi.fn()} onOpen={vi.fn()} onEnd={null} /></>;
  }
  show(<Register />);
  expect(await screen.findByText('Recent reviewer')).toBeVisible();
  expect(screen.getByText(i18n.t('thread.agents.count', { count: 3 }))).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('ui.pagination.more') }));
  expect(await screen.findByText('Earlier reviewer')).toBeVisible();
  await userEvent.type(screen.getByRole('textbox', { name: i18n.t('thread.agents.search') }), 'Hidden reviewer');
  expect(await screen.findByText('Hidden reviewer')).toBeVisible();
  expect(screen.getByRole('button', { name: i18n.t('thread.agents.main') })).toBeVisible();
  expect(calls.outline).toHaveBeenCalledWith(expect.objectContaining({ selection: 'agents', q: 'hidden reviewer' }), null);
  expect(calls.read).not.toHaveBeenCalled();
  expect(threadViewOf(THREAD).items.size).toBe(0);
});

it.each(['en', 'zh-CN'] as const)('keeps a foreign parent and an unavailable child stream from opening a reader in %s', async (locale) => {
  await setLocale(locale);
  const row = parent();
  const foreign = { ...row, thread_id: 'another-thread' };
  const view = show(<TimelineContextValue.Provider value={context}><SubagentLane item={foreign} /></TimelineContextValue.Provider>);
  expect(view.container.querySelector('[data-conversation-lane]')).toBeNull();
  expect(calls.read).not.toHaveBeenCalled();
  const tool = row.payload.tool;
  if (tool.kind !== 'subagent') throw new Error('The fixture has a subagent tool');
  const unavailable = { ...row, payload: { ...row.payload, tool: { ...tool, child_stream: '' } } };
  view.rerender(<TimelineContextValue.Provider value={context}><SubagentLane item={unavailable} /></TimelineContextValue.Provider>);
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('timeline.subagent.unavailable'));
  expect(calls.read).not.toHaveBeenCalled();
});

it('does not acquire the owned subagent reader while its workbench view is hidden', () => {
  const view = show(<ViewOpeningContext.Provider value={null}><SubagentPanel area="right" tab={{ id: 'hidden-child', view: 'subagent', of: { thread: THREAD, item: 'review-call' } }} /></ViewOpeningContext.Provider>);
  expect(view.container.querySelector('[data-subagent-panel]')).toBeNull();
  expect(calls.read).not.toHaveBeenCalled();
});

it('keeps the native Open in panel act visible with its reason when the child stream is unavailable', async () => {
  const row = parent(), tool = row.payload.tool;
  if (tool.kind !== 'subagent') throw new Error('The fixture has a subagent tool');
  act(() => threadWriter.apply(THREAD, { items: [{ ...row, seq_updated: 24, payload: { ...row.payload, tool: { ...tool, child_stream: '' } } }] }));
  draw(true);
  const open = screen.getByRole('button', { name: i18n.t('timeline.subagent.openPanel') });
  expect(open).toHaveAttribute('aria-disabled', 'true');
  await userEvent.click(open);
  expect(calls.panel).not.toHaveBeenCalled();
  expect(calls.read).not.toHaveBeenCalled();
});

it.each(['en', 'zh-CN'] as const)('inspects two concurrently running child lanes without mixing their logs or adding a composer in %s', async (locale) => {
  await setLocale(locale);
  const first = parent(), tool = first.payload.tool;
  if (tool.kind !== 'subagent') throw new Error('The fixture has a subagent tool');
  const second = { ...first, id: 'tester-call', payload: { ...first.payload, tool: { ...tool, child_stream: 'sub:tester', name: 'tester' } } };
  show(<TimelineContextValue.Provider value={context}><SubagentLane item={first} panel /><SubagentLane item={second} panel /></TimelineContextValue.Provider>);
  const review = screen.getByRole('log', { name: i18n.t('timeline.subagent.activity', { name: 'reviewer' }) });
  const testing = screen.getByRole('log', { name: i18n.t('timeline.subagent.activity', { name: 'tester' }) });
  expect(review).toHaveTextContent('The redirect is safe on the same origin.');
  expect(review).not.toHaveTextContent('Only the tester');
  expect(testing).toHaveTextContent('Only the tester sees this stream.');
  expect(testing).not.toHaveTextContent('The redirect is safe');
  expect(screen.queryByRole('textbox')).toBeNull();
  await waitFor(() => expect(calls.read).toHaveBeenCalledTimes(2));
  expect(new Set(calls.read.mock.calls.map((call) => call[1]))).toEqual(new Set([STREAM, 'sub:tester']));
});
