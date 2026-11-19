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
    expect(live.personalSourceRevision()).toBeNull();
    wire.ready();
    const sub = await wire.subscriptions.next();
    const revision = { personal: 7, inbox: 3, meta: 2, authority: 11 };
    wire.say({ op: 'subscribed', id: sub.id, feed: 'user', mode: 'delta', seq: 3, personal_revision: revision });
    await joining;
    expect(live.personalSourceJoined()).toBe(true);
    expect(live.personalSourceRevision()).toEqual(revision);
    expect(scheduledReads.version()).toBe(cut);
    // The connection came back and joined again: that is a change of source, and what was read before it is retired.
    wire.say({ op: 'subscribed', id: sub.id, feed: 'user', mode: 'delta', seq: 3, personal_revision: { ...revision, personal: 8 } });
    expect(scheduledReads.version()).toBeGreaterThan(cut);
    expect(live.personalSourceRevision()?.personal).toBe(8);
    live.feeds.clear();
    expect(live.personalSourceRevision()).toBeNull();
  } finally { live.socket.stop(); live.feeds.clear(); }
});
