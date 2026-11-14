/** A match inside the folded work of a turn opens that group, and the conversation goes to it, through the one call that brings an item into view. */
import { useImperativeHandle, useRef } from 'react';
import { cleanup, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Personal from '@/api/personal';
import type * as Live from '@/realtime/live';
import type * as ViewFind from '@/features/search/ViewFind';
import { client } from '@/api/client';
import { captureThreadRead } from '@/api/threadRead';
import { FindInConversation } from '@/features/timeline/FindInConversation';
import { timelineUi, useTimelineUi } from '@/features/timeline/state';
import { Timeline, type TimelineHandle } from '@/features/timeline/Timeline';
import type { SearchHit } from '@/generated/api-types';
import { threadWriter } from '@/realtime/stores/thread';
import type { VirtualListProps } from '@/ui/VirtualList';
import { agentMessage, command, reasoning, turn, userMessage } from '../doubles/thread-rows';
import { me, userRef } from '../doubles/fixtures';
import { snapshot } from '../unit/continuation-fixtures';
import { show, skipOverflowFallback } from './support';

skipOverflowFallback();

const found = vi.hoisted(() => ({ hit: undefined as ((hit: SearchHit) => void) | undefined, scrolled: [] as { key: string; align: string | undefined }[] }));
vi.mock('@/features/search/ViewFind', async (original) => ({ ...await original<typeof ViewFind>(),
  useViewFind: (options: { onHit: (hit: SearchHit) => void }) => { found.hit = options.onHit; return { open: false, show: () => undefined, bar: null }; } }));
vi.mock('@/api/personal', async (original) => ({ ...await original<typeof Personal>(),
  meStateQuery: { queryKey: ['me-state'], queryFn: () => Promise.resolve({}), staleTime: Infinity },
  setMeState: () => Promise.resolve(),
}));
vi.mock('@/realtime/live', async (original) => ({ ...await original<typeof Live>(), useSocketStatus: () => ({ phase: 'ready' }) }));
// Geometry belongs to the browser controls: the list draws every row and says which rows it was asked to go to.
vi.mock('@/ui/VirtualList', () => ({ VirtualList: (props: VirtualListProps) => {
  const keys = Array.from({ length: props.count }, (_, index) => props.keyOf(index));
  useImperativeHandle(props.ref, () => ({ scrollToEnd: () => undefined, isAtEnd: () => true, element: () => null,
    scrollToKey: (key: string, align?: string) => { found.scrolled.push({ key, align }); return keys.includes(key); } }));
  return <ol aria-label={props.label}>{keys.map((key, index) => <li key={key}>{props.render(index)}</li>)}</ol>;
} }));

const source = snapshot(null);
const id = source.thread.id;
const session = client.session.getState();
const hitOn = (item: string): SearchHit => ({ kind: 'item', thread: { id }, item: { id: item } } as unknown as SearchHit);
beforeEach(() => {
  client.session.setState({ status: 'signed_in', userId: 'find-reader', readerVersion: 1 });
  const done = turn('t1', { ordinal: 1, state: 'completed', inputId: 'one-input' });
  const at = (item: ReturnType<typeof command>, seq: number) => ({ ...item, thread_id: id, seq_created: seq, seq_updated: seq });
  threadWriter.reset(id, { ...source, turns: [done], items: [
    at(userMessage('ask', 'Fix the redirect', { turn: 't1', author: userRef(me(1, 'ada', 'Ada')) }), 2),
    at(command('step', { turn: 't1', command: 'npm test', exit: 0 }), 3),
    at(reasoning('thought', 'Looking at the redirect', { turn: 't1' }), 4),
    at(agentMessage('says', 'Trying a fix', { turn: 't1', phase: 'commentary' }), 5),
    at(agentMessage('answer', 'Fixed', { turn: 't1' }), 6),
  ] }, false, captureThreadRead(id));
});
afterEach(() => { cleanup(); threadWriter.evict(id); timelineUi.forget(id); found.hit = undefined; found.scrolled = []; client.session.setState(session); });

function Page() {
  const timeline = useRef<TimelineHandle>(null);
  return <>
    <Timeline ref={timeline} threadId={id} item={null} empty={null} />
    <FindInConversation threadId={id} timeline={timeline} root={{ current: document.body as HTMLDivElement }} />
  </>;
}

it.each(['step', 'thought', 'says'])('opens the folded work of the turn, draws the step and goes to it for a match in its %s', async (item) => {
  const view = show(<Page />);
  await screen.findByText('Fixed');
  expect(view.container.querySelector(`[data-item="${item}"]`)).toBeNull();
  found.hit?.(hitOn(item));
  await waitFor(() => expect(view.container.querySelector(`[data-item="${item}"]`)).not.toBeNull());
  expect(useTimelineUi.getState().byThread.get(id)?.steps.get('work:step')).toBe(true);
  // The step is inside the group's row, so the list goes to that row.
  await waitFor(() => expect(found.scrolled).toContainEqual({ key: 'work:step', align: 'center' }));
});

it.each(['ask', 'answer'])('leaves the work folded for a match in the person’s message or the answer: %s', async (item) => {
  const view = show(<Page />);
  await screen.findByText('Fixed');
  found.hit?.(hitOn(item));
  await waitFor(() => expect(found.scrolled).toContainEqual({ key: item, align: 'center' }));
  expect(useTimelineUi.getState().byThread.get(id)?.steps.get('work:step') ?? false).toBe(false);
  expect(view.container.querySelector('[data-item="step"]')).toBeNull();
});
