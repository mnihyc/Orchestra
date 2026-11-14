/** What changes only with a release, or with an act that says so, is read once for the life of the page. */
import { QueryClient } from '@tanstack/react-query';
import { readFreshness, readFreshnessSettings } from '@/api/freshness';
import { describe, expect, it, vi } from 'vitest';

import { permissionsQuery } from '@/api/access';
import { notifiersQuery as notifierKinds } from '@/api/notifications';
import { capabilitiesQuery, driversQuery, harnessKindsQuery } from '@/api/resources';
import { notifiersQuery, registryQuery } from '@/api/settings';
import { settingRegistryQuery } from '@/api/search';

describe('reads that cannot change while the page stands', () => {
  it.each([
    ['the kinds of harness and their builds', harnessKindsQuery],
    ['the drivers', driversQuery],
    ['what an environment may be granted', capabilitiesQuery],
    ['the permission catalogue', permissionsQuery],
    ['the kinds of channel (as the shell lists them)', notifierKinds],
    ['the kinds of channel (as the settings list them)', notifiersQuery],
    ['the registry of settings', registryQuery],
    ['the registry of settings (as the finder reads it)', settingRegistryQuery],
  ])('keeps %s for the life of the page', async (_name, query) => {
    expect(query.staleTime).toBe(Number.POSITIVE_INFINITY);
    expect(query.gcTime).toBe(Number.POSITIVE_INFINITY);
    const cache = new QueryClient();
    const read = vi.fn(() => Promise.resolve('Release catalogue'));
    vi.useFakeTimers();
    try {
      const fresh = query.staleTime;
      if (typeof fresh !== 'number') throw new Error('A release catalogue must declare its lifetime.');
      const options = { queryKey: [...query.queryKey], staleTime: fresh, gcTime: query.gcTime, queryFn: read };
      await cache.query(options);
      await vi.advanceTimersByTimeAsync(10 * 60_000);
      expect(await cache.query(options)).toBe('Release catalogue');
      expect(read).toHaveBeenCalledOnce();
    } finally { cache.clear(); vi.useRealTimers(); }
  });
});

it('revalidates mutable answers on return while directories searches and live status use their own windows', () => {
  readFreshnessSettings(() => ({ directory: 120_000, lookup: 20_000, status: 2_000 }));
  try {
    expect(readFreshness(['settings', 'chosen-place'])).toBe(0);
    expect(readFreshness(['thread-shared-tree', 'thread', 'inventory'])).toBe(120_000);
    expect(readFreshness(['names', 'thread', 'chosen words'])).toBe(20_000);
    expect(readFreshness(['usage', 'health', 'provider'])).toBe(2_000);
    expect(readFreshness(['permissions'])).toBe(Infinity);
  } finally { readFreshnessSettings(() => ({})); }
});
