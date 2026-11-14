/** The first join of the person's own feed on a page retires none of the reads made beside it; a later join does. */
import { afterEach, expect, it, vi } from 'vitest';

import { me } from '../doubles/fixtures';
import { Wire } from './continuation-fixtures';

afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); vi.unstubAllGlobals(); });

it('lets the first join retire nothing that was read beside it, and a join after a reconnect retire what was read before it', async () => {
  vi.useFakeTimers();
  const wire = new Wire();
  vi.stubGlobal('WebSocket', function FakeWebSocket() { return wire; });
  const { client } = await import('@/api/client');
  const { scheduledReads } = await import('@/api/readScheduler');
  const live = await import('@/realtime/live');
  try {
    client.adopt({ user: me(1, 'first', 'First reader'), access_token: 'synthetic-token', access_expires_at: new Date(Date.now() + 900_000).toISOString() });
    // The read of the shell is made beside the join: the feed is asked to join, and the cut is taken now.
    const joining = live.ensurePersonalSource(true);
    const cut = scheduledReads.version();
    expect(live.personalSourceJoined()).toBe(false);
    wire.ready();
    const sub = await wire.subscriptions.next();
    wire.say({ op: 'subscribed', id: sub.id, feed: 'user', mode: 'delta', seq: 3 });
    await joining;
    expect(live.personalSourceJoined()).toBe(true);
    expect(scheduledReads.version()).toBe(cut);
    // The connection came back and joined again: that is a change of source, and what was read before it is retired.
    wire.say({ op: 'subscribed', id: sub.id, feed: 'user', mode: 'delta', seq: 3 });
    expect(scheduledReads.version()).toBeGreaterThan(cut);
  } finally { live.socket.stop(); live.feeds.clear(); }
});
