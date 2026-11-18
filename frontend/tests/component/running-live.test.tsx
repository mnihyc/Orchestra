/** Header observations stay current without creating a second HTTP inventory or replaying an old opening. */
import { act, cleanup, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { fetchBackground } from '@/api/background';
import type { ThreadBackground } from '@/generated/api-types';
import { useRunning } from '@/features/running/live';
import { ViewOpeningContext } from '@/features/workbench/viewOpening';
import type { FeedFrame, SocketPhase } from '@/realtime/socket';
import { show } from './support';

interface Sink { feed: string; onFrame: (frame: FeedFrame) => void; unsubscribe: () => void }
interface Phase { phase: SocketPhase }
const source = vi.hoisted(() => ({ sinks: [] as Sink[], phase: 'ready',
  statuses: new Set<(next: Phase, previous: Phase) => void>() }));
vi.mock('@/api/background', () => ({ fetchBackground: vi.fn() }));
vi.mock('@/realtime/live', async (loadOriginal) => ({ ...await loadOriginal<object>(), socket: {
  subscribe: (feed: string, options: { onFrame: (frame: FeedFrame) => void }) => {
    const sink = { feed, onFrame: options.onFrame, unsubscribe: vi.fn() };
    source.sinks.push(sink); return { unsubscribe: sink.unsubscribe };
  },
  status: { getState: () => ({ phase: source.phase }), subscribe: (listener: (next: Phase, previous: Phase) => void) => {
    source.statuses.add(listener); return () => source.statuses.delete(listener);
  } },
} }));

const before = client.session.getState();
const roster = (total = 2): ThreadBackground => ({ supported: true, state: 'live', total, items: [], next_cursor: null,
  list_ref: null, expires_at: null, as_of: null, can_end: true, reason: null });
function Facts({ thread, enabled = true }: { thread: string; enabled?: boolean }) {
  const value = useRunning(thread, enabled);
  return <output aria-label="Native count">{value.closed ? 'closed' : value.roster?.total ?? 'unknown'}</output>;
}
const content = (opening: string | null, thread = 'one') => <ViewOpeningContext value={opening}><Facts thread={thread} /></ViewOpeningContext>;
const count = () => screen.getByLabelText('Native count');
function emit(sink: Sink, total = 2) { act(() => sink.onFrame({ op: 'event', id: 1, feed: sink.feed, data: { ...roster(total) } })); }
beforeEach(() => {
  source.sinks = []; source.phase = 'ready'; source.statuses.clear(); vi.clearAllMocks();
  vi.mocked(fetchBackground).mockImplementation(() => new Promise<ThreadBackground>(() => undefined));
});
afterEach(() => { cleanup(); client.session.setState(before); });

it('takes the count from the native feed without a duplicate startup HTTP request', () => {
  show(content('first'));
  expect(fetchBackground).not.toHaveBeenCalled();
  expect(count()).toHaveTextContent('unknown');
  emit(source.sinks[0]!);
  expect(count()).toHaveTextContent('2');
});

for (const change of ['view opening', 'thread target', 'reader authority'] as const) it(`rejects a late previous frame after ${change}`, () => {
  const view = show(content('first')), old = source.sinks[0]!;
  emit(old); expect(count()).toHaveTextContent('2');
  if (change === 'reader authority') act(() => client.session.setState({ readerVersion: (before.readerVersion ?? 0) + 1 }));
  view.rerender(content(change === 'view opening' ? 'second' : 'first', change === 'thread target' ? 'two' : 'one'));
  expect(count()).toHaveTextContent('unknown');
  emit(old, 9); expect(count()).toHaveTextContent('unknown');
  emit(source.sinks.at(-1)!, 3); expect(count()).toHaveTextContent('3');
  expect(old.unsubscribe).toHaveBeenCalledOnce();
});

it('drops facts on reconnect and retires a forbidden source instead of showing zero', () => {
  show(content('first')); const sink = source.sinks[0]!;
  emit(sink); expect(count()).toHaveTextContent('2');
  act(() => { source.phase = 'waiting'; for (const listener of source.statuses) listener({ phase: 'waiting' }, { phase: 'ready' }); });
  expect(count()).toHaveTextContent('unknown');
  act(() => { source.phase = 'ready'; for (const listener of source.statuses) listener({ phase: 'ready' }, { phase: 'waiting' }); });
  expect(fetchBackground).not.toHaveBeenCalled();
  expect(count()).toHaveTextContent('unknown');
  emit(sink, 0); expect(count()).toHaveTextContent('0');
  act(() => sink.onFrame({ op: 'error', id: 1, code: 'forbidden', detail: 'Current View is required' }));
  expect(count()).toHaveTextContent('closed');
});

it('starts no native observation for a hidden view', () => {
  show(content(null));
  expect(source.sinks).toHaveLength(0);
  expect(fetchBackground).not.toHaveBeenCalled();
});

it('observes no live work for a historical view and requires fresh facts when it becomes live again', () => {
  const view = show(<ViewOpeningContext value="history"><Facts thread="one" enabled={false} /></ViewOpeningContext>);
  expect(source.sinks).toHaveLength(0);
  expect(count()).toHaveTextContent('unknown');
  view.rerender(<ViewOpeningContext value="history"><Facts thread="one" /></ViewOpeningContext>);
  const old = source.sinks.at(-1);
  if (old === undefined) throw new Error('The live view must subscribe once');
  emit(old, 2);
  expect(count()).toHaveTextContent('2');
  view.rerender(<ViewOpeningContext value="history"><Facts thread="one" enabled={false} /></ViewOpeningContext>);
  expect(count()).toHaveTextContent('unknown');
  emit(old, 9);
  expect(count()).toHaveTextContent('unknown');
  view.rerender(<ViewOpeningContext value="history"><Facts thread="one" /></ViewOpeningContext>);
  expect(count()).toHaveTextContent('unknown');
  expect(fetchBackground).not.toHaveBeenCalled();
});
