/** The native opening primes exactly the caches a thread's visible controls already observe. */
import { QueryClient, QueryClientProvider, useQuery, useInfiniteQuery } from '@tanstack/react-query';
import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createRootRoute, createRouter, createMemoryHistory, RouterProvider } from '@tanstack/react-router';
import { createRef, useState } from 'react';
import type { NodeChatOpening, ThreadOutlinePage, ThreadParameters } from '@/generated/api-types';
import { beforeAll, expect, it, vi } from 'vitest';
import { RanWith } from '@/features/composer/RanWith';
import { i18n } from '@/i18n';
import { settingsKey, settingsQuery } from '@/api/settings';
import { threadGatewayQuery } from '@/api/gateways';
import { pendingAgentSchedulesQuery, type ThreadPendingAgentSchedules } from '@/api/inputSchedules';
import { draftQuery, hydrateThreadOpening, limitsQuery, outlineQuery, parametersQuery, threadKeys } from '@/api/threads';
import { nodeKey } from '@/api/nodes';
import { clearPageBodies, installPageReadStarters, startPageReads } from '@/features/workbench/pageReads';
import { ACME, MODEL_ESTIMATE } from '../doubles/fixtures';
import { threadOpening } from '../doubles/thread-opening';
import { World } from '../doubles/world';
import { show, skipOverflowFallback } from './support';

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

it('labels recorded turn options from compact agent declarations without reading a profile', async () => {
  const world = new World(), person = world.people.get('ada');
  if (person === undefined) throw new Error('The opening needs its native reader');
  const opening = await threadOpening(world, person, ACME.mainThread, 40);
  const parameters = opening.parameters;
  if (parameters?.agent === null || parameters?.agent === undefined) throw new Error('The turn needs its compact agent declaration');
  parameters.agent.parameters = [{ key: 'recorded_context', type: 'integer', title: 'Context', description: '', range: { min: 32_000, max: { from_model: 'context_max' } }, default: { from_model: 'default_context' }, apply: 'new_session' }];
  parameters.parameters = [];
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  hydrateThreadOpening(cache, opening);
  const network = vi.spyOn(globalThis, 'fetch');
  const view = show(<RanWith threadId={ACME.mainThread} turn={{model:'previous-model',params:{recorded_context:400_000}}} />, cache);
  try {
    expect(await screen.findByText(i18n.t('timeline.footer.options', { choices: '400K' }))).toBeVisible();
    expect(network).not.toHaveBeenCalled();
  } finally { view.unmount(); cache.clear(); network.mockRestore(); }
});

it('keeps a visible older model speed human-readable before and after full choices arrive', async () => {
  const world = new World(), person = world.people.get('ada');
  if (person === undefined) throw new Error('The historical label needs its native reader');
  const opening = await threadOpening(world, person, ACME.mainThread, 40);
  const parameters = opening.parameters;
  if (parameters?.agent === null || parameters?.agent === undefined) throw new Error('The recorded option needs its short declaration');
  parameters.agent.parameters = [{ key: 'service_tier', options: { from_model: 'service_tiers' } }];
  parameters.parameters = [];
  const older = { ...MODEL_ESTIMATE, model_name: 'older-model', display_name: 'Older model', allowed: true, reason: null,
    parameters: { service_tiers: [{ id: 'priority', label: 'Fast lane' }] } };
  parameters.model.options.push(older);
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  hydrateThreadOpening(cache, opening);
  const view = show(<RanWith threadId={ACME.mainThread} turn={{ model: 'older-model', params: { service_tier: 'priority' } }} />, cache);
  try {
    const label = i18n.t('timeline.footer.options', { choices: 'Fast lane' });
    expect(await screen.findByText(label)).toBeVisible();
    expect(view.container).not.toHaveTextContent('priority');
    act(() => { cache.setQueryData(threadKeys.parameters(ACME.mainThread), {
      ...parameters, model: { ...parameters.model, options: [...parameters.model.options,
        { ...older, model_name: 'another-model', display_name: 'Another model' }], options_partial: false },
    }); });
    expect(screen.getByText(label)).toBeVisible();
    expect(view.container).not.toHaveTextContent('priority');
  } finally { view.unmount(); cache.clear(); }
});


/** Screen code is fixture preparation; timing begins only once transport inputs are ready. */
const pageScreenModules = Promise.all([
    import('@/api/client'), import('@/api/query'), import('@/api/access'), import('@/realtime/live'), import('@/realtime/stores/chat'),
    import('@/realtime/stores/tree'), import('@/shell/screen'), import('@/shell/pageFirstReads'), import('@/features/workbench/contentAddress'),
    import('@/features/workbench/layout'), import('@/features/composer/Composer'), import('@/features/topic/Topic'), import('@/features/org/Home'),
    import('@/features/chat/ChatPanel'),
  ]);
const pageControlModules = Promise.all([
    import('@/realtime/stores/thread'), import('@/features/timeline/StatusLine'), import('@/features/thread/AudienceBanner'),
  ]);
const fullThreadModules = Promise.all([
  import('@/features/workbench/PageView'), import('@/features/inspector/Knowledge'), import('@/features/inspector/Outline'),
  import('@/features/workbench/instance'), import('@/realtime/threadReadContext'), import('@/realtime/threadScope'),
  import('@/features/workbench/store'), import('@/features/workbench/layout'), import('@/features/workbench/pages'), import('@/routes/thread/$threadId/index'), import('@/shell/bootstrap'),
]);
beforeAll(async () => {
  const [, , modules] = await Promise.all([pageScreenModules, pageControlModules, fullThreadModules]);
  const definition = await modules[8].pageDefinition('/thread/$threadId/');
  await (definition.Body as unknown as { preload?: () => Promise<unknown> }).preload?.();
}, 60_000);

/** Every response takes one long round trip; first-paint consumers must not invent later stages. */
async function delayedPageOpening(kind: 'thread' | 'topic' | 'org', full = false, chatShown = true) {
  const [{ client }, { queryClient: cache }, { permissionsKey }, live, { chatWriter }, { treeWriter }, { clearScreenHeading: clearHeading, focusScreenHeading, useScreenHeading },
    { startAddressedPageReads }, { parsePageAddress }, { makeTab }, { Composer }, { Topic }, { OrgHome }, { ChatPanel }] = await pageScreenModules;
  const [{ useThreadView, threadViewOf, threadWriter }, { StatusLine }, { AudienceBanner }] = await pageControlModules;
  const [{ PageView }, { KnowledgeView }, { OutlineView }, { ViewInstanceProvider }, { ThreadReadProvider },
    { contentThreadRead }, { useWorkbench }, { makeGroup }, , { Route: ThreadRoute }, { ensureShellBootstrap }] = await fullThreadModules;
  const world = new World(), person = world.people.get('ada');
  if (person === undefined) throw new Error('The page needs its native reader');
  const { bootstrap } = await import('../doubles/bootstrap');
  const initial = await bootstrap(world, person);
  const nativeScreens = await import('../doubles/screen-openings');
  const session = client.session.getState();
  const path = kind === 'thread' ? `/thread/${ACME.mainThread}` : kind === 'topic' ? `/topic/${ACME.fixLogin}` : '/org/acme';
  const id = kind === 'thread' ? ACME.mainThread : kind === 'topic' ? ACME.fixLogin : ACME.org;
  const target = parsePageAddress(path);
  if (target === null) throw new Error('The native address must resolve');
  const owner = `long-line-${kind}`, read = full ? contentThreadRead(id, owner) : id;
  const workbench = useWorkbench.getState();
  const calls: { path: string; stage: number }[] = [];
  const cursors: { feed: string; since: number | null; stage: number }[] = [];
  skipOverflowFallback();
  vi.useFakeTimers({ toFake: ['Date'] });
  vi.setSystemTime(new Date('2026-11-16T12:00:00.000Z'));
  const trip = 700;
  let wave = 1;
  const phase = () => wave;
  let replayRequested = false;
  const following = vi.spyOn(live.socket, 'subscribe').mockImplementation((feed, listener) => {
    cursors.push({ feed, since: listener.since(), stage: phase() });
    if (full && feed === `thread:${id}`) queueMicrotask(() => {
      const held = threadViewOf(read);
      listener.onFrame({ op: 'subscribed', id: 1, feed, seq: held.lastSeq, mode: 'delta', projection: held.read?.projection ?? null });
      // The server requires one replay after its admitted opening cursor.
      if (!replayRequested) {
        replayRequested = true;
        listener.onFrame({ op: 'resync', id: 1, feed, reload: false });
      }
    });
    return { unsubscribe: () => undefined };
  });
  vi.stubGlobal('fetch', async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const request = input instanceof Request ? input : new Request(input, init);
    request.signal.throwIfAborted();
    const url = new URL(request.url), native = url.pathname.replace('/api/v1', '');
    if (request.method !== 'GET' && native !== '/summaries/batch') return Response.json({});
    calls.push({ path: native, stage: phase() });
    const sent = phase();
    const delayed = new Promise<void>((resolve, reject) => {
      const timer = setTimeout(resolve, trip);
      request.signal.addEventListener('abort', () => { clearTimeout(timer); reject(new DOMException('Superseded page', 'AbortError')); }, { once: true });
    });
    const served = (async () => {
      try {
        const parts = native.split('/');
        let body: unknown;
        if (request.method === 'POST' && native === '/summaries/batch') {
          const body = await request.json() as Record<string, unknown>;
          const reply = world.summaries.handle(person, 'POST', native, url.searchParams, body);
          if (reply === null) throw new Error('The native summary batch must have a producer');
          return Response.json(reply.body, { status: reply.status });
        }
        if (native === '/me/bootstrap') body = initial;
        else if (native === '/meta') body = world.metaFor(undefined);
        else if (parts[1] === 'threads' && parts[3] === 'opening') body = await threadOpening(world, person, parts[2] ?? '', 40);
        else if (parts[1] === 'nodes' && parts[3] === 'chat-opening') body = await nativeScreens.nodeChatOpening(world, person, parts[2] ?? '', url.searchParams);
        else if (parts[1] === 'nodes' && parts[3] === 'overview') body = await nativeScreens.nodeOverview(world, person, parts[2] ?? '', url.searchParams);
        else if (parts[1] === 'orgs' && parts[3] === 'overview') body = { mode: 'overview', data: await nativeScreens.orgOverview(world, person, parts[2] ?? '', url.searchParams) };
        else if (parts[1] === 'orgs' && parts[2] === 'by-slug') {
          const reply = world.orgBySlug(person, decodeURIComponent(parts[3] ?? ''));
          return Response.json(reply.body, { status: reply.status });
        } else if (parts[1] === 'orgs' && parts[3] === 'tree') {
          const reply = world.tree(person, parts[2] ?? '', null, url.searchParams);
          return Response.json(reply.body, { status: reply.status });
        } else {
          const reply = await world.requestFeeds.read(person, `${native}${url.search}`);
          return Response.json(reply.body, { status: reply.status, headers: reply.headers });
        }
        return Response.json(body);
      } catch (error) {
        const { Refusal } = await import('../doubles/world');
        if (!(error instanceof Refusal)) throw error;
        return Response.json(error.answer.body, { status: error.answer.status });
      }
    })();
    const [response] = await Promise.all([served, delayed]);
    wave = Math.max(wave, sent + 1);
    return response;
  });
  // The session has answered; only its synthetic fixture token is used by the ordinary client.
  client.adopt({ user: person.me, access_token: 'synthetic-stage-reader', access_expires_at: new Date(Date.now() + 900_000).toISOString() });
  cache.clear(); live.feeds.clear(); chatWriter.clear(); treeWriter.clear(); clearHeading();
  const socketState = live.socket.status.getState();
  live.socket.status.setState({ phase: 'ready' });
  if (!full) cache.setQueryData(permissionsKey, initial.permission_catalogue);
  const page = { ...makeTab('page', { href: path }), id: owner };
  if (full) {
    const panels = [makeTab('chat'), makeTab('knowledge'), makeTab('outline')];
    focusScreenHeading(owner);
    useWorkbench.setState({ activeContent: owner, layout: { ...workbench.layout, main: makeGroup([page]),
      right: { ...workbench.layout.right, open: true, active: 'conversation', containers: [{ id: 'native-inspectors', name: 'conversation', active: panels[1]?.id ?? null, sections: panels.map((tab) => ({ tab, open: tab.view !== 'chat' || chatShown, hidden: false, size: 1 })) }] } } });
  }
  if (full) {
    void ensureShellBootstrap(cache, person.me.id);
    installPageReadStarters({ thread: (threadId, tabId) => live.warmThread(contentThreadRead(threadId, tabId)), page: startAddressedPageReads });
    startPageReads(page);
  }
  const warm = !full && kind === 'thread' ? live.warmThread(read) : () => undefined;
  const lease = full ? clearPageBodies : startAddressedPageReads(target, owner);
  function ThreadFirstScreen() {
    const { thread } = useThreadView(id);
    useScreenHeading({ title: thread?.name ?? '', nodeId: thread?.id, views: { ready: thread !== null } });
    return thread === null ? null : <><h1>{thread.name}</h1><AudienceBanner thread={thread} /><StatusLine threadId={id} /><Composer thread={thread} /></>;
  }
  const complete = <ViewInstanceProvider value={{ id: owner, root: createRef<HTMLDivElement>(), target: { kind: 'thread', id }, headingOwner: owner, content: true, focusedContent: true }}>
    <ThreadReadProvider read={contentThreadRead(id, owner)}><PageView tab={page} area="main" />
      <KnowledgeView tab={makeTab('knowledge')} area="right" /><OutlineView tab={makeTab('outline')} area="right" /></ThreadReadProvider>
  </ViewInstanceProvider>;
  const history = createMemoryHistory({ initialEntries: [path] });
  const root = createRootRoute({ component: () => <>{complete}{chatShown && <ChatPanel tab={makeTab('chat')} area="right" />}</> });
  Object.assign(ThreadRoute.options, { id: '/thread/$threadId/', path: '/thread/$threadId/', getParentRoute: () => root });
  const router = createRouter({ routeTree: root.addChildren([ThreadRoute]), history });
  let view: ReturnType<typeof show> | undefined;
  const settle = async (ms: number) => { await new Promise((resolve) => setTimeout(resolve, ms)); await act(async () => { await Promise.resolve(); }); };
  try {
    if (full) { await router.load(); view = show(<RouterProvider router={router} />, cache); }
    await settle(0);
    if (kind !== 'org') {
      if (chatShown) expect(calls).toContainEqual({ path: `/nodes/${id}/chat-opening`, stage: 1 });
      if (kind === 'thread') expect(calls).toContainEqual({ path: `/nodes/${id}/permissions`, stage: 1 });
      if (full) {
        expect(calls, JSON.stringify(calls)).toContainEqual({ path: '/me/bootstrap', stage: 1 });
        expect(calls, JSON.stringify(calls)).toContainEqual({ path: `/threads/${id}/shared/tree`, stage: 1 });
      }
    }
    if (full && !chatShown) await waitFor(() => { expect(calls, JSON.stringify(calls)).toContainEqual({ path: '/summaries/batch', stage: 1 }); });
    await settle(trip);
    if (kind === 'org') {
      expect(calls).toContainEqual({ path: `/orgs/${id}/overview`, stage: 2 });
      expect(calls).toContainEqual({ path: `/nodes/${id}/chat-opening`, stage: 2 });
    }
    if (kind === 'thread') {
      expect(cursors.some((cursor) => cursor.feed === `thread:${id}` && cursor.since === threadViewOf(read).lastSeq && cursor.stage === 2), JSON.stringify(cursors)).toBe(true);
    }
    // The native page and native Chat now take over those same in-flight or completed readers.
    if (!full) view = show(<>{kind === 'thread' ? <ThreadFirstScreen />
      : kind === 'topic' ? <Topic topicId={id} tab="overview" onTab={() => undefined} section="general" onSection={() => undefined} />
        : <OrgHome slug="acme" tab="overview" onTab={() => undefined} />}<ChatPanel tab={makeTab('chat')} area="right" /></>, cache);
    await settle(trip);
    if (chatShown) {
      const held = cache.getQueryData<NodeChatOpening>(['node-chat-opening', id]);
      expect(held?.opening).not.toBeNull();
      if (held?.opening === null || held?.opening === undefined) throw new Error(`The visible native chat must be admitted in two stages: ${JSON.stringify({ calls, status: cache.getQueryState(['node-chat-opening', id])?.status, error: cache.getQueryState(['node-chat-opening', id])?.error })}`);
      const chat = held.opening.sync;
      expect(cursors.some((cursor) => cursor.feed === `chat:${chat.chat.id}` && cursor.since === chat.seq), JSON.stringify({ cursors, calls, heading: (await import('@/shell/screen')).currentHeading() })).toBe(true);
    } else {
      expect(calls.some((call) => call.path === `/nodes/${id}/chat-opening`)).toBe(false);
      expect(cache.getQueryCache().findAll({ queryKey: ['resource-summary'] }).some((query) => query.state.status === 'success')).toBe(true);
    }
    const reads = calls.filter((call) => !call.path.startsWith('/notifications'));
    expect(reads.every((call) => call.stage <= 2), JSON.stringify(reads)).toBe(true);
    expect(calls.filter((call) => call.path === `/nodes/${id}/chat-opening`)).toHaveLength(chatShown ? 1 : 0);
    expect(calls.some((call) => call.path.endsWith('/parameters') || call.path.endsWith('/settings'))).toBe(false);
    expect(calls.filter((call) => call.path === `/threads/${id}/sync`)).toHaveLength(full ? 1 : 0);
    if (full) {
      expect(calls.filter((call) => call.path === `/nodes/${id}/permissions`), JSON.stringify(calls)).toHaveLength(1);
      expect(calls.some((call) => call.path === `/threads/${id}/shared/tree`)).toBe(true);
      expect(calls.some((call) => call.path === `/threads/${id}/outline`)).toBe(true);
      expect(screen.getByTestId('thread-knowledge')).toBeVisible();
      expect(screen.getByTestId('outline-panel')).toBeVisible();
      expect(view?.container.querySelector('[data-screen="thread"]')).toBeVisible();
      // Reading the conversation needs no parent item or turn from a deep link.
      expect(cache.getQueryCache().findAll({ queryKey: ['thread-item', id, ''] })).toHaveLength(0);
      expect(cache.getQueryCache().findAll({ queryKey: ['thread-items', id, { turn: 0 }] })).toHaveLength(0);
      expect(cache.getQueryCache().getAll().filter((query) => query.state.status === 'error').map((query) => ({ key: query.queryKey, error: query.state.error }))).toEqual([]);
    }
    if (kind === 'thread') {
      await userEvent.click(screen.getByRole('button', { name: /^Approval mode:/ }));
      await settle(trip);
      expect(calls.filter((call) => call.path === `/threads/${id}/parameters`)).toHaveLength(1);
      expect(calls.filter((call) => call.path === `/nodes/${id}/settings`)).toHaveLength(1);
      expect(screen.getByRole('dialog', { name: 'Approval mode' })).toBeVisible();
    }
  } finally {
    view?.unmount(); lease(); warm(); live.feeds.clear(); cache.clear(); chatWriter.clear(); treeWriter.clear(); clearHeading();
    if (full) installPageReadStarters({});
    threadWriter.clear(); useWorkbench.setState(workbench, true); history.destroy(); focusScreenHeading(null); live.socket.status.setState(socketState);
    following.mockRestore(); vi.clearAllTimers(); vi.useRealTimers(); vi.unstubAllGlobals(); client.session.setState(session);
  }
}

it.each(['org', 'topic', 'thread'] as const)('draws the actual %s page and Chat in at most two post-session stages on a 700 ms transport', { timeout: 15_000 }, async (kind) => {
  await delayedPageOpening(kind);
});

it('opens the full thread and visible inspectors in two stages with one permission read and one catch-up', { timeout: 60_000 }, async () => {
  await delayedPageOpening('thread', true);
});


it('starts the full thread summary beside its opening when Chat is closed', { timeout: 60_000 }, async () => {
  await delayedPageOpening('thread', true, false);
});
