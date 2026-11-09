/** The frame's first read must leave the native overview ready for the subsequently mounted screen. */
import { QueryClient, QueryClientProvider, useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { cleanup, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { nodeKey } from '@/api/nodes';
import { outsideOpeningsQuery } from '@/api/ports';
import { scheduledReads } from '@/api/readScheduler';
import { usageQuery } from '@/api/usage';
import { useUsageBoardParams, useWeeklyUsageParams } from '@/features/usage/Board';
import { parsePageAddress } from '@/features/workbench/contentAddress';
import { PageContext } from '@/features/workbench/pageContext';
import { i18n, setLocale } from '@/i18n';
import { TargetContext } from '@/shell/RouteContext';
import { clearScreenHeading, useCurrentHeading } from '@/shell/screen';
import { ACME } from '../doubles/fixtures';
import { nodeOverview } from '../doubles/screen-openings';
import { World } from '../doubles/world';
import { show } from './support';

function Caption() { return <output aria-label="Frame title">{useCurrentHeading()?.title}</output>; }
function OverviewParts({ id, kind }: { id: string; kind: 'project' | 'topic' }) {
  const board = useUsageBoardParams(id), weekly = useWeeklyUsageParams(id);
  const usage = useQuery(usageQuery(kind === 'project' ? weekly : board));
  const outside = useInfiniteQuery(outsideOpeningsQuery(id));
  return <output aria-label="Native sections">{usage.isSuccess && outside.isSuccess ? i18n.t('common.done') : 'pending'}</output>;
}

it.each(['en', 'zh-CN'] as const)('topic settings metadata keeps overview controls out of its address in %s', async (locale) => {
  await setLocale(locale);
  const world = new World(), person = world.people.get('ada');
  if (person === undefined) throw new Error('The settings frame needs its reader');
  const target = parsePageAddress(`/topic/${ACME.fixLogin}?tab=settings&section=general`);
  if (target === null) throw new Error('The settings frame needs its native page address');
  const node = world.places.node(person, ACME.fixLogin), navigate = vi.fn(() => Promise.resolve(undefined));
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  const requests: string[] = [];
  vi.stubGlobal('fetch', (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(input, init), path = new URL(request.url).pathname;
    requests.push(path);
    if (path === `/api/v1/nodes/${ACME.fixLogin}`) return Promise.resolve(Response.json(node));
    return Promise.reject(new Error(`Unexpected settings-frame read: ${path}`));
  });
  try {
    show(<QueryClientProvider client={cache}><PageContext.Provider value={{ id: 'settings-frame', target, navigate }}><TargetContext target={target} /></PageContext.Provider></QueryClientProvider>);
    await waitFor(() => expect(cache.getQueryData(nodeKey(ACME.fixLogin))).toEqual(node));
    expect(requests).toEqual([`/api/v1/nodes/${ACME.fixLogin}`]);
    expect(navigate).not.toHaveBeenCalled();
  } finally { cleanup(); cache.clear(); }
});
afterEach(async () => { cleanup(); clearScreenHeading(); vi.unstubAllGlobals(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const kind of ['project', 'topic'] as const) {
  it(`the ${kind} frame and late overview share one native read in ${locale}`, async () => {
    await setLocale(locale);
    const world = new World(), person = world.people.get('ada');
    if (person === undefined) throw new Error('The native overview needs its reader');
    const id = kind === 'project' ? ACME.web : ACME.fixLogin;
    const target = parsePageAddress(`/${kind}/${id}`);
    if (target === null) throw new Error('The place needs its native page address');
    const before = client.session.getState();
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
    const requests: URL[] = [];
    vi.stubGlobal('fetch', (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(input, init), url = new URL(request.url);
      requests.push(url);
      if (url.pathname === `/api/v1/nodes/${id}/overview`) return Response.json(await nodeOverview(world, person, id, url.searchParams));
      throw new Error(`Unexpected overview read: ${url.pathname}`);
    });
    client.session.setState({ status: 'signed_in', userId: person.me.id }); scheduledReads.advance();
    try {
      const draw = (parts = false) => <QueryClientProvider client={cache}><TargetContext target={target} /><Caption />{parts && <OverviewParts id={id} kind={kind} />}</QueryClientProvider>;
      const view = show(draw());
      await waitFor(() => expect(screen.getByLabelText('Frame title')).toHaveTextContent(kind === 'project' ? 'Web' : 'Fix login'));
      view.rerender(draw(true));
      expect(await screen.findByLabelText('Native sections')).toHaveTextContent(i18n.t('common.done'));
      expect(requests.map((url) => url.pathname)).toEqual([`/api/v1/nodes/${id}/overview`]);
      expect(requests[0]?.searchParams.get('breakdowns')).toBe(kind === 'project' ? '' : 'person,model');
    } finally { cleanup(); cache.clear(); scheduledReads.advance(); client.session.setState(before); }
  });
}
