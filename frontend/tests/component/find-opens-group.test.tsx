/** A match inside the folded work of a turn opens that group, and the conversation goes to it, through the one call that brings an item into view. */
import { useImperativeHandle, useRef } from 'react';
import { cleanup, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Personal from '@/api/personal';
import type * as Threads from '@/api/threads';
import type * as Live from '@/realtime/live';
import type * as ViewFind from '@/features/search/ViewFind';
import { client } from '@/api/client';
import { captureThreadRead } from '@/api/threadRead';
import { FindInConversation } from '@/features/timeline/FindInConversation';
import { timelineUi, useTimelineUi } from '@/features/timeline/state';
import { Timeline, type TimelineHandle } from '@/features/timeline/Timeline';
import { SubagentConversation } from '@/features/timeline/SubagentLane';
import type { SearchHit } from '@/generated/api-types';
import { threadViewOf, threadWriter } from '@/realtime/stores/thread';
import type { VirtualListProps } from '@/ui/VirtualList';
import { agentMessage, command, reasoning, toolCall, turn, userMessage } from '../doubles/thread-rows';
import { me, userRef } from '../doubles/fixtures';
import { snapshot } from '../unit/continuation-fixtures';
import { show, skipOverflowFallback } from './support';

skipOverflowFallback();

const found = vi.hoisted(() => ({ around: vi.fn<typeof Threads.fetchItems>(), child: vi.fn<typeof Threads.fetchSubagentItems>(), filters: undefined as ViewFind.ViewFindOptions['filters'] | undefined, hit: undefined as ((hit: SearchHit) => void) | undefined, scrolled: [] as { key: string; align: string | undefined }[], targetScrolls: [] as number[] }));
vi.mock('@/features/search/ViewFind', async (original) => ({ ...await original<typeof ViewFind>(),
  useViewFind: (options: ViewFind.ViewFindOptions) => { found.filters = options.filters; found.hit = options.onHit; return { open: false, show: () => undefined, bar: null }; } }));
vi.mock('@/api/personal', async (original) => ({ ...await original<typeof Personal>(),
  meStateQuery: { queryKey: ['me-state'], queryFn: () => Promise.resolve({}), staleTime: Infinity },
  setMeState: () => Promise.resolve(),
}));
vi.mock('@/api/threads', async (original) => ({ ...await original<typeof Threads>(), fetchItems: found.around, fetchSubagentItems: found.child }));
vi.mock('@/realtime/live', async (original) => ({ ...await original<typeof Live>(), useSocketStatus: () => ({ phase: 'ready' }) }));
// The fixture draws every row; explicit rectangles below exercise scrolling to an actual child inside a tall group.
vi.mock('@/ui/VirtualList', () => ({ VirtualList: (props: VirtualListProps) => {
  const viewport = useRef<HTMLDivElement>(null);
  const keys = Array.from({ length: props.count }, (_, index) => props.keyOf(index));
  useImperativeHandle(props.ref, () => ({ scrollToEnd: () => undefined, isAtEnd: () => true, element: () => viewport.current,
    scrollToKey: (key: string, align?: string) => { found.scrolled.push({ key, align }); return keys.includes(key); } }));
  return <div data-find-viewport ref={(element) => {
    viewport.current = element;
    if (element !== null) element.scrollTo = (options?: ScrollToOptions | number, top?: number) => {
      const value = typeof options === 'number' ? top ?? 0 : options?.top ?? 0;
      element.scrollTop = value; found.targetScrolls.push(value);
    };
  }}><ol aria-label={props.label}>{keys.map((key, index) => <li key={key}>{props.render(index)}</li>)}</ol></div>;
} }));

const source = snapshot(null);
const id = source.thread.id;
const session = client.session.getState();
const hitOn = (item: string, stream = 'main'): SearchHit => ({ kind: 'item', thread: { id }, item: { id: item, stream } } as unknown as SearchHit);
beforeEach(() => {
  found.around.mockReset();
  found.child.mockReset().mockResolvedValue({ items: [], turns: [], segments: [], has_older: false, has_newer: false });
  client.session.setState({ status: 'signed_in', userId: 'find-reader', readerVersion: 1 });
  const done = turn('t1', { ordinal: 1, state: 'completed', inputId: 'one-input' });
  const at = (item: ReturnType<typeof command>, seq: number) => ({ ...item, thread_id: id, seq_created: seq, seq_updated: seq });
  threadWriter.reset(id, { ...source, turns: [done], items: [
    at(userMessage('ask', 'Fix the redirect', { turn: 't1', author: userRef(me(1, 'ada', 'Ada')) }), 2),
    at(command('step', { turn: 't1', command: 'npm test', output: 'The redirect check passes', exit: 0 }), 3),
    at(reasoning('thought', 'Looking at the redirect', { turn: 't1' }), 4),
    at(agentMessage('says', 'Trying a fix', { turn: 't1', phase: 'commentary' }), 5),
    at(agentMessage('answer', 'Fixed', { turn: 't1' }), 6),
  ] }, false, captureThreadRead(id));
});
afterEach(() => { cleanup(); threadWriter.evict(id); timelineUi.forget(id); found.hit = undefined; found.scrolled = []; found.targetScrolls = []; vi.restoreAllMocks(); client.session.setState(session); });

function Page() {
  const timeline = useRef<TimelineHandle>(null);
  return <>
    <Timeline ref={timeline} threadId={id} item={null} empty={null} />
    <FindInConversation threadId={id} timeline={timeline} root={{ current: document.body as HTMLDivElement }} />
  </>;
}

it.each(['step', 'thought', 'says'] as const)('opens the folded work of the turn, draws the step and goes to it for a match in its %s', async (item) => {
  const view = show(<Page />);
  await screen.findByText('Fixed');
  expect(view.container.querySelector(`[data-item="${item}"]`)).toBeNull();
  found.hit?.(hitOn(item));
  await waitFor(() => expect(view.container.querySelector(`[data-item="${item}"]`)).not.toBeNull());
  const expected = { step: 'The redirect check passes', thought: 'Looking at the redirect', says: 'Trying a fix' }[item];
  expect(await screen.findByText(expected)).toBeVisible();
  expect(useTimelineUi.getState().byThread.get(id)?.steps.get('work:step')).toBe(true);
  // The step is inside the group's row, so the list goes to that row.
  await waitFor(() => expect(found.scrolled).toContainEqual({ key: 'work:step', align: 'center' }));
});

it('fetches an off-window tool match and opens its actual output after the group mounts', async () => {
  const items = [userMessage('older-ask', 'Earlier work', { turn: 't1', author: userRef(me(1, 'ada', 'Ada')) }),
    command('older-step', { turn: 't1', command: 'check earlier work', output: 'The earlier redirect is safe', exit: 0 }),
    agentMessage('older-answer', 'Earlier answer', { turn: 't1' })].map((item, index) => ({ ...item, thread_id: id, seq_created: index + 20, seq_updated: index + 20 }));
  found.around.mockResolvedValue({ items, turns: [], segments: [], has_older: true, has_newer: true });
  show(<Page />);
  await screen.findByText('Fixed');
  found.hit?.(hitOn('older-step'));
  expect(await screen.findByText('The earlier redirect is safe')).toBeVisible();
  expect(found.around).toHaveBeenCalledOnce();
});

it('finds and reveals an older match in the displayed child without replacing the main history window', async () => {
  const stream = 'sub:reviewer';
  const parent = { ...toolCall('review-call', { kind: 'subagent', child_stream: stream, name: 'Reviewer', role: null, prompt: null, state: 'completed' }, { turn: 't1', title: 'Review', status: 'completed' }), thread_id: id, seq_created: 7, seq_updated: 7 };
  threadWriter.apply(id, { items: [parent] });
  const main = [...threadViewOf(id).order];
  const items = [command('child-match', { turn: 't1', stream, command: 'check earlier redirect', output: 'An older child output matches', exit: 0 }),
    agentMessage('child-answer', 'The child answer', { turn: 't1', stream, phase: 'final' })].map((item, index) => ({ ...item, thread_id: id, seq_created: 30 + index, seq_updated: 30 + index }));
  found.around.mockResolvedValue({ items, turns: [], segments: [], has_older: true, has_newer: true });
  function ChildPage() {
    const timeline = useRef<TimelineHandle>(null);
    return <><SubagentConversation threadId={id} itemId="review-call" ref={timeline} />
      <FindInConversation threadId={id} stream={stream} timeline={timeline} root={{ current: document.body as HTMLDivElement }} /></>;
  }
  show(<ChildPage />);
  expect(found.filters).toMatchObject({ scope: id, stream });
  found.hit?.(hitOn('child-match', stream));
  expect(await screen.findByText('An older child output matches')).toBeVisible();
  expect(found.around).toHaveBeenCalledWith(expect.objectContaining({ threadId: id }), { around: 'child-match', stream }, expect.any(Number), expect.any(AbortSignal));
  expect(threadViewOf(id).order).toEqual(main);
  expect(threadViewOf(id).streams.get(stream)).toContain('child-match');
});

it('brings the actual first and last matching children into view at different places in one tall work group', async () => {
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    const viewport = document.querySelector<HTMLElement>('[data-find-viewport]');
    if (this === viewport) return new DOMRect(0, 10, 500, 300);
    const position = this.dataset.item === 'step' ? 300 : this.dataset.item === 'thought' ? 1500 : null;
    return position === null ? Element.prototype.getBoundingClientRect.call(this) : new DOMRect(0, 10 + position - (viewport?.scrollTop ?? 0), 500, 40);
  });
  show(<Page />);
  await screen.findByText('Fixed');
  found.hit?.(hitOn('step'));
  await waitFor(() => expect(found.targetScrolls.at(-1)).toBe(170));
  found.hit?.(hitOn('thought'));
  await waitFor(() => expect(found.targetScrolls.at(-1)).toBe(1370));
  expect(screen.getByText('Looking at the redirect')).toBeVisible();
});

it.each(['ask', 'answer'])('leaves the work folded for a match in the person’s message or the answer: %s', async (item) => {
  const view = show(<Page />);
  await screen.findByText('Fixed');
  found.hit?.(hitOn(item));
  await waitFor(() => expect(found.scrolled).toContainEqual({ key: item, align: 'center' }));
  expect(useTimelineUi.getState().byThread.get(id)?.steps.get('work:step') ?? false).toBe(false);
  expect(view.container.querySelector('[data-item="step"]')).toBeNull();
});
