/** The actual shell opens Find before its single bootstrap answers; pending screens never own the binding. */
import type * as LiveModule from '@/realtime/live';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { createMemoryHistory, createRootRoute, createRouter, RouterProvider } from '@tanstack/react-router';
import { act, cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { type ComponentType, StrictMode, Suspense } from 'react';
import type * as LazyModule from '@/features/files/lazy';
import { afterEach, beforeAll, beforeEach, expect, it, vi } from 'vitest';
import type * as ClientModule from '@/api/client';
import { client } from '@/api/client';
import { meStateKey } from '@/api/personal';
import { scheduledReads } from '@/api/readScheduler';
import { closeFinder, openFinder, useFinder } from '@/features/finder/store';
import { useHelpState } from '@/features/help/state';
import { i18n, setLocale } from '@/i18n';
import { SHIPPED_CHOICES } from '@/shell/keyActions';
import { setKeyChoices } from '@/shell/keys';
import { Shell } from '@/shell/Shell';
import { ShellControls } from '@/shell/ShellControls';
import { ScreenLoading } from '@/shell/ScreenLoading';
import { clearSelectedOrg } from '@/shell/org';
import { clearScreenHeading } from '@/shell/screen';
import { queryClient as applicationQueries } from '@/api/query';
import { installPageReadStarters } from '@/features/workbench/pageReads';
import { startAddressedPageReads } from '@/shell/pageFirstReads';
import { useWorkbench } from '@/features/workbench/store';
import { World } from '../doubles/world';
import { nodeOverview } from '../doubles/screen-openings';
import { parsePageAddress } from '@/features/workbench/contentAddress';
import { show } from './support';

const calls = vi.hoisted(() => ({ get: vi.fn<(path: string, options?: { params?: { path?: { id?: string }; query?: Record<string, unknown> } }) => Promise<unknown>>(), admitted: vi.fn<() => Promise<void>>() }));
vi.mock('@/features/topic/Topic', async () => {
  const { useQuery } = await import('@tanstack/react-query');
  const { topicQuery } = await import('@/api/nodes');
  return { Topic: ({ topicId }: { topicId: string }) => { const query = useQuery(topicQuery(topicId)); return query.data === undefined ? <p aria-busy>Topic is loading</p> : <p data-testid="native-topic-body">{query.data.name}</p>; } };
});
const chunks = vi.hoisted(() => ({ wait: null as Promise<void> | null, fail: false, generation: 0 }));
vi.mock('@/features/files/lazy', async (loadOriginal) => {
  const original = await loadOriginal<typeof LazyModule>();
  return { ...original, lazyPart: <Props extends object>(load: () => Promise<{ default: ComponentType<Props> }>) => {
    let generation = -1;
    let Body: ComponentType<Props> | undefined;
    return function ColdPart(props: Props) {
      if (Body === undefined || generation !== chunks.generation) {
        generation = chunks.generation;
        Body = original.lazyPart(async () => {
          if (chunks.wait !== null) await chunks.wait;
          if (chunks.fail) throw new Error('The requested chunk did not arrive.');
          return load();
        });
      }
      return <Body {...props} />;
    };
  } };
});
vi.mock('@/realtime/live', async (original) => ({ ...await original<typeof LiveModule>(), ensurePersonalSource: calls.admitted }));
vi.mock('@/api/client', async (original) => {
  const actual = await original<typeof ClientModule>();
  return { ...actual, api: { ...actual.api, GET: calls.get }, ok: (request: Promise<unknown>) => request };
});
// The finder's code is fetched when it is first opened; here it is fetched before, so that no case waits for it.
beforeAll(async () => { await import('@/features/finder/Finder'); });
const session = client.session.getState();
const finder = useFinder.getState();
const help = useHelpState.getState();
const caches: QueryClient[] = [];
beforeEach(() => {
  clearSelectedOrg(); clearScreenHeading();
  chunks.wait = null; chunks.fail = false; chunks.generation += 1;
  scheduledReads.advance();
  calls.admitted.mockReset().mockResolvedValue(undefined);
  calls.get.mockReset();
  calls.get.mockImplementation((path) => {
    if (path === '/me/bootstrap') return new Promise<unknown>(() => { /* Held until this mounted test shell is retired. */ });
    if (path === '/search/names') return Promise.resolve({ items: [], partial: false });
    if (path === '/settings/registry') return Promise.resolve([]);
    if (path === '/permissions') return Promise.resolve({ permissions: [], roles: [] });
    return Promise.reject(new Error(`Unexpected startup read: ${path}`));
  });
  closeFinder(); useHelpState.setState({ overlay: false }); setKeyChoices(SHIPPED_CHOICES);
  client.session.setState({ status: 'signed_in', userId: 'loading-reader' });
});
afterEach(async () => {
  cleanup(); for (const cache of caches.splice(0)) cache.clear();
  clearSelectedOrg(); clearScreenHeading();
  scheduledReads.advance();
  client.session.setState(session); useFinder.setState(finder); useHelpState.setState(help); setKeyChoices(SHIPPED_CHOICES);
  document.documentElement.removeAttribute('data-theme'); await setLocale('en');
});

function draw(keys?: { single_key: boolean; bindings: { finder: string | null } }, admission?: Promise<void>) {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } }); caches.push(cache);
  if (keys !== undefined) cache.setQueryData(meStateKey, { keys });
  const root = createRootRoute({ shellComponent: ShellControls, beforeLoad: () => admission, pendingComponent: ScreenLoading,
    component: () => <Shell><p>Screen body has not loaded</p></Shell> });
  const router = createRouter({ routeTree: root, history: createMemoryHistory({ initialEntries: ['/'] }), defaultPendingMs: 0, defaultPendingMinMs: 0 });
  return show(<StrictMode><QueryClientProvider client={cache}><RouterProvider router={router} /></QueryClientProvider></StrictMode>);
}

it('shows an addressed topic from its native read while the shell bootstrap is still held', { timeout: 30_000 }, async () => {
  const world = new World(), person = world.people.get('ada');
  const row = [...world.trees.values()].flatMap((tree) => [...tree.nodes.values()]).find((node) => node.kind === 'topic');
  if (person === undefined || row === undefined) throw new Error('The startup world must contain a reader and a topic.');
  const topic = world.places.topic(person, row);
  applicationQueries.clear(); localStorage.clear(); useWorkbench.getState().leave();
  client.session.setState({ status: 'signed_in', userId: person.me.id });
  calls.get.mockImplementation((path, options) => {
    if (path === '/me/bootstrap') return new Promise<unknown>(() => undefined);
    if (path === '/nodes/{id}/overview') {
      const query = new URLSearchParams(Object.entries(options?.params?.query ?? {}).filter(([, value]) => value !== null && value !== undefined).map(([key, value]) => [key, String(value)]));
      return nodeOverview(world, person, topic.id, query);
    }
    if (path === '/nodes/{id}') return Promise.resolve(topic);
    if (path === '/topics/{id}/input-schedules') return Promise.resolve({ items: [], next_cursor: null });
    return Promise.reject(new Error(`Unexpected addressed read: ${path}`));
  });
  installPageReadStarters({ page: startAddressedPageReads });
  const root = createRootRoute({ shellComponent: ShellControls, component: () => <Shell><p>Unused routed body</p></Shell> });
  const { Route: leaf } = await import('@/routes/topic/$topicId/index');
  // File routes receive these fields from the generated tree in the application.
  // Initialize that same route here so its real body reads its own PageContext.
  Object.assign(leaf.options, { getParentRoute: () => root, id: '/topic/$topicId/', path: '/topic/$topicId/' });
  const router = createRouter({ routeTree: root.addChildren([leaf]), history: createMemoryHistory({ initialEntries: [`/topic/${topic.id}`] }), defaultPendingMs: 0, defaultPendingMinMs: 0 });
  const view = show(<QueryClientProvider client={applicationQueries}><RouterProvider router={router} /></QueryClientProvider>, applicationQueries);
  try {
    await waitFor(() => {
      const state = useWorkbench.getState();
      const evidence = JSON.stringify({ active: state.activeContent, layout: state.layout,
        calls: calls.get.mock.calls.map(([path]) => path), query: applicationQueries.getQueryState(['node', topic.id])?.status,
        slots: [...document.querySelectorAll<HTMLElement>('[data-view-instance]')].map((element) => ({ view: element.dataset.viewInstance, text: element.textContent })) });
      expect(screen.queryByTestId('native-topic-body'), evidence).not.toBeNull();
    }, { timeout: 10_000 });
    const body = screen.getByTestId('native-topic-body');
    expect(body).toBeVisible(); expect(body).toHaveTextContent(topic.name);
    expect(applicationQueries.getQueryData(['shell-bootstrap', person.me.id])).toBeUndefined();
    expect(calls.get.mock.calls.filter(([path]) => path === '/nodes/{id}/overview')).toHaveLength(1);
    expect(calls.get.mock.calls.some(([path]) => path === '/topics/{id}/input-schedules')).toBe(true);
  } finally { view.unmount(); applicationQueries.clear(); installPageReadStarters({}); useWorkbench.getState().leave(); }
});

it('starts a resource list from its address before any list body or bootstrap result', async () => {
  const target = parsePageAddress('/resources/providers?status=ready&sort=created&direction=desc');
  if (target === null) throw new Error('The providers address must be registered.');
  calls.get.mockImplementation((path) => path === '/everything' ? new Promise<unknown>(() => undefined) : Promise.reject(new Error(`Unexpected list opening: ${path}`)));
  const release = startAddressedPageReads(target, 'early-provider-list');
  try {
    await waitFor(() => expect(calls.get).toHaveBeenCalledOnce());
    expect(calls.get.mock.calls[0]?.[0]).toBe('/everything');
    expect(calls.get.mock.calls[0]?.[1]?.params?.query).toMatchObject({ kinds: 'provider', resource_state: 'ready', sort: 'created', direction: 'desc' });
    expect(applicationQueries.getQueryData(['shell-bootstrap', 'loading-reader'])).toBeUndefined();
  } finally { release(); applicationQueries.clear(); }
});

it('keeps the conversation visible while first Find loads and retries a failed chunk inside its dialog', { timeout: 30_000 }, async () => {
  let finish!: () => void;
  chunks.wait = new Promise<void>((resolve) => { finish = resolve; });
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } }); caches.push(cache);
  const view = show(<Suspense fallback={<p>The whole workbench is waiting</p>}><ShellControls><p>Conversation stays visible</p></ShellControls></Suspense>, cache);
  try {
    act(() => { openFinder(); });
    const dialog = await screen.findByRole('dialog', { name: i18n.t('finder.title') });
    expect(screen.getByText('Conversation stays visible')).toBeVisible();
    expect(within(dialog).getByRole('status')).toBeInTheDocument();
    chunks.fail = true;
    await act(async () => { finish(); await Promise.resolve(); });
    await screen.findByRole('alert', {}, { timeout: 5000 });
    const failedDialog = screen.getByRole('dialog', { name: i18n.t('finder.title') });
    expect(screen.getByText('Conversation stays visible')).toBeVisible();
    chunks.fail = false;
    chunks.wait = new Promise<void>((resolve) => { finish = resolve; });
    fireEvent.click(within(failedDialog).getByRole('button', { name: i18n.t('common.tryAgain') }));
    const retrying = await screen.findByRole('dialog', { name: i18n.t('finder.title') });
    expect(within(retrying).getByRole('status')).toBeVisible();
    await act(async () => { finish(); await Promise.resolve(); });
    expect(await screen.findByRole('combobox', { name: i18n.t('finder.field') })).toBeVisible();
    expect(screen.queryByText('The whole workbench is waiting')).not.toBeInTheDocument();
  } finally { view.unmount(); finish(); chunks.wait = null; chunks.fail = false; closeFinder(); }
});

it('opens Find even before route admission and the bootstrap reader can mount', async () => {
  let finish: (() => void) | undefined;
  const admission = new Promise<void>((resolve) => { finish = resolve; });
  try {
    draw(undefined, admission);
    await screen.findByRole('status', { name: i18n.t('common.loading') });
    expect(calls.get).not.toHaveBeenCalled();
    await userEvent.setup().keyboard('{Control>}k{/Control}');
    expect(await screen.findByRole('combobox', { name: i18n.t('finder.field') })).toHaveFocus();
    expect(calls.get.mock.calls.every(([path]) => path === '/search/names')).toBe(true);
    expect(calls.get.mock.calls.map(([path]) => path)).not.toContain('/me/bootstrap');
  } finally { cleanup(); finish?.(); }
});

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`opens and focuses Find during bootstrap, keeps typing and toggles only once at ${locale}/${theme}`, { timeout: 30_000 }, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    const user = userEvent.setup(); const view = draw();
    // Route admission also has a short-lived skeleton. Assert the current status once Shell's
    // actual held bootstrap has mounted, rather than retaining the retired admission element.
    await waitFor(() => {
      expect(calls.get.mock.calls.map(([path]) => path)).toEqual(['/me/bootstrap']);
      expect(screen.getByRole('status', { name: i18n.t('common.loading') })).toBeVisible();
    });
    expect(screen.queryByText('Screen body has not loaded')).not.toBeInTheDocument();
    expect(calls.get.mock.calls.map(([path]) => path)).toEqual(['/me/bootstrap']);
    const opening = useFinder.getState().opened;
    await user.keyboard('{Control>}k{/Control}');
    const field = await screen.findByRole('combobox', { name: i18n.t('finder.field') });
    expect(field).toHaveFocus(); expect(useFinder.getState().opened).toBe(opening + 1);
    await user.type(field, 'Acme');
    expect(field).toHaveValue('Acme');
    const reads = calls.get.mock.calls.map(([path]) => path);
    expect(reads).toContain('/search/names');
    expect(reads.filter((path) => path === '/me/bootstrap')).toHaveLength(1);
    expect(reads.filter((path) => !['/me/bootstrap', '/search/names', '/settings/registry', '/permissions'].includes(path))).toEqual([]);
    await user.keyboard('{Control>}k{/Control}');
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    await user.keyboard('{Control>}k{/Control}');
    expect(await screen.findByRole('dialog', { name: i18n.t('finder.title') })).toBeVisible();
    expect(useFinder.getState().opened).toBe(opening + 2);
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    view.unmount();
    const after = useFinder.getState().opened;
    expect(fireEvent.keyDown(document.body, { key: 'k', code: 'KeyK', ctrlKey: true })).toBe(true);
    expect(useFinder.getState().opened).toBe(after); expect(useFinder.getState().open).toBe(false);
  });
}

it('uses cached personal keys without fetching state while bootstrap is pending', async () => {
  const user = userEvent.setup(); draw({ single_key: false, bindings: { finder: 'Mod+P' } });
  await screen.findByRole('status', { name: i18n.t('common.loading') });
  await user.keyboard('{Control>}k{/Control}');
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  await user.keyboard('{Control>}p{/Control}');
  expect(await screen.findByRole('dialog', { name: i18n.t('finder.title') })).toBeVisible();
  const reads = calls.get.mock.calls.map(([path]) => path);
  expect(reads.filter((path) => path === '/me/bootstrap')).toHaveLength(1);
  expect(reads.filter((path) => path !== '/me/bootstrap').every((path) => path === '/search/names')).toBe(true);
});

it('keeps composing keys and an absent session out of the shell binding', async () => {
  draw(); await screen.findByRole('status', { name: i18n.t('common.loading') });
  fireEvent.keyDown(document.body, { key: 'k', code: 'KeyK', ctrlKey: true, isComposing: true });
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  act(() => { client.session.setState({ userId: null, status: 'signed_out' }); });
  expect(fireEvent.keyDown(document.body, { key: 'k', code: 'KeyK', ctrlKey: true })).toBe(true);
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});
