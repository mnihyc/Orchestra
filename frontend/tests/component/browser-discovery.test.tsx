/** The actual Environment > Tools > Browsers route handles missing programs and keeps navigation around a failed child. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState, type ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import type * as ClientApi from '@/api/client';
import type * as EnvironmentApi from '@/api/environments';
import { browserKeys } from '@/api/browsers';
import { EnvironmentScreen, type EnvironmentTab } from '@/features/resources/environments/Environment';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { applyQueryFrame } from '@/realtime/queryFeeds';
import type { FeedFrame } from '@/realtime/socket';
import { browserEnvironment as environment, browserSnapshot, runningBrowser } from './browser-discovery-fixtures';
import { show } from './support';

interface Reply { data: unknown; response: Response }
const wire = vi.hoisted(() => {
  const state: { answer: unknown; broken: boolean } = { answer: null, broken: false };
  return { state, read: vi.fn<(path: string, options?: unknown) => Promise<Reply>>() };
});
vi.mock('@/api/client', async (load) => { const actual = await load<typeof ClientApi>(); return { ...actual, api: { ...actual.api, GET: wire.read, POST: (path: string) => { if (path !== '/resources/library') throw new Error(`Unexpected mutation ${path}`); return Promise.resolve(reply([])); } } }; });
vi.mock('@/api/environments', async (load) => {
  const actual = await load<typeof EnvironmentApi>();
  return { ...actual, environmentQuery: (id: string) => ({ queryKey: actual.environmentKey(id), queryFn: () => Promise.resolve(environment), staleTime: Infinity }) };
});
vi.mock('@tanstack/react-router', () => ({ useNavigate: () => vi.fn(),
  Link: ({ children, className }: { children: ReactNode; className?: string }) => <a href="#environment" className={className}>{children}</a> }));
vi.mock('@/features/resources/trail', () => ({ useResourceHeading: () => undefined }));
vi.mock('@/realtime/environmentFeed', () => ({ useEnvironmentFeed: () => undefined }));
vi.mock('@/realtime/live', () => ({ socket: { subscribe: () => ({ unsubscribe: () => undefined }) } }));
vi.mock('@/features/resources/environments/acts', () => ({ useEnvironmentActs: () => ({ page: () => ({ primary: null, edit: null, menu: [] }), keepAwake: () => null, dialogs: null }) }));
vi.mock('@/features/resources/environments/McpServers', () => ({ McpServers: () => <p>Tools ready</p> }));
vi.mock('@/features/resources/environments/Doors', () => ({ EnvironmentDoor: () => <p>Terminal ready</p> }));
vi.mock('@/features/ports/Ports', () => ({ PortsPanel: () => <p>Ports ready</p> }));
vi.mock('@/features/vnc/View', () => ({ DisplaysTab: () => <p>Display ready</p> }));
// A deliberately failing picture proves the real panel boundary independently of malformed API data.
vi.mock('@/features/browser/Front', () => ({ Front: () => {
  if (wire.state.broken) throw new Error('The browser picture failed to render');
  return <p>Browser picture</p>;
} }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/features/help/Hint', () => ({ Hint: () => null }));

const caches = new Set<QueryClient>();
const appearance = useAppearance.getState();
function Scene() {
  const [tab, setTab] = useState<EnvironmentTab>('browsers');
  return <EnvironmentScreen environmentId={environment.id} tab={tab} onTab={setTab} />;
}
function draw(theme: 'light' | 'dark') {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity }, mutations: { retry: false } } });
  caches.add(cache); useAppearance.setState({ theme, resolvedTheme: theme });
  show(<QueryClientProvider client={cache}><Scene /></QueryClientProvider>);
  return cache;
}
function reply(data: unknown): Reply { return { data, response: new Response(null, { status: 200 }) }; }
function tools() { return screen.getByRole('navigation', { name: i18n.t('resourceDetails.layout.tools') }); }
function expectNavigation() {
  const navigation = tools();
  for (const label of ['mcpServers.title', 'views.terminal.title', 'ports.title', 'browser.tab', 'vnc.tab'] as const)
    expect(within(navigation).getByRole('link', { name: i18n.t(label) })).toBeVisible();
  expect(screen.getByRole('heading', { level: 1, name: environment.name })).toBeVisible();
}
function receive(cache: QueryClient, data: Record<string, unknown>) {
  const frame: FeedFrame = { op: 'event', feed: `env:${environment.id}:browsers`, id: 1, data };
  act(() => { applyQueryFrame(cache, browserKeys.list(environment.id), frame); });
}

beforeEach(() => { wire.state.answer = browserSnapshot(); wire.state.broken = false; wire.read.mockReset().mockImplementation(() => Promise.resolve(reply(wire.state.answer))); });
afterEach(async () => {
  cleanup();
  for (const cache of caches) { await cache.cancelQueries(); cache.clear(); }
  caches.clear(); useAppearance.setState(appearance); await setLocale('en');
});

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  const missingPrograms = locale === 'en'
    ? 'The browser response is missing its program information.'
    : '浏览器响应中缺少程序信息。';
  for (const missing of ['support', 'programs'] as const) {
    it(`keeps missing ${missing} recoverable inside the actual browser tab (${locale}/${theme})`, async () => {
      await setLocale(locale);
      const full = browserSnapshot(), { support, ...rest } = full;
      const { programs: _programs, ...withoutPrograms } = support;
      wire.state.answer = missing === 'support' ? rest : { ...rest, support: withoutPrograms };
      const user = userEvent.setup(); draw(theme);
      expect(await screen.findByRole('alert')).toHaveTextContent(missingPrograms);
      expectNavigation();
      expect(screen.queryByText(i18n.t('browser.none.title', { environment: environment.name }))).not.toBeInTheDocument();
      expect(screen.queryByRole('button', { name: i18n.t('browser.list.open') })).not.toBeInTheDocument();
      expect(wire.read).toHaveBeenCalledTimes(1);
      wire.state.answer = full;
      await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
      expect(await screen.findByRole('button', { name: i18n.t('browser.list.open') })).toBeEnabled();
      expect(screen.queryByRole('alert')).not.toBeInTheDocument();
      expectNavigation(); expect(wire.read).toHaveBeenCalledTimes(2);
      await user.click(within(tools()).getByRole('link', { name: i18n.t('views.terminal.title') }));
      expect(await screen.findByText('Terminal ready')).toBeVisible(); expectNavigation();
    });
  }

  it(`keeps a completed empty discovery distinct from missing facts and recovers an explicit Look again (${locale}/${theme})`, async () => {
    await setLocale(locale);
    const full = browserSnapshot();
    wire.state.answer = { ...full, support: { ...full.support, programs: [] }, presets: full.presets.map((preset) => ({ ...preset, available: false, reason: 'No browser found' })) };
    const user = userEvent.setup(); draw(theme);
    expect(await screen.findByText(i18n.t('browser.none.title', { environment: environment.name }))).toBeVisible();
    expect(screen.queryByText(missingPrograms)).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: i18n.t('browser.list.open') })).not.toBeInTheDocument();
    expectNavigation();
    const { support: _support, ...missingSupport } = full;
    wire.state.answer = missingSupport;
    await user.click(screen.getByRole('button', { name: i18n.t('browser.none.lookAgain') }));
    expect(await screen.findByText(missingPrograms)).toBeVisible();
    expect(wire.read).toHaveBeenCalledTimes(2); expectNavigation();
    wire.state.answer = full;
    await user.click(screen.getByRole('button', { name: i18n.t('browser.none.lookAgain') }));
    expect(await screen.findByRole('button', { name: i18n.t('browser.list.open') })).toBeEnabled();
    expect(screen.queryByText(missingPrograms)).not.toBeInTheDocument();
    expect(wire.read).toHaveBeenCalledTimes(3);
    expect(wire.read).toHaveBeenLastCalledWith('/environments/{id}/browsers', { params: { path: { id: environment.id }, query: { look: true } } });
    expectNavigation();
  });

  it(`keeps a cold row-only live answer pending until the full browser answer arrives (${locale}/${theme})`, async () => {
    await setLocale(locale);
    let accept: (value: Reply) => void = () => { throw new Error('The browser read must start first'); };
    wire.read.mockImplementationOnce(() => new Promise<Reply>((resolve) => { accept = resolve; }));
    const cache = draw(theme);
    await waitFor(() => expect(wire.read).toHaveBeenCalledTimes(1));
    receive(cache, { browsers: [], profiles: [] });
    expect(cache.getQueryData(browserKeys.list(environment.id))).toBeUndefined();
    expect(cache.getQueryState(browserKeys.list(environment.id))?.status).toBe('pending');
    expectNavigation(); expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    const full = browserSnapshot();
    await act(async () => { accept(reply(full)); await Promise.resolve(); });
    expect(await screen.findByRole('button', { name: i18n.t('browser.list.open') })).toBeEnabled();
    const profiles = [{ name: 'saved', size_bytes: null, used_at: null, browser: null }];
    receive(cache, { browsers: [], profiles });
    expect(await screen.findByText('saved')).toBeVisible();
    expect(cache.getQueryData(browserKeys.list(environment.id))).toEqual({ ...full, profiles });
    expectNavigation(); expect(wire.read).toHaveBeenCalledTimes(1);
  });

  it(`keeps Tools navigation, neighbours and Retry outside a failed browser picture (${locale}/${theme})`, async () => {
    await setLocale(locale); wire.state.broken = true;
    wire.state.answer = { ...browserSnapshot(), browsers: [runningBrowser()], limit: { open: 1, max: 4 } };
    const user = userEvent.setup(); draw(theme);
    expect(await screen.findByRole('alert')).toHaveTextContent(i18n.t('ui.error.partFailed'));
    expectNavigation();
    await user.click(within(tools()).getByRole('link', { name: i18n.t('views.terminal.title') }));
    expect(await screen.findByText('Terminal ready')).toBeVisible();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument(); expectNavigation();
    wire.state.broken = false;
    await user.click(within(tools()).getByRole('link', { name: i18n.t('browser.tab') }));
    expect(await screen.findByRole('alert')).toHaveTextContent(i18n.t('ui.error.partFailed'));
    await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    expect(await screen.findByText('Browser picture')).toBeVisible();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument(); expectNavigation();
    expect(wire.read).toHaveBeenCalledTimes(1);
  });
}
