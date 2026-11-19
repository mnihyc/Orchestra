/** Org, department and own-space headers have one star, one main act and one lazy secondary menu. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { OrgHome } from '@/features/org/Home';
import { NodeMenuButton } from '@/features/navigation/NodeMenu';
import { placeOfNode } from '@/features/navigation/places';
import { DepartmentCard } from '@/features/org/ProjectCards';
import type { Org } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { actionName } from '@/ui/actions';
import { me, memberAccess, org, userRef } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ place: null as Org | null, dialog: vi.fn(), star: vi.fn<() => Promise<void>>(),
  unstar: vi.fn<() => Promise<void>>(), tags: vi.fn(), announcements: vi.fn(), notifications: vi.fn(), detail: vi.fn(),
}));
const owner = userRef(me(1, 'ada', 'Ada'));
const root = org('root', 'acme', 'Acme', owner, { description: 'The complete plan stays available on demand.',
  access: { ...memberAccess, owns: true, permissions: ['org.view', 'org.interact', 'org.manage', 'org.archive', 'doc.create'] } });
vi.mock('@/api/nodes', async (original) => ({ ...await original<object>(),
  orgBySlugQuery: (slug: string) => ({ queryKey: ['simple-org-header', slug], queryFn: () => Promise.resolve(calls.place ?? root) }),
  nodeQuery: (id: string) => ({ queryKey: ['simple-org-node', id], queryFn: () => { calls.detail(); return Promise.resolve(calls.place ?? root); } }),
}));
vi.mock('@/api/personal', async (original) => ({ ...await original<object>(),
  favoritesQuery: { queryKey: ['favorites'], staleTime: Infinity, queryFn: () => Promise.resolve([]) },
  starNode: calls.star, unstarNode: calls.unstar,
}));
vi.mock('@/features/navigation/places', async (original) => ({ ...await original<object>(), useGoToPlace: () => vi.fn() }));
vi.mock('@/features/navigation/NodeDialogs', () => ({ openNodeDialog: calls.dialog }));
vi.mock('@/features/announcements/Composer', () => ({ PlaceAnnouncementContents: () => { calls.announcements(); return <div data-testid="announcement-composer" />; } }));
vi.mock('@/features/thread/SubscriptionBell', () => ({ SubscriptionBell: () => { calls.notifications(); return <div data-testid="notification-options" />; } }));
vi.mock('@/features/topic/TagPicker', () => ({ TagPicker: () => { calls.tags(); return <div data-testid="tag-picker" />; } }));
vi.mock('@/features/org/Activity', () => ({ OrgActivity: () => null, ActivityFeed: () => null }));
vi.mock('@/realtime/live', async (original) => ({ ...await original<object>(), useOrgTreeFeed: () => undefined }));
vi.mock('@/shell/org', () => ({ selectOrg: () => undefined }));
vi.mock('@/shell/screen', () => ({ useScreenHeading: () => undefined }));
vi.mock('@tanstack/react-router', () => ({ Link: ({ children }: { children: ReactNode }) => <a href="#place">{children}</a> }));
const appearance = useAppearance.getState();
const previousTheme = document.documentElement.dataset.theme;
const wasDark = document.documentElement.classList.contains('dark');
beforeEach(() => { vi.clearAllMocks(); calls.place = root; calls.star.mockResolvedValue(undefined); calls.unstar.mockResolvedValue(undefined); });
afterEach(async () => {
  useAppearance.setState(appearance);
  if (previousTheme === undefined) delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = previousTheme;
  document.documentElement.classList.toggle('dark', wasDark);
  await setLocale('en');
});
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
async function look(locale: 'en' | 'zh-CN', theme: 'light' | 'dark') {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
}

for (const [locale, theme] of modes) {
  it(`keeps all org kinds readable with a native Settings link and a named Star menu counterpart (${locale}/${theme})`, async () => {
    await look(locale, theme);
    const department = { ...root, id: 'department', name: 'Research', address: 'acme.research', parent_id: root.id, depth: 1 };
    const own: Org = { ...root, id: 'own', name: 'Ada', address: 'ada', org_kind: 'personal' };
    const user = userEvent.setup();
    for (const data of [root, department, own]) {
      calls.place = data; calls.star.mockClear(); calls.dialog.mockClear();
      const view = show(<OrgHome slug={data.address} tab="activity" onTab={vi.fn()} />);
      const title = data.org_kind === 'personal' ? i18n.t('places.org.personalTitle', { name: data.name }) : data.name;
      const heading = await screen.findByRole('heading', { level: 1, name: title });
      const header = heading.closest('header');
      if (header === null) throw new Error('the page owns its single header');
      expect(within(header).getByRole('button', { name: i18n.t('places.org.newProject') })).toBeVisible();
      const star = within(header).getByRole('button', { name: i18n.t('places.star.star') });
      expect(star).not.toHaveTextContent(i18n.t('places.star.star'));
      expect(within(header).getByText(data.description)).toBeVisible();
      expect(calls.tags).not.toHaveBeenCalled(); expect(calls.announcements).not.toHaveBeenCalled(); expect(calls.notifications).not.toHaveBeenCalled();
      const menu = within(header).getByRole('button', { name: actionName('actions', data.name) });
      await user.click(menu);
      const items = screen.getByRole('menu');
      expect(within(items).getByRole('menuitem', { name: i18n.t('places.org.settings') })).toHaveAttribute('href', `/org/${data.address}/settings/general`);
      expect(within(items).getByRole('menuitem', { name: i18n.t('places.star.star') })).toBeVisible();
      expect(within(items).queryByRole('menuitem', { name: i18n.t('places.menu.newProject') })).not.toBeInTheDocument();
      if (data.org_kind === 'personal') expect(within(items).queryByRole('menuitem', { name: i18n.t('places.menu.newDepartment') })).not.toBeInTheDocument();
      else expect(within(items).getByRole('menuitem', { name: i18n.t('places.menu.newDepartment') })).toBeVisible();
      expect(calls.detail).not.toHaveBeenCalled();
      await user.keyboard('{Escape}');
      await waitFor(() => expect(menu).toHaveFocus());
      await user.click(star);
      await waitFor(() => expect(calls.star).toHaveBeenCalledExactlyOnceWith(data.id));
      expect(await within(header).findByRole('button', { name: i18n.t('places.star.unstar') })).toHaveAttribute('aria-pressed', 'true');
      await user.click(within(header).getByRole('button', { name: i18n.t('places.org.newProject') }));
      expect(calls.dialog).toHaveBeenCalledExactlyOnceWith({ type: 'new-project', parent: placeOfNode(data) });
      view.unmount();
    }
  });

  it(`opens descriptions, announcements and notifications only through the secondary menu (${locale}/${theme})`, async () => {
    await look(locale, theme);
    show(<OrgHome slug={root.address} tab="activity" onTab={vi.fn()} />);
    const user = userEvent.setup();
    const heading = await screen.findByRole('heading', { name: root.name, level: 1 });
    const header = heading.closest('header');
    if (header === null) throw new Error('the page owns its header');
    const menu = within(header).getByRole('button', { name: actionName('actions', root.name) });
    await user.click(menu);
    await user.click(screen.getByRole('menuitem', { name: i18n.t('places.details.title') }));
    const details = await screen.findByRole('dialog', { name: i18n.t('places.details.title') });
    expect(within(details).getByText(root.description)).toBeVisible();
    expect(within(details).getByTestId('tag-picker')).toBeVisible();
    await user.keyboard('{Escape}');
    await waitFor(() => expect(menu).toHaveFocus());
    await user.click(menu);
    await user.click(screen.getByRole('menuitem', { name: i18n.t('placeAnnouncements.title') }));
    expect(await screen.findByTestId('announcement-composer')).toBeVisible();
    await user.keyboard('{Escape}');
    await waitFor(() => expect(menu).toHaveFocus());
    await user.click(menu);
    await user.click(screen.getByRole('menuitem', { name: i18n.t('notificationBell.place') }));
    expect(await screen.findByTestId('notification-options')).toBeVisible();
    expect(calls.detail).not.toHaveBeenCalled();
    await user.keyboard('{Escape}');
    await waitFor(() => expect(menu).toHaveFocus());
  });

  it(`keeps Settings in a navigation-row org menu too (${locale}/${theme})`, async () => {
    await look(locale, theme);
    show(<NodeMenuButton place={placeOfNode(root)} node={root} where="row" />);
    await userEvent.click(screen.getByRole('button', { name: actionName('actions', root.name) }));
    expect(screen.getByRole('menuitem', { name: i18n.t('places.org.settings') })).toHaveAttribute('href', '/org/acme/settings/general');
    expect(calls.detail).not.toHaveBeenCalled();
  });

  it(`keeps unavailable primary actions guarded and department people distinct from domain members (${locale}/${theme})`, async () => {
    await look(locale, theme);
    calls.place = { ...root, access: { ...root.access, owns: false, permissions: ['org.view'] } };
    const view = show(<OrgHome slug={root.address} tab="activity" onTab={vi.fn()} />);
    const create = await screen.findByRole('button', { name: i18n.t('places.org.newProject') });
    expect(create).toHaveAttribute('aria-disabled', 'true');
    expect(create).toHaveAttribute('aria-description', expect.stringContaining(owner.display_name));
    await userEvent.click(create); expect(calls.dialog).not.toHaveBeenCalled(); view.unmount();
    const department: Org = { ...root, member_count: 0, people_count: 4, parent_id: 'parent', address: 'acme.research', name: 'Research' };
    const stats = { running: 0, waiting: 0, approvals: 0, blocked: 0, folders: 0, openTopics: 0, lastActivity: null };
    const card = show(<DepartmentCard org={department} stats={stats} />);
    expect(screen.getByText(i18n.t('places.cards.peopleWorking', { count: 4 }))).toBeVisible();
    expect(screen.queryByText(i18n.t('places.cards.peopleWorking', { count: 0 }))).not.toBeInTheDocument();
    expect(screen.queryByText(department.address)).not.toBeInTheDocument();
    card.rerender(<DepartmentCard org={{ ...department, people_count: null }} stats={stats} />);
    expect(screen.queryByText(i18n.t('places.cards.peopleWorking', { count: 0 }))).not.toBeInTheDocument();
    expect(screen.queryByText(i18n.t('places.cards.peopleWorking', { count: 4 }))).not.toBeInTheDocument();
  });
}
