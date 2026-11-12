/** Setup consumes the observed scoped status; an unsupplied status retains its native reader. */
import type * as AwayApi from '@/api/away';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { screen, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { outsideStatusQuery } from '@/api/away';
import { ConfiguredAway } from '@/features/settings/personal/away/Status';
import type { OutsideStatus } from '@/generated/api-types';
import { ACME } from '../doubles/fixtures';
import { show } from './support';

const observed = vi.hoisted(() => ({ native: vi.fn<() => Promise<OutsideStatus>>() }));
vi.mock('@/api/away', async (original) => ({ ...await original<typeof AwayApi>(), outsideStatusQuery: (nodeId: string) => ({ queryKey: ['setup-outside', nodeId], queryFn: observed.native, retry: false }) }));

it('uses the supplied scoped pause and keeps the native fallback for an unsupplied observation', async () => {
  const outside: OutsideStatus = { scope: { id: ACME.org, kind: 'org', name: 'Acme', redacted: false }, pause_until: '2027-01-01T00:00:00Z', ways: [] };
  observed.native.mockReset().mockResolvedValue(outside);
  const cache = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity } } });
  cache.setQueryData(outsideStatusQuery(ACME.org).queryKey, outside);
  const view = show(<QueryClientProvider client={cache}><ConfiguredAway configured nodeId={ACME.org} /></QueryClientProvider>);
  expect(screen.getByText('Configured')).toBeVisible(); expect(observed.native).not.toHaveBeenCalled();
  view.unmount(); cache.clear();
  const empty = new QueryClient();
  const fresh = show(<QueryClientProvider client={empty}><ConfiguredAway configured nodeId={ACME.org} /></QueryClientProvider>);
  try { await waitFor(() => { expect(observed.native).toHaveBeenCalledOnce(); }); }
  finally { fresh.unmount(); empty.clear(); }
});
