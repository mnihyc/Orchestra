/** The native opening primes exactly the caches a thread's visible controls already observe. */
import { QueryClient, QueryClientProvider, useQuery, useInfiniteQuery } from '@tanstack/react-query';
import { screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { settingsKey, settingsQuery } from '@/api/settings';
import { draftQuery, hydrateThreadOpening, limitsQuery, outlineQuery, parametersQuery, threadKeys } from '@/api/threads';
import { nodeKey } from '@/api/nodes';
import { ACME } from '../doubles/fixtures';
import { threadOpening } from '../doubles/thread-opening';
import { World } from '../doubles/world';
import { show } from './support';

it('shows the supplied thread, model, draft, limits, settings and outline without six follow-up reads', async () => {
  const world = new World();
  const person = world.people.get('ada');
  if (person === undefined) throw new Error('The opening needs its native reader');
  const opening = await threadOpening(world, person, ACME.mainThread, 200);
  const parameters = opening.parameters, limits = opening.limits;
  if (parameters === null || limits === null) throw new Error('Ada can read the thread controls');
  const reads = {
    draft: vi.fn(() => Promise.resolve(opening.draft)),
    parameters: vi.fn(() => Promise.resolve(parameters)),
    limits: vi.fn(() => Promise.resolve(limits)),
    settings: vi.fn(() => Promise.resolve(opening.settings)),
    outline: vi.fn(() => Promise.resolve(opening.outline)),
    node: vi.fn(() => Promise.resolve(opening.sync.thread)),
  };
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  hydrateThreadOpening(cache, opening);
  function Screen() {
    const draft = useQuery({ ...draftQuery(ACME.mainThread), queryFn: reads.draft });
    const model = useQuery({ ...parametersQuery(ACME.mainThread), queryFn: reads.parameters });
    const reached = useQuery({ ...limitsQuery(ACME.mainThread), queryFn: reads.limits });
    const settings = useQuery({ ...settingsQuery(ACME.mainThread), queryFn: reads.settings });
    const outline = useInfiniteQuery({ ...outlineQuery(ACME.mainThread), queryFn: reads.outline });
    const node = useQuery({ queryKey: nodeKey(ACME.mainThread), queryFn: reads.node, staleTime: Infinity });
    return <main><h1>{node.data?.name}</h1><output aria-label="Model">{model.data?.model.value ?? 'Not set'}</output>
      <output aria-label="Draft">{draft.data === null ? 'Empty' : draft.data?.delivery}</output>
      <output aria-label="Limits">{reached.data?.reached.length}</output>
      <output aria-label="Settings">{settings.data?.length}</output><output aria-label="Outline">{outline.data?.pages.flatMap((page) => page.entries).length}</output></main>;
  }
  const view = show(<QueryClientProvider client={cache}><Screen /></QueryClientProvider>);
  try {
    expect(await screen.findByRole('heading', { name: opening.sync.thread.name })).toBeVisible();
    expect(screen.getByLabelText('Model')).toHaveTextContent(parameters.model.value ?? 'Not set');
    expect(screen.getByLabelText('Draft')).toHaveTextContent(opening.draft?.delivery ?? 'Empty');
    expect(screen.getByLabelText('Limits')).toHaveTextContent(String(limits.reached.length));
    expect(screen.getByLabelText('Settings')).toHaveTextContent(String(opening.settings.length));
    expect(screen.getByLabelText('Outline')).toHaveTextContent(String(opening.outline.entries.length));
    expect(cache.getQueryData(nodeKey(ACME.mainThread))).toEqual(opening.sync.thread);
    expect(cache.getQueryData(settingsKey(ACME.mainThread))).toEqual(opening.settings);
    for (const read of Object.values(reads)) expect(read).not.toHaveBeenCalled();
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
