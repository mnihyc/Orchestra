/** What a page asked before its first content is what the next document of its address asks for in one request. */
import { beforeEach, expect, it, vi } from 'vitest';

import { askedRead, fetchedFiles, forgetOpenings, nextPlan, openingShown } from '@/api/openingPlan';
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

it('keeps only this origin\'s built scripts and styles among what the document fetched', () => {
  const at = (path: string) => ({ name: new URL(path, location.href).href }) as PerformanceEntry;
  expect(fetchedFiles([at('/assets/route-a.js'), at('/assets/route-a.css'), at('/api/v1/me/bootstrap'), at('/assets/route-a.js'),
    { name: 'https://elsewhere.invalid/assets/x.js' } as PerformanceEntry, at('/fonts/inter.woff2'), at('/assets/words/en-1.json'), at('/assets/other.json')]))
    .toEqual(['assets/route-a.js', 'assets/route-a.css', 'assets/words/en-1.json']);
});

it('keeps what the document asked until its first content and the reads that content started, for its own reader', () => {
  vi.useFakeTimers();
  try {
    askedRead('/me/bootstrap');
    askedRead('/everything?kinds=machine');
    openingShown('reader');
    // The left bar's tree, asked by the content's own answers, still belongs to the opening.
    vi.advanceTimersByTime(300);
    askedRead('/orgs/o1/tree?parent_id=o1');
    vi.advanceTimersByTime(399);
    expect(kept()).toBeNull();
    vi.advanceTimersByTime(1);
    askedRead('/later');
    openingShown('reader');
    vi.advanceTimersByTime(5_000);
    const address = `${location.pathname}${location.search}`;
    expect(kept()?.user).toBe('reader');
    expect(kept()?.plans[address]?.reads).toEqual(['/everything?kinds=machine', '/orgs/o1/tree?parent_id=o1']);
    forgetOpenings();
    expect(kept()).toBeNull();
  } finally { vi.useRealTimers(); }
});

it('keeps the plan of a document that went on to another address before its content at that address too', async () => {
  const start = `${location.pathname}${location.search}`;
  vi.useFakeTimers();
  try {
    history.replaceState(null, '', '/');
    vi.resetModules();
    const fresh = await import('@/api/openingPlan');
    fresh.askedRead('/me/bootstrap');
    fresh.askedRead('/orgs/o1/overview');
    // Home goes on to the person's org: the next document opens there.
    history.replaceState(null, '', '/org/acme');
    fresh.openingShown('reader');
    vi.advanceTimersByTime(5_000);
    expect(kept()?.plans['/']?.reads).toEqual(['/orgs/o1/overview']);
    expect(kept()?.plans['/org/acme']?.reads).toEqual(['/orgs/o1/overview']);
  } finally {
    vi.useRealTimers();
    history.replaceState(null, '', start);
  }
});

it('keeps the confirmed reader’s requested paths when a tab closes before its content or quiet timer', async () => {
  vi.resetModules();
  const fresh = await import('@/api/openingPlan');
  const address = location.pathname + location.search;
  localStorage.setItem('orch.openings', JSON.stringify({
    user: 'reader', plans: { [address]: { reads: ['/orgs/o/tree?parent_id=p'], unstable: [], at: 1 } },
  }));
  fresh.openingFor('reader');
  fresh.askedRead('/orgs/o/tree?parent_id=o');
  window.dispatchEvent(new Event('pagehide'));
  expect(kept()?.plans[address]?.reads).toEqual(['/orgs/o/tree?parent_id=o', '/orgs/o/tree?parent_id=p']);
  fresh.openingShown('reader');
  fresh.askedRead('/orgs/o/overview');
  window.dispatchEvent(new Event('pagehide'));
  expect(kept()?.plans[address]?.reads).toContain('/orgs/o/overview');
  fresh.forgetOpenings();
  window.dispatchEvent(new Event('pagehide'));
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
