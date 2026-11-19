/** Accepted native asides reveal only their actual read owner; controlled replies are not native execution proof. */
import { QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, fireEvent, render, renderHook, screen, waitFor, within } from '@testing-library/react';
import { useEffect, type ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Forks from '@/api/forks';
import type * as Nodes from '@/api/nodes';
import type * as Live from '@/realtime/live';
import { client } from '@/api/client';
import { queryClient } from '@/api/query';
import { forkKeys } from '@/api/forks';
import { captureThreadRead, selectThreadContinuation } from '@/api/threadRead';
import { restorePresentations } from '@/features/workbench/presentation';
import { composerWriter, useComposerStore } from '@/features/composer/Draft';
import { openSideAsk, useAskOnSide, useSideAsk } from '@/features/side-threads/ask';
import { SideAskHost } from '@/features/side-threads/AskOnSide';
import { SideLaneSlot } from '@/features/timeline/SideThreadLane';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import { sideLaneKey, timelineUi, useThreadUi } from '@/features/timeline/state';
import { ThreadReadProvider, useThreadReadRef } from '@/realtime/threadReadContext';
import { contentThreadRead, defaultThreadRead, resultThreadRead, type ThreadReadRef, type ThreadReadTarget } from '@/realtime/threadScope';
import { threadViewOf, threadWriter, useThreadView } from '@/realtime/stores/thread';
import { createRealtimeSocket } from '@/realtime/socket';
import { applyThreadFeedRows, createThreadReadFeed } from '@/realtime/threadFeed';
import { followFeed } from '@/realtime/sync';
import { TooltipProvider } from '@/ui/Tooltip';
import type { Thread } from '@/generated/api-types';
import { ACME, userRef } from '../doubles/fixtures';
import { World, type Subscriber } from '../doubles/world';
import { threadOpening } from '../doubles/thread-opening';
import { marker, threadInput } from '../doubles/thread-rows';
import { A, B, Wire, deferred, projection, snapshot } from '../unit/continuation-fixtures';

const calls = vi.hoisted(() => ({ start: vi.fn<typeof Forks.startSide>(), node: vi.fn<() => Promise<Thread>>(),
  feed: vi.fn<(read: ThreadReadTarget) => void>() }));
vi.mock('@/api/forks', async (load) => ({ ...await load<typeof Forks>(), startSide: calls.start }));
vi.mock('@/api/nodes', async (load) => ({ ...await load<typeof Nodes>(), nodeQuery: (id: string) => ({ queryKey: ['origin-node', id], queryFn: calls.node }) }));
vi.mock('@/realtime/live', async (load) => ({ ...await load<typeof Live>(), useSocketStatus: () => ({ phase: 'ready' }),
  useThreadFeed: function useDemand(target: ThreadReadTarget | null) {
    const read = useThreadReadRef(target ?? '');
    useEffect(() => { if (target !== null) calls.feed(read); }, [target, read]);
  },
}));
const PARENT = ACME.mainThread, SIDE = '01950000-0000-7000-8000-000000000090';
const one = contentThreadRead(PARENT, 'opener-one'), other = contentThreadRead(PARENT, 'opener-two');
const parent = snapshot().thread;
type Reply = Awaited<ReturnType<typeof Forks.startSide>>;
const savedSession = client.session.getState(), savedComposer = useComposerStore.getState();
const stopped: (() => void)[] = [];
function reply(id = SIDE): Reply {
  return { thread: { ...parent, id, seq: 20, current_turn_id: null, pending_inputs: 1, name: 'Accepted aside', side_of: { id: PARENT, kind: 'thread', name: parent.name, redacted: false }, sides: null, side_reading: 'read_only' },
    input: { ...threadInput(`input-${id}`, 'An exact side question', { author: parent.owner, position: '1' }), thread_id: id, seq_updated: 20 }, seq: 20 };
}
function seed(ref: ThreadReadRef, selected: string | null = null) {
  selectThreadContinuation(ref, selected);
  const scope = selected === null ? projection(A) : projection(selected, A, 1, false);
  threadWriter.reset(ref, snapshot(scope), false, captureThreadRead(ref));
}
function Ui({ read: target, name, lane = false }: { read: ThreadReadRef; name: string; lane?: boolean }) {
  return <ThreadReadProvider read={target}><OwnedUi name={name} lane={lane} /></ThreadReadProvider>;
}
function OwnedUi({ name, lane }: { name: string; lane: boolean }) {
  const ui = useThreadUi(PARENT), view = useThreadView(PARENT);
  const started = [...view.items.values()].find((item) => item.kind === 'marker' && item.payload.marker === 'side_started');
  const context: TimelineContext = { threadId: PARENT, threadName: parent.name, detail: 'steps', recent: new Set(), layout: 'one',
    setLayout: () => undefined, meId: parent.owner.id, canInteract: true, canManage: false, live: true, lit: null, selection: NOT_PICKING };
  return <section data-testid={name} data-open={String(ui.steps.get(sideLaneKey(SIDE)) === true)} data-focus={ui.sideFocus?.id ?? ''}>
    {lane && started?.kind === 'marker' && started.payload.marker === 'side_started'
      && <TimelineContextValue.Provider value={context}><SideLaneSlot side={started.payload.data.side} /></TimelineContextValue.Provider>}
  </section>;
}
function Shell({ children }: { children: ReactNode }) {
  return <QueryClientProvider client={queryClient}><TooltipProvider>{children}</TooltipProvider></QueryClientProvider>;
}
function mount(target = one, accepted = vi.fn()) {
  const wrapper = ({ children }: { children: ReactNode }) => <Shell><ThreadReadProvider read={target}>{children}</ThreadReadProvider></Shell>;
  return { ...renderHook(() => useAskOnSide(PARENT, { onAccepted: accepted }), { wrapper }), accepted };
}
function opened(ref: ThreadReadRef): boolean {
  // This is a real consumer of the same inherited presentation state used by SideLaneSlot.
  const hook = renderHook(() => useThreadUi(PARENT), { wrapper: ({ children }) => <ThreadReadProvider read={ref}>{children}</ThreadReadProvider> });
  const current = hook.result.current.steps.get(sideLaneKey(SIDE)) === true; hook.unmount(); return current;
}
beforeEach(() => {
  threadWriter.clear(); queryClient.clear(); restorePresentations({}); useSideAsk.getState().forget(); calls.start.mockReset(); calls.node.mockReset(); calls.feed.mockClear();
  for (const ref of [one, other, defaultThreadRead(PARENT)]) timelineUi.forget(ref);
  client.session.setState({ status: 'signed_in', userId: parent.owner.id });
  seed(one); seed(other, B); composerWriter.setText(PARENT, 'Unsent parent words');
});
afterEach(async () => {
  cleanup(); for (const stop of stopped.splice(0).reverse()) stop();
  await queryClient.cancelQueries(); queryClient.clear(); threadWriter.clear(); useSideAsk.getState().forget();
  for (const ref of [one, other, defaultThreadRead(PARENT)]) timelineUi.forget(ref);
  restorePresentations({}); client.session.setState(savedSession); useComposerStore.setState(savedComposer); vi.restoreAllMocks();
});

it('an inherited content owner receives the exact side reveal while another history, default read and parent draft stay unchanged', async () => {
  calls.start.mockResolvedValue(reply()); const previous = threadViewOf(other), view = mount();
  act(() => view.result.current.mutate({ text: 'An exact side question', toTurn: null }));
  await waitFor(() => expect(view.accepted).toHaveBeenCalledTimes(1));
  expect(opened(one)).toBe(true); expect(opened(other)).toBe(false); expect(opened(defaultThreadRead(PARENT))).toBe(false);
  expect(threadViewOf(other)).toBe(previous); expect(useComposerStore.getState().text[PARENT]).toBe('Unsent parent words');
  expect(calls.start).toHaveBeenCalledTimes(1); expect(calls.start.mock.calls[0]?.[0]).toBe(PARENT);
  expect(calls.start.mock.calls[0]?.[1].content).toEqual([{ type: 'text', text: 'An exact side question' }]);
});

it('the global dialog retains its real opener, uses that held native view without a default GET, and opens only its source', async () => {
  calls.start.mockResolvedValue(reply()); render(<Shell><SideAskHost /><Ui read={one} name="one" /><Ui read={other} name="other" /></Shell>);
  act(() => openSideAsk({ threadId: PARENT, threadName: parent.name, toTurn: null, text: 'An exact side question', read: one }));
  const dialog = await screen.findByRole('dialog', { name: 'Ask on the side' });
  expect(calls.node).not.toHaveBeenCalled(); expect(useSideAsk.getState().current?.read).toEqual(one);
  fireEvent.click(within(dialog).getByRole('button', { name: 'Ask' }));
  await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  expect(screen.getByTestId('one')).toHaveAttribute('data-open', 'true'); expect(screen.getByTestId('other')).toHaveAttribute('data-open', 'false');
  expect(useComposerStore.getState().text[PARENT]).toBe('Unsent parent words'); expect(calls.node).not.toHaveBeenCalled();
});

it('a global question whose captured source changed cannot submit fresh work or borrow the other window', async () => {
  render(<Shell><SideAskHost /></Shell>);
  act(() => openSideAsk({ threadId: PARENT, threadName: parent.name, toTurn: null, text: 'An exact side question', read: one }));
  const dialog = await screen.findByRole('dialog', { name: 'Ask on the side' });
  act(() => selectThreadContinuation(one, B));
  expect(within(dialog).getByRole('button', { name: 'Ask' })).toBeDisabled();
  fireEvent.keyDown(within(dialog).getByRole('textbox'), { key: 'Enter' }); expect(calls.start).not.toHaveBeenCalled();
  expect(threadViewOf(other).read?.selection).toBe(B); expect(opened(other)).toBe(false);
});

it.each(['closed', 'selection', 'away-back', 'reader'] as const)('a late accepted receipt cannot reveal into a %s replacement or a peer', async (change) => {
  const waiting = deferred<Reply>(); calls.start.mockReturnValue(waiting.promise); const view = mount();
  act(() => view.result.current.mutate({ text: 'An exact side question', toTurn: null }));
  await waitFor(() => expect(calls.start).toHaveBeenCalledTimes(1));
  act(() => {
    if (change === 'closed') threadWriter.evict(one);
    else if (change === 'reader') client.session.setState({ readerVersion: (client.session.getState().readerVersion ?? 0) + 1 });
    else { selectThreadContinuation(one, B); if (change === 'away-back') { selectThreadContinuation(one, null); seed(one); } }
  });
  await act(async () => { waiting.resolve(reply()); await waiting.promise; });
  await waitFor(() => expect(view.result.current.isPending).toBe(false));
  expect(opened(one)).toBe(false); expect(opened(other)).toBe(false); expect(opened(defaultThreadRead(PARENT))).toBe(false);
  expect(useComposerStore.getState().text[PARENT]).toBe('Unsent parent words');
  if (change === 'reader') expect(view.accepted).not.toHaveBeenCalled();
});

it('one window freezes/retries its original native identity while another owner starts an independent question', async () => {
  seed(other); const secondSide = '01950000-0000-7000-8000-000000000091';
  calls.start.mockRejectedValueOnce(new Error('Unknown receipt')).mockResolvedValueOnce(reply(secondSide)).mockResolvedValueOnce(reply());
  const first = mount(), second = mount(other);
  act(() => first.result.current.mutate({ text: 'An exact side question', toTurn: null }));
  await waitFor(() => expect(first.result.current.unconfirmed).toBe(true));
  expect(second.result.current.unconfirmed).toBe(false);
  act(() => second.result.current.mutate({ text: 'A different pane question', toTurn: null }));
  await waitFor(() => expect(second.accepted).toHaveBeenCalledTimes(1));
  act(() => first.result.current.mutate({ text: 'Do not replace the frozen body', toTurn: null }));
  await waitFor(() => expect(first.accepted).toHaveBeenCalledTimes(1));
  expect(calls.start).toHaveBeenCalledTimes(3);
  expect(calls.start.mock.calls[2]).toEqual(calls.start.mock.calls[0]);
  expect(calls.start.mock.calls[1]?.[1].client_id).not.toBe(calls.start.mock.calls[0]?.[1].client_id);
  expect(calls.start.mock.calls[1]?.[1].content).toEqual([{ type: 'text', text: 'A different pane question' }]);
});

it('a mismatched native child/input receipt stays unknown and retry keeps the same body and client identity', async () => {
  const wrong = reply(); calls.start.mockResolvedValueOnce({ ...wrong, input: { ...wrong.input, thread_id: PARENT } }).mockResolvedValueOnce(reply());
  const view = mount(); act(() => view.result.current.mutate({ text: 'An exact side question', toTurn: null }));
  await waitFor(() => expect(view.result.current.unconfirmed).toBe(true));
  expect(opened(one)).toBe(false); expect(threadViewOf(SIDE).thread).toBeNull();
  act(() => view.result.current.mutate({ text: 'A changed caller body', toTurn: null }));
  await waitFor(() => expect(view.accepted).toHaveBeenCalledTimes(1));
  expect(calls.start.mock.calls[1]).toEqual(calls.start.mock.calls[0]); expect(opened(one)).toBe(true);
});

it('retargeting a pending hook cannot add the old side to a different native parent list', async () => {
  const waiting = deferred<Reply>(); calls.start.mockReturnValue(waiting.promise);
  const target = resultThreadRead('another-native-thread', 'retargeted-owner');
  const cut = snapshot(); threadWriter.reset(target, { ...cut, thread: { ...cut.thread, id: target.threadId } }, false, captureThreadRead(target));
  queryClient.setQueryData<Thread[]>(forkKeys.sides(PARENT, 'open'), []); queryClient.setQueryData<Thread[]>(forkKeys.sides(target.threadId, 'open'), []);
  const view = renderHook(({ read }) => useAskOnSide(read), { initialProps: { read: one }, wrapper: Shell });
  act(() => view.result.current.mutate({ text: 'An exact side question', toTurn: null }));
  await waitFor(() => expect(calls.start).toHaveBeenCalledTimes(1));
  act(() => { threadWriter.evict(one); view.rerender({ read: target }); });
  await act(async () => { waiting.resolve(reply()); await waiting.promise; });
  await waitFor(() => expect(view.result.current.isPending).toBe(false));
  expect(queryClient.getQueryData<Thread[]>(forkKeys.sides(PARENT, 'open'))?.map((row) => row.id)).toEqual([SIDE]);
  expect(queryClient.getQueryData<Thread[]>(forkKeys.sides(target.threadId, 'open'))).toEqual([]);
  expect(opened(target)).toBe(false);
});

it('the real legacy double marker passes one socket to both owned caches and demands the side only for its captured opener', async () => {
  const world = new World(), person = world.people.get('ada');
  if (person === undefined) throw new Error('The synthetic native reader is missing');
  const initial = (await threadOpening(world, person, PARENT, 200)).sync;
  for (const read of [one, other]) { threadWriter.evict(read); threadWriter.reset(read, initial, false, captureThreadRead(read)); }
  const wire = new Wire(), frames: Record<string, unknown>[] = [];
  const connect = vi.fn(() => wire);
  const socket = createRealtimeSocket({ url: () => 'ws://synthetic.test/api/v1/ws', token: () => 'synthetic-token', renew: () => Promise.resolve(true),
    onSuspended: () => undefined, random: () => 0.5, connect });
  stopped.push(() => socket.stop()); socket.start(); wire.ready();
  const subscriber: Subscriber = { send: (frame) => {
    if (typeof frame !== 'object' || frame === null || Array.isArray(frame)) throw new Error('The native frame must be an object');
    const value: Record<string, unknown> = Object.fromEntries(Object.entries(frame)); frames.push(value); wire.say(value);
  }, close: () => undefined };
  stopped.push(() => world.threads.gone(subscriber));
  for (const read of [one, other]) {
    const spec = createThreadReadFeed(read, { rows: (rows, reset) => { applyThreadFeedRows(read, rows, reset); } });
    const follower = followFeed(socket, { ...spec, initial: () => ({ seq: initial.seq, complete: true, data: initial }) }); stopped.push(() => follower.stop());
    const sub = await wire.subscriptions.next();
    expect(sub.feed).toBe(`thread:${PARENT}`); expect(sub).not.toHaveProperty('continuation_id');
    expect(world.threads.frame(subscriber, person, sub)).toBe(true);
  }
  calls.start.mockImplementation(() => {
    const native = reply();
    world.threads.emit(PARENT, { items: [marker('accepted-side-marker', { marker: 'side_started', data: {
      side: { id: SIDE, kind: 'thread', name: native.thread.name, redacted: false }, by: userRef(person.me), from_turn: 0,
    } })], thread: { sides: { allowed: true, open: 1, max: 4, mode: 'read_only', reason: null } } });
    return Promise.resolve(native);
  });
  render(<Shell><Ui read={one} name="one" lane /><Ui read={other} name="other" lane /></Shell>);
  const view = mount(); act(() => view.result.current.mutate({ text: 'An exact side question', toTurn: null }));
  await waitFor(() => expect(view.accepted).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(calls.feed).toHaveBeenCalledWith(resultThreadRead(SIDE, one.key.owner)));
  expect(calls.feed).not.toHaveBeenCalledWith(resultThreadRead(SIDE, other.key.owner));
  expect(threadViewOf(one).items.has('accepted-side-marker')).toBe(true); expect(threadViewOf(other).items.has('accepted-side-marker')).toBe(true);
  expect(screen.getByTestId('one')).toHaveAttribute('data-open', 'true'); expect(screen.getByTestId('other')).toHaveAttribute('data-open', 'false');
  const acks = frames.filter((frame) => frame.op === 'subscribed'); expect(acks).toHaveLength(2);
  expect(acks.every((frame) => !Object.hasOwn(frame, 'projection'))).toBe(true);
  expect(threadViewOf(one).read?.projection).toBeNull(); expect(threadViewOf(other).read?.projection).toBeNull();
  expect(wire.sent.filter((frame) => frame.op === 'sub')).toHaveLength(2); expect(connect).toHaveBeenCalledTimes(1); expect(calls.start).toHaveBeenCalledTimes(1);
});
