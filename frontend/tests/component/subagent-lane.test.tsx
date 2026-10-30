/** A reported sub-agent opens its own live stream in the turn, using the same displays without a child composer. */
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type * as ThreadsApi from '@/api/threads';
import type * as OpenView from '@/features/workbench/openView';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import { ToolCallRow } from '@/features/timeline/items/ToolCall';
import { SubagentChips } from '@/features/timeline/SubagentChips';
import { SubagentLane } from '@/features/timeline/SubagentLane';
import { timelineUi, useTimelineUi } from '@/features/timeline/state';
import { buildRows } from '@/features/timeline/rows';
import type { Item, Thread, ThreadItemListReply } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { defaultThreadRead } from '@/realtime/threadScope';
import { threadViewOf, threadWriter } from '@/realtime/stores/thread';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { agentMessage, change, command, fileChange, LOGIN_DIFF, toolCall, turn } from '../doubles/thread-rows';
import { show } from './support';
import { retiredCopy } from './retired-copy';

const calls = vi.hoisted(() => ({ read: vi.fn<typeof ThreadsApi.fetchSubagentItems>(), panel: vi.fn() }));
vi.mock('@/api/threads', async (importOriginal) => ({ ...await importOriginal<typeof ThreadsApi>(), fetchSubagentItems: calls.read }));
vi.mock('@/features/workbench/openView', async (importOriginal) => ({ ...await importOriginal<typeof OpenView>(), revealView: calls.panel }));

const THREAD = 'thread-1';
const STREAM = 'sub:reviewer';
const context: TimelineContext = { threadId: THREAD, threadName: 'Repair login', detail: 'steps', recent: new Set(['turn-1']), layout: 'one', setLayout: () => undefined,
  meId: null, canInteract: false, canManage: false, live: true, lit: null, selection: NOT_PICKING };

function parent(state: 'running' | 'completed' = 'running', model?: string) {
  const item = toolCall('review-call', { kind: 'subagent', child_stream: STREAM, name: 'reviewer', role: 'review', prompt: 'Review the redirect.', state, ...(model === undefined ? {} : { model }) },
    { turn: 'turn-1', title: 'Review', status: state === 'running' ? 'in_progress' : 'completed' });
  if (item.kind !== 'tool_call') throw new Error('The tool fixture must return a tool call');
  return item;
}

function numbered(item: Item, seq: number): Item { return { ...item, thread_id: THREAD, segment_id: 'segment-1', seq_created: seq, seq_updated: seq }; }
const child = numbered(agentMessage('review-result', 'The redirect is safe on the same origin.', { turn: 'turn-1', stream: STREAM }), 5);

function page(items: Item[] = [], older = false): ThreadItemListReply { return { turns: [], segments: [], items, has_older: older, has_newer: false }; }

beforeEach(() => {
  calls.read.mockReset(); calls.read.mockResolvedValue(page()); calls.panel.mockClear();
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

function draw(open = true) {
  timelineUi.setStep(THREAD, 'review-call', open);
  return show(<TimelineContextValue.Provider value={context}><ToolCallRow threadId={THREAD} id="review-call" /></TimelineContextValue.Provider>);
}

describe('a sub-agent lane', () => {
  it('keeps a running child collapsed without a history request until its row is opened', async () => {
    show(<TimelineContextValue.Provider value={context}><ToolCallRow threadId={THREAD} id="review-call" /></TimelineContextValue.Provider>);
    expect(screen.queryByRole('region', { name: 'Sub-agent reviewer' })).not.toBeInTheDocument();
    expect(calls.read).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: /^Sub-agent reviewer/, expanded: false }));
    expect(screen.getByRole('region', { name: 'Sub-agent reviewer' })).toHaveTextContent('The redirect is safe on the same origin.');
    await waitFor(() => expect(calls.read).toHaveBeenCalledTimes(1));
  });

  it('names the model its program says the sub-agent runs on, and nothing where it says none', async () => {
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

  it.each(['steps', 'summary', 'everything'] as const)('starts recent edits, successful commands and failed commands as closed rows at %s detail', (detail) => {
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

  it('uses the running header chip to open the turn and lane, then removes it when the child completes', async () => {
    const reveal = vi.fn();
    timelineUi.toggleRaw(THREAD, 'review-call');
    show(<SubagentChips threadId={THREAD} onReveal={reveal} />);
    await userEvent.click(screen.getByRole('button', { name: "Show reviewer's activity" }));
    expect(reveal).toHaveBeenCalledExactlyOnceWith('review-call');
    const ui = useTimelineUi.getState().byThread.get(THREAD)!;
    expect(ui.steps.get('review-call')).toBe(true);
    expect(ui.raw.has('review-call')).toBe(false);
    const rows = buildRows(threadViewOf(THREAD), { detail: 'summary', hideHistory: false, foldOpened: ui.folds, meId: null });
    expect(rows.some((row) => row.kind === 'item' && row.id === 'review-call')).toBe(true);
    expect(rows.some((row) => row.kind === 'item' && row.id === child.id)).toBe(false);
    act(() => threadWriter.apply(THREAD, { items: [{ ...numbered(parent('completed'), 1), seq_updated: 22 }] }));
    expect(screen.queryByRole('group', { name: 'Running sub-agents' })).not.toBeInTheDocument();
  });
});
