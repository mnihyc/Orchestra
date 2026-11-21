/** What a page asked before its first content is what the next document of its address asks for in one request. */
import { beforeEach, expect, it, vi } from 'vitest';

import { askedRead, forgetOpenings, nextPlan, openingShown } from '@/api/openingPlan';
import { createApiClient } from '@/api/client';

const kept = () => JSON.parse(localStorage.getItem('orch.openings') ?? 'null') as { user: string; plans: Record<string, { reads: string[]; unstable: string[] }> } | null;

beforeEach(() => { localStorage.clear(); });

it('keeps the reads a page asked in their order, without the shell read every opening carries', () => {
  expect(nextPlan(['/me/bootstrap', '/everything?kinds=provider', '/orgs/o/tree?parent_id=o', '/everything?kinds=provider'], undefined, 5))
    .toEqual({ reads: ['/everything?kinds=provider', '/orgs/o/tree?parent_id=o'], unstable: [], at: 5 });
});

it('drops a read the page no longer asks and leaves out a read whose query changes between openings', () => {
  const before = { reads: ['/usage?from=1', '/orgs/o/tree?parent_id=o', '/nodes/gone/settings'], unstable: [], at: 1 };
  const next = nextPlan(['/usage?from=2', '/orgs/o/tree?parent_id=o'], before, 2);
  expect(next).toEqual({ reads: ['/orgs/o/tree?parent_id=o'], unstable: ['/usage'], at: 2 });
  // Once found changing, the read stays out even when it is asked alone.
  expect(nextPlan(['/usage?from=3'], next, 3).reads).toEqual([]);
});

it('stops before the opening request would be longer than the edge accepts', () => {
  const reads = Array.from({ length: 200 }, (_, index) => `/nodes/${'n'.repeat(40)}-${index}/settings`);
  const plan = nextPlan(reads, undefined, 1);
  expect(plan.reads.length).toBeGreaterThan(10);
  expect(plan.reads.length).toBeLessThan(reads.length);
  expect(plan.reads.reduce((bytes, read) => bytes + 'read='.length + encodeURIComponent(read).length + 1, 0)).toBeLessThanOrEqual(6_000);
});

it('keeps what the document asked until its first content for its own address, for its own reader only', () => {
  askedRead('/me/bootstrap');
  askedRead('/everything?kinds=machine');
  openingShown('reader');
  askedRead('/later');
  openingShown('reader');
  const address = `${location.pathname}${location.search}`;
  expect(kept()?.user).toBe('reader');
  expect(kept()?.plans[address]?.reads).toEqual(['/everything?kinds=machine']);
  forgetOpenings();
  expect(kept()).toBeNull();
});

it('tells the opening every read the client asks, as its path and query below the base', async () => {
  const asked = vi.fn<(read: string) => void>();
  const fetch = vi.fn((request: Request) => Promise.resolve(Response.json(request.method === 'GET' ? { ok: true } : null)));
  const client = createApiClient({ baseUrl: 'http://orch.test/api/v1', fetch, askedRead: asked });
  await client.api.GET('/orgs/{id}/tree', { params: { path: { id: 'o1' }, query: { sort: 'navigation', parent_id: 'o1' } } });
  await client.api.POST('/auth/logout');
  expect(asked.mock.calls).toEqual([['/orgs/o1/tree?sort=navigation&parent_id=o1']]);
});
