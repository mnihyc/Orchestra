/** The light on an item that a link named goes out after its moment, and is not asked for again by a timeline that has gone. */
import { useImperativeHandle } from 'react';
import { act, cleanup, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Personal from '@/api/personal';
import type * as Live from '@/realtime/live';
import { Timeline } from '@/features/timeline/Timeline';
import { client } from '@/api/client';
import { captureThreadRead } from '@/api/threadRead';
import { threadWriter } from '@/realtime/stores/thread';
import type { VirtualListProps } from '@/ui/VirtualList';
import { agentMessage, turn, userMessage } from '../doubles/thread-rows';
import { A, snapshot } from '../unit/continuation-fixtures';
import { show } from './support';

vi.mock('@/api/personal', async (original) => ({ ...await original<typeof Personal>(),
  meStateQuery: { queryKey: ['me-state'], queryFn: () => Promise.resolve({}), staleTime: Infinity },
  setMeState: () => Promise.resolve(),
}));
vi.mock('@/realtime/live', async (original) => ({ ...await original<typeof Live>(), useSocketStatus: () => ({ phase: 'ready' }) }));
// Geometry belongs to the browser controls; the list only has to find the row of the item that was named.
vi.mock('@/ui/VirtualList', () => ({ VirtualList: (props: VirtualListProps) => {
  useImperativeHandle(props.ref, () => ({ scrollToEnd: () => undefined, scrollToKey: () => true, isAtEnd: () => true, element: () => null }));
  return <ol aria-label={props.label}>{Array.from({ length: props.count }, (_, index) => <li key={props.keyOf(index)}>{props.render(index)}</li>)}</ol>;
} }));
vi.mock('@/features/timeline/StatusLine', () => ({ StatusLine: () => null }));

const session = client.session.getState();
const ids: string[] = [];
afterEach(() => {
  cleanup(); vi.useRealTimers(); vi.restoreAllMocks();
  for (const id of ids.splice(0)) threadWriter.evict(id);
  client.session.setState(session);
});

/** A thread of one finished turn, a question and its answer, held by the store as the timeline reads it. */
function held() {
  client.session.setState({ status: 'signed_in', userId: 'light-reader', readerVersion: 1 });
  const source = snapshot(), id = source.thread.id;
  ids.push(id);
  const done = turn('light-turn', { ordinal: 1, state: 'completed', inputId: 'light-question' });
  threadWriter.reset(id, { ...source, turns: [done], items: [
    userMessage('light-question', 'Where is the redirect?', { turn: done.id, author: source.thread.owner }),
    agentMessage('light-answer', 'In the login handler.', { turn: done.id }),
  ].map((item) => ({ ...item, thread_id: id, continuation_id: A })) }, false, captureThreadRead(id));
  return id;
}
const LIT = /ring-accent/;

it('lights the item a link named and puts the light out after its moment while the timeline is there', async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  const id = held();
  const view = show(<Timeline threadId={id} item="light-answer" empty={null} />);
  await waitFor(() => expect(view.container.querySelector('[data-item="light-answer"]')?.className).toMatch(LIT));
  expect(view.container.querySelector('[data-item="light-question"]')?.className).not.toMatch(LIT);
  await act(async () => { await vi.advanceTimersByTimeAsync(2_100); });
  expect(view.container.querySelector('[data-item="light-answer"]')?.className).not.toMatch(LIT);
});

it('leaves no timer to put the light out when its timeline goes before the moment is over', async () => {
  const set = vi.spyOn(globalThis, 'setTimeout'), cleared = vi.spyOn(globalThis, 'clearTimeout');
  const id = held();
  const view = show(<Timeline threadId={id} item="light-answer" empty={null} />);
  await waitFor(() => expect(view.container.querySelector('[data-item="light-answer"]')?.className).toMatch(LIT));
  const waits = set.mock.calls.flatMap((call, index) => call[1] === 2_000 ? [set.mock.results[index]?.value as unknown] : []);
  expect(waits.length).toBeGreaterThan(0);
  view.unmount();
  // Every wait of two seconds that the timeline asked for is let go with it, the light's among them.
  for (const wait of waits) expect(cleared).toHaveBeenCalledWith(wait);
});
