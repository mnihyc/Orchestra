/** The native opening primes exactly the caches a thread's visible controls already observe. */
import { QueryClient, QueryClientProvider, useQuery, useInfiniteQuery } from '@tanstack/react-query';
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import type { ThreadOutlinePage, ThreadParameters } from '@/generated/api-types';
import { expect, it, vi } from 'vitest';
import { settingsKey, settingsQuery } from '@/api/settings';
import { threadGatewayQuery } from '@/api/gateways';
import { pendingAgentSchedulesQuery, type ThreadPendingAgentSchedules } from '@/api/inputSchedules';
import { draftQuery, hydrateThreadOpening, limitsQuery, outlineQuery, parametersQuery, threadKeys } from '@/api/threads';
import { nodeKey } from '@/api/nodes';
import { ACME } from '../doubles/fixtures';
import { threadOpening } from '../doubles/thread-opening';
import { World } from '../doubles/world';
import { show } from './support';

it('shows the first screen without follow-up reads and loads settings, outline and choices when opened', async () => {
  const world = new World();
  const person = world.people.get('ada');
  if (person === undefined) throw new Error('The opening needs its native reader');
  const opening = await threadOpening(world, person, ACME.mainThread, 200);
  const parameters = opening.parameters, limits = opening.limits;
  if (parameters === null || limits === null) throw new Error('Ada can read the thread controls');
  if (opening.gateway?.status !== 'ready') throw new Error('The opening needs its native carrier observation');
  const gateway = opening.gateway.data;
  const settings = world.threads.settingsOf(ACME.mainThread);
  const outline = (await world.requestFeeds.read(person, `/threads/${ACME.mainThread}/outline`)).body as ThreadOutlinePage;
  const fullParameters = (await world.requestFeeds.read(person, `/threads/${ACME.mainThread}/parameters`)).body as ThreadParameters;
  const reads = {
    draft: vi.fn(() => Promise.resolve(opening.draft)),
    parameters: vi.fn(() => Promise.resolve(fullParameters)),
    limits: vi.fn(() => Promise.resolve(limits)),
    settings: vi.fn(() => Promise.resolve(settings)),
    outline: vi.fn(() => Promise.resolve(outline)),
    node: vi.fn(() => Promise.resolve(opening.sync.thread)),
    gateway: vi.fn(() => Promise.resolve(gateway)),
    pending: vi.fn(() => Promise.resolve({ seq: 0, items: [] })),
  };
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  hydrateThreadOpening(cache, opening);
  const pending = cache.getQueryData<ThreadPendingAgentSchedules>(pendingAgentSchedulesQuery(ACME.mainThread, opening.sync.thread.topic_id).queryKey);
  if (pending === undefined) throw new Error('The current opening needs its pending schedule projection');
  function OpenedChoices() {
    useQuery({ ...parametersQuery(ACME.mainThread, true), queryFn: reads.parameters });
    return null;
  }
  function Screen() {
    const [expanded, setExpanded] = useState(false);
    const draft = useQuery({ ...draftQuery(ACME.mainThread), queryFn: reads.draft });
    const model = useQuery({ ...parametersQuery(ACME.mainThread), queryFn: reads.parameters });
    const reached = useQuery({ ...limitsQuery(ACME.mainThread), queryFn: reads.limits });
    const settings = useQuery({ ...settingsQuery(ACME.mainThread), queryFn: reads.settings, enabled: expanded });
    const outline = useInfiniteQuery({ ...outlineQuery(ACME.mainThread), queryFn: reads.outline, enabled: expanded });
    const node = useQuery({ queryKey: nodeKey(ACME.mainThread), queryFn: reads.node, staleTime: Infinity });
    const carrier = useQuery({ ...threadGatewayQuery(ACME.mainThread, opening.sync.thread.execution_revision), queryFn: reads.gateway, staleTime: Infinity });
    const scheduled = useQuery({ ...pendingAgentSchedulesQuery(ACME.mainThread, opening.sync.thread.topic_id), queryFn: reads.pending });
    return <main><button onClick={() => setExpanded(true)}>Open details</button>{expanded && <OpenedChoices />}<h1>{node.data?.name}</h1><output aria-label="Model">{model.data?.model.value ?? 'Not set'}</output>
      <output aria-label="Draft">{draft.data === null ? 'Empty' : draft.data?.delivery}</output>
      <output aria-label="Limits">{reached.data?.reached.length}</output>
      <output aria-label="Settings">{settings.data?.length}</output><output aria-label="Outline">{outline.data?.pages.flatMap((page) => page.entries).length}</output><output aria-label="Carrier">{carrier.data?.state}</output><output aria-label="Pending schedules">{scheduled.data?.items.length}</output></main>;
  }
  const view = show(<QueryClientProvider client={cache}><Screen /></QueryClientProvider>);
  try {
    expect(await screen.findByRole('heading', { name: opening.sync.thread.name })).toBeVisible();
    expect(screen.getByLabelText('Model')).toHaveTextContent(parameters.model.value ?? 'Not set');
    expect(screen.getByLabelText('Draft')).toHaveTextContent(opening.draft?.delivery ?? 'Empty');
    expect(screen.getByLabelText('Limits')).toHaveTextContent(String(limits.reached.length));
    expect(screen.getByLabelText('Carrier')).toHaveTextContent(gateway.state);
    expect(screen.getByLabelText('Pending schedules')).toHaveTextContent(String(pending.items.length));
    expect(cache.getQueryData(nodeKey(ACME.mainThread))).toEqual(opening.sync.thread);
    expect(cache.getQueryData(settingsKey(ACME.mainThread))).toBeUndefined();
    expect(cache.getQueryData(threadKeys.outline(ACME.mainThread))).toBeUndefined();
    for (const read of Object.values(reads)) expect(read).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: 'Open details' }));
    await waitFor(() => {
      expect(reads.parameters).toHaveBeenCalledOnce();
      expect(reads.settings).toHaveBeenCalledOnce();
      expect(reads.outline).toHaveBeenCalledOnce();
    });
    expect(screen.getByLabelText('Settings')).toHaveTextContent(String(settings.length));
    expect(screen.getByLabelText('Outline')).toHaveTextContent(String(outline.entries.length));
    expect(cache.getQueryData(threadKeys.parameters(ACME.mainThread))).toEqual(fullParameters);
  } finally { view.unmount(); cache.clear(); }
});

it('keeps unavailable controls distinct from empty measured or permitted controls', async () => {
  const world = new World(), person = world.people.get('ada');
  if (person === undefined) throw new Error('The opening needs its native reader');
  const opening = await threadOpening(world, person, ACME.mainThread, 200);
  const cache = new QueryClient();
  try {
    hydrateThreadOpening(cache, opening);
    hydrateThreadOpening(cache, { ...opening, parameters: null, limits: null });
    expect(cache.getQueryData(threadKeys.parameters(ACME.mainThread))).toBeUndefined();
    expect(cache.getQueryData([...threadKeys.limits(ACME.mainThread), null])).toBeUndefined();
    expect(cache.getQueryData(threadKeys.draft(ACME.mainThread))).toEqual(opening.draft);
    expect(cache.getQueryData(nodeKey(ACME.mainThread))).toEqual(opening.sync.thread);
  } finally { cache.clear(); }
});

it('rejects an incomplete opening before it can overwrite the complete thread or seed any control', async () => {
  const world = new World(), person = world.people.get('ada');
  if (person === undefined) throw new Error('The opening needs its native reader');
  const opening = await threadOpening(world, person, ACME.mainThread, 200);
  const cache = new QueryClient();
  cache.setQueryData(nodeKey(ACME.mainThread), opening.sync.thread);
  const incomplete = structuredClone(opening);
  Reflect.deleteProperty(incomplete.sync.thread, 'can');
  try {
    expect(() => hydrateThreadOpening(cache, incomplete)).toThrow('complete thread view');
    expect(cache.getQueryData(nodeKey(ACME.mainThread))).toEqual(opening.sync.thread);
    expect(cache.getQueryData(threadKeys.parameters(ACME.mainThread))).toBeUndefined();
    expect(cache.getQueryData(threadKeys.draft(ACME.mainThread))).toBeUndefined();
    expect(cache.getQueryData(settingsKey(ACME.mainThread))).toBeUndefined();
  } finally { cache.clear(); }
});
