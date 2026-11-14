/** A cold load of a thread's address asks the session first and, at its answer, the shell, the thread and the navigation tree together. */
import type { AnyRouter } from '@tanstack/react-router';
import { afterEach, expect, it, vi } from 'vitest';

import { me } from '../doubles/fixtures';
import { Wire } from './continuation-fixtures';

const marked = vi.hoisted(() => vi.fn<(name: string) => void>());
vi.mock('@/lib/marks', () => ({ mark: marked }));

const ORG = '01950000-0000-7000-8000-0000000000aa';
const THREAD = '01950000-0000-7000-8000-0000000000bb';

interface Call { name: string; answeredBefore: number; answer: (response: Response) => void }
const calls: Call[] = [];
let answered = 0;

afterEach(() => {
  // Whatever is still out is answered with a refusal, so that nothing is left waiting.
  for (const call of calls) call.answer(new Response(null, { status: 500 }));
  vi.clearAllTimers(); vi.useRealTimers(); vi.unstubAllGlobals();
});

it('asks the session first, then the shell, the thread and the tree together, none of them waiting for another\'s answer', async () => {
  vi.useFakeTimers();
  const reader = me(1, 'first', 'First reader');
  // The device remembers which organization this person last had in the left bar.
  localStorage.setItem(`orch.${reader.id}.org`, ORG);
  vi.stubGlobal('fetch', (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const request = input instanceof Request ? input : new Request(input, init);
    return new Promise<Response>((resolve) => {
      const name = `${request.method} ${new URL(request.url).pathname.replace('/api/v1', '')}`;
      calls.push({ name, answeredBefore: answered, answer: (response) => { answered += 1; resolve(response); } });
    });
  });
  // The live connection is made but never opens here.
  vi.stubGlobal('WebSocket', function FakeWebSocket() { return new Wire(); });
  const { beginStartup } = await import('@/shell/startup');
  const router = { matchRoutes: () => [], routesById: {}, loadRouteChunk: () => undefined } as unknown as AnyRouter;
  const names = () => calls.map((call) => call.name);

  beginStartup(router, `/thread/${THREAD}`, false);
  await vi.waitFor(() => { expect(names()).toEqual(['POST /auth/session']); });

  calls[0]?.answer(Response.json({ user: reader, access_token: 'synthetic-token', access_expires_at: new Date(Date.now() + 900_000).toISOString() }));
  await vi.waitFor(() => { expect(names()).toContain('GET /me/bootstrap'); });
  // The real session fan-out opens the addressed tab and starts it without a mounted body.
  await vi.waitFor(() => { expect(names()).toHaveLength(4); });

  expect(names()[0]).toBe('POST /auth/session');
  expect(names().slice(1).sort()).toEqual([`GET /me/bootstrap`, `GET /orgs/${ORG}/tree`, `GET /threads/${THREAD}/opening`].sort());
  // Each was sent before any of the three had been answered: one wave, not a chain.
  expect(calls.slice(1).map((call) => call.answeredBefore)).toEqual([1, 1, 1]);
  // And every step of it is marked, in the order it was taken.
  expect(marked.mock.calls.map(([name]) => name)).toEqual(['start/session-asked', 'start/session-answered', 'start/shell-asked', 'start/tree-asked', 'start/page-read-asked', 'tab/opened']);
});
