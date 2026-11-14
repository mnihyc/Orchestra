/** The page as its two halves stand together: the rows of a turn drawn by the conversation, and the frame round them (the dock's one status line, the moves between people's messages, the round button's count, the message on its way). */
import { useImperativeHandle, useRef, useState } from 'react';
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Personal from '@/api/personal';
import type * as Live from '@/realtime/live';
import { client } from '@/api/client';
import { captureThreadRead } from '@/api/threadRead';
import { useMessageNav } from '@/features/thread/messageMoves';
import { StickyPrompt } from '@/features/thread/MessageParts';
import { SentMessageBlock } from '@/features/thread/SentMessageBlock';
import { StatusLine } from '@/features/timeline/StatusLine';
import { type FollowState, Timeline, type TimelineHandle } from '@/features/timeline/Timeline';
import type { Item, Turn } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { formatSpan } from '@/lib/time';
import { threadWriter } from '@/realtime/stores/thread';
import type { VirtualListProps } from '@/ui/VirtualList';
import { me, userRef } from '../doubles/fixtures';
import { ALLOW_DENY, agentMessage, approvalItem, command, harnessApproval, turn, userMessage } from '../doubles/thread-rows';
import { executionDisplay } from '../doubles/thread-display';
import { snapshot } from '../unit/continuation-fixtures';
import { show, skipOverflowFallback } from './support';

skipOverflowFallback();

const drawn = vi.hoisted(() => ({ list: undefined as VirtualListProps | undefined, scrolled: [] as { key: string; align: string | undefined }[] }));
vi.mock('@/api/personal', async (original) => ({ ...await original<typeof Personal>(),
  meStateQuery: { queryKey: ['me-state'], queryFn: () => Promise.resolve({}), staleTime: Infinity },
  setMeState: () => Promise.resolve(),
}));
vi.mock('@/realtime/live', async (original) => ({ ...await original<typeof Live>(), useSocketStatus: () => ({ phase: 'ready' }) }));
// Geometry belongs to the browser controls: the list draws every row, keeps what it was given for the case to call, and says which rows it was asked to go to.
vi.mock('@/ui/VirtualList', () => ({ VirtualList: (props: VirtualListProps) => {
  drawn.list = props;
  const keys = Array.from({ length: props.count }, (_, index) => props.keyOf(index));
  useImperativeHandle(props.ref, () => ({ scrollToEnd: () => undefined, isAtEnd: () => true, element: () => null,
    scrollToKey: (key: string, align?: string) => { drawn.scrolled.push({ key, align }); return keys.includes(key); } }));
  return <ol aria-label={props.label}>{keys.map((key, index) => <li key={key}>{props.render(index)}</li>)}</ol>;
} }));

const source = snapshot(null);
const THREAD = source.thread.id;
const ADA = userRef(me(1, 'ada', 'Ada'));
const START = Date.parse('2026-10-01T10:00:00.000Z');
const at = (seconds: number): string => new Date(START + seconds * 1_000).toISOString();
const appearance = useAppearance.getState();
const session = client.session.getState();
beforeEach(() => {
  client.session.setState({ status: 'signed_in', userId: 'join-reader', readerVersion: 1 });
  useAppearance.setState({ locale: 'en', timeZone: 'UTC', theme: 'light', resolvedTheme: 'light' });
  drawn.list = undefined;
  drawn.scrolled = [];
});
afterEach(async () => {
  cleanup(); vi.useRealTimers(); threadWriter.evict(THREAD); client.session.setState(session); useAppearance.setState(appearance); await setLocale('en');
});

/** The thread held by the store the way the conversation reads it: items numbered in the order given, from 2. */
function hold(items: Item[], turns: Turn[], state: 'idle' | 'running' | 'waiting_approval' = 'idle'): void {
  const numbered = items.map((item, index) => ({ ...item, thread_id: THREAD, seq_created: index + 2, seq_updated: index + 2 }));
  const running = turns.find((one) => one.state === 'running');
  threadWriter.reset(THREAD, { ...source, thread: { ...source.thread, my_last_read_seq: 99, display: executionDisplay(state), state, current_turn_id: running?.id ?? null }, turns, items: numbered }, false, captureThreadRead(THREAD));
}

/** The conversation, and the dock under it where the one status line stands. */
function Page() {
  return <>
    <div data-conversation><Timeline threadId={THREAD} item={null} empty={null} /></div>
    <div data-dock><StatusLine threadId={THREAD} /></div>
  </>;
}

it('draws the status line once, in the dock, with the turn’s one clock, and the work line of the running turn carries none', async () => {
  vi.useFakeTimers(); vi.setSystemTime(START + 75_000);
  hold([userMessage('ask', 'Run the checks', { turn: 'run', author: ADA }), command('c1', { turn: 'run', command: 'make', exit: 0 }),
    command('c2', { turn: 'run', command: 'make test', at: at(70) })], [turn('run', { ordinal: 1, state: 'running', inputId: 'input', startedAt: at(0) })], 'running');
  const view = show(<Page />);
  await act(async () => { await vi.advanceTimersByTimeAsync(0); });
  const clock = formatSpan(75_000, { locale: 'en', timeZone: 'UTC' });
  expect(document.querySelectorAll('[data-status-line]')).toHaveLength(1);
  expect(view.container.querySelector('[data-conversation] [data-status-line]')).toBeNull();
  expect(view.container.querySelectorAll('[data-status-item="elapsed"]')).toHaveLength(1);
  expect(view.container.querySelector('[data-dock] [data-status-item="elapsed"]')).toHaveTextContent(clock);
  const line = view.container.querySelector('[data-conversation] [data-fold="run"]');
  expect(line).toHaveTextContent(i18n.t('timeline.group.working'));
  expect(line).not.toHaveTextContent(clock);
});

it('stands a waiting approval under the group of work it came from, and the dock says it needs the person’s input', async () => {
  const approval = harnessApproval('wait-request', { threadId: THREAD, itemId: 'ask-run', requester: ADA, title: 'Run cleanup',
    request: { request_kind: 'command', options: ALLOW_DENY, detail: { command: 'clean output', cwd: '/work', reason: null } } });
  const waiting: Item = { ...approvalItem('ask-run', approval.id, { turn: 'run' }), kind: 'approval', payload: { approval_id: approval.id, approval } };
  hold([userMessage('ask', 'Clean up', { turn: 'run', author: ADA }), command('c1', { turn: 'run', command: 'ls', exit: 0 }), waiting],
    [turn('run', { ordinal: 1, state: 'running', inputId: 'input', startedAt: at(0) })], 'waiting_approval');
  const view = show(<Page />);
  const group = await waitFor(() => { const found = view.container.querySelector('[data-conversation] [data-fold="run"]'); if (found === null) throw new Error('waiting'); return found; });
  const card = view.container.querySelector('[data-conversation] [data-item="ask-run"]');
  if (card === null) throw new Error('The waiting approval stands in the conversation');
  expect(group.compareDocumentPosition(card) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  // It is not one of the steps the group holds.
  expect(view.container.querySelector('[data-work] [data-item="ask-run"]')).toBeNull();
  expect(view.container.querySelector('[data-dock] [data-status-line]')).toHaveTextContent(i18n.t('thread.now.needsInput'));
});

/** A turn of two messages: the person's, work, the person's message steered in, more work, and the answer. */
function steered(): Item[] {
  return [userMessage('first', 'Fix the redirect', { turn: 't1', author: ADA }), command('c1', { turn: 't1', command: 'ls', exit: 0 }), command('c2', { turn: 't1', command: 'pwd', exit: 0 }),
    userMessage('steer', 'Use the other file', { turn: 't1', author: ADA, delivery: 'steer' }), command('c3', { turn: 't1', command: 'cat a', exit: 0 }), agentMessage('answer', 'Done.', { turn: 't1' })];
}

function Moves() {
  const timeline = useRef<TimelineHandle>(null);
  const [top, setTop] = useState<string | null>(null);
  const [inView, setInView] = useState<ReadonlySet<string>>(new Set());
  const nav = useMessageNav(THREAD, { top, visible: inView }, (item) => timeline.current?.scrollToItem(item, { align: 'start', highlight: false }));
  return <>
    <StickyPrompt nav={nav} />
    <Timeline ref={timeline} threadId={THREAD} item={null} empty={null} onTop={setTop} onVisible={setInView} />
  </>;
}

it('finds a person’s message steered into a turn: the sticky prompt names it from the work after it, and previous goes to it as a row of its own', async () => {
  hold(steered(), [turn('t1', { ordinal: 1, state: 'completed', inputId: 'first-input', startedAt: at(0), completedAt: at(30) })]);
  show(<Moves />);
  await screen.findByText('Done.');
  // Rows: the message, its work, the steered message, the work after it, the answer, the end of the turn.
  const rows = drawn.list;
  if (rows === undefined) throw new Error('The conversation is drawn by the list');
  expect(Array.from({ length: rows.count }, (_, index) => rows.keyOf(index))).toEqual(['first', 'work:c1', 'steer', 'work:c3', 'answer', 'footer:t1']);
  const inView = (first: number, last: number) => act(() => drawn.list?.onRange?.(first, last));
  inView(1, 1);
  expect(document.querySelector('[data-sticky-prompt]')).toHaveTextContent('Fix the redirect');
  inView(3, 3);
  const prompt = document.querySelector('[data-sticky-prompt]');
  expect(prompt).toHaveTextContent('Use the other file');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('thread.messages.previous') }));
  await waitFor(() => expect(drawn.scrolled).toContainEqual({ key: 'steer', align: 'start' }));
  // Going to a message opens no group of work.
  expect(document.querySelector('[data-item="c3"]')).toBeNull();
});

it('counts the rows as they are drawn for the round button: a group of work is one row however many steps it gains', async () => {
  hold([userMessage('first', 'Hello', { turn: 't1', author: ADA }), agentMessage('answer-1', 'Hi.', { turn: 't1' })], [turn('t1', { ordinal: 1, state: 'completed', inputId: 'first-input', startedAt: at(0), completedAt: at(2) })]);
  const follows: FollowState[] = [];
  show(<Timeline threadId={THREAD} item={null} empty={null} onFollow={(state) => follows.push(state)} />);
  await screen.findByText('Hi.');
  act(() => drawn.list?.onAtEnd?.(false));
  expect(follows.at(-1)).toMatchObject({ atEnd: false, away: 0 });
  const later = (items: Item[], seq: number) => items.map((item, index) => ({ ...item, thread_id: THREAD, seq_created: seq + index, seq_updated: seq + index }));
  const second = turn('t2', { ordinal: 2, state: 'running', inputId: 'second-input', startedAt: at(10) });
  const steps = Array.from({ length: 30 }, (_, index) => command(`s${index}`, { turn: 't2', command: `step ${index}`, exit: 0 }));
  act(() => threadWriter.apply(THREAD, { thread: { ...source.thread, seq: 100, state: 'running', current_turn_id: 't2' }, turns: [second],
    items: later([userMessage('next', 'Again', { turn: 't2', author: ADA }), ...steps], 50) }));
  // The message and the one group of its thirty-one steps, not thirty-two.
  expect(follows.at(-1)).toMatchObject({ atEnd: false, away: 2 });
  act(() => threadWriter.apply(THREAD, { thread: { ...source.thread, seq: 101, state: 'running', current_turn_id: 't2' }, items: later([command('more', { turn: 't2', command: 'one more', exit: 0 })], 90) }));
  expect(follows.at(-1)).toMatchObject({ atEnd: false, away: 2 });
});

it('draws a message on its way to the agent in the block of a person’s message, with the gap a row of the conversation has', async () => {
  hold([userMessage('first', 'Hello', { turn: 't1', author: ADA }), agentMessage('answer-1', 'Hi.', { turn: 't1' })], [turn('t1', { ordinal: 1, state: 'completed', inputId: 'first-input', startedAt: at(0), completedAt: at(2) })]);
  const view = show(<><Timeline threadId={THREAD} item={null} empty={null} /><SentMessageBlock threadId={THREAD} message={{ key: 'sent', content: [{ type: 'text', text: 'And now the tests' }], at: at(5), input: null }} /></>);
  await screen.findByText('Hi.');
  const taken = view.container.querySelector('[data-user-message] > div');
  const waitingBlock = view.container.querySelector('[data-sent-message] .rounded-xl');
  if (taken === null || waitingBlock === null) throw new Error('Both stand as blocks');
  const sent = new Set(waitingBlock.className.split(/\s+/));
  for (const name of taken.className.split(/\s+/)) expect(sent.has(name)).toBe(true);
  expect(waitingBlock).toHaveClass('opacity-60');
  // The gap before it is the one a row has: the message's own margin, over the padding round the row.
  expect(view.container.querySelector('[data-sent-message]')?.firstElementChild).toHaveClass('mt-(--message-gap)');
  expect(view.container.querySelector('[data-user-message]')).toHaveClass('mt-(--message-gap)');
});
