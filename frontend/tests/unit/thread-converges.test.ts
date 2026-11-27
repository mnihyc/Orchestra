/** A thread whose reads failed while it changed on the server comes back to the server's state by itself: a read that
 * failed for the line or for the server's own trouble is made again with the shared pauses, and at once when the
 * socket is back. A refusal still waits for Try again. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { queryClient } from '@/api/query';
import { READ_RETRY_PAUSES_MS } from '@/lib/requests';
import { createRealtimeSocket } from '@/realtime/socket';
import { createThreadReadFeed } from '@/realtime/threadFeed';
import { followFeed } from '@/realtime/sync';
import { defaultThreadRead, threadReadRegistryKey, type ThreadReadRef } from '@/realtime/threadScope';
import { flushDeltas, setThreadReadChangeHandler, setThreadReadHolder, setThreadReadRetainer, threadViewOf,
  threadWriter } from '@/realtime/stores/thread';
import type { ThreadSync } from '@/generated/api-types';
import { Wire, opening, projection, snapshot } from './continuation-fixtures';
import { T, coordinator, ownedRows, serve } from './thread-read-fixtures';

const route = defaultThreadRead(T);
const stopped: (() => void)[] = [];

function setup() {
  const joined = coordinator(); stopped.push(() => joined.registry.clear());
  const wires: Wire[] = [];
  const socket = createRealtimeSocket({ url: () => 'ws://synthetic.test/api/v1/ws', token: () => 'synthetic-token', renew: () => Promise.resolve(true),
    onSuspended: () => undefined, random: () => 0.5, connect: () => { const wire = new Wire(); wires.push(wire); return wire; } });
  socket.start(); const wire = wires[0]; if (wire === undefined) throw new Error('Expected a native socket'); wire.ready(); stopped.push(() => socket.stop());
  const follow = (ref: ThreadReadRef) => {
    const follower = followFeed(socket, createThreadReadFeed(ref, ownedRows(ref)));
    stopped.push(joined.registry.use(threadReadRegistryKey(ref), () => follower, () => threadWriter.evict(ref)));
    return follower;
  };
  return { wire, wires, follow };
}

/** The thread as the server holds it at `seq`, in `state`. */
function cut(state: 'running' | 'idle', seq: number): ThreadSync {
  const rows = snapshot(projection(), seq);
  return { ...rows, thread: { ...rows.thread, id: T, state, seq } };
}
const failure = (status: number, code: string) => Response.json({ type: `orch:error:${code}`, code, status, title: 'Synthetic failure',
  detail: 'The server could not read the thread', request_id: 'synthetic' }, { status });
/** The thread's openings, in order; the last one answers every later opening. */
function openings(...answers: (() => Response)[]) {
  let made = 0;
  const served = serve((request) => {
    if (!new URL(request.url).pathname.endsWith('/opening')) return undefined;
    const answer = answers[Math.min(made, answers.length - 1)];
    made += 1;
    return answer === undefined ? undefined : Promise.resolve(answer());
  });
  return { ...served, made: () => made };
}
const state = () => threadViewOf(route).thread?.state;
function ack(wire: Wire, sub: { id: number; feed: string }) {
  wire.say({ op: 'subscribed', id: sub.id, feed: sub.feed, mode: 'delta', seq: threadViewOf(route).thread?.seq ?? 0, projection: projection() });
}

beforeEach(() => {
  vi.useFakeTimers(); vi.spyOn(Math, 'random').mockReturnValue(0); threadWriter.clear(); queryClient.clear();
  setThreadReadChangeHandler(() => undefined); setThreadReadRetainer(() => undefined); setThreadReadHolder(() => () => undefined);
});
afterEach(() => {
  for (const stop of stopped.splice(0).reverse()) stop(); flushDeltas(); threadWriter.clear(); queryClient.clear();
  setThreadReadChangeHandler(() => undefined); setThreadReadRetainer(() => undefined); setThreadReadHolder(() => () => undefined);
  vi.unstubAllGlobals(); vi.clearAllTimers(); vi.useRealTimers(); vi.restoreAllMocks();
});

it('reads a thread again by itself after the server could not serve its reload, and shows the state the server holds', async () => {
  const served = openings(() => Response.json(opening(cut('running', 20))), () => failure(500, 'internal'),
    () => Response.json(opening(cut('idle', 30))));
  const rig = setup(); rig.follow(route); const sub = await rig.wire.subscriptions.next(); ack(rig.wire, sub);
  expect(state()).toBe('running');
  rig.wire.say({ op: 'resync', id: sub.id, feed: sub.feed, reload: true });
  await vi.advanceTimersByTimeAsync(0);
  expect(served.made()).toBe(2);
  expect(threadViewOf(route).error).not.toBeNull();
  expect(state()).toBe('running');
  await vi.advanceTimersByTimeAsync(READ_RETRY_PAUSES_MS[0]);
  expect(served.made()).toBe(3);
  expect(state()).toBe('idle');
  expect(threadViewOf(route).error).toBeNull();
  // It is followed again from where the answer stands.
  const again = await rig.wire.subscriptions.next();
  expect(again).toMatchObject({ feed: `thread:${T}`, since: 30 });
});

it('keeps asking with the shared pauses while the server stays in trouble', async () => {
  const served = openings(() => Response.json(opening(cut('running', 20))), () => failure(503, 'internal'), () => failure(503, 'internal'),
    () => failure(503, 'internal'), () => Response.json(opening(cut('idle', 30))));
  const rig = setup(); rig.follow(route); const sub = await rig.wire.subscriptions.next(); ack(rig.wire, sub);
  rig.wire.say({ op: 'resync', id: sub.id, feed: sub.feed, reload: true });
  await vi.advanceTimersByTimeAsync(0);
  for (const [attempt, pause] of READ_RETRY_PAUSES_MS.entries()) {
    expect(served.made()).toBe(attempt + 2);
    expect(state()).toBe('running');
    await vi.advanceTimersByTimeAsync(pause);
  }
  expect(served.made()).toBe(5);
  expect(state()).toBe('idle');
});

it('reads what it missed again by itself when that read failed', async () => {
  let syncs = 0;
  serve((request) => {
    const path = new URL(request.url).pathname;
    if (path.endsWith('/opening')) return Promise.resolve(Response.json(opening(cut('running', 20))));
    if (!path.endsWith('/sync')) return undefined;
    syncs += 1;
    return Promise.resolve(syncs === 1 ? failure(500, 'internal') : Response.json({ ...cut('idle', 30), since: 20 }));
  });
  const rig = setup(); rig.follow(route); const sub = await rig.wire.subscriptions.next(); ack(rig.wire, sub);
  rig.wire.say({ op: 'resync', id: sub.id, feed: sub.feed });
  await vi.advanceTimersByTimeAsync(0);
  expect(syncs).toBe(1);
  expect(threadViewOf(route).error).not.toBeNull();
  expect(state()).toBe('running');
  await vi.advanceTimersByTimeAsync(READ_RETRY_PAUSES_MS[0]);
  expect(syncs).toBe(2);
  expect(state()).toBe('idle');
  expect(threadViewOf(route).error).toBeNull();
});

it('reads at once when the socket is back instead of waiting out the pause', async () => {
  const served = openings(() => Response.json(opening(cut('running', 20))), () => failure(500, 'internal'), () => failure(500, 'internal'),
    () => Response.json(opening(cut('idle', 30))));
  const rig = setup(); rig.follow(route); const sub = await rig.wire.subscriptions.next(); ack(rig.wire, sub);
  rig.wire.say({ op: 'resync', id: sub.id, feed: sub.feed, reload: true });
  await vi.advanceTimersByTimeAsync(0);
  await vi.advanceTimersByTimeAsync(READ_RETRY_PAUSES_MS[0]);
  expect(served.made()).toBe(3);
  // The second pause is two seconds; the socket is back after half a second.
  rig.wire.onclose?.({ code: 1011 });
  await vi.advanceTimersByTimeAsync(600);
  const wire = rig.wires.at(-1); if (wire === undefined || wire === rig.wire) throw new Error('The socket must connect again');
  wire.ready();
  await vi.advanceTimersByTimeAsync(0);
  expect(served.made()).toBe(4);
  expect(state()).toBe('idle');
});

it('leaves a refusal for Try again and asks nothing more by itself', async () => {
  const served = openings(() => Response.json(opening(cut('running', 20))), () => failure(404, 'not_found'));
  const rig = setup(); rig.follow(route); const sub = await rig.wire.subscriptions.next(); ack(rig.wire, sub);
  rig.wire.say({ op: 'resync', id: sub.id, feed: sub.feed, reload: true });
  await vi.advanceTimersByTimeAsync(30_000);
  expect(served.made()).toBe(2);
});

it('asks nothing more once the thread is no longer followed', async () => {
  const served = openings(() => Response.json(opening(cut('running', 20))), () => failure(500, 'internal'));
  const rig = setup(); const follower = rig.follow(route); const sub = await rig.wire.subscriptions.next(); ack(rig.wire, sub);
  rig.wire.say({ op: 'resync', id: sub.id, feed: sub.feed, reload: true });
  await vi.advanceTimersByTimeAsync(0);
  follower.stop();
  await vi.advanceTimersByTimeAsync(30_000);
  expect(served.made()).toBe(2);
});
