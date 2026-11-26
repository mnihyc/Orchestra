/** Page sections remain inside their page while the global navigation keeps the chosen org. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type * as Router from '@tanstack/react-router';
import type * as NodesApi from '@/api/nodes';
import type * as LiveModule from '@/realtime/live';
import { act, cleanup, fireEvent, screen, within, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { AnchorHTMLAttributes, ReactNode } from 'react';
import { useState } from 'react';
import userEvent from '@testing-library/user-event';
import { LiveThreadStateIcon } from '@/features/workbench/ThreadIcon';
import { client } from '@/api/client';
import { Sidebar } from '@/features/navigation/Sidebar';
import { navigationRoot } from '@/features/navigation/currentOrg';
import { AreaSections } from '@/features/navigation/AreaSections';
import { helpSearch } from '@/features/help/navigation';
import { meKey } from '@/api/auth';
import { adminModeKey } from '@/api/adminMode';
import { orgsKey, orgBySlugQuery, projectQuery, topicQuery } from '@/api/nodes';
import type { Project, Topic } from '@/generated/api-types';
import { SettingsFrame } from '@/features/settings/Frame';
import { placeSettingsSections, settingsNavigation, type PlaceSettingsNavigation } from '@/features/settings/navigation';
import { ViewOpeningContext } from '@/features/workbench/viewOpening';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { useScreenHeading,clearScreenHeading } from '@/shell/screen';
import { hierarchyFeed } from '@/api/hierarchyFeed';
import { treeWriter,useTreeStore } from '@/realtime/stores/tree';
import { treeNode } from '../doubles/tree-node';
import { useSelectedOrg } from '@/shell/org';
import { me, org, userRef } from '../doubles/fixtures';
import { show } from './support';

const world = vi.hoisted(() => {
  const search: Record<string, unknown> = {};
  return { path: '/settings/profile', search, pages:vi.fn<(root:string,selection:NodesApi.TreeSelection,signal?:AbortSignal)=>Promise<NodesApi.TreeReply>>(), tree: vi.fn(), chats: vi.fn(), followed: vi.fn() };
});
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(),
  useRouterState: ({ select }: { select: (state: { location: { pathname: string; search: Record<string, unknown> } }) => unknown }) => select({ location: { pathname: world.path, search: world.search } }),
  Link: ({ to, search: selected, params, children, activeOptions: _activeOptions, onClick, ...props }: AnchorHTMLAttributes<HTMLAnchorElement> & { to: string; search?: Record<string, string | number | boolean | undefined>; params?: Record<string, string>; children?: ReactNode; activeOptions?: unknown }) => {
    const query = new URLSearchParams(Object.entries(selected ?? {}).flatMap(([key, value]): [string, string][] => value === undefined ? [] : [[key, String(value)]])).toString();
    const href = `${to.replace(/\$(\w+)/g, (_match: string, key: string) => params?.[key] ?? '')}${query === '' ? '' : `?${query}`}`;
    return <a {...props} href={href} onClick={(event) => { onClick?.(event); event.preventDefault(); world.followed(href); }}>{children}</a>;
  },
}));
vi.mock('@/api/nodes',async(load)=>({...await load<typeof NodesApi>(),fetchTreePage:world.pages}));
vi.mock('@/realtime/live', async (load) => {
  const actual = await load<typeof LiveModule>();
  return { ...actual, useOrgTreeFeed: world.tree };
});
vi.mock('@/features/navigation/counts', async (load) => ({ ...await load<object>(), useUnreadChats: () => { world.chats(); return 0; } }));

const appearance = useAppearance.getState();
const person = { ...me(1, 'ada', 'Ada'), is_system_admin: true };
const remembered = org('team', 'acme', 'Acme', userRef(person));
const department = org('department', 'research', 'Research', userRef(person), { address: 'acme.research', org_id: remembered.id, parent_id: remembered.id });
const project: Project = { ...remembered, id: 'project', kind: 'project', name: 'Web', parent_id: remembered.id, topic_count: 1,
  path: [...remembered.path, { id: 'project', kind: 'project', name: 'Web', redacted: false }], access: { ...remembered.access, permissions: ['project.view'] } };
const topic: Topic = { ...remembered, id: 'topic', kind: 'topic', name: 'Sign in', parent_id: project.id, status: 'open', done_at: null, thread_display_state: null,
  path: [...project.path, { id: 'topic', kind: 'topic', name: 'Sign in', redacted: false }], access: { ...remembered.access, permissions: ['topic.view'] },
  thread_count: 0, attention: { running: 0, waiting_approval: 0, blocked: 0 }, work_attention: { uncommitted: 0, unpushed: 0, failed: 0 },
  environments: [], default_environment_id: null, last_activity_at: null, seq: 0 };
const clients: QueryClient[] = [];
function draw(ui: ReactNode = <Sidebar />, asAdmin = true) {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } }); clients.push(cache);
  cache.setQueryData(meKey, person); cache.setQueryData(orgsKey, [remembered]);
  cache.setQueryData(adminModeKey, { as_admin: asAdmin, available: true });
  cache.setQueryData(orgBySlugQuery(remembered.address).queryKey, remembered);
  cache.setQueryData(orgBySlugQuery(department.address).queryKey, department);
  cache.setQueryData(projectQuery(project.id).queryKey, project);
  cache.setQueryData(topicQuery(topic.id).queryKey, topic);
  useSelectedOrg.setState({ orgId: remembered.id });
  return show(<QueryClientProvider client={cache}>{ui}</QueryClientProvider>);
}
afterEach(async () => { cleanup();treeWriter.clear();clearScreenHeading();world.pages.mockReset(); for (const cache of clients.splice(0)) cache.clear(); useSelectedOrg.setState({ orgId: null }); useAppearance.setState(appearance); world.search = {}; vi.clearAllMocks(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`keeps the chosen org navigation across local page sections at ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    for (const [path, label] of [
      ['/settings/profile', i18n.t('settings.personal.nav'), '/settings/security'],
      ['/admin/overview', i18n.t('admin.nav.label'), '/admin/settings'],
      ['/help', i18n.t('workbench.areaNavigation.help.sections'), '/help?part=keys'],
      ['/usage', i18n.t('usagePerf.views'), '/usage?view=performance'],
    ] as const) {
      world.path = path; const view = draw();
      const nav = screen.getByRole('navigation', { name: i18n.t('views.navigation.title') });
      expect(within(nav).getByRole('button', { name: i18n.t('navigation.projects') })).toBeVisible();
      expect(screen.queryByRole('navigation', { name: label })).not.toBeInTheDocument();
      // The order of the tree is named once both of its parts have arrived; the remembered org is followed before that.
      expect(world.tree.mock.calls.some(([id]) => id === remembered.id)).toBe(true);
      expect(world.tree.mock.calls.every(([id]) => id === remembered.id)).toBe(true);
      view.unmount();
    }
  });
}

it('keeps a usage section’s scope, dates and filters in its actual destination', () => {
  world.path = '/usage'; world.search = { scope: 'node:00000000-0000-4000-8000-000000000001', range: 'last_week', model: 'selected-model', view: 'performance', show: 'speed' };
  draw(<AreaSections area="usage" />); const nav = screen.getByRole('navigation', { name: i18n.t('usagePerf.views') });
  const requests = within(nav).getByRole('link', { name: i18n.t('usagePerf.viewNames.requests') });
  const href = requests.getAttribute('href');
  if (href === null) throw new Error('The section has no native destination');
  const query = new URL(href, 'https://orch.test').searchParams;
  expect(query.get('scope')).toBe(world.search.scope); expect(query.get('range')).toBe('last_week');
  expect(query.get('model')).toBe('selected-model'); expect(query.get('view')).toBe('requests'); expect(query.has('show')).toBe(false);
  fireEvent.click(requests); expect(world.followed).toHaveBeenCalledWith(href);
});

it('retains the chosen org for page-local navigation, including articles and nested settings', () => {
  for (const path of ['/settings/profile', '/settings/security/recovery', '/admin/settings', '/help', '/help/approvals', '/usage']) {
    expect(navigationRoot(path, [remembered], remembered.id, { heading: remembered.id })).toBe(remembered.id);
  }
  expect(navigationRoot('/org/acme', [remembered], null, {})).toBe(remembered.org_id);
  expect(helpSearch({ part: 'keys' })).toEqual({ part: 'keys' });
  expect(helpSearch({ part: 'unknown' })).toEqual({});
});

function PlaceFrame({ scope, name }: { scope: PlaceSettingsNavigation; name: string }) {
  const [shown, setShown] = useState(true);
  return <><button onClick={() => setShown(!shown)}>Toggle navigation</button>
    <div hidden={!shown}><ViewOpeningContext.Provider value={shown ? 'settings-opening' : null}><Sidebar /></ViewOpeningContext.Provider></div>
    <SettingsFrame title="Settings section" label={i18n.t('settings.org.sections', { name })} current={scope.current} items={placeSettingsSections(scope, i18n.t)}>
      <input aria-label="Retained setting draft" defaultValue="" />
    </SettingsFrame>
  </>;
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) it(`puts every place's settings in one visible navigation and retains the fallback draft at ${locale}/${theme}`, async () => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  const user = userEvent.setup();
  for (const [path, search, name] of [
    ['/org/acme/settings/defaults', {}, remembered.name],
    ['/org/acme.research/settings/members', {}, department.name],
    ['/project/project/settings', { section: 'tags' }, project.name],
    ['/topic/topic', { tab: 'settings', section: 'defaults' }, topic.name],
  ] as const) {
    world.path = path; world.search = search;
    const scope = settingsNavigation(path, search);
    if (scope === null || scope.kind === 'personal' || scope.kind === 'system') throw new Error('The place has no settings navigation');
    const view = draw(<PlaceFrame scope={scope} name={name} />);
    const label = i18n.t('settings.org.sections', { name });
    const nav = screen.getByRole('navigation', { name: label });
    expect(screen.getAllByRole('navigation', { name: label })).toHaveLength(1);
    expect(nav.closest('[data-settings-frame]')).not.toBeNull();
    expect(nav.querySelectorAll('[aria-current="page"]')).toHaveLength(1);
    expect(nav.querySelector(`[data-section-id="${scope.current}"]`)).toHaveAttribute('aria-current', 'page');
    expect(screen.getByRole('navigation', { name: i18n.t('views.navigation.title') })).toBeVisible();
    expect(world.tree.mock.calls.some(([id]) => id === remembered.id)).toBe(true);
    const draft = screen.getByRole('textbox', { name: 'Retained setting draft' });
    await user.type(draft, 'Unsaved words');
    await user.click(screen.getByRole('button', { name: 'Toggle navigation' }));
    const fallback = screen.getByRole('navigation', { name: label });
    expect(fallback.closest('[data-settings-frame]')).not.toBeNull();
    expect(fallback.querySelectorAll('[aria-current="page"]')).toHaveLength(1);
    expect(screen.getByRole('textbox', { name: 'Retained setting draft' })).toBe(draft);
    expect(draft).toHaveValue('Unsaved words');
    view.unmount();
  }
});

it('keeps Settings explicit, preserves exact setting links and org aliases, and lets plain topic links out', () => {
  expect(settingsNavigation('/topic/topic', {})).toBeNull();
  expect(settingsNavigation('/topic/topic', { tab: 'threads' })).toBeNull();
  expect(settingsNavigation('/topic/topic', { tab: 'settings', setting: 'agent.model' })).toEqual({ kind: 'topic', id: 'topic', current: 'defaults' });
  expect(settingsNavigation('/project/project/settings', { setting: 'agent.model' })).toEqual({ kind: 'project', id: 'project', current: 'defaults' });
  expect(settingsNavigation('/org/acme/settings/destinations')).toEqual({ kind: 'org', address: 'acme', current: 'channels' });
  expect(navigationRoot('/topic/topic', [remembered], remembered.id, { node: remembered.id })).toBe(remembered.id);
  expect(navigationRoot('/topic/topic', [remembered], remembered.id, { node: remembered.id })).toBe(remembered.id);
});


it('reveals a deep address after its held path read meets the first root answer and later live changes',async()=>{
  world.path='/topic/topic';
  const root=treeNode({id:remembered.id,kind:'org',parent_id:null,name:remembered.name,folded:{has_children:true,small_tree:false,waiting:0,unread:false,mentions:0,urgency:null}});
  const folder=treeNode({id:project.id,kind:'project',parent_id:root.id,name:project.name,folded:{has_children:true,waiting:0,unread:false,mentions:0,urgency:null}});
  const leaf=treeNode({id:topic.id,kind:'topic',parent_id:folder.id,name:topic.name});
  let finish!:(page:NodesApi.TreeReply)=>void;
  const held=new Promise<NodesApi.TreeReply>((resolve)=>{finish=resolve;});
  world.pages.mockImplementation((_root,selection)=>selection.path_to!==undefined?held:Promise.resolve({nodes:selection.parent_id===folder.id?[leaf]:[],ancestors:[root,folder],hidden:[],seq:20,since:null,complete:true,feed:hierarchyFeed()}));
  function Address(){useScreenHeading({title:leaf.name,nodeId:leaf.id});return <Sidebar/>;}
  draw(<Address/>);
  await waitFor(()=>expect(world.pages.mock.calls.some(([,selection])=>selection.path_to===leaf.id)).toBe(true));
  act(()=>treeWriter.branch(root.id,root.id,[folder],10,[],[root]));
  act(()=>treeWriter.branch(root.id,root.id,[{...folder,name:'Web current',tree_seq:20}],20,[],[root]));
  await act(async()=>{finish({nodes:[root,folder,leaf],ancestors:[],hidden:[],seq:10,since:null,complete:true,feed:hierarchyFeed()});await Promise.resolve();});
  await waitFor(()=>expect(useTreeStore.getState().byOrg.get(root.id)?.nodes.has(leaf.id)).toBe(true));
  expect(await screen.findByRole('treeitem',{name:new RegExp(leaf.name)})).toBeVisible();
  expect(useTreeStore.getState().byOrg.get(root.id)?.nodes.get(folder.id)?.name).toBe('Web current');
});

it('shares one addressed path between the visible thread tab and navigation across the first root answer', async () => {
  const session = client.session.getState();
  client.session.setState({ status: 'signed_in', userId: person.id });
  const root = treeNode({ id: remembered.id, kind: 'org', parent_id: null, name: remembered.name });
  const parent = treeNode({ id: topic.id, kind: 'topic', parent_id: root.id, name: topic.name });
  const leaf = treeNode({ id: 'visible-thread', kind: 'thread', parent_id: parent.id, name: 'Selected thread' });
  world.path = `/thread/${leaf.id}`;
  let answer!: (page: NodesApi.TreeReply) => void;
  const pending = new Promise<NodesApi.TreeReply>((resolve) => { answer = resolve; });
  const page = { nodes: [root, parent, leaf], hidden: [], seq: 10, since: null, complete: true, feed: hierarchyFeed() };
  world.pages.mockImplementation((_root, selection) => selection.path_to ? pending : Promise.resolve(page));
  function Address() {
    useScreenHeading({ title: leaf.name, nodeId: leaf.id, path: [root, parent, leaf].map(({id, name, kind}) => ({id, name, kind, redacted: false})) });
    return <><LiveThreadStateIcon display={null} threadId={leaf.id} rootId={root.id} /><Sidebar /></>;
  }
  const paths = () => world.pages.mock.calls.filter(([, selection]) => selection.path_to === leaf.id);
  try {
    const view = draw(<Address />);
    await waitFor(() => expect(paths().length).toBeGreaterThan(0));
    act(() => treeWriter.branch(root.id, root.id, [parent], 10, [], [root]));
    // The thread opening already supplied the readable parent identities.
    await waitFor(() => expect(world.pages.mock.calls.some(([, selection]) => selection.parent_id === parent.id)).toBe(true));
    await act(async () => { answer(page); await pending; });
    await waitFor(() => expect(useTreeStore.getState().byOrg.get(root.id)?.nodes.get(leaf.id)?.name).toBe(leaf.name));
    expect(paths()).toHaveLength(1);
    view.unmount();
  } finally { client.session.setState(session); }
});
