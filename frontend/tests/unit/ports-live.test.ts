/** Native Ports frames keep their domain snapshot and never turn missing discovery into a successful empty page. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { describe, expect, it } from 'vitest';

import { applyChanges, findAct } from '@/api/acts';
import { hostPortsQuery, portsKey } from '@/api/ports';
import { hostKey } from '@/api/resources';
import type { EnvironmentPorts } from '@/features/ports/presentation';
import { applyQueryFrame } from '@/realtime/queryFeeds';
import type { FeedFrame } from '@/realtime/socket';

import { portsEnvironment, portsProjection } from '../component/ports-fixtures';
import { detailHost } from '../component/resource-detail-fixtures';
import { environment as nativePorts } from '../doubles/ports-wire';

const key = portsKey(portsEnvironment.id);
const event = (data: Record<string, unknown>): FeedFrame => ({ op: 'event', feed: `env:${portsEnvironment.id}:ports`, id: 1, data });

it('shows the accepted host declaration through the live host row', async () => {
  const cache = new QueryClient();
  cache.setQueryData(hostKey(detailHost.id), detailHost);
  const observer = new QueryObserver(cache, { ...hostPortsQuery(detailHost.id), enabled: false });
  const off = observer.subscribe(() => undefined);
  try {
    expect(observer.getCurrentResult().data?.current).toBeNull();
    const ports = { incoming_address: 'host.example', listen: 'all', ranges: [{ from: 41000, to: 41015 }] };
    const answer = { ...detailHost, ports, revision: 2, ports_revision: 1 };
    const act = findAct('PATCH', `/hosts/${detailHost.id}`, { input: { ports, expected_revision: 1 }, answer });
    if (act === null) throw new Error('The host declaration write must have a native act');
    await applyChanges(act.changes, cache);
    expect(observer.getCurrentResult().data).toMatchObject({
      current: { address: ports.incoming_address, bind: ports.listen, ranges: ports.ranges }, revision: 2,
    });
  } finally {
    off();
    cache.clear();
  }
});

it('removes the confirmed closure from its loaded outside pages and retains the history row', async () => {
  const cache = new QueryClient();
  try {
    const opening = { id: 'closing-opening', state: 'listening', revision: 1 };
    const other = { id: 'other-opening', state: 'listening', revision: 1 };
    const key = ['outside-port-openings', 'place'];
    const historyKey = ['port-history', 'place'];
    const page = { items: [opening, other], next_cursor: 'next-page' };
    cache.setQueryData(key, { pages: [page], pageParams: [null] });
    cache.setQueryData(historyKey, [opening]);
    const answer = { ...opening, state: 'closed', revision: 2 };
    const act = findAct('DELETE', `/port-openings/${opening.id}`, { answer });
    if (act === null) throw new Error('The opening closure must have a native act');
    await applyChanges(act.changes, cache);
    expect(cache.getQueryData(key)).toEqual({ pages: [{ ...page, items: [other] }], pageParams: [null] });
    expect(cache.getQueryData(historyKey)).toEqual([answer]);
  } finally {
    cache.clear();
  }
});

describe('the Ports live projection', () => {
  it('projects the full native payload without mistaking its own snapshot for an envelope', () => {
    const cache = new QueryClient();
    try {
      const served = nativePorts(portsProjection());
      applyQueryFrame(cache, key, event({ ...served }));
      const held = cache.getQueryData<EnvironmentPorts>(key);
      expect(held?.snapshot.discovery).toBe('native');
      expect(held?.snapshot.generation).toBe(served.snapshot.generation);
      expect(held?.ports.map((port) => port.port)).toEqual(served.targets.map((target) => target.port));
      expect(held?.capabilities).toEqual(served.capabilities);
      expect(held?.policy.max_open.value).toBe(served.limit);
      expect(held?.host_ports?.address).toBe(served.host_ports?.incoming_address);
    } finally { cache.clear(); }
  });

  it('keeps missing/partial discovery pending, then accepts the late full answer', () => {
    const cache = new QueryClient();
    try {
      const served = nativePorts(portsProjection());
      const { snapshot, ...withoutSnapshot } = served;
      const { discovery: _discovery, ...withoutDiscovery } = snapshot;
      const incomplete = [{}, withoutSnapshot, snapshot, { ...served, snapshot: withoutDiscovery }, { ...served, snapshot: null }];
      for (const data of incomplete) {
        applyQueryFrame(cache, key, event(data));
        expect(cache.getQueryData(key)).toBeUndefined();
      }
      applyQueryFrame(cache, key, event({ ...served }));
      const held = cache.getQueryData<EnvironmentPorts>(key);
      expect(held?.snapshot.discovery).toBe('native');
      expect(held?.listeners.length).toBeGreaterThan(0);
      for (const data of incomplete) {
        applyQueryFrame(cache, key, event(data));
        expect(cache.getQueryData(key)).toBe(held);
      }
      applyQueryFrame(cache, key, event({ ...served, environment_id: 'another-environment' }));
      expect(cache.getQueryData(key)).toBe(held);
    } finally { cache.clear(); }
  });

  for (const discovery of ['native', 'offline', 'unsupported'] as const) {
    it(`retains the served ${discovery} state for a genuinely empty complete answer`, () => {
      const cache = new QueryClient();
      try {
        const served = nativePorts(portsProjection('empty'));
        const data = { ...served, targets: [], snapshot: { ...served.snapshot, listeners: [], discovery } };
        applyQueryFrame(cache, key, { op: 'sync', feed: `env:${portsEnvironment.id}:ports`, id: 1, prev: 0, seq: 1, data });
        const held = cache.getQueryData<EnvironmentPorts>(key);
        expect(held?.snapshot.discovery).toBe(discovery);
        expect(held?.ports).toEqual([]);
        expect(held?.listeners).toEqual([]);
      } finally { cache.clear(); }
    });
  }
});
