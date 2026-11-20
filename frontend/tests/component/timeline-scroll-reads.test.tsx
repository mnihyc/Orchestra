/** Quick scrolling never ends in an error page: a read that was cancelled or overtaken is no failure, one that failed leaves a small line, and the conversation stays drawn. */
import { useImperativeHandle } from 'react';
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Live from '@/realtime/live';
import type * as Personal from '@/api/personal';
import type * as Threads from '@/api/threads';
import { client } from '@/api/client';
import { captureThreadRead } from '@/api/threadRead';
import { Timeline } from '@/features/timeline/Timeline';
import { keepTimelinePosition } from '@/features/timeline/position';
import { i18n, setLocale } from '@/i18n';
import { threadViewOf, threadWriter } from '@/realtime/stores/thread';
import { ThreadReadProvider } from '@/realtime/threadReadContext';
import { resultThreadRead, type ThreadReadTarget } from '@/realtime/threadScope';
import type { VirtualListProps } from '@/ui/VirtualList';
import type { Item } from '@/generated/api-types';
import { agentMessage, command, turn, userMessage } from '../doubles/thread-rows';
import { snapshot } from '../unit/continuation-fixtures';
import { show } from './support';

type Reply = Awaited<ReturnType<typeof Threads.fetchItems>>;
const calls = vi.hoisted(() => ({ fetch: vi.fn<typeof Threads.fetchItems>(), nearStart: undefined as (() => void) | undefined, list: undefined as VirtualListProps | undefined }));
vi.mock('@/api/threads', async (original) => ({ ...await original<typeof Threads>(), fetchItems: calls.fetch }));
vi.mock('@/api/personal', async (original) => ({ ...await original<typeof Personal>(),
  meStateQuery: { queryKey: ['me-state'], queryFn: () => Promise.resolve({}), staleTime: Infinity },
  setMeState: () => Promise.resolve(),
}));
vi.mock('@/realtime/live', async (original) => ({ ...await original<typeof Live>(), useSocketStatus: () => ({ phase: 'ready' }) }));
// Geometry belongs to the browser controls; the list only has to draw every row and say when the reader is near the top.
vi.mock('@/ui/VirtualList', () => ({ VirtualList: (props: VirtualListProps) => {
  calls.nearStart = props.onNearStart;
  calls.list = props;
  useImperativeHandle(props.ref, () => ({ scrollToEnd: () => undefined, scrollToKey: () => true, isAtEnd: () => true, element: () => null }));
  return <ol aria-label={props.label}>{Array.from({ length: props.count }, (_, index) => <li key={props.keyOf(index)}>{props.render(index)}</li>)}</ol>;
} }));
vi.mock('@/features/timeline/StatusLine', () => ({ StatusLine: () => null }));

const session = client.session.getState();
const reads: ThreadReadTarget[] = [];
beforeEach(() => {
  calls.fetch.mockReset();
  calls.nearStart = undefined;
  calls.list = undefined;
  vi.spyOn(Math, 'random').mockReturnValue(0);
});
afterEach(async () => {
  cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
  for (const read of reads.splice(0)) threadWriter.evict(read);
  client.session.setState(session);
  await setLocale('en');
});

const stamp = (item: Item, thread: string, seq: number): Item => ({ ...item, thread_id: thread, seq_created: seq, seq_updated: seq });
const reply = (items: Item[], more: Partial<Reply> = {}): Reply => ({ items, has_older: false, has_newer: false, turns: [], segments: [], ...more });

/** A thread held by the store the way the timeline reads it: two turns, newest part only, with older items on the server. */
function held(owner?: string) {
  client.session.setState({ status: 'signed_in', userId: 'scroll-reader', readerVersion: 1 });
  const source = snapshot();
  const id = source.thread.id;
  const read = owner === undefined ? id : resultThreadRead(id, owner);
  reads.push(read);
  const done = turn('t-new', { ordinal: 2, state: 'completed', inputId: 'input-new' });
  const items = [
    stamp(userMessage('new-question', 'The newest question', { turn: done.id, author: source.thread.owner }), id, 20),
    stamp(agentMessage('new-answer', 'The newest answer', { turn: done.id }), id, 21),
  ];
  const opening = { ...source, turns: [done], items };
  threadWriter.reset(read, opening, true, captureThreadRead(read));
  return { id, read, opening };
}

it.each([true, false])('fills an unscrollable opening after paint when the last end report was %s and stops if its height stays the same', async (reportedAtEnd) => {
  const { id, read, opening } = held();
  const viewport = document.createElement('div');
  const draw = (painted: boolean) => <div data-timeline-space>
    <Timeline threadId={id} viewport={viewport} item={null} empty={null} fillAfterPaint={painted} />
  </div>;
  vi.spyOn(Element.prototype, 'clientHeight', 'get').mockImplementation(function (this: Element) {
    return this === viewport || this.hasAttribute('data-timeline-space') ? 500 : 0;
  });
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockImplementation(function (this: HTMLElement) {
    return this.parentElement?.hasAttribute('data-timeline-space') === true ? 140 : 0;
  });
  const folded = stamp(command('prior-step', { turn: 't-new', command: 'inspect', exit: 0 }), id, 15);
  calls.fetch.mockResolvedValue(reply([folded], { has_older: true }));
  const view = show(draw(false));
  act(() => calls.list?.onAtEnd?.(reportedAtEnd));
  await act(() => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())));
  expect(calls.fetch).not.toHaveBeenCalled();
  if (!reportedAtEnd) {
    // While there is room to scroll away, keep the reader where they left the conversation.
    const overflow = vi.spyOn(viewport, 'scrollHeight', 'get').mockReturnValue(750);
    view.rerender(draw(true));
    await act(() => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())));
    expect(calls.fetch).not.toHaveBeenCalled();
    view.rerender(draw(false));
    overflow.mockRestore();
  }
  // The old tail is still drawn while its opening is refreshed. Positioned reads are
  // accepted only after that opening answers, even when it returns the same oldest row.
  act(() => { threadWriter.beginRead(read, client.session.getState().readerVersion); });
  view.rerender(draw(true));
  await act(() => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())));
  expect(calls.fetch).not.toHaveBeenCalled();
  act(() => { threadWriter.reset(read, opening, true, captureThreadRead(read)); });
  await waitFor(() => expect(threadViewOf(id).items.has(folded.id)).toBe(true));
  await act(() => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())));
  expect(calls.fetch).toHaveBeenCalledTimes(1);
  expect(calls.fetch.mock.calls[0]?.[1]).toEqual({ before: 20, stream: 'main' });
});

it('does not cancel the read around the place the reader left each time a row arrives, and draws the conversation when it answers', async () => {
  const { id, read } = held('tab-scroll');
  if (typeof read === 'string') throw new Error('The thread is held for a tab');
  keepTimelinePosition(read, null, { key: 'left-row', item: 'left-item', offset: 0, atEnd: false });
  const signals: (AbortSignal | undefined)[] = [];
  let answer: (value: Reply) => void = () => undefined;
  calls.fetch.mockImplementation((_target, _window, _limit, signal) => {
    signals.push(signal);
    return new Promise<Reply>((resolve) => { answer = resolve; });
  });
  show(<ThreadReadProvider read={read}><Timeline threadId={id} item={null} empty={null} /></ThreadReadProvider>);
  await waitFor(() => expect(calls.fetch).toHaveBeenCalledTimes(1));
  // The thread goes on while the read is under way: a live row must not be what cancels it.
  act(() => threadWriter.apply(read, { items: [stamp(agentMessage('live-row', 'A live row', { turn: 't-new' }), id, 22)] }, captureThreadRead(read)));
  act(() => threadWriter.apply(read, { items: [stamp(agentMessage('live-row-two', 'Another live row', { turn: 't-new' }), id, 23)] }, captureThreadRead(read)));
  expect(calls.fetch).toHaveBeenCalledTimes(1);
  expect(signals[0]?.aborted).toBe(false);
  await act(async () => { answer(reply([stamp(agentMessage('left-item', 'Where the reader left', { turn: 't-new' }), id, 10)])); await Promise.resolve(); });
  await waitFor(() => expect(screen.getByText('Where the reader left')).toBeVisible());
});

it.each(['en', 'zh-CN'] as const)('draws a small line with Try again when older items could not be read, keeps the conversation, and reads again when the reader returns to the top in %s', async (locale) => {
  await setLocale(locale);
  vi.useFakeTimers({ shouldAdvanceTime: true });
  const { id } = held();
  calls.fetch.mockRejectedValueOnce(new Error('The older items could not be read'));
  show(<Timeline threadId={id} item={null} empty={null} />);
  expect(screen.getByText('The newest answer')).toBeVisible();
  act(() => calls.nearStart?.());
  const failed = await screen.findByRole('alert');
  expect(failed).toHaveTextContent(i18n.t('timeline.earlierFailed'));
  expect(screen.getByRole('button', { name: i18n.t('common.tryAgain') })).toBeVisible();
  // The rest of the conversation stays on the screen.
  expect(screen.getByText('The newest answer')).toBeVisible();
  expect(screen.getByText('The newest question')).toBeVisible();
  // A scroll that comes straight after does not ask again at once.
  act(() => calls.nearStart?.());
  expect(calls.fetch).toHaveBeenCalledTimes(1);
  // Once the pause is over, the reader coming to the top reads again; the line goes and the older items are there.
  calls.fetch.mockResolvedValueOnce(reply([stamp(agentMessage('old-answer', 'An older answer', { turn: 't-old' }), id, 5)]));
  await act(async () => { await vi.advanceTimersByTimeAsync(2_000); });
  act(() => calls.nearStart?.());
  await waitFor(() => expect(screen.getByText('An older answer')).toBeVisible());
  expect(calls.fetch).toHaveBeenCalledTimes(2);
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(screen.getByText('The newest answer')).toBeVisible();
});

it('reads again at once with Try again', async () => {
  const { id } = held();
  calls.fetch.mockRejectedValueOnce(new Error('The older items could not be read'));
  show(<Timeline threadId={id} item={null} empty={null} />);
  act(() => calls.nearStart?.());
  await screen.findByRole('alert');
  calls.fetch.mockResolvedValueOnce(reply([stamp(agentMessage('old-answer', 'An older answer', { turn: 't-old' }), id, 5)]));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
  await waitFor(() => expect(screen.getByText('An older answer')).toBeVisible());
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
});

it('leaves no line for a read that was cancelled or overtaken', async () => {
  const { id } = held();
  calls.fetch.mockRejectedValueOnce(new DOMException('The thread read selection changed', 'AbortError'));
  show(<Timeline threadId={id} item={null} empty={null} />);
  act(() => calls.nearStart?.());
  await waitFor(() => expect(calls.fetch).toHaveBeenCalledTimes(1));
  await act(async () => { await Promise.resolve(); });
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(screen.getByText('The newest answer')).toBeVisible();
  // The next scroll to the top asks at once: nothing was waiting on a failure.
  calls.fetch.mockResolvedValueOnce(reply([stamp(agentMessage('old-answer', 'An older answer', { turn: 't-old' }), id, 5)]));
  act(() => calls.nearStart?.());
  await waitFor(() => expect(screen.getByText('An older answer')).toBeVisible());
});

it('puts the reader back where they were when the main conversation returns from a sub-agent’s, with the position it was left at', () => {
  const { id, read } = held('tab-switch');
  if (typeof read === 'string') throw new Error('The thread is held for a tab');
  const main = () => <ThreadReadProvider read={read}><Timeline threadId={id} item={null} empty={null} /></ThreadReadProvider>;
  const first = show(main());
  expect(calls.list?.initialPosition).toBeNull();
  // The reader scrolled up into the conversation, and then went into a sub-agent: the screen takes the list away.
  act(() => calls.list?.onPosition?.({ key: 'row-of-the-reader', offset: 12, atEnd: false }));
  first.unmount();
  show(main());
  expect(calls.list?.initialPosition).toMatchObject({ key: 'row-of-the-reader', offset: 12, atEnd: false });
});

it('pages a historical window through 400 items without revealing the live tail', async () => {
  const { id, read } = held('older-pages');
  if (typeof read === 'string') throw new Error('The test holds a historical tab window');
  const author = snapshot().thread.owner;
  const all = Array.from({ length: 400 }, (_, index) => stamp(index % 25 === 0
    ? userMessage(`msg-${index}`, `Turn ${index / 25 + 1}`, { turn: 't-new', author })
    : command(`work-${index}`, { turn: 't-new', command: 'inspect', exit: 0 }), id, index + 1));
  threadWriter.window(read, all.slice(299,399), true, true);
  threadWriter.apply(read, { items: [all[0]!, all[399]!] });
  calls.fetch.mockImplementation((_target, cut) => {
    if (!('before' in cut)) throw new Error('Expected an older page');
    const cursor = cut.before;
    return Promise.resolve(reply(all.filter((row) => row.seq_created < cursor).slice(-100), { has_older: cursor > 101, has_newer: true }));
  });
  show(<ThreadReadProvider read={read}><Timeline threadId={id} item={null} empty={null} /></ThreadReadProvider>);
  for (const cursor of [300, 200, 100]) {
    act(() => { calls.nearStart?.(); calls.nearStart?.(); });
    await waitFor(() => expect(threadViewOf(read).order[0]).toBe(all[Math.max(0, cursor - 101)]!.id));
  }
  expect(calls.fetch.mock.calls.map(([, cut]) => 'before' in cut ? cut.before : null)).toEqual([300, 200, 100]);
  expect(threadViewOf(read).order).toEqual(all.slice(0,399).map((row) => row.id));
  expect(screen.getByText('Turn 1')).toBeVisible();
});
