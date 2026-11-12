/** A model or an effort chosen in another window is what a control here shows at once, and the reads that follow it are asked together, without a pause. */
import { QueryObserver } from '@tanstack/react-query';
import { act, cleanup, render } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { queryClient } from '@/api/query';
import { settingsQuery } from '@/api/settings';
import { parametersQuery } from '@/api/threads';
import type { ResolvedSetting, ThreadParameters } from '@/generated/api-types';
import { feeds, socket, useThreadFeed } from '@/realtime/live';
import { flushDeltas, threadViewOf, threadWriter } from '@/realtime/stores/thread';
import { Wire, deferred } from '../unit/continuation-fixtures';
import { T, serve } from '../unit/thread-read-fixtures';

vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  return { ...actual, client: { ...actual.client, accessToken: () => 'synthetic-choice-token' } };
});

function Follows() {
  useThreadFeed(T);
  return <div>Followed</div>;
}

const tick = async () => { await act(async () => { await vi.advanceTimersByTimeAsync(0); }); };
const held = (model: string, effort: string): ThreadParameters => ({
  approval_mode: 'on_request', approval_reviewer: null, parameters: [],
  model: { value: model, writable: true, options: [] },
  effort: { value: effort, writable: true, supported: true },
} as unknown as ThreadParameters);

beforeEach(() => {
  vi.useFakeTimers(); vi.spyOn(Math, 'random').mockReturnValue(0.5);
  feeds.clear(); socket.stop(); threadWriter.clear(); queryClient.clear();
});
afterEach(() => {
  cleanup(); feeds.clear(); socket.stop(); flushDeltas(); threadWriter.clear(); queryClient.clear();
  vi.unstubAllGlobals(); vi.restoreAllMocks(); vi.clearAllTimers(); vi.useRealTimers();
});

async function openedThread() {
  serve();
  const wires: Wire[] = [];
  vi.stubGlobal('WebSocket', class extends Wire { constructor(_url: string) { super(); wires.push(this); } });
  render(<Follows />);
  await tick();
  socket.start();
  const wire = wires[0]; if (wire === undefined) throw new Error('The socket was not opened');
  wire.ready(); await tick();
  const sub = await wire.subscriptions.next();
  wire.say({ op: 'subscribed', id: sub.id, feed: sub.feed, mode: 'delta', seq: 20, projection: threadViewOf(T).read?.projection });
  await tick();
  const row = threadViewOf(T).thread;
  if (row === null) throw new Error('The thread row was not read');
  return { wire, sub, row };
}

it('shows the model the frame carries in the parameters held, and reads the settings and the parameters again at once, together', async () => {
  const { wire, sub, row } = await openedThread();
  const settings = deferred<ResolvedSetting[]>(), parameters = deferred<ThreadParameters>();
  const readSettings = vi.fn(() => settings.promise), readParameters = vi.fn(() => parameters.promise);
  queryClient.setQueryData(parametersQuery(T).queryKey, held(row.model ?? 'gpt-sol', row.effort ?? 'medium'));
  queryClient.setQueryData(settingsQuery(T).queryKey, []);
  const stops = [
    new QueryObserver(queryClient, { ...parametersQuery(T), queryFn: readParameters, staleTime: Infinity }).subscribe(() => undefined),
    new QueryObserver(queryClient, { ...settingsQuery(T), queryFn: readSettings, staleTime: Infinity }).subscribe(() => undefined),
  ];
  try {
    wire.say({ op: 'sync', id: sub.id, feed: sub.feed, prev: row.seq, seq: row.seq + 1, data: { projection: threadViewOf(T).read?.projection, thread: { ...row, model: 'gpt-mini', effort: 'low', seq: row.seq + 1 } } });
    await tick();
    // No pause has run: what a person chose is on screen as the row says it, and both reads are out, neither waiting for the other.
    expect(threadViewOf(T).thread?.model).toBe('gpt-mini');
    expect(queryClient.getQueryData<ThreadParameters>(parametersQuery(T).queryKey)).toMatchObject({ model: { value: 'gpt-mini' }, effort: { value: 'low' } });
    expect(readSettings).toHaveBeenCalledTimes(1);
    expect(readParameters).toHaveBeenCalledTimes(1);
    settings.resolve([]); parameters.resolve(held('gpt-mini', 'low'));
    await tick();
    expect(readSettings).toHaveBeenCalledTimes(1);
    expect(readParameters).toHaveBeenCalledTimes(1);
  } finally { stops.forEach((stop) => stop()); }
});

it('keeps what the parameters say where the row changes neither the model nor the effort', async () => {
  const { wire, sub, row } = await openedThread();
  const readParameters = vi.fn(() => Promise.resolve(held('gpt-sol', 'medium')));
  queryClient.setQueryData(parametersQuery(T).queryKey, held('gpt-sol', 'high'));
  const stop = new QueryObserver(queryClient, { ...parametersQuery(T), queryFn: readParameters, staleTime: Infinity }).subscribe(() => undefined);
  try {
    wire.say({ op: 'sync', id: sub.id, feed: sub.feed, prev: row.seq, seq: row.seq + 1, data: { projection: threadViewOf(T).read?.projection, thread: { ...row, name: 'Another title', seq: row.seq + 1 } } });
    await tick();
    expect(threadViewOf(T).thread?.name).toBe('Another title');
    expect(queryClient.getQueryData<ThreadParameters>(parametersQuery(T).queryKey)).toMatchObject({ model: { value: 'gpt-sol' }, effort: { value: 'high' } });
    expect(readParameters).not.toHaveBeenCalled();
  } finally { stop(); }
});
