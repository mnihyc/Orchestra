/** Real ViewHost/Activity removal retires hidden owners and their child reads before socket grace expires. */
import { QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, render } from '@testing-library/react';
import { MessageSquareText } from 'lucide-react';
import { useLayoutEffect, useMemo } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { queryClient } from '@/api/query';
import { captureThreadRead, isCurrentThreadRead, selectThreadContinuation } from '@/api/threadRead';
import { fetchOutput } from '@/api/threads';
import { ViewHost, ViewSlot } from '@/features/workbench/Board';
import { shippedLayout } from '@/features/workbench/Arrangements';
import { allTabs, close, open } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';
import { registerView, type ViewProps } from '@/features/workbench/views';
import { clearViewOpenings } from '@/features/workbench/viewOpening';
import { feeds, socket, useThreadFeed } from '@/realtime/live';
import { flushDeltas, threadViewOf, threadWriter } from '@/realtime/stores/thread';
import { resultThreadRead } from '@/realtime/threadScope';
import { useThreadReadScope } from '@/realtime/useThreadRead';
import { Wire, deferred } from '../unit/continuation-fixtures';
import { A, B, OTHER, SHARED, T, bodyOf, serve } from '../unit/thread-read-fixtures';

vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  return { ...actual, client: { ...actual.client, accessToken: () => 'synthetic-view-lifetime-token' } };
});

let unregister: (() => void) | undefined;
const tick = async () => { await act(async () => { await vi.advanceTimersByTimeAsync(0); }); };
function Body({ tab }: ViewProps) {
  const read = useMemo(() => resultThreadRead(T, tab.id), [tab.id]);
  const child = useMemo(() => resultThreadRead(OTHER, tab.id), [tab.id]);
  useLayoutEffect(() => selectThreadContinuation(read, B), [read]);
  useThreadFeed(read);
  useThreadReadScope(child);
  return <div>Owned preview</div>;
}
function Harness({ visible, owner }: { visible: boolean; owner: string }) {
  const layout = useWorkbench((state) => state.layout);
  const tab = allTabs(layout).find((one) => one.id === owner);
  useThreadFeed(T);
  return <QueryClientProvider client={queryClient}><ViewHost layout={layout} />{visible && tab !== undefined && <ViewSlot tab={tab} />}</QueryClientProvider>;
}

beforeEach(() => {
  vi.useFakeTimers(); vi.spyOn(Math, 'random').mockReturnValue(0.5);
  feeds.clear(); socket.stop(); threadWriter.clear(); queryClient.clear(); clearViewOpenings();
});
afterEach(() => {
  cleanup(); unregister?.(); unregister = undefined;
  feeds.clear(); socket.stop(); threadWriter.clear(); queryClient.clear(); clearViewOpenings();
  vi.unstubAllGlobals(); vi.restoreAllMocks(); vi.clearAllTimers(); vi.useRealTimers();
});

for (const kind of ['search-result', 'subagent', 'side-thread']) it(`closing a hidden ${kind} retires its owner and child without reloading the route`, async () => {
  const pending = deferred<Response>();
  const server = serve((request) => new URL(request.url).pathname.endsWith('/output') ? pending.promise : undefined);
  const wires: Wire[] = [];
  vi.stubGlobal('WebSocket', class extends Wire { constructor(_url: string) { super(); wires.push(this); } });
  unregister = registerView({ name: kind, title: 'thread.screen.title', about: 'threadViews.given.title', icon: MessageSquareText, screens: 'all', area: 'main', body: Body });
  const layout = open(shippedLayout('thread'), kind, 'main', { kind: 'thread', id: T, thread: T, side: T });
  const tab = allTabs(layout).find((one) => one.view === kind);
  if (tab === undefined) throw new Error('The actual view tab must exist');
  useWorkbench.setState({ layout });
  const shown = render(<Harness visible owner={tab.id} />);
  await tick();
  socket.start();
  const wire = wires[0]; if (wire === undefined) throw new Error('The actual socket was not opened');
  wire.ready(); await tick();
  const one = await wire.subscriptions.next(), two = await wire.subscriptions.next();
  const preview = resultThreadRead(T, tab.id), child = resultThreadRead(OTHER, tab.id);
  const previewSub = one.continuation_id === B ? one : two, routeSub = one.continuation_id === B ? two : one;
  for (const [sub, target] of [[routeSub, T], [previewSub, preview]] as const) {
    wire.say({ op: 'subscribed', id: sub.id, feed: sub.feed, mode: 'delta', seq: 20, projection: threadViewOf(target).read?.projection });
  }
  const old = captureThreadRead(preview), oldChild = captureThreadRead(child), route = captureThreadRead(T);
  const late = fetchOutput(preview, SHARED, 0, 32);
  const discarded = expect(late).rejects.toMatchObject({ name: 'AbortError' });
  shown.rerender(<Harness visible={false} owner={tab.id} />); await tick();
  expect(isCurrentThreadRead(old)).toBe(true);
  act(() => useWorkbench.getState().change((held) => close(held, tab.id))); await tick();
  expect(threadViewOf(preview).read).toBeNull(); expect(threadViewOf(child).read).toBeNull();
  expect(isCurrentThreadRead(old)).toBe(false); expect(isCurrentThreadRead(oldChild)).toBe(false);
  expect(isCurrentThreadRead(route)).toBe(true);
  const count = server.calls.length;
  wire.say({ op: 'subscribed', id: previewSub.id, feed: previewSub.feed, mode: 'reload', seq: 999 });
  wire.say({ op: 'delta', id: previewSub.id, feed: previewSub.feed, item_id: SHARED, field: 'text', offset: 0, len: 3, data: 'old' });
  pending.resolve(new Response('Retired body')); await discarded; flushDeltas(); await tick();
  expect(server.calls).toHaveLength(count); expect(bodyOf(threadViewOf(T).items.get(SHARED))).toBe(`body-${A}`);
  wire.onclose?.({ code: 1006 }); await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
  const reconnect = wires[1]; if (reconnect === undefined) throw new Error('Expected one reconnect');
  reconnect.ready(); await tick();
  const subscriptions = reconnect.sent.filter((frame) => frame.op === 'sub');
  expect(subscriptions).toHaveLength(1); expect(subscriptions[0]).toMatchObject({ feed: `thread:${T}` });
  expect(subscriptions[0]).not.toHaveProperty('continuation_id');
  expect(server.calls).toHaveLength(count);
});
