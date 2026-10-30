/** A cached page remembers its route on this device without an HTTP write; native reads keep the portable place. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { setMeState } from '@/api/personal';
import { queryClient } from '@/api/query';
import { rememberPlace, savedPlace, devicePlace } from '@/features/navigation/lastPlace';
import { client } from '@/api/client';

vi.mock('@/api/personal', async (original) => ({ ...await original<object>(), setMeState: vi.fn() }));

describe('the place kept for the person', () => {
  it('is read with the address of its screen', () => {
    expect(savedPlace({ sidebar: { folded: [], last_place: { node_id: 'n1', route: '/topic/n1?tab=threads' } } })).toEqual({ node_id: 'n1', route: '/topic/n1?tab=threads' });
  });

  it('is ignored when its address leads out of the application, or when it is no place at all', () => {
    expect(savedPlace({ sidebar: { last_place: { node_id: 'n1', route: 'https://elsewhere.example/topic' } } })).toBeNull();
    expect(savedPlace({ sidebar: { last_place: { node_id: 'n1', route: '//elsewhere.example/topic' } } })).toBeNull();
    expect(savedPlace({ sidebar: { last_place: { node_id: 7, route: '/topic/n1' } } })).toBeNull();
    expect(savedPlace({ sidebar: { last_place: null } })).toBeNull();
    expect(savedPlace({ sidebar: 'folded' })).toBeNull();
    expect(savedPlace(undefined)).toBeNull();
  });
});

describe('remembering where the person is', () => {
  let session = client.session.getState();
  beforeEach(() => {
    session = client.session.getState();
    client.session.setState({ userId: 'last-place-reader' });
    localStorage.removeItem('orch.last-place-reader.last-place');
    vi.useFakeTimers();
    vi.mocked(setMeState).mockResolvedValue(undefined);
  });
  afterEach(() => {
    client.session.setState(session);
    localStorage.removeItem('orch.last-place-reader.last-place');
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('keeps the newest place, its full route and person isolation with no immediate or delayed HTTP', async () => {
    const read = vi.spyOn(queryClient, 'query').mockResolvedValue({ sidebar: { folded: ['documents'], last_org_id: 'acme' }, help: { closed: [] } });
    rememberPlace({ node_id: 'a', route: '/topic/a' });
    rememberPlace({ node_id: 'b', route: '/topic/b' });
    expect(devicePlace()).toEqual({ node_id: 'b', route: '/topic/b' });

    rememberPlace({ node_id: 'c', route: '/topic/c?tab=threads' });
    expect(devicePlace()).toEqual({ node_id: 'c', route: '/topic/c?tab=threads' });
    await vi.advanceTimersByTimeAsync(60_000);
    expect(setMeState).not.toHaveBeenCalled();
    expect(read).not.toHaveBeenCalled();
    client.session.setState({ userId: 'another-last-place-reader' });
    expect(devicePlace()).toBeNull();
  });
});
