/** What a person sees of a turn: their message, the work of the agent as one line that opens to the steps, the answer, and what stands under it. */
import { useImperativeHandle } from 'react';
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Personal from '@/api/personal';
import type * as Live from '@/realtime/live';
import { client } from '@/api/client';
import { captureThreadRead } from '@/api/threadRead';
import { Timeline } from '@/features/timeline/Timeline';
import type { Item, MarkerPayload, Turn } from '@/generated/api-types';
import { ViewCompactContext } from '@/features/workbench/Narrow';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { formatSpan } from '@/lib/time';
import { threadWriter } from '@/realtime/stores/thread';
import type { VirtualListProps } from '@/ui/VirtualList';
import { me, userRef } from '../doubles/fixtures';
import { agentMessage, approvalItem, change, command, fileChange, harnessApproval, ALLOW_DENY, marker, reasoning, turn, userMessage } from '../doubles/thread-rows';
import { A, snapshot } from '../unit/continuation-fixtures';
import { show, skipOverflowFallback } from './support';

skipOverflowFallback();

const wire = vi.hoisted(() => { const state: Record<string, unknown> = {}; return { state }; });
vi.mock('@/api/personal', async (original) => ({ ...await original<typeof Personal>(),
  meStateQuery: { queryKey: ['me-state'], queryFn: () => Promise.resolve(wire.state), staleTime: Infinity },
  setMeState: (key: string, value: unknown) => { wire.state = { ...wire.state, [key]: value }; return Promise.resolve(); },
}));
vi.mock('@/realtime/live', async (original) => ({ ...await original<typeof Live>(), useSocketStatus: () => ({ phase: 'ready' }) }));
// Geometry belongs to the browser controls; render every planned row to inspect its actual content here.
vi.mock('@/ui/VirtualList', () => ({ VirtualList: (props: VirtualListProps) => {
  useImperativeHandle(props.ref, () => ({ scrollToEnd: () => undefined, scrollToKey: () => true, isAtEnd: () => true, element: () => null }));
  return <ol aria-label={props.label}>{Array.from({ length: props.count }, (_, index) => <li key={props.keyOf(index)}>{props.render(index)}</li>)}</ol>;
} }));
vi.mock('@/features/timeline/StatusLine', () => ({ StatusLine: () => null }));

const appearance = useAppearance.getState();
const themeBefore = document.documentElement.dataset.theme;
const session = client.session.getState();
const ids: string[] = [];
beforeEach(() => { wire.state = {}; client.session.setState({ status: 'signed_in', userId: 'turn-shape-reader', readerVersion: 1 }); });
afterEach(async () => {
  cleanup(); for (const id of ids.splice(0)) threadWriter.evict(id); client.session.setState(session); useAppearance.setState(appearance);
  if (themeBefore === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = themeBefore;
  await setLocale('en');
});

const START = Date.parse('2026-10-01T10:00:00.000Z');
const at = (seconds: number): string => new Date(START + seconds * 1_000).toISOString();
const report = (index: number): Item => marker(`state-${index}`, { marker: 'harness_changed', data: { program: 'Codex', change: 'session_status', subject: null, previous: { state: 'unsupported' },
  value: { type: 'active', active_flags: [] }, expected: { state: 'unsupported' }, changed: false, diverged: false, next_turn: false, turn_id: null, reported_at: at(0) } } as unknown as MarkerPayload);

/** Puts a thread into the store the way the timeline reads it: items numbered in the order given, each carrying its thread. */
function hold(items: Item[], turns: Turn[], more: { running?: boolean } = {}): string {
  const source = snapshot(); const id = source.thread.id; ids.push(id);
  const numbered = items.map((item, index) => ({ ...item, thread_id: id, continuation_id: A, seq_created: index + 1, seq_updated: index + 1 }));
  const last = turns.at(-1);
  threadWriter.reset(id, { ...source, thread: { ...source.thread, state: more.running === true ? 'running' : 'idle', current_turn_id: more.running === true ? last?.id ?? null : null }, turns, items: numbered }, false, captureThreadRead(id));
  return id;
}

/** The second turn walked on a real thread: 33 sentences of the agent, 29 commands (one of them failed), 3 file changes, 26 lines of the program's own state, a context line and the person's message. */
function longTurn(): Item[] {
  const items: Item[] = [userMessage('person', 'Make the package and run its tests', { turn: 'long', author: userRef(me(1, 'ada', 'Ada Lovelace')) })];
  for (let step = 0; step < 29; step += 1) {
    items.push(agentMessage(`say-${step}`, `Step ${step}: working on it`, { turn: 'long', phase: 'commentary' }));
    items.push(command(`run-${step}`, { command: `step ${step}`, output: 'ok', exit: step === 3 ? 1 : 0, turn: 'long' }));
    if (step < 26) items.push(report(step));
  }
  for (let file = 0; file < 3; file += 1) items.push(fileChange(`edit-${file}`, { turn: 'long', changes: [change(`src/file-${file}.py`, '+one\n+two\n-three')] }));
  for (let extra = 29; extra < 32; extra += 1) items.push(agentMessage(`say-${extra}`, `Then ${extra}`, { turn: 'long', phase: 'commentary' }));
  items.push(marker('context', { marker: 'orch_context', data: { model_input_id: 'record', purpose: 'input' } }, { turn: 'long' }));
  items.push(agentMessage('answer', 'Done. The package is built.', { turn: 'long', phase: 'final' }));
  return items;
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  it(`draws a finished turn of 93 items as the message, one line, the answer and one card, and opens the steps from the line at ${locale}/${theme}/${width}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme, resolvedTheme: theme, timeZone: 'UTC' }); document.documentElement.dataset.theme = theme;
    const items = longTurn();
    expect(items).toHaveLength(93);
    const id = hold(items, [turn('long', { ordinal: 1, state: 'completed', inputId: 'input', startedAt: at(0), completedAt: at(91), tokens: 1_100_000 })]);
    const view = show(<ViewCompactContext.Provider value={width === 400}><div style={{ width }}><Timeline threadId={id} item={null} empty={null} /></div></ViewCompactContext.Provider>);
    expect(await screen.findByText('Make the package and run its tests')).toBeVisible();
    expect(view.container.querySelector('[data-user-message]')).not.toBeNull();
    // One line says what the work came to: how long, and in a few words what it did.
    const line = view.container.querySelector<HTMLElement>('[data-fold="long"]');
    if (line === null) throw new Error('The work of the turn is one line');
    expect(line).toHaveTextContent(i18n.t('timeline.group.workedFor', { time: formatSpan(91_000, { locale, timeZone: 'UTC' }) }));
    expect(line).toHaveTextContent(i18n.t('timeline.group.commands', { count: 29 }));
    expect(line).toHaveTextContent(i18n.t('timeline.group.edited', { count: 3 }));
    expect(line).toHaveAttribute('aria-expanded', 'false');
    // A command of the turn failed and was got past: the line does not read failed, and no step is drawn until it is opened.
    expect(line).not.toHaveTextContent(i18n.t('timeline.group.failed'));
    expect(line).not.toHaveClass('text-danger-text');
    expect(view.container.querySelector('[data-item="run-0"]')).toBeNull();
    expect(view.container.querySelector('[data-item="say-0"]')).toBeNull();
    // The answer is plain text on the page, and the files stand in one card.
    expect(screen.getByText('Done. The package is built.')).toBeVisible();
    expect(view.container.querySelectorAll('[data-turn-files]')).toHaveLength(1);
    expect(view.container.querySelector('[data-turn-files="long"]')).toHaveTextContent(i18n.t('timeline.footer.changed', { count: 3, added: 6, removed: 3 }));
    // No figure that sums the requests is drawn, and none of the program's own state.
    expect(view.container).not.toHaveTextContent(/tokens|令牌/);
    expect(view.container).not.toHaveTextContent(/session status|会话状态/);
    if (width !== 1440) return;
    await userEvent.click(line);
    expect(line).toHaveAttribute('aria-expanded', 'true');
    expect(view.container.querySelector('[data-item="run-0"]')).not.toBeNull();
    // The sentences between the steps are inside, in quiet type; the failed step carries its own mark.
    expect(screen.getByText('Step 0: working on it').closest('[data-user-content]')?.className).toContain('text-text-muted');
    const failed = view.container.querySelector<HTMLElement>('[data-item="run-3"]');
    if (failed === null) throw new Error('The failed step is among the steps');
    expect(within(failed).getByRole('img', { name: i18n.t('timeline.step.failed') })).toBeInTheDocument();
    expect(line).not.toHaveClass('text-danger-text');
    await userEvent.click(line);
    expect(view.container.querySelector('[data-item="run-0"]')).toBeNull();
  });
}

it('says failed on the line only when the turn failed, and stopped, with who stopped it, when a person did', async () => {
  for (const [state, expected] of [['failed', 'timeline.group.failedAfter'], ['interrupted', 'timeline.group.stoppedAfter']] as const) {
    const ended = { ...turn(state, { ordinal: 1, state, inputId: 'input', startedAt: at(0), completedAt: at(30), interruptedBy: state === 'interrupted' ? userRef(me(2, 'grace', 'Grace Hopper')) : null }) };
    const id = hold([userMessage(`${state}-person`, 'Go', { turn: state, author: userRef(me(1, 'ada', 'Ada')) }), command(`${state}-run`, { command: 'make', output: '', exit: 0, turn: state })], [ended]);
    const view = show(<Timeline threadId={id} item={null} empty={null} />);
    await screen.findByText('Go');
    const line = view.container.querySelector<HTMLElement>(`[data-fold="${state}"]`);
    expect(line).toHaveTextContent(i18n.t(expected, { time: formatSpan(30_000, { locale: 'en', timeZone: 'UTC' }) }));
    if (state === 'interrupted') expect(line).toHaveTextContent('Grace Hopper');
    cleanup(); threadWriter.evict(id); ids.pop();
  }
});

it('draws thought as one step of the group, and a thought without text as its label alone', async () => {
  const person = userMessage('person', 'Think', { turn: 'thinking', author: userRef(me(1, 'ada', 'Ada')) });
  const id = hold([person, reasoning('thought-text', 'First I read the file', { turn: 'thinking' }), reasoning('thought-label', '', { turn: 'thinking' }),
    command('run', { command: 'ls', output: '', exit: 0, turn: 'thinking' }), agentMessage('answer', 'Read it.', { turn: 'thinking', phase: 'final' })],
  [turn('thinking', { ordinal: 1, state: 'completed', inputId: 'input', startedAt: at(0), completedAt: at(9) })]);
  const view = show(<Timeline threadId={id} item={null} empty={null} />);
  const line = await waitFor(() => { const found = view.container.querySelector<HTMLElement>('[data-fold="thinking"]'); if (found === null) throw new Error('waiting'); return found; });
  // No row of its own in the conversation.
  expect(view.container.querySelector('[data-item="thought-text"]')).toBeNull();
  await userEvent.click(line);
  const withText = view.container.querySelector<HTMLElement>('[data-item="thought-text"] button');
  const labelOnly = view.container.querySelector<HTMLElement>('[data-item="thought-label"] button');
  if (withText === null || labelOnly === null) throw new Error('Both thoughts are steps');
  expect(withText).toHaveAttribute('aria-expanded', 'false');
  expect(screen.queryByText('First I read the file')).not.toBeInTheDocument();
  await userEvent.click(withText);
  expect(screen.getByText('First I read the file')).toBeVisible();
  expect(labelOnly).toBeDisabled();
  expect(labelOnly).not.toHaveAttribute('aria-expanded');
});

it('opens the group of a running turn as a bounded preview, keeps the reply and a waiting question under it, and folds it to its line when the turn ends', async () => {
  const running = turn('live', { ordinal: 1, state: 'running', inputId: 'input', startedAt: at(0) });
  const source = snapshot();
  const request = harnessApproval('live-request', { threadId: source.thread.id, itemId: 'live-ask', requester: source.thread.owner, title: 'Inspect changes',
    request: { request_kind: 'command', options: ALLOW_DENY, detail: { command: 'inspect files', cwd: '/work', reason: 'Check the result' } } });
  const made = approvalItem('live-ask', request.id, { turn: 'live' });
  if (made.kind !== 'approval') throw new Error('The question is an approval');
  const ask = { ...made, payload: { approval_id: request.id, approval: request } };
  const id = hold([userMessage('person', 'Fix it', { turn: 'live', author: source.thread.owner }), command('first', { command: 'ls', output: '', exit: 0, turn: 'live' }),
    agentMessage('between', 'Now the second file', { turn: 'live', phase: 'commentary' }), command('second', { command: 'pwd', output: '', exit: null, turn: 'live' }),
    ask,
    agentMessage('reply', 'The reply so far', { turn: 'live', phase: 'commentary', status: 'in_progress' })], [running], { running: true });
  const view = show(<Timeline threadId={id} item={null} empty={null} />);
  const line = await waitFor(() => { const found = view.container.querySelector<HTMLElement>('[data-fold="live"]'); if (found === null) throw new Error('waiting'); return found; });
  expect(line).toHaveAttribute('aria-expanded', 'true');
  expect(line).toHaveTextContent(i18n.t('timeline.group.working'));
  const group = view.container.querySelector<HTMLElement>('[data-work]');
  const box = document.getElementById(line.getAttribute('aria-controls') ?? '');
  if (group === null || box === null) throw new Error('The running group has its preview');
  expect(box).toHaveClass('max-h-(--work-preview-height)');
  expect(within(box).getByText('Now the second file')).toBeVisible();
  expect(box.querySelector('[data-item="first"]')).not.toBeNull();
  expect(box.querySelector('[data-item="second"]')).not.toBeNull();
  // The question and the reply being written stand under the group, not inside its box.
  expect(box.querySelector('[data-item="live-ask"]')).toBeNull();
  expect(view.container.querySelector('[data-item="live-ask"]')).not.toBeNull();
  expect(screen.getByText('The reply so far')).toBeVisible();
  expect(box.contains(screen.getByText('The reply so far'))).toBe(false);
  act(() => threadWriter.apply(id, { turns: [{ ...running, state: 'completed', completed_at: at(20), seq_updated: 99 }],
    items: [{ ...ask, status: 'approved' as const, thread_id: id, seq_updated: 99 }], thread: { ...source.thread, state: 'idle', current_turn_id: null, seq: 99 } }, captureThreadRead(id)));
  await waitFor(() => expect(view.container.querySelector('[data-fold="live"]')).toHaveAttribute('aria-expanded', 'false'));
  expect(view.container.querySelector('[data-fold="live"]')).toHaveTextContent(i18n.t('timeline.group.workedFor', { time: formatSpan(20_000, { locale: 'en', timeZone: 'UTC' }) }));
  expect(view.container.querySelector('[data-item="first"]')).toBeNull();
});

it('names the sender above a person’s message only where more than one person wrote in the thread', async () => {
  const ada = userRef(me(1, 'ada', 'Ada Lovelace')), grace = userRef(me(2, 'grace', 'Grace Hopper'));
  const done = turn('talk', { ordinal: 1, state: 'completed', inputId: 'input', startedAt: at(0), completedAt: at(5) });
  const alone = hold([userMessage('first', 'Hello from me', { turn: 'talk', author: ada }), userMessage('second', 'And again', { turn: 'talk', author: ada })], [done]);
  const one = show(<Timeline threadId={alone} item={null} empty={null} />);
  await screen.findByText('Hello from me');
  expect(one.container.querySelectorAll('[data-user-message]')).toHaveLength(2);
  expect(one.container).not.toHaveTextContent('Ada Lovelace');
  cleanup(); threadWriter.evict(alone); ids.pop();
  const together = hold([userMessage('first', 'Hello from me', { turn: 'talk', author: ada }), userMessage('second', 'Me too', { turn: 'talk', author: grace })], [done]);
  const two = show(<Timeline threadId={together} item={null} empty={null} />);
  await screen.findByText('Me too');
  expect(two.container.querySelector('[data-item="first"] [data-user-message]')).toHaveTextContent('Ada Lovelace');
  expect(two.container.querySelector('[data-item="second"] [data-user-message]')).toHaveTextContent('Grace Hopper');
});
