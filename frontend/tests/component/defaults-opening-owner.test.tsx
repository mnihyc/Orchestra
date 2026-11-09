/** The actual content metadata owner admits one visible Defaults/Chat opening before the first Node read. */
import { QueryClientProvider, useQuery } from '@tanstack/react-query';
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useLayoutEffect, type AnchorHTMLAttributes, type ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client, RENEW_BEFORE_END_MS } from '@/api/client';
import { nodeQuery } from '@/api/nodes';
import { LiveQueryClient, queryClient } from '@/api/query';
import { ChatPanel } from '@/features/chat/ChatPanel';
import { ProjectSettings, ScopeEditor } from '@/features/settings/ScopeSettings';
import { makeGroup, makeTab, type Tab } from '@/features/workbench/layout';
import { FocusedContentMetadata } from '@/features/workbench/PageView';
import { useWorkbench } from '@/features/workbench/store';
import { ViewInstanceContext } from '@/features/workbench/instance';
import { clearViewOpenings, forgetViewOpening, openingForView, showViewOpening, ViewOpening } from '@/features/workbench/viewOpening';
import { i18n } from '@/i18n';
import type { NodeChatOpening, NodeDefaultsOpening, Provider, ResolvedSetting, SettingDef } from '@/generated/api-types';
import { clearScreenHeading } from '@/shell/screen';
import { chatWriter } from '@/realtime/stores/chat';
import { ACME } from '../doubles/fixtures';
import { nodeChatOpening } from '../doubles/screen-openings';
import { World } from '../doubles/world';
import { show } from './support';
import { deferred } from '../unit/continuation-fixtures';

const route = vi.hoisted((): { location: { pathname: string; search: Record<string, unknown>; searchStr: string; href: string } } => ({ location: { pathname: '/', search: {}, searchStr: '', href: '/' } }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), useLocation: () => route.location,
  Link: ({ to, params, children, activeOptions: _activeOptions, ...props }: AnchorHTMLAttributes<HTMLAnchorElement> & { to: string; params?: Record<string, string>; children?: ReactNode; activeOptions?: unknown }) => <a {...props} href={to.replace(/\$(\w+)/g, (_match: string, key: string) => params?.[key] ?? '')}>{children}</a>,
  useRouterState: ({ select }: { select: (state: { location: typeof route.location }) => unknown }) => select({ location: route.location }),
  useRouter: () => ({ state: { location: route.location }, history: { replace: vi.fn() } }) }));
vi.mock('@/shell/FrameReadiness', () => ({ useFrameReady: () => true }));
vi.mock('@/features/chat/ChatView', () => ({ ChatView: ({ chatId }: { chatId: string }) => <output aria-label="Visible chat">{chatId}</output> }));

const world = new World(), ada = world.people.get('ada')!;
const providerId = '00000000-0000-4000-8022-000000000001';
const savedSession = client.session.getState(), savedWorkbench = useWorkbench.getState();
let cache: LiveQueryClient, calls: URL[], metadata: NodeDefaultsOpening, native: NodeChatOpening, content: Tab, chat: Tab;
let openingReply: ((url: URL) => Response | Promise<Response>) | undefined;
function Slot({ tab }: { tab: Tab }) { useLayoutEffect(() => showViewOpening(tab.id, openingForView(tab.id)), [tab]); return null; }
function Editor({ id }: { id: string }) {
  const query = useQuery({ ...nodeQuery(id), enabled: false });
  const node = query.data;
  return <>{node !== undefined && <output aria-label="Visible place">{node.name}</output>}{id === ACME.web ? <ProjectSettings projectId={id} section="defaults" /> : node?.kind === 'topic' ? <ScopeEditor node={node} /> : null}</>;
}
function draw({ contentShown = true, chatShown = true }: { contentShown?: boolean; chatShown?: boolean } = {}) {
  return show(<QueryClientProvider client={cache}>
    <FocusedContentMetadata />
    <ViewOpening view={content.id} route={openingForView(content.id)}><ViewInstanceContext.Provider value={{ id: content.id, root: { current: null }, target: null, headingOwner: content.id, content: true, focusedContent: true }}><Editor id={metadata.node.id} /></ViewInstanceContext.Provider></ViewOpening>
    <ViewOpening view={chat.id} route={openingForView(chat.id)}><ViewInstanceContext.Provider value={{ id: chat.id, root: { current: null }, target: null, headingOwner: content.id, content: false, focusedContent: false }}><ChatPanel tab={chat} area="main" /></ViewInstanceContext.Provider></ViewOpening>
    {contentShown && <Slot tab={content} />}{chatShown && <Slot tab={chat} />}
  </QueryClientProvider>);
}
async function mounted(options?: Parameters<typeof draw>[0]) { await act(async () => { draw(options); await Promise.resolve(); }); }
async function prepare(kind: 'project' | 'topic', filtered = false) {
  const id = kind === 'project' ? ACME.web : ACME.fixLogin;
  const pathname = `/${kind}/${id}${kind === 'project' ? '/settings' : ''}`;
  const search: Record<string, string> = { ...(kind === 'topic' ? { tab: 'settings' } : {}), section: 'defaults', ...(filtered ? { setting: 'agent.default_provider_id' } : {}) };
  route.location = { pathname, search, searchStr: `?${new URLSearchParams(search)}`, href: `${pathname}?${new URLSearchParams(search)}` };
  content = makeTab('page', { href: route.location.href }); chat = makeTab('chat');
  const layout = { ...savedWorkbench.layout, main: { kind: 'split' as const, id: 'defaults-and-chat', direction: 'row' as const,
    children: [makeGroup([content]), makeGroup([chat])], sizes: [65, 35] } };
  useWorkbench.setState({ layout, activeContent: content.id, focusedMainTab: content.id, focusedTab: content.id });
  const node = world.places.node(ada, id);
  native = await nodeChatOpening(world, ada, id, new URLSearchParams());
  const registry = (await world.requestFeeds.read(ada, '/settings/registry')).body as SettingDef[];
  const provider = (await world.requestFeeds.read(ada, `/providers/${providerId}`)).body as Provider;
  const setting: ResolvedSetting = { key: 'agent.default_provider_id', value: providerId, own: null, source: { scope: 'system', node: null }, locked_by: null, writable: true, parts: [] };
  metadata = { node, registry: registry.filter((def) => def.key === setting.key), registry_problem: null, registry_retry_after: null,
    settings: [setting], settings_problem: null, settings_retry_after: null, secrets: [], secrets_problem: null, secrets_retry_after: null,
    providers: [{ setting: setting.key, id: providerId, provider, problem: null, retry_after: null }] };
  vi.stubGlobal('fetch', (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(input, init), url = new URL(request.url); calls.push(url);
    if (url.pathname === `/api/v1/nodes/${id}/chat-opening`) return Promise.resolve(openingReply?.(url) ?? Response.json({ ...native, ...(url.searchParams.get('defaults') === 'true' ? { defaults: metadata } : {}) }));
    if (url.pathname === `/api/v1/nodes/${id}`) return Promise.resolve(Response.json(node));
    if (url.pathname === '/api/v1/settings/registry') return Promise.resolve(Response.json(metadata.registry));
    if (url.pathname === `/api/v1/nodes/${id}/settings`) return Promise.resolve(Response.json(metadata.settings));
    if (url.pathname === `/api/v1/nodes/${id}/secrets`) return Promise.resolve(Response.json([]));
    if (url.pathname === `/api/v1/providers/${providerId}`) return Promise.resolve(Response.json(provider));
    throw new Error(`Unexpected Defaults read: ${request.method} ${url.pathname}`);
  });
}
const count = (path: string) => calls.filter((url) => url.pathname === path).length;
beforeEach(() => {
  calls = []; openingReply = undefined; queryClient.clear(); clearViewOpenings(); clearScreenHeading(); chatWriter.clear();
  client.adopt({ access_token: 'synthetic-defaults-owner', access_expires_at: new Date(Date.now() + RENEW_BEFORE_END_MS + 60_000).toISOString(), user: ada.me });
  cache = new LiveQueryClient({ defaultOptions: { ...queryClient.getDefaultOptions(), queries: { ...queryClient.getDefaultOptions().queries, retry: false } } });
});
afterEach(() => { cleanup(); cache.clear(); queryClient.clear(); clearViewOpenings(); clearScreenHeading(); chatWriter.clear(); vi.unstubAllGlobals(); client.session.setState(savedSession); useWorkbench.setState(savedWorkbench); });

for (const kind of ['project', 'topic'] as const) {
  it(`admits cold ${kind} metadata and its real visible Chat before the FIRST standalone Node GET`, async () => {
    await prepare(kind); await mounted();
    await screen.findByLabelText('Visible chat');
    await screen.findByLabelText('Visible place');
    await waitFor(() => expect(document.querySelector('[data-setting-row]')).not.toBeNull());
    await act(async () => { await Promise.resolve(); });
    expect(count(`/api/v1/nodes/${metadata.node.id}`)).toBe(0);
    expect(count(`/api/v1/nodes/${metadata.node.id}/chat-opening`)).toBe(1);
    expect(calls[0]?.pathname).toBe(`/api/v1/nodes/${metadata.node.id}/chat-opening`);
    expect(calls[0]?.searchParams.get('defaults')).toBe('true');
    expect(count('/api/v1/settings/registry')).toBe(0); expect(count(`/api/v1/nodes/${metadata.node.id}/settings`)).toBe(0);
    expect(count(`/api/v1/nodes/${metadata.node.id}/secrets`)).toBe(0); expect(count(`/api/v1/providers/${providerId}`)).toBe(0);
  });

  it(`keeps canonical ${kind} readers and makes NO Chat opening when Chat has no visible slot`, async () => {
    await prepare(kind); await mounted({ chatShown: false }); await screen.findByLabelText('Visible place');
    await screen.findByRole('button', { name: i18n.t('settings.secrets.showNames') });
    expect(count(`/api/v1/nodes/${metadata.node.id}/secrets`)).toBe(0);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('settings.secrets.showNames') }));
    await waitFor(() => expect(count(`/api/v1/nodes/${metadata.node.id}/secrets`)).toBe(1));
    expect(count(`/api/v1/nodes/${metadata.node.id}`)).toBe(1); expect(count(`/api/v1/nodes/${metadata.node.id}/chat-opening`)).toBe(0);
    expect(count('/api/v1/settings/registry')).toBe(1); expect(count(`/api/v1/nodes/${metadata.node.id}/settings`)).toBe(1);
    expect(screen.queryByLabelText('Visible chat')).not.toBeInTheDocument();
  });
}

it('does not opt a hidden Defaults body into metadata just because a following Chat remains visible', async () => {
  await prepare('topic'); await mounted({ contentShown: false }); await screen.findByLabelText('Visible chat');
  expect(count(`/api/v1/nodes/${metadata.node.id}`)).toBe(1);
  expect(calls.filter((url) => url.searchParams.get('defaults') === 'true')).toEqual([]);
  expect(count('/api/v1/settings/registry')).toBe(0); expect(count(`/api/v1/nodes/${metadata.node.id}/settings`)).toBe(0);
});

it('does not preload filtered-out secret or provider readers from an initially filtered Defaults editor', async () => {
  await prepare('topic', true); await mounted(); await screen.findByLabelText('Visible chat');
  await waitFor(() => expect(document.querySelector('[data-setting-row]')).not.toBeNull());
  const requested = calls.find((url) => url.pathname.endsWith('/chat-opening'));
  expect(requested?.searchParams.get('defaults')).toBeNull();
  expect(requested?.searchParams.get('secret_names')).toBeNull(); expect(requested?.searchParams.get('default_provider_name')).toBeNull();
  expect(count('/api/v1/settings/registry')).toBe(1); expect(count(`/api/v1/nodes/${metadata.node.id}/settings`)).toBe(1);
  expect(count(`/api/v1/nodes/${metadata.node.id}/secrets`)).toBe(0);
  expect(count(`/api/v1/providers/${providerId}`)).toBe(0);
});

for (const filter of ['local search', 'onlyHere'] as const) it(`does not re-request hidden metadata suppliers after ${filter} and opening invalidation`, async () => {
  await prepare('topic'); await mounted(); await screen.findByLabelText('Visible chat');
  await waitFor(() => expect(document.querySelector('[data-setting-row]')).not.toBeNull());
  if (filter === 'local search') await userEvent.type(screen.getByRole('searchbox'), 'a value outside this fixture');
  else await userEvent.click(screen.getByRole('switch', { name: i18n.t('settings.editor.onlyHere') }));
  await waitFor(() => expect(document.querySelector('[data-setting-row]')).toBeNull());
  const before = calls.length;
  await act(async () => { await cache.invalidateQueries({ queryKey: ['node-chat-opening', metadata.node.id], exact: true }); });
  const after = calls.slice(before); expect(after).toHaveLength(1);
  expect(after[0]?.pathname).toBe(`/api/v1/nodes/${metadata.node.id}/chat-opening`);
  for (const flag of ['defaults', 'secret_names', 'default_provider_name', 'embeddings_provider_name']) expect(after[0]?.searchParams.get(flag)).toBeNull();
  expect(document.querySelector('[data-setting-row]')).toBeNull();
  if (filter === 'local search') expect(screen.getByRole('searchbox')).toHaveValue('a value outside this fixture');
  else expect(screen.getByRole('switch', { name: i18n.t('settings.editor.onlyHere') })).toBeChecked();
});

it('recovers the canonical visible editor when a cancelled first opening loses its Chat slot', async () => {
  await prepare('topic'); const started = deferred<undefined>(), released = deferred<Response>();
  openingReply = () => { started.resolve(undefined); return released.promise; };
  await mounted(); await started.promise;
  await act(async () => { forgetViewOpening(chat.id); await cache.cancelQueries({ queryKey: ['node-chat-opening', metadata.node.id], exact: true }); });
  await screen.findByLabelText('Visible place');
  await waitFor(() => expect(document.querySelector('[data-setting-row]')).not.toBeNull());
  expect(count(`/api/v1/nodes/${metadata.node.id}`)).toBe(1);
  expect(count(`/api/v1/nodes/${metadata.node.id}/settings`)).toBe(1);
  expect(screen.queryByLabelText('Visible chat')).not.toBeInTheDocument();
  await act(async () => { released.resolve(Response.json({ ...native, defaults: { ...metadata, node: { ...metadata.node, name: 'Late retired name' } } })); await Promise.resolve(); });
  expect(screen.getByLabelText('Visible place')).toHaveTextContent(metadata.node.name);
  expect(screen.getByLabelText('Visible place')).not.toHaveTextContent('Late retired name');
});
