/** The org's first content is the person's named daily work, before setup or org summaries. */
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type * as Router from '@tanstack/react-router';
import type { ComponentProps, ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type * as Auth from '@/api/auth';
import type * as FirstStepsApi from '@/api/firstSteps';
import type * as Nodes from '@/api/nodes';
import type { NodePage } from '@/api/nodes';
import { resourceRows } from '@/api/listed';
import type * as Personal from '@/api/personal';
import type * as Search from '@/api/search';
import { OrgHome } from '@/features/org/Home';
import { OrgWork, WorkThreads } from '@/features/org/Work';
import { orgWork } from '@/features/org/workModel';
import type { FirstSteps, NameHit, NameSearchPage, Org, TreeNode } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { NetworkError } from '@/lib/requests';
import type * as Live from '@/realtime/live';
import { treeWriter, useOrgTree, useTreeStore } from '@/realtime/stores/tree';
import type * as Screen from '@/shell/screen';
import { ACME, acmeTree, me, memberAccess, org, userRef } from '../doubles/fixtures';
import { executionDisplay } from '../doubles/thread-display';
import { show } from './support';

const calls = vi.hoisted(() => ({
  place: null as Org | null, hits: [] as NameHit[], setupData: null as FirstSteps | null,
  names: vi.fn<() => Promise<NameSearchPage>>(), searched: vi.fn<(search: Search.NameSearch) => void>(),
  children: vi.fn<() => Promise<NodePage>>(), firstSteps: vi.fn<(id: string | null) => Promise<FirstSteps>>(),
  retryTree: vi.fn(),
}));
const person = me(1, 'ada', 'Ada');
const root = org(ACME.org, 'acme', 'Acme', userRef(person), { member_count: 5,
  access: { ...memberAccess, owns: true, permissions: ['org.view', 'org.interact', 'org.manage'] } });
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const appearance = useAppearance.getState();
const oldTheme = document.documentElement.dataset.theme;
const wasDark = document.documentElement.classList.contains('dark');

vi.mock('@/api/search', async (original) => {
  const api = await original<typeof Search>();
  return { ...api, namesQuery: (search: Search.NameSearch) => {
    calls.searched(search);
    return { ...api.namesQuery(search), queryFn: () => calls.names(), staleTime: Infinity };
  } };
});
vi.mock('@/api/nodes', async (original) => {
  const api = await original<typeof Nodes>();
  return { ...api,
    orgBySlugQuery: (slug: string) => ({ queryKey: ['home-org', slug], staleTime: Infinity, queryFn: () => Promise.resolve(calls.place ?? root), initialData: () => calls.place ?? root }),
    childrenQuery: (...args: Parameters<typeof api.childrenQuery>) => ({ ...api.childrenQuery(...args), queryFn: () => calls.children(), staleTime: Infinity }),
    orgsQuery: { queryKey: ['home-orgs'], staleTime: Infinity, initialData: () => [root], queryFn: () => Promise.resolve([root]) },
  };
});
vi.mock('@/api/firstSteps', async (original) => ({ ...await original<typeof FirstStepsApi>(),
  firstStepsQuery: (id: string | null) => ({ queryKey: ['home-first-steps', id], staleTime: Infinity, queryFn: () => calls.firstSteps(id) }),
}));
vi.mock('@/api/auth', async (original) => ({ ...await original<typeof Auth>(),
  meQuery: { queryKey: ['home-me'], staleTime: Infinity, initialData: () => person, queryFn: () => Promise.resolve(person) },
}));
vi.mock('@/api/personal', async (original) => ({ ...await original<typeof Personal>(),
  meStateQuery: { queryKey: ['me-state'], staleTime: Infinity, initialData: {}, queryFn: () => Promise.resolve({}) },
}));
vi.mock('@/realtime/live', async (original) => ({ ...await original<typeof Live>(), useOrgTreeFeed: () => undefined, reloadOrgTree: calls.retryTree }));
vi.mock('@/shell/screen', async (original) => ({ ...await original<typeof Screen>(), useScreenHeading: () => undefined }));
vi.mock('@/shell/org', () => ({ selectOrg: () => undefined }));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<typeof Router>(),
  Link: ({ children, to, params, ...anchor }: ComponentProps<'a'> & { to?: string; params?: Record<string, string>; children: ReactNode }) =>
    <a {...anchor} href={params?.threadId === undefined ? '#place' : `/thread/${params.threadId}`} data-route={to}>{children}</a>,
}));
vi.mock('@/features/org/Activity', () => ({ ActivityFeed: () => null, OrgActivity: () => <p>Activity body</p> }));
vi.mock('@/features/usage/Board', () => ({ UsageSection: () => null }));
vi.mock('@/features/org/ProjectCards', () => ({ DepartmentCard: () => <p>Department summary</p>, ProjectCard: () => <p>Project summary</p> }));
vi.mock('@/features/settings/org/Members', () => ({ Members: () => <p>Members body</p> }));
vi.mock('@/features/navigation/NodeMenu', () => ({ NodeMenuButton: () => null }));
vi.mock('@/features/navigation/favorites', () => ({ StarButton: () => null }));
vi.mock('@/features/navigation/OrgSwitcher', () => ({ orgInitial: () => 'A', orgColor: () => 1 }));
vi.mock('@/features/topic/TagPicker', () => ({ TagPicker: () => null }));
vi.mock('@/features/announcements/Composer', () => ({ PlaceAnnouncements: () => null }));
vi.mock('@/features/ports/Outside', () => ({ OutsideOpenings: () => null }));

function changed(id: string, name: string, patch: Partial<NonNullable<TreeNode['thread']>> = {}, row: Partial<TreeNode> = {}): TreeNode {
  const base = acmeTree().find((node) => node.id === id);
  if (base?.thread === undefined || base.thread === null) throw new Error('the fixture must name a thread');
  return { ...base, name, thread: { ...base.thread, display: executionDisplay(patch.state ?? base.thread.state, {
    blocked_reason: patch.blocked_reason ?? base.thread.blocked_reason,
    waiting_for_answer: (patch.waiting === undefined ? base.thread.waiting : patch.waiting)?.kind === 'answer',
    ...(patch.attention?.failed ? { state: 'failed' } : {}),
  }), ...patch }, ...row };
}
function hit(node: TreeNode): NameHit {
  return { id: node.id, kind: 'thread', name: 'Old lookup title', address: null, avatar: null, owner: null, username: null,
    path: [], archived: false, recent: true, color: null, custom_color: null, tag: null, thread_attention: null };
}
function fill(nodes: readonly TreeNode[]) {
  const rows = new Map<string, TreeNode>(acmeTree().map((node) => [node.id, { ...node, participating: false }]));
  for (const node of nodes) rows.set(node.id, node);
  treeWriter.reset(ACME.org, [...rows.values()], 100);
}
function facts(): FirstSteps {
  return { org: { id: root.id, kind: 'org', name: root.name, redacted: false }, work_mode: 'team', start_here: null, invitation_roles: [], as_of: '2026-10-15T18:00:00Z', steps: [
    { id: 'provider', state: 'open', fact: 'not_done', scripted: false, source: null, action: null },
    { id: 'teammate', state: 'done', fact: 'member_joined', scripted: false, source: null, action: null },
  ] };
}
beforeEach(() => {
  vi.clearAllMocks(); calls.place = root; calls.hits = []; calls.setupData = facts();
  calls.names.mockImplementation(() => Promise.resolve({ items: calls.hits, partial: false }));
  calls.children.mockResolvedValue({ items: resourceRows([]), next_cursor: null });
  calls.firstSteps.mockImplementation(() => Promise.resolve(calls.setupData ?? facts()));
  useTreeStore.setState({ byOrg: new Map() });
});
afterEach(async () => {
  cleanup(); useTreeStore.setState({ byOrg: new Map() }); useAppearance.setState(appearance);
  document.documentElement.classList.toggle('dark', wasDark);
  if (oldTheme === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = oldTheme;
  await setLocale('en');
});

describe.each(modes)('org daily work in %s/%s', (locale, theme) => {
  async function setAppearance() {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
  }

  it('leads with each named waiting/recent thread once, before setup and filters, even when org children fail', async () => {
    await setAppearance();
    const approval = changed(ACME.repro, 'Deploy staging', { waiting: { kind: 'approval', for_caller: true } });
    const blocked = changed(ACME.idleThread, 'Upgrade the session store', { state: 'blocked', blocked_reason: 'binding_invalid', blocked_cause: 'sandbox' }, { participating: true });
    const recent = changed(ACME.mainThread, 'Hunt the login bug', { state: 'idle' });
    fill([approval, blocked, recent]); calls.hits = [hit(recent), hit(approval)];
    calls.children.mockRejectedValue(new Error('child summaries are unavailable'));
    const view = show(<OrgHome slug="acme" tab="overview" onTab={() => undefined} />);
    const daily = await screen.findByRole('region', { name: i18n.t('places.org.work.waiting') });
    expect(within(daily).getAllByText('Deploy staging')).toHaveLength(1);
    expect(within(daily).getByText(locale === 'en' ? 'Waiting for your approval' : '等待你批准')).toBeVisible();
    expect(within(daily).getByText(locale === 'en' ? 'Blocked: no suitable sandbox' : '已阻塞：没有适用的沙箱')).toBeVisible();
    const recents = screen.getByRole('region', { name: i18n.t('places.org.work.recent') });
    expect(await within(recents).findByText('Hunt the login bug')).toBeVisible();
    expect(within(recents).queryByText('Deploy staging')).not.toBeInTheDocument();
    expect(view.container.querySelectorAll('[data-work-thread]')).toHaveLength(3);
    const work = view.container.querySelector('[data-org-work]');
    if (work === null) throw new Error('daily work is the first body of the overview');
    expect(work.compareDocumentPosition(await screen.findByTestId('first-steps')) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    const filters = view.container.querySelector('[data-filter-bar]');
    if (filters === null) throw new Error('org filters still have their own row');
    expect(daily.compareDocumentPosition(filters) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(view.container.querySelector('[data-org-work]')).not.toHaveTextContent(/Old lookup title|Checked|Refresh|sandbox\.mode|binding_invalid/);
    expect(calls.names).toHaveBeenCalledTimes(1);
    expect(calls.searched.mock.calls.every(([search]) => search.q === '' && search.kinds.join(',') === 'thread' && search.orgId === root.org_id)).toBe(true);
  });

  it('keeps viewer wording honest and applies a live answer, rename and loss of access without another read', async () => {
    await setAppearance();
    const question = changed(ACME.repro, 'Choose the redirect', { waiting: { kind: 'answer', for_caller: false } });
    fill([question]); calls.hits = [hit(question)];
    const view = show(<OrgWork org={root} />);
    expect(await screen.findByText(locale === 'en' ? 'Waiting for an answer' : '等待回答')).toBeVisible();
    expect(screen.queryByText(i18n.t('places.org.work.yourAnswer'))).not.toBeInTheDocument();
    const after = changed(question.id, 'Redirect chosen', { state: 'idle', waiting: null }, { tree_seq: 101 });
    act(() => treeWriter.apply(root.org_id, [after], 101));
    expect(await screen.findByText('Redirect chosen')).toBeVisible();
    expect(screen.getByText(i18n.t('state.idle'))).toBeVisible();
    expect(view.container.querySelector(`[data-work-thread="${question.id}"] [data-work-state="idle"]`)).toBeVisible();
    expect(screen.queryByRole('region', { name: i18n.t('places.org.work.waiting') })).not.toBeInTheDocument();
    act(() => treeWriter.apply(root.org_id, [{ ...after, name: '', deleted: true, thread: null, tree_seq: 102 }], 102));
    await waitFor(() => expect(view.container.querySelectorAll('[data-work-thread]')).toHaveLength(0));
    expect(screen.queryByText('Redirect chosen')).not.toBeInTheDocument();
    expect(calls.names).toHaveBeenCalledTimes(1);
  });

  it('exposes the remaining rows with a compact keyboard control and keeps whole names as real links', async () => {
    await setAppearance();
    const base = changed(ACME.mainThread, 'Recent thread');
    const rows = Array.from({ length: 8 }, (_, index) => ({ ...base, id: `recent-${index}`, name: `A complete thread title ${index}` }));
    fill(rows);
    function List() {
      const tree = useOrgTree(root.org_id);
      return <WorkThreads rows={orgWork(tree, root.id, rows.map(hit)).recent} id="personal-recents" />;
    }
    show(<List />);
    expect(screen.getAllByRole('link')).toHaveLength(6);
    const more = screen.getByRole('button', { name: i18n.t('places.org.work.showMore', { count: 2 }) });
    expect(more).toHaveClass('h-(--control-sm)');
    expect(more).toHaveAttribute('aria-expanded', 'false');
    more.focus(); await userEvent.keyboard('{Enter}');
    expect(screen.getAllByRole('link')).toHaveLength(8);
    const last = screen.getByRole('link', { name: /A complete thread title 7/ });
    expect(last).toHaveAttribute('href', '/thread/recent-7');
    expect(within(last).getByText('A complete thread title 7')).not.toHaveClass('truncate');
    expect(screen.getByRole('button', { name: i18n.t('places.org.work.showLess') })).toHaveAttribute('aria-expanded', 'true');
    expect(calls.names).not.toHaveBeenCalled();
  });

  it('starts no setup read for a non-owner or department and no work read on hidden Overview tabs', async () => {
    await setAppearance(); fill([]);
    for (const place of [
      { ...root, access: { ...root.access, owns: false, administers: true } },
      { ...root, id: ACME.research, name: 'Research', address: 'acme.research', parent_id: ACME.org, depth: 1 },
    ]) {
      calls.place = place;
      const view = show(<OrgHome slug={place.address} tab="overview" onTab={() => undefined} />);
      await screen.findByText(i18n.t('places.org.work.empty'));
      expect(screen.queryByTestId('first-steps')).not.toBeInTheDocument();
      expect(calls.firstSteps).not.toHaveBeenCalled(); view.unmount();
    }
    calls.names.mockClear(); calls.place = root;
    for (const tab of ['members', 'activity'] as const) {
      const view = show(<OrgHome slug="acme" tab={tab} onTab={() => undefined} />);
      expect(screen.queryByRole('region', { name: i18n.t('places.org.work.recent') })).not.toBeInTheDocument();
      expect(calls.names).not.toHaveBeenCalled(); expect(calls.firstSteps).not.toHaveBeenCalled(); view.unmount();
    }
  });
});

it('does not hide already-known waiting work behind a slow or failed recent-visits request', async () => {
  const pending = changed(ACME.repro, 'Approve deployment', { waiting: { kind: 'approval', for_caller: true } });
  fill([pending]);
  let finish: (page: NameSearchPage) => void = () => undefined;
  const pendingVisits = new Promise<NameSearchPage>((resolve) => { finish = resolve; });
  calls.names.mockReturnValueOnce(pendingVisits);
  const view = show(<OrgWork org={root} />);
  try {
    expect(screen.getByText('Approve deployment')).toBeVisible();
    expect(screen.queryByText(i18n.t('places.org.work.empty'))).not.toBeInTheDocument();
  } finally {
    await act(async () => { finish({ items: [], partial: false }); await pendingVisits; });
    view.unmount();
  }
  calls.names.mockRejectedValue(new Error('visits unavailable'));
  show(<OrgWork org={root} />);
  expect(await screen.findByText(i18n.t('places.org.work.failedRead'))).toBeVisible();
  expect(screen.getByText('Approve deployment')).toBeVisible();
  expect(screen.queryByText(i18n.t('places.org.work.empty'))).not.toBeInTheDocument();
});

it('a failed tree stops saying Loading, offers its exact retry and never claims the person has no work', async () => {
  useTreeStore.setState({ byOrg: new Map([[root.org_id, { nodes: new Map(), loaded: false, lastSeq: 0, error: new NetworkError('failed') }]]) });
  show(<OrgWork org={root} />);
  await waitFor(() => expect(calls.names).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(screen.queryByText(i18n.t('common.loading'))).not.toBeInTheDocument());
  expect(screen.queryByText(i18n.t('places.org.work.empty'))).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
  expect(calls.retryTree).toHaveBeenCalledWith(root.org_id);
});
