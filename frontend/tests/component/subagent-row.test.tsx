/** A sub-agent in its parent's work is one compact row the way its program draws it: kind and task, how it stands, its figures and one line of what it does or came to. */
import { act, cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type * as ThreadsApi from '@/api/threads';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import { ToolCallRow } from '@/features/timeline/items/ToolCall';
import { AgentSwitchContext } from '@/features/thread/agentSwitch';
import { AgentsPanel } from '@/features/thread/AgentsPanel';
import { listAgents } from '@/features/thread/agents';
import { timelineUi } from '@/features/timeline/state';
import type { Item, Thread, ToolPayload } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { threadViewOf, threadWriter } from '@/realtime/stores/thread';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { ALLOW_DENY, agentMessage, approvalItem, command, harnessApproval, reasoning, toolCall, turn } from '../doubles/thread-rows';
import { me, userRef } from '../doubles/fixtures';
import { ItemRow } from '@/features/timeline/ItemRow';
import { SubagentLane } from '@/features/timeline/SubagentLane';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn<typeof ThreadsApi.fetchSubagentItems>() }));
vi.mock('@/api/threads', async (importOriginal) => ({ ...await importOriginal<typeof ThreadsApi>(), fetchSubagentItems: calls.read }));

const THREAD = 'thread-rows';
const context: TimelineContext = { threadId: THREAD, threadName: 'Find the login flow', detail: 'normal', layout: 'one', setLayout: () => undefined,
  meId: null, canInteract: false, canManage: false, live: true, lit: null, selection: NOT_PICKING };

type SubagentPayload = Extract<ToolPayload, { kind: 'subagent' }>;

function numbered(item: Item, seq: number): Item { return { ...item, thread_id: THREAD, segment_id: 'segment-1', seq_created: seq, seq_updated: seq }; }

/** A sub-agent the agent started, as Claude Code reports it: its task as the name, its kind as the role. */
function started(id: string, stream: string, seq: number, extra: Partial<SubagentPayload> = {}, status: 'in_progress' | 'completed' | 'cancelled' = 'in_progress', durationMs: number | null = null): Item {
  const state = status === 'in_progress' ? 'running' : status === 'cancelled' ? 'interrupted' : 'completed';
  const tool: SubagentPayload = { kind: 'subagent', child_stream: stream, name: 'Find the login flow', role: 'Explore', prompt: 'Find where login is handled.', state, ...extra };
  return numbered(toolCall(id, tool, { turn: 'turn-1', title: 'Find the login flow', status, durationMs }), seq);
}

beforeEach(() => {
  calls.read.mockReset(); calls.read.mockResolvedValue({ turns: [], segments: [], items: [], has_older: false, has_newer: false });
  timelineUi.forget(THREAD);
  threadWriter.evict(THREAD);
  const world = new World(), person = world.people.get('ada'), node = world.places.rowOf(ACME.mainThread);
  if (person === undefined || node === undefined) throw new Error('The rows need their native thread fixture');
  const native = world.threads.thread(person, node);
  const thread: Thread = { ...native, id: THREAD, name: 'Find the login flow', seq: 40, revision: 1, state: 'running', side_of: null, lock: null, sides: null,
    current_segment_id: 'segment-1', cwd: '/workspace/web', directory: 'web', environment: null, environment_permissions: [],
    path: native.path.map((part) => part.id === native.id ? { ...part, id: THREAD, name: 'Find the login flow' } : part),
    access: { ...native.access, permissions: ['thread.view'] } };
  threadWriter.reset(THREAD, { thread, turns: [turn('turn-1', { ordinal: 1, state: 'running', inputId: 'input-1' })], items: [] }, false);
});
afterEach(async () => { cleanup(); threadWriter.evict(THREAD); timelineUi.forget(THREAD); await setLocale('en'); });

function row(id: string, switchTo: ((step: string) => void) | null = vi.fn()) {
  return <AgentSwitchContext.Provider value={switchTo}><TimelineContextValue.Provider value={context}><ToolCallRow threadId={THREAD} id={id} /></TimelineContextValue.Provider></AgentSwitchContext.Provider>;
}

describe('a sub-agent row', () => {
  it('names a finished sub-agent by its kind and task, with its tool uses, its model and the first line of what it came to in plain words', () => {
    act(() => threadWriter.apply(THREAD, { items: [
      started('call-1', 'sub:one', 1, { model: 'haiku' }, 'completed', 38_000),
      numbered(command('one-ls', { turn: 'turn-1', stream: 'sub:one', command: 'ls -la', output: 'src', exit: 0 }), 2),
      numbered(command('one-grep', { turn: 'turn-1', stream: 'sub:one', command: 'grep -rn login src', output: 'src/login.ts', exit: 0 }), 3),
      numbered(agentMessage('one-answer', '## Findings\n\nThe **login flow** is in `src/login.ts`.\n\n- `web/` (directory)', { turn: 'turn-1', stream: 'sub:one' }), 4),
    ] }));
    show(row('call-1'));
    const step = document.querySelector<HTMLElement>('[data-subagent-row="call-1"]');
    if (step === null) throw new Error('The sub-agent has its row');
    expect(step.querySelector('[data-subagent-role]')).toHaveTextContent('Explore');
    expect(step.querySelector('[data-subagent-task]')).toHaveTextContent('Find the login flow');
    expect(step.querySelector('[data-subagent-figures]')).toHaveTextContent('2 tool uses · haiku');
    // What it came to is the first line that says something, without the Markdown marks a row cannot draw.
    expect(step.querySelector('[data-subagent-summary="result"]')).toHaveTextContent('The login flow is in src/login.ts.');
    expect(step).not.toHaveTextContent('web/');
    expect(within(step).getByRole('img', { name: i18n.t('timeline.step.done') })).toBeVisible();
    expect(step.querySelector('[data-tool-duration]')).toHaveTextContent('38s');
    // Its accessible name says the same in one line.
    expect(within(step).getByRole('button', { name: /^Sub-agent Find the login flow · Explore · done · 38s · 2 tool uses · haiku · The login flow/ })).toBeVisible();
  });

  it('says what a running sub-agent does now and counts its tool uses as they arrive', () => {
    act(() => threadWriter.apply(THREAD, { items: [started('call-1', 'sub:one', 1),
      numbered(reasoning('one-thought', 'Where is login handled?', { turn: 'turn-1', stream: 'sub:one' }), 2)] }));
    show(row('call-1'));
    const step = () => document.querySelector<HTMLElement>('[data-subagent-row="call-1"]');
    expect(step()?.querySelector('[data-subagent-summary="activity"]')).toHaveTextContent(i18n.t('thread.now.thinking'));
    expect(step()?.querySelector('[data-subagent-figures]')).toBeNull();
    expect(within(step()!).getByText(i18n.t('timeline.step.running'))).toBeInTheDocument();
    act(() => threadWriter.apply(THREAD, { items: [numbered(command('one-grep', { turn: 'turn-1', stream: 'sub:one', command: 'grep -rn login src', status: 'in_progress' }), 3)] }));
    expect(step()?.querySelector('[data-subagent-summary="activity"]')).toHaveTextContent('grep -rn login src');
    expect(step()?.querySelector('[data-subagent-figures]')).toHaveTextContent('1 tool use');
  });

  it('stands two sub-agents started at once one under the other, each with its own task and line', () => {
    act(() => threadWriter.apply(THREAD, { items: [
      started('call-1', 'sub:one', 1),
      started('call-2', 'sub:two', 2, { name: 'Find the three largest source files' }),
      numbered(command('two-find', { turn: 'turn-1', stream: 'sub:two', command: 'find . -type f', status: 'in_progress' }), 3),
      numbered(command('one-ls', { turn: 'turn-1', stream: 'sub:one', command: 'ls -la', status: 'in_progress' }), 4),
    ] }));
    show(<>{row('call-1')}{row('call-2')}</>);
    const [first, second] = [...document.querySelectorAll<HTMLElement>('[data-subagent-row]')];
    expect(first?.querySelector('[data-subagent-task]')).toHaveTextContent('Find the login flow');
    expect(first?.querySelector('[data-subagent-summary]')).toHaveTextContent('ls -la');
    expect(second?.querySelector('[data-subagent-task]')).toHaveTextContent('Find the three largest source files');
    expect(second?.querySelector('[data-subagent-summary]')).toHaveTextContent('find . -type f');
    expect(first).not.toHaveTextContent('find . -type f');
  });

  it('opens its own conversation on a click and offers it in a panel beside, without a chevron that would only fold', async () => {
    const select = vi.fn();
    act(() => threadWriter.apply(THREAD, { items: [started('call-1', 'sub:one', 1)] }));
    show(row('call-1', select));
    const button = screen.getByRole('button', { name: /^Sub-agent Find the login flow/ });
    expect(button).not.toHaveAttribute('aria-expanded');
    await userEvent.click(button);
    expect(select).toHaveBeenCalledExactlyOnceWith('call-1');
    expect(screen.getByRole('button', { name: i18n.t('timeline.subagent.openPanel') })).toBeVisible();
  });

  it('says a stopped sub-agent was stopped, not refused', () => {
    act(() => threadWriter.apply(THREAD, { items: [started('call-1', 'sub:one', 1, {}, 'cancelled', 4_000)] }));
    show(row('call-1'));
    expect(screen.getByRole('img', { name: i18n.t('timeline.group.stopped') })).toBeVisible();
    expect(screen.queryByRole('img', { name: i18n.t('timeline.step.refused') })).toBeNull();
  });

  it('names an unnamed sub-agent by the first line of what it was asked, and a bare one as a sub-agent', () => {
    act(() => threadWriter.apply(THREAD, { items: [started('call-1', 'sub:one', 1, { name: null, role: null, prompt: '# Task\nList the files at the top level.' }),
      started('call-2', 'sub:two', 2, { name: null, role: null, prompt: null })] }));
    show(<>{row('call-1')}{row('call-2')}</>);
    expect(document.querySelector('[data-subagent-row="call-1"] [data-subagent-task]')).toHaveTextContent(/^Task$/);
    expect(document.querySelector('[data-subagent-row="call-2"] [data-subagent-task]')).toHaveTextContent(i18n.t('timeline.step.subagentUnnamed'));
  });

  it('leaves the tool uses unsaid where the page does not hold the end of the thread', () => {
    act(() => threadWriter.apply(THREAD, { items: [started('call-1', 'sub:one', 1, {}, 'completed', 2_000),
      numbered(command('one-ls', { turn: 'turn-1', stream: 'sub:one', command: 'ls', exit: 0 }), 2)] }));
    threadWriter.window(THREAD, [...threadViewOf(THREAD).items.values()], false, true);
    show(row('call-1'));
    expect(document.querySelector('[data-subagent-figures]')).toBeNull();
  });

  it.each(['en', 'zh-CN'] as const)('lists a sub-agent in the agents panel by its kind, task and tool uses in %s', async (locale) => {
    await setLocale(locale);
    act(() => threadWriter.apply(THREAD, { items: [started('call-1', 'sub:one', 1, {}, 'completed', 6_000),
      numbered(command('one-ls', { turn: 'turn-1', stream: 'sub:one', command: 'ls', exit: 0 }), 2),
      numbered(agentMessage('one-answer', 'No README at the top level.', { turn: 'turn-1', stream: 'sub:one' }), 3)] }));
    const view = show(<AgentsPanel entries={listAgents(threadViewOf(THREAD), [], i18n.t)} current={null} onMain={vi.fn()} onOpen={vi.fn()} onEnd={null} />);
    const entry = within(screen.getByRole('region', { name: i18n.t('thread.agents.done') })).getByRole('button', { name: /Find the login flow/ });
    expect(entry).toHaveTextContent('Explore');
    expect(entry).toHaveTextContent(i18n.t('timeline.subagent.tools', { count: 1, number: '1' }));
    expect(entry).toHaveTextContent('No README at the top level.');
    // The panel fits the card it opens in: no row of it is wider than the card.
    expect(view.container.querySelector('[data-agents-panel]')).toHaveClass('w-80', 'max-w-full');
  });

  it('says which sub-agent asks an approval filed beside the main conversation, opens it, and shows the request in its own conversation', async () => {
    const asked = 'find . -name "*.ts" | xargs wc -l';
    const approval = harnessApproval('approval-1', { threadId: THREAD, itemId: 'asked', title: `Run ${asked}`, requester: userRef(me(1, 'ada', 'Ada')),
      request: { request_kind: 'command', options: ALLOW_DENY, detail: { command: asked, cwd: '/workspace/web', reason: 'Measure the source files' } } });
    const pending = approvalItem('asked', 'approval-1', { turn: 'turn-1' });
    if (pending.kind !== 'approval') throw new Error('The fixture is an approval');
    act(() => threadWriter.apply(THREAD, { items: [
      started('call-1', 'sub:one', 1),
      started('call-2', 'sub:two', 2, { name: 'Find the three largest source files' }),
      numbered(command('two-find', { turn: 'turn-1', stream: 'sub:two', command: asked, status: 'in_progress' }), 3),
      numbered({ ...pending, payload: { ...pending.payload, approval } }, 4),
    ] }));
    const select = vi.fn();
    show(<AgentSwitchContext.Provider value={select}><TimelineContextValue.Provider value={context}><ItemRow id="asked" /></TimelineContextValue.Provider></AgentSwitchContext.Provider>);
    const asker = screen.getByRole('button', { name: i18n.t('approvals.askedBy', { agent: 'Explore · Find the three largest source files' }) });
    await userEvent.click(asker);
    expect(select).toHaveBeenCalledExactlyOnceWith('call-2');
    cleanup();
    // In that sub-agent's own conversation the request stands among its steps, without saying again who asks.
    const parent = threadViewOf(THREAD).items.get('call-2');
    if (parent?.kind !== 'tool_call') throw new Error('The sub-agent has its step');
    show(<TimelineContextValue.Provider value={context}><SubagentLane item={parent} panel /></TimelineContextValue.Provider>);
    const lane = screen.getByRole('region', { name: i18n.t('timeline.subagent.lane', { name: 'Find the three largest source files' }) });
    expect(lane.querySelector('[data-approval="asked"]')).not.toBeNull();
    expect(lane.querySelector('[data-approval-asker]')).toBeNull();
    cleanup();
    // Decided, it is one step: its long title keeps to two lines, and it still says who asked.
    const decided = { ...approval, state: 'approved' as const, decision: { option_id: 'allow_once' }, decided_by: userRef(me(1, 'ada', 'Ada')), decided_at: '2026-10-01T10:00:05.000Z' };
    act(() => threadWriter.apply(THREAD, { items: [numbered({ ...pending, status: 'approved', payload: { ...pending.payload, approval: decided } }, 5)] }));
    show(<AgentSwitchContext.Provider value={select}><TimelineContextValue.Provider value={context}><ItemRow id="asked" /></TimelineContextValue.Provider></AgentSwitchContext.Provider>);
    const title = screen.getByText(`Run ${asked}`);
    expect(title).toHaveClass('line-clamp-2');
    expect(document.querySelector('[data-approval-asker]')).toHaveTextContent(i18n.t('approvals.askedBy', { agent: 'Explore · Find the three largest source files' }));
  });
});

