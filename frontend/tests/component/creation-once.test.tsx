/** UA-07: each screen keeps one creation entry, including empty lists and inline starters. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Box } from 'lucide-react';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as NodesApi from '@/api/nodes';
import type * as Starts from '@/features/start/QuickStart';
import { Messages } from '@/features/chat/Messages';
import { DocumentList } from '@/features/documents/DocumentList';
import { placeOfNode } from '@/features/navigation/places';
import { OrgHome } from '@/features/org/Home';
import { Project as ProjectPage } from '@/features/project/Project';
import { ResourceListFrame } from '@/features/resources/ResourceList';
import { TopicOverview } from '@/features/topic/Overview';
import type { Org, Project, Topic, TreeNode } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { acmeTree, me, org, userRef } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ dialog: vi.fn(), document: vi.fn() }));
const owner = userRef(me(1, 'ada', 'Ada'));
function makeOrg(personal = false): Org {
  const value = org('team', personal ? 'ada' : 'acme', personal ? 'Ada' : 'Acme', owner);
  return { ...value, org_kind: personal ? 'personal' : 'team', member_count: 0, project_count: 0,
    access: { ...value.access, permissions: ['org.view', 'org.interact', 'org.manage', 'doc.create'] } };
}
let root = makeOrg();
const project: Project = { ...root, kind: 'project', id: 'project', name: 'Release project', parent_id: root.id, topic_count: 0,
  path: [...root.path, { id: 'project', kind: 'project', name: 'Release project', redacted: false }],
  access: { ...root.access, permissions: ['project.view', 'project.interact', 'project.manage', 'doc.create'] } };
let projectData = project;
let children: (Org | Project)[] = [];
const topic: Topic = { ...project, kind: 'topic', id: 'topic', name: 'Release topic', parent_id: project.id, status: 'open', done_at: null, thread_display_state: null,
  path: [...project.path, { id: 'topic', kind: 'topic', name: 'Release topic', redacted: false }],
  thread_count: 0, attention: { blocked: 0, running: 0, waiting_approval: 0 }, work_attention: { uncommitted: 0, unpushed: 0, failed: 0 },
  default_environment_id: null, environments: [], last_activity_at: null, seq: 0,
  access: { ...root.access, permissions: ['topic.view', 'topic.interact', 'topic.manage', 'doc.create'] } };
let nodes = new Map<string, TreeNode>();
function emptyPages(key: readonly unknown[]) {
  return { queryKey: key, initialPageParam: null, queryFn: () => Promise.resolve({ items: [], next_cursor: null }), getNextPageParam: () => undefined };
}

vi.mock('@tanstack/react-router', () => ({
  Link: ({ to, params, children, className }: { to: string; params?: Record<string, string>; children: ReactNode; className?: string }) => <a href={`${to}/${Object.values(params ?? {}).join('/')}`} className={className}>{children}</a>,
  // The project's "…" names its Settings and goes there with the router.
  useRouter: () => ({ navigate: vi.fn() }),
}));
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof NodesApi>(),
  orgBySlugQuery: (slug: string) => ({ queryKey: ['one-create-org', slug], queryFn: () => Promise.resolve(root) }),
  projectQuery: (id: string) => ({ queryKey: ['one-create-project', id], staleTime: Infinity, queryFn: () => Promise.resolve(projectData) }),
  tagsQuery: (id: string) => ({ queryKey: ['one-create-tags', id], queryFn: () => Promise.resolve([]) }),
  childrenQuery: (id: string) => ({ ...emptyPages(['one-create-children', id]), queryFn: () => Promise.resolve({ items: children, next_cursor: null }) }),
  topicsQuery: (id: string) => emptyPages(['one-create-topics', id]),
}));
vi.mock('@/api/directory', async (original) => ({ ...await original<object>(), membersQuery: (id: string) => emptyPages(['one-create-members', id]) }));
vi.mock('@/api/personal', async (original) => ({ ...await original<object>(), meStateQuery: { queryKey: ['one-create-preferences'], queryFn: () => Promise.resolve({}) } }));
vi.mock('@/api/day100', async (original) => ({ ...await original<object>(), saveProjectView: () => Promise.resolve({}) }));
vi.mock('@/api/firstSteps', () => ({ firstStepsQuery: (id: string | null) => ({ queryKey: ['one-create-checklist', id], queryFn: () => Promise.resolve(null) }) }));
vi.mock('@/api/documents', async (original) => ({ ...await original<object>(), documentsOfNodeQuery: (id: string, filters: unknown) => emptyPages(['one-create-documents', id, filters]) }));
vi.mock('@/api/chats', async (original) => ({ ...await original<object>(), orgChatsQuery: (id: string) => emptyPages(['one-create-chats', id]) }));
vi.mock('@/realtime/live', async (original) => ({ ...await original<object>(), useOrgTreeFeed: vi.fn() }));
vi.mock('@/realtime/stores/tree', async (original) => ({ ...await original<object>(), useOrgTree: () => ({ nodes, loaded: true, error: null }) }));
vi.mock('@/shell/screen', () => ({ useScreenHeading: vi.fn() }));
vi.mock('@/shell/org', () => ({ selectOrg: vi.fn() }));
vi.mock('@/features/navigation/NodeDialogs', () => ({ openNodeDialog: calls.dialog }));
vi.mock('@/features/navigation/NodeMenu', () => ({ NodeMenuButton: () => null }));
vi.mock('@/features/navigation/places', async (original) => ({ ...await original<object>(),
  PlaceLink: ({ place, children }: { place: { id: string }; children: ReactNode }) => <a href={`#${place.id}`}>{children}</a>,
}));
vi.mock('@/features/navigation/PlaceChain', async (original) => ({ ...await original<object>(), PlaceChain: () => null }));
vi.mock('@/ui/UserChip', () => ({ UserChip: ({ user }: { user?: { display_name: string } | null }) => <span>{user?.display_name}</span> }));
vi.mock('@/features/resources/trail', () => ({ useResourceHeading: vi.fn(), rememberList: vi.fn() }));
vi.mock('@/features/resources/ResourceNav', () => ({ ResourceNav: () => null }));
vi.mock('@/features/topic/TagPicker', () => ({ TagPicker: () => <span data-place-tags /> }));
vi.mock('@/features/announcements/Composer', () => ({ PlaceAnnouncements: () => null }));
vi.mock('@/features/usage/Board', () => ({ UsageSection: () => null }));
vi.mock('@/features/ports/Outside', () => ({ OutsideOpenings: () => null }));
vi.mock('@/features/onboarding/Checklist', () => ({ Checklist: () => null }));
vi.mock('@/features/documents/Readme', () => ({ PlaceReadme: () => null }));
vi.mock('@/features/documents/useMakeDocument', () => ({ useMakeDocument: () => ({ mutate: calls.document, isPending: false, isError: false, error: null }) }));
vi.mock('@/features/people/Templates', () => ({ Templates: () => null }));
vi.mock('@/features/people/ImportDialog', () => ({ ImportDialog: () => null }));
vi.mock('@/features/settings/org/Members', () => ({ Members: () => null }));
vi.mock('@/features/org/Activity', () => ({ ActivityFeed: () => null, OrgActivity: () => null }));
vi.mock('@/features/org/Work', () => ({ OrgWork: () => null }));
vi.mock('@/features/project/Activity', () => ({ ProjectActivity: () => null }));
vi.mock('@/features/project/FolderCards', () => ({ FolderCards: () => null }));
vi.mock('@/features/project/ProjectStar', () => ({ ProjectStar: () => null }));
vi.mock('@/features/chat/AllChats', () => ({ AllChats: () => null }));
vi.mock('@/features/chat/ChatView', () => ({ ChatView: () => null, chatTitle: () => '', chatPageTitle: () => '' }));
vi.mock('@/features/chat/NewChat', () => ({ NewChat: ({ open }: { open: boolean }) => open ? <div role="dialog" aria-label={i18n.t('chat.new.title')} /> : null }));
vi.mock('@/features/start/QuickStart', async (original) => {
  const actual = await original<typeof Starts>();
  return { ...actual, QuickStart: ({ node }: { node: Topic | Project }) => actual.canQuickStart(node)
    ? <textarea aria-label={i18n.t(node.kind === 'topic' ? 'start.quick.label.topic' : 'start.quick.label.project')} /> : null };
});

const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const appearance = useAppearance.getState();
const previousTheme = document.documentElement.dataset.theme;
const wasDark = document.documentElement.classList.contains('dark');
beforeEach(() => { vi.clearAllMocks(); root = makeOrg(); projectData = project; children = []; nodes = new Map(); });
afterEach(async () => {
  useAppearance.setState(appearance);
  document.documentElement.classList.toggle('dark', wasDark);
  if (previousTheme === undefined) delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = previousTheme;
  await setLocale('en');
});
async function look(locale: 'en' | 'zh-CN', theme: 'light' | 'dark') {
  await setLocale(locale);
  useAppearance.setState({ theme, resolvedTheme: theme });
  document.documentElement.dataset.theme = theme;
  document.documentElement.classList.toggle('dark', theme === 'dark');
}

for (const [locale, theme] of modes) {
  it(`keeps the toolbar create action once and preserves an empty-only fallback (${locale}/${theme})`, async () => {
    await look(locale, theme);
    const create = vi.fn();
    const action = <button onClick={create}>{i18n.t('resourceDetails.mount.add')}</button>;
    const frame = (toolbar: ReactNode) => <ResourceListFrame<{ id: string; owner: null }> kind="mounts" search={{}}
      title={i18n.t('screens.mounts.title')} icon={Box} lead={null} label={i18n.t('screens.mounts.title')}
      load={{ isPending: false, isError: false, isRefetching: false, error: null, refetch: vi.fn() }} all={[]} rows={[]} columns={[]} rowKey={(row) => row.id}
      text="" onText={vi.fn()} searchLabel={i18n.t('resourceDetails.mount.list.search')} searchPlaceholder={i18n.t('resourceDetails.mount.list.search')}
      action={toolbar} empty={{ icon: <Box />, title: i18n.t('resourceDetails.mount.list.none'), action }} noMatch={i18n.t('resourceDetails.mount.list.noMatch')} />;
    const view = show(frame(action));
    const name = i18n.t('resourceDetails.mount.add');
    expect(screen.getAllByRole('button', { name })).toHaveLength(1);
    const user = userEvent.setup();
    screen.getByRole('button', { name }).focus();
    await user.keyboard('{Enter}');
    expect(create).toHaveBeenCalledTimes(1);
    view.unmount();
    show(frame(undefined));
    expect(screen.getAllByRole('button', { name })).toHaveLength(1);
    await user.click(screen.getByRole('button', { name }));
    expect(create).toHaveBeenCalledTimes(2);
  });

  it(`uses the topic starter once on empty and populated overviews (${locale}/${theme})`, async () => {
    await look(locale, theme);
    const view = show(<TopicOverview topic={topic} place={placeOfNode(topic)} />);
    expect(screen.getByRole('textbox', { name: i18n.t('start.quick.label.topic') })).toBeVisible();
    expect(screen.queryByRole('button', { name: i18n.t('places.topic.newThread') })).not.toBeInTheDocument();
    const thread = acmeTree().find((node) => node.kind === 'thread');
    if (thread === undefined) throw new Error('the fixture contains a thread');
    nodes = new Map([[thread.id, { ...thread, parent_id: topic.id }]]);
    view.rerender(<TopicOverview topic={topic} place={placeOfNode(topic)} />);
    expect(screen.getAllByRole('textbox', { name: i18n.t('start.quick.label.topic') })).toHaveLength(1);
    expect(screen.getByRole('link', { name: thread.name })).toBeVisible();
    expect(screen.queryByRole('button', { name: i18n.t('places.topic.newThread') })).not.toBeInTheDocument();
  });

  it(`keeps one gated topic entry when the reader cannot start inline (${locale}/${theme})`, async () => {
    await look(locale, theme);
    const readable = { ...topic, access: { ...topic.access, permissions: ['topic.view'] } };
    show(<TopicOverview topic={readable} place={placeOfNode(readable)} />);
    expect(screen.queryByRole('textbox', { name: i18n.t('start.quick.label.topic') })).not.toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: i18n.t('places.topic.newThread') })).toHaveLength(1);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('places.topic.newThread') }));
    expect(calls.dialog).not.toHaveBeenCalled();
  });

  it(`keeps the project's one header action in every view without an inline starter (${locale}/${theme})`, async () => {
    await look(locale, theme);
    for (const mode of ['list', 'board', 'activity'] as const) {
      calls.dialog.mockClear();
      const view = show(<ProjectPage projectId={project.id} search={{ view: mode }} onSearch={vi.fn()} />);
      const create = await screen.findByRole('button', { name: i18n.t('places.project.newTopic') });
      expect(create.closest('header')).not.toBeNull();
      expect(screen.getAllByRole('button', { name: i18n.t('places.project.newTopic') })).toHaveLength(1);
      expect(screen.queryByRole('textbox', { name: i18n.t('start.quick.label.project') })).not.toBeInTheDocument();
      expect(view.container.querySelectorAll('[data-filter-bar]')).toHaveLength(mode === 'activity' ? 0 : 1);
      if (mode !== 'activity') expect(await screen.findByText(i18n.t('places.project.empty.title'))).toBeVisible();
      await userEvent.click(create);
      expect(calls.dialog).toHaveBeenCalledExactlyOnceWith({ type: 'new-topic', parent: placeOfNode(project) });
      view.unmount();
    }
  });

  it(`keeps authored descriptions in the header and sends their edit to the existing dialog (${locale}/${theme})`, async () => {
    await look(locale, theme);
    const description = 'The complete release plan, including work that wraps onto another line.';
    projectData = { ...project, description };
    const view = show(<ProjectPage projectId={project.id} search={{ view: 'list' }} onSearch={vi.fn()} />);
    const heading = await screen.findByRole('heading', { name: project.name, level: 1 });
    const header = heading.closest('header');
    if (header === null) throw new Error('the place has its common header');
    const filter = view.container.querySelector('[data-filter-bar]');
    const topics = view.container.querySelector('[data-project-topics]');
    if (filter === null || topics === null) throw new Error('the collection has its filter and content');
    expect(header.compareDocumentPosition(filter) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(filter.compareDocumentPosition(topics) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(within(header).getByText(description)).toBeVisible();
    expect(view.container.querySelector('[data-place-tags]')).not.toBeInTheDocument();
    const details = within(header).getByRole('button', { name: i18n.t('places.details.title') });
    const user = userEvent.setup();
    details.focus(); await user.keyboard('{Enter}');
    expect(await screen.findByText(description)).toBeVisible();
    expect(screen.getAllByText(description)).toHaveLength(1);
    await user.keyboard('{Escape}');
    expect(details).toHaveFocus();
    await user.click(details);
    await user.click(screen.getByRole('button', { name: i18n.t('places.details.edit') }));
    expect(calls.dialog).toHaveBeenCalledExactlyOnceWith({ type: 'rename', node: placeOfNode(projectData) });
  });

  it(`preserves participation when filters navigate and explains a gated primary action (${locale}/${theme})`, async () => {
    await look(locale, theme);
    projectData = { ...project, access: { ...project.access, permissions: ['project.view'] } };
    const onSearch = vi.fn();
    show(<ProjectPage projectId={project.id} search={{ view: 'list' }} onSearch={onSearch} />);
    const create = await screen.findByRole('button', { name: i18n.t('places.project.newTopic') });
    expect(create).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(create);
    expect(calls.dialog).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('switch', { name: i18n.t('adminReach.focus.all') }));
    expect(onSearch).toHaveBeenCalledWith(expect.objectContaining({ participating: true }));
  });

  it(`puts the root and department places directly after their filter, before guidance (${locale}/${theme})`, async () => {
    await look(locale, theme);
    for (const parent of [null, 'parent-org']) {
      root = { ...makeOrg(), parent_id: parent, description: 'A description available when requested' };
      children = [{ ...project, parent_id: root.id }];
      const view = show(<OrgHome slug={parent === null ? 'acme' : 'acme.research'} tab="overview" onTab={vi.fn()} />);
      const row = await screen.findByRole('link', { name: project.name });
      const filter = view.container.querySelector('[data-filter-bar]');
      const list = row.closest('[data-places-list]');
      if (filter === null || list === null) throw new Error('places share the compact collection frame');
      expect(filter.compareDocumentPosition(list) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
      const header = screen.getByRole('heading', { name: root.name, level: 1 }).closest('header');
      if (header === null) throw new Error('the org has one header');
      expect(within(header).getAllByRole('button', { name: i18n.t('places.org.newProject') })).toHaveLength(1);
      expect(within(header).getByText(root.description)).toBeVisible();
      expect(screen.queryByText(i18n.t('places.org.departments'), { exact: true })).not.toBeInTheDocument();
      view.unmount();
    }
  });

  it(`keeps one document action and creates immediately without a preliminary naming form (${locale}/${theme})`, async () => {
    await look(locale, theme);
    show(<DocumentList node={topic} />);
    expect(await screen.findByText(i18n.t('documents.list.empty', { kind: i18n.t('documents.kind.topic') }))).toBeVisible();
    expect(screen.getAllByRole('button', { name: i18n.t('documents.list.new') })).toHaveLength(1);
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: i18n.t('documents.list.new') }));
    expect(screen.queryByRole('textbox', { name: i18n.t('places.fields.name') })).not.toBeInTheDocument();
    expect(calls.document).toHaveBeenCalledExactlyOnceWith({ private: false });
  });

  it(`keeps one reset action above a filtered empty project (${locale}/${theme})`, async () => {
    await look(locale, theme);
    const onSearch = vi.fn();
    show(<ProjectPage projectId={project.id} search={{ view: 'list', q: 'absent' }} onSearch={onSearch} />);
    expect(await screen.findByText(i18n.t('places.project.noMatch'))).toBeVisible();
    expect(screen.getAllByRole('button', { name: i18n.t('places.project.clear') })).toHaveLength(1);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('places.project.clear') }));
    expect(onSearch).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('searchbox', { name: i18n.t('places.project.search') })).toHaveValue('');
  });

  it(`keeps one New message entry on an empty org message list (${locale}/${theme})`, async () => {
    await look(locale, theme);
    show(<Messages slug="acme" search={{}} onSearch={vi.fn()} />);
    expect(await screen.findByText(i18n.t('chat.messages.empty'))).toBeVisible();
    expect(screen.getAllByRole('button', { name: i18n.t('chat.new.title') })).toHaveLength(1);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('chat.new.title') }));
    expect(screen.getByRole('dialog', { name: i18n.t('chat.new.title') })).toBeVisible();
  });

  it(`keeps personal-org project and document creation in their actual headers (${locale}/${theme})`, async () => {
    await look(locale, theme);
    root = makeOrg(true);
    show(<><OrgHome slug="ada" tab="overview" onTab={vi.fn()} /><DocumentList node={root} /></>);
    expect(await screen.findByText(i18n.t('places.org.ownEmpty.title'))).toBeVisible();
    expect(screen.getAllByRole('button', { name: i18n.t('places.org.newProject') })).toHaveLength(1);
    expect(screen.getAllByRole('button', { name: i18n.t('documents.list.new') })).toHaveLength(1);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('places.org.newProject') }));
    expect(calls.dialog).toHaveBeenCalledExactlyOnceWith({ type: 'new-project', parent: placeOfNode(root) });
  });

  it(`retains the sole useful creation action on a personal message list (${locale}/${theme})`, async () => {
    await look(locale, theme);
    root = makeOrg(true);
    show(<Messages slug="ada" search={{}} onSearch={vi.fn()} />);
    expect(await screen.findByText(i18n.t('craftMessages.onlyYou'))).toBeVisible();
    expect(screen.queryByRole('button', { name: i18n.t('chat.new.title') })).not.toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: i18n.t('shell.rail.newOrg') })).toHaveLength(1);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('shell.rail.newOrg') }));
    expect(calls.dialog).toHaveBeenCalledExactlyOnceWith({ type: 'new-org' });
  });
}
