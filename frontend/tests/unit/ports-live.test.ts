/** Native Ports frames keep their domain snapshot and never turn missing discovery into a successful empty page. */
import { QueryClient } from '@tanstack/react-query';
import { describe, expect, it } from 'vitest';

import { portsKey } from '@/api/ports';
import type { EnvironmentPorts } from '@/features/ports/presentation';
import { applyQueryFrame } from '@/realtime/queryFeeds';
import type { FeedFrame } from '@/realtime/socket';

import { portsEnvironment, portsProjection } from '../component/ports-fixtures';
import { environment as nativePorts } from '../doubles/ports-wire';

const key = portsKey(portsEnvironment.id);
const event = (data: Record<string, unknown>): FeedFrame => ({ op: 'event', feed: `env:${portsEnvironment.id}:ports`, id: 1, data });

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
