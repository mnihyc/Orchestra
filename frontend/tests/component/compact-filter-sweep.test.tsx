/** Compact screen selectors retain their meaning, visible filter labels and existing acts in every appearance. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Router from '@tanstack/react-router';
import type * as ResourceListModule from '@/features/resources/ResourceList';
import type { EverythingFilter } from '@/api/everything';
import type * as ResourcesApi from '@/api/resources';
import type * as AdminApi from '@/api/admin';
import type * as AuthApi from '@/api/auth';
import type * as UsersApi from '@/api/users';
import type * as PersonalApi from '@/api/personal';
import type * as Day100Api from '@/api/day100';
import type * as NodesApi from '@/api/nodes';
import type * as Places from '@/features/navigation/places';
import type * as PlaceLabels from '@/features/navigation/placeLabels';
import type * as SettingRows from '@/features/admin/SettingRows';
import type * as Grouping from '@/features/inbox/away/Grouping';
import type * as PageModule from '@/shell/Page';
import { activityQuery } from '@/api/nodes';
import { Users } from '@/features/admin/Users';
import { Runtimes } from '@/features/admin/Runtimes';
import { Inbox } from '@/features/inbox/Inbox';
import { ActivityFeed } from '@/features/org/Activity';
import { Project } from '@/features/project/Project';
import { EnvironmentList } from '@/features/resources/environments/EnvironmentList';
import { AgentList } from '@/features/resources/agents/AgentList';
import { HostList } from '@/features/resources/hosts/HostList';
import { ProviderList } from '@/features/resources/providers/ProviderList';
import { InterfaceSettings } from '@/features/settings/personal/Interface';
import { Security } from '@/features/settings/personal/Security';
import type { Agent, Project as ProjectData } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { id, me, meta, org, userRef } from '../doubles/fixtures';
import { show } from './support';
import { retiredCopy } from './retired-copy';
import { harnessName } from '../doubles/harness-names';

const fixture = vi.hoisted(() => ({ inventory:vi.fn<(filter:EverythingFilter)=>void>(),keyPending: false, saveKeys: vi.fn<(change: unknown) => void>(),
  user: { loaded: true, error: null, edge: 0, reachedEnd: true, nextCursor: null, counters: { unread: 3 } },
}));
function rowsQuery(key: readonly unknown[]) { return { queryKey: key, queryFn: () => Promise.resolve([]) }; }
function pageQuery(key: readonly unknown[]) { return { queryKey: key, queryFn: () => Promise.resolve({ items: [], next_cursor: null }), initialPageParam: null, getNextPageParam: () => undefined }; }
function agentData(): Agent {
  return { id: id(9, 1), name: 'Codex', address: 'system.Codex', harness_kind: 'codex', harness: harnessName('codex'), home: null, owner: null, org_id: null, machine: null,
    access: { administers: false, participating: true, admin_only: [], permissions: ['agent.view'], through: [] },
    disabled: false, profile: {}, descriptor: {}, descriptor_overridden: [], revision: 1,
    created_at: '2026-10-15T12:00:00Z', updated_at: '2026-10-15T12:00:00Z' };
}
function projectData(): ProjectData {
  const home = org(id(1, 1), 'acme', 'Acme', userRef(me(1, 'ada', 'Ada')));
  const projectId = id(2, 1);
  return { ...home, id: projectId, kind: 'project', name: 'Web', org_id: home.id,
    parent_id: home.id, topic_count: 0, project_count: 0,
    path: [...home.path, { id: projectId, kind: 'project', name: 'Web', redacted: false }],
    access: { ...home.access, permissions: ['project.view', 'project.interact'] } };
}

vi.mock('@tanstack/react-router', async (original) => ({ ...await original<typeof Router>(),
  Link: ({ children, to }: { children: ReactNode; to: string }) => <a href={to}>{children}</a>,
  useNavigate: () => vi.fn(),
}));
vi.mock('@/shell/Page', async (original) => ({ ...await original<typeof PageModule>(), boxClasses: '', Page: ({ children }: { children: ReactNode }) => <main>{children}</main>,
  PlaceMark: ({ children }: { children: ReactNode }) => <span>{children}</span>,
  PageHeader: ({ title, lead, facts, actions }: { title: string; lead?: ReactNode; facts?: ReactNode; actions?: ReactNode }) => <header><h1>{title}</h1>{lead}<div>{facts}</div>{actions}</header>,
  Section: ({ title, lead, children }: { title: string; lead?: ReactNode; children?: ReactNode }) => <section><h2>{title}</h2>{lead}{children}</section>,
}));
vi.mock('@/shell/screen', () => ({ useScreenHeading: vi.fn() }));
vi.mock('@/shell/org', () => ({ selectOrg: vi.fn() }));
vi.mock('@/features/resources/ResourceNav',()=>({ResourceNav:()=>null}));
vi.mock('@/features/resources/trail',()=>({rememberList:vi.fn(),useResourceHeading:vi.fn()}));
vi.mock('@/features/resources/ResourceList', async (original) => {
  const actual=await original<typeof ResourceListModule>();
  const ResourceFrame=actual.ResourceList;
  return {...actual,ResourceList:(props:Parameters<typeof ResourceFrame>[0])=>{
    if(props.kind==='providers')return <ResourceFrame {...props}/>;
    if(props.kind!=='agents')return <section>{props.filters}</section>;
    const offered=typeof props.columns==='function'?props.columns([agentData()]):props.columns;
    return <section>{props.filters}{offered.find((column)=>column.id==='name')?.cell(agentData(),new Set(['name','state']))}</section>;
  },useSearchText:(text:string)=>[text,vi.fn()]};
});
vi.mock('@/api/everything',()=>({everythingQuery:(_reader:string,_parent:string,filter:EverythingFilter)=>{
  fixture.inventory(filter);return pageQuery(['compact-inventory',filter]);
}}));
vi.mock('@/api/resources', async (original) => ({ ...await original<typeof ResourcesApi>(), machinesQuery: rowsQuery(['compact-machines']), hostsQuery: rowsQuery(['compact-hosts']), harnessKindsQuery: { queryKey: ['compact-harnesses'], queryFn: () => Promise.resolve([{ kind: 'codex' }, { kind: 'claude_code' }]) } }));
vi.mock('@/api/admin', async (original) => ({ ...await original<typeof AdminApi>(), usersQuery: (filter: unknown) => pageQuery(['compact-users', filter]), runtimesQuery: (filter: unknown) => rowsQuery(['compact-runtimes', filter]) }));
vi.mock('@/api/auth', async (original) => ({ ...await original<typeof AuthApi>(), meQuery: { queryKey: ['compact-me'], queryFn: () => Promise.resolve(me(1, 'ada', 'Ada')) }, metaQuery: { queryKey: ['compact-meta'], queryFn: () => Promise.resolve(meta()) } }));
vi.mock('@/api/users', async (original) => ({ ...await original<typeof UsersApi>(), sessionsQuery: rowsQuery(['compact-sessions']) }));
vi.mock('@/api/personal', async (original) => ({ ...await original<typeof PersonalApi>(), meStateQuery: { queryKey: ['compact-state'], queryFn: () => Promise.resolve({}) } }));
vi.mock('@/api/day100', async (original) => ({ ...await original<typeof Day100Api>(), saveProjectView: vi.fn(() => Promise.resolve()) }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof NodesApi>(),
  projectQuery: () => ({ queryKey: ['compact-project'], queryFn: () => Promise.resolve(projectData()) }),
  tagsQuery: () => rowsQuery(['compact-tags']),
  activityQuery: vi.fn((node: string, kinds: unknown, depth: string) => pageQuery(['compact-activity', node, kinds, depth])),
}));
vi.mock('@/features/resources/environments/acts', () => ({ useEnvironmentActs: () => ({ list: [] }) }));
vi.mock('@/features/resources/agents/acts', () => ({ useAgentActs: () => ({ list: [] }) }));
vi.mock('@/features/resources/hosts/acts', () => ({ useHostActs: () => ({ list: [] }) }));
vi.mock('@/features/resources/providers/acts', () => ({ useProviderActs: () => ({ list: [] }) }));
vi.mock('@/features/resources/HomePicker', () => ({useViewedOrg:()=>null,useViewedOrgState:()=>({org:null,load:{isSuccess:true,isPending:false,isError:false}})}));
vi.mock('@/features/resources/harnesses', () => ({ useHarnessTitle: () => (kind: string) => kind === 'codex' ? 'Codex' : kind }));
vi.mock('@/features/navigation/placeLabels', async (original) => ({ ...await original<typeof PlaceLabels>(), useKnownPlaces: () => new Map() }));
vi.mock('@/features/navigation/places', async (original) => ({ ...await original<typeof Places>(), usePlaceLookup: () => new Map() }));
vi.mock('@/features/admin/SettingRows', async (original) => ({ ...await original<typeof SettingRows>(), useSystemSettings: () => ({ find: () => undefined }) }));
vi.mock('@/features/trace/Person', () => ({ useTraceEntry: () => vi.fn(), TracePerson: ({ children }: { children: ReactNode }) => <>{children}</> }));
vi.mock('@/realtime/live', async () => {
  const { createStore } = await import('zustand/vanilla');
  return { useUserFeed: vi.fn(), useOrgTreeFeed: vi.fn(), reloadUserFeed: vi.fn(),
    socket: { subscribe: vi.fn(() => ({ unsubscribe: vi.fn() })), status: createStore(() => ({ phase: 'idle' })) } };
});
vi.mock('@/realtime/stores/user', () => ({ useHeldRows: () => [], useUserStore: <Value,>(read: (value: typeof fixture.user) => Value) => read(fixture.user), userWriter: { older: vi.fn(), found: vi.fn() } }));
vi.mock('@/features/settings/personal/away/live', () => ({ useAwayLive: vi.fn() }));
vi.mock('@/features/settings/personal/away/Pause', () => ({ PauseControls: () => null }));
vi.mock('@/features/inbox/away/Grouping', async (original) => ({ ...await original<typeof Grouping>(), useInboxGrouping: () => ({ value: 'day', choose: vi.fn(), loading: false, saving: false, error: null }) }));
vi.mock('@/features/settings/PersonalFrame', () => ({ PersonalFrame: ({ title, children }: { title: string; children: ReactNode }) => <article><h1>{title}</h1>{children}</article> }));
vi.mock('@/shell/keys', () => ({ useKeyChoices: () => ({ single_key: false }), requestKeyList: vi.fn() }));
vi.mock('@/features/help/KeyList', () => ({ useSaveKeys: () => ({ isPending: fixture.keyPending, variables: fixture.keyPending ? { single_key: true } : undefined, mutate: fixture.saveKeys, error: null }) }));
vi.mock('@/features/access/AccessDialog', () => ({ openAccess: vi.fn() }));
vi.mock('@/features/navigation/NodeMenu', () => ({ NodeMenuButton: () => null }));
vi.mock('@/features/topic/TagPicker', () => ({ TagPicker: () => null }));
vi.mock('@/features/announcements/Composer', () => ({ PlaceAnnouncements: () => null }));
vi.mock('@/features/project/ProjectStar', () => ({ ProjectStar: () => null }));
vi.mock('@/features/project/FolderCards', () => ({ FolderCards: () => null }));
vi.mock('@/features/project/Topics', () => ({ Topics: () => null }));
vi.mock('@/features/project/Activity', () => ({ ProjectActivity: () => null }));
vi.mock('@/features/usage/Board', () => ({ UsageSection: () => null }));
vi.mock('@/features/start/QuickStart', () => ({ QuickStart: () => null }));
vi.mock('@/features/ports/Outside', () => ({ OutsideOpenings: () => null }));
vi.mock('@/features/access/ParticipationFilter', () => ({ ParticipationFilter: () => null }));

const appearance = useAppearance.getState();
beforeEach(() => { fixture.keyPending = false; fixture.inventory.mockClear(); fixture.saveKeys.mockClear(); fixture.user.counters.unread = 3; });
afterEach(async () => { cleanup(); useAppearance.setState(appearance); await setLocale('en'); });

function namedControl(group: HTMLElement, name: string): HTMLButtonElement {
  const button = within(group).getByText(name, { exact: true }).closest('button');
  if (button === null) throw new Error(`No control for ${name}`);
  return button;
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`keeps list/admin options compact and callbacks intact in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme });
    const change = vi.fn<(patch: unknown) => void>();
    const cases = [
      { draw: () => <EnvironmentList search={{}} onSearch={change} />, label: i18n.t('environments.list.showingKinds.all'), name: i18n.t('choiceCatalog.environmentFilter.machine.name'), description: i18n.t('choiceCatalog.environmentFilter.machine.description'), patch: { kind: 'machine' }, field: i18n.t('environments.list.statusFilter') },
      { draw: () => <HostList search={{}} onSearch={change} />, label: i18n.t('hosts.list.stateFilter'), name: i18n.t('choiceCatalog.hostAvailability.closed.name'), description: i18n.t('choiceCatalog.hostAvailability.closed.description'), patch: { state: 'closed' }, field: null },
      { draw: () => <Users search={{}} onSearch={change} />, label: i18n.t('admin.users.status'), name: i18n.t('choiceCatalog.accountState.suspended.name'), description: i18n.t('choiceCatalog.accountState.suspended.description'), patch: { status: 'suspended' }, field: null },
      { draw: () => <Runtimes search={{}} onSearch={change} />, label: i18n.t('admin.runtimes.pool'), name: i18n.t('choiceCatalog.runtimePool.provider.name'), description: i18n.t('choiceCatalog.runtimePool.provider.description'), patch: { kind: 'provider' }, field: i18n.t('admin.runtimes.host') },
    ];
    for (const item of cases) {
      change.mockClear();
      const view = show(item.draw());
      const group = await screen.findByLabelText(item.label);
      expect(group).not.toHaveTextContent(item.description);
      if (item.field !== null) expect(screen.getByText(item.field, { exact: true })).toBeVisible();
      await userEvent.click(namedControl(group, item.name));
      expect(change).toHaveBeenCalledWith(item.patch);
      view.unmount();
    }
    const provider=show(<ProviderList search={{}} onSearch={change}/>);
    const reach=screen.getByRole('combobox',{name:i18n.t('library.show')});
    expect(reach).toHaveTextContent(i18n.t('library.views.usable'));
    await userEvent.click(screen.getByRole('button',{name:i18n.t('resources.controls.filter')}));
    expect(screen.getByText(i18n.t('providers.list.statusFilter'),{exact:true})).toBeVisible();
    await userEvent.keyboard('{Escape}');
    await userEvent.click(reach);
    const menu=await screen.findByRole('listbox');expect(menu.querySelectorAll('[data-choice-description]')).toHaveLength(0);
    await userEvent.click(within(menu).getByRole('option',{name:i18n.t('library.views.managed')}));
    expect(reach).toHaveTextContent(i18n.t('library.views.managed'));
    expect(fixture.inventory.mock.calls.at(-1)?.[0].resource?.view).toBe('managed');
    provider.unmount();
    const env = show(<EnvironmentList search={{ kind: 'template' }} onSearch={change} />);
    expect(screen.getByText(i18n.t('environments.list.showingKinds.template'), { exact: true })).toBeVisible();
    expect(screen.getByLabelText(i18n.t('environments.list.showingKinds.template'))).toBeVisible();
    env.unmount();
    const agents = show(<AgentList search={{}} onSearch={change} />);
    expect(await screen.findByText(i18n.t('agents.list.harnessFilter'), { exact: true })).toBeVisible();
    const harness = screen.getByRole('combobox', { name: i18n.t('agents.list.harnessFilter') });
    expect(harness).toHaveValue(i18n.t('choiceCatalog.statusFilter.any.name'));
    expect(screen.getAllByText('Codex', { exact: true })).toHaveLength(1);
    expect(screen.getByRole('link', { name: 'Codex' })).toBeVisible();
    expect(agents.container).not.toHaveTextContent('system.Codex');
    change.mockClear();
    await userEvent.click(harness);
    await userEvent.click(await screen.findByRole('option', { name: 'Codex' }));
    expect(change).toHaveBeenCalledWith({ harness: 'codex' });
    agents.unmount();
  });

  it(`keeps Inbox count and grouping labels and Activity names in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme });
    const change = vi.fn();
    const inbox = show(<Inbox filter="all" onFilter={change} />);
    const group = screen.getByLabelText(i18n.t('inbox.filter.label'));
    const unreadName = i18n.t('inbox.filter.unreadWithCount', { count: 3 });
    expect(namedControl(group, unreadName)).toHaveAccessibleName(unreadName);
    fixture.user.counters.unread = 0;
    inbox.rerender(<Inbox filter="all" onFilter={change} />);
    expect(namedControl(group, i18n.t('inbox.filter.unread'))).toHaveAccessibleName(i18n.t('inbox.filter.unread'));
    fixture.user.counters.unread = 4;
    inbox.rerender(<Inbox filter="all" onFilter={change} />);
    const updatedName = i18n.t('inbox.filter.unreadWithCount', { count: 4 });
    const unread = namedControl(group, updatedName);
    expect(unread).toHaveAccessibleName(updatedName);
    expect(change).not.toHaveBeenCalled();
    expect(group).not.toHaveTextContent(i18n.t('choiceCatalog.inboxFilter.unread.description'));
    expect(screen.getByText(i18n.t('away.inbox.grouping'), { exact: true })).toBeVisible();
    await userEvent.click(unread);
    expect(change).toHaveBeenCalledWith('unread');
    inbox.unmount();
    const activity = show(<ActivityFeed nodeId="project" chips />);
    const depth = screen.getByLabelText(i18n.t('places.activity.depth'));
    expect(depth).not.toHaveTextContent(i18n.t('choiceCatalog.activityDepth.every.description'));
    await userEvent.click(namedControl(depth, i18n.t('choiceCatalog.activityDepth.every.name')));
    expect(activityQuery).toHaveBeenLastCalledWith('project', expect.anything(), 'every');
    expect(screen.getByLabelText(i18n.t('places.activity.filter'))).not.toHaveTextContent(i18n.t('choiceCatalog.activityKind.all.description'));
    activity.unmount();
  });

  it(`names each provider status once and keeps its filter value in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme });
    const change = vi.fn();
    show(<ProviderList search={{}} onSearch={change} />);
    await userEvent.click(screen.getByRole('button',{name:i18n.t('resources.controls.filter')}));
    await userEvent.click(screen.getByRole('combobox', { name: i18n.t('providers.list.statusFilter') }));
    const list = await screen.findByRole('listbox');
    expect(within(list).getAllByRole('option')).toHaveLength(7);
    expect(list.querySelectorAll('[data-choice-description]')).toHaveLength(0);
    for (const status of ['ready', 'needs_setup', 'exhausted', 'offline', 'error', 'disabled'] as const) {
      expect(within(list).getByRole('option', { name: i18n.t(`providers.status.${status}`) })).toBeVisible();
    }
    await userEvent.click(within(list).getByRole('option', { name: i18n.t('providers.status.error') }));
    expect(change).toHaveBeenCalledExactlyOnceWith({ status: 'error' });
  });

  it(`keeps Project names short and both extra filters labelled in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme });
    const change = vi.fn<(patch: unknown) => void>();
    show(<Project projectId={id(2, 1)} search={{ view: 'list' }} onSearch={change} />);
    const view = await screen.findByLabelText(i18n.t('places.project.view'));
    expect(view).not.toHaveTextContent(i18n.t('choiceCatalog.projectView.board.description'));
    const status = screen.getByLabelText(i18n.t('places.project.status'));
    expect(status).not.toHaveTextContent(i18n.t('choiceCatalog.topicState.done.description'));
    expect(screen.getByRole('button', { name: `${i18n.t('places.project.attention')}: ${i18n.t('choiceCatalog.attention.any.name')}` })).toBeVisible();
    expect(screen.getByRole('button', { name: `${i18n.t('places.project.sort')}: ${i18n.t('choiceCatalog.sortBy.activity.name')}` })).toBeVisible();
    expect(screen.queryByText(retiredCopy(locale, 'day100.views.scopeOwn', { name: 'Web' }))).not.toBeInTheDocument();
    await userEvent.click(namedControl(view, i18n.t('choiceCatalog.projectView.board.name')));
    expect(change).toHaveBeenCalledWith(expect.objectContaining({ view: 'board' }));
  });

  it(`keeps the single-key switch state in its label without a paragraph in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme });
    const view = show(<InterfaceSettings />);
    const off = await screen.findByRole('switch', { name: i18n.t('shell.keyChoice.single.labelOff') });
    expect(screen.queryByText(retiredCopy(locale, 'shell.keyChoice.single.hint'))).not.toBeInTheDocument();
    expect(screen.queryByText(retiredCopy(locale, 'settings.interface.viewers.lead'))).not.toBeInTheDocument();
    await userEvent.click(off);
    expect(fixture.saveKeys).toHaveBeenCalledWith({ single_key: true });
    fixture.keyPending = true;
    view.rerender(<InterfaceSettings />);
    expect(screen.getByRole('switch', { name: i18n.t('shell.keyChoice.single.labelOn') })).toBeChecked();
  });

  it(`keeps the password form without a second sentence restating its title in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme });
    show(<Security />);
    expect(await screen.findByLabelText(i18n.t('security.password.new'))).toBeVisible();
    expect(screen.getByRole('heading', { name: i18n.t('security.password.title') })).toBeVisible();
    expect(screen.queryByText(/Change it here\.|在这里修改/)).not.toBeInTheDocument();
  });
}
