/** Personal, organization and Management frames expose the same complete navigation and one page heading. */
import { cleanup, screen, within } from '@testing-library/react';
import type * as Router from '@tanstack/react-router';
import type { HTMLAttributes, ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';

import type * as Auth from '@/api/auth';
import type * as Nodes from '@/api/nodes';
import type * as Live from '@/realtime/live';
import type * as Screen from '@/shell/screen';
import { Management } from '@/features/admin/Management';
import { LISTED } from '@/features/admin/sections';
import { PersonalFrame } from '@/features/settings/PersonalFrame';
import { OrgSettings, sectionsOf } from '@/features/settings/org/OrgSettings';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { me, org } from '../doubles/fixtures';
import { ADA } from '../unit/general-access-fixtures';
import { show } from './support';

const admin = { ...me(1, 'reader', 'Reader'), is_system_admin: true };
const department = org('research', 'research', 'Research', ADA, { org_id: 'acme', parent_id: 'acme', depth: 1, address: 'acme.research' });
const appearance = useAppearance.getState();
const themeBefore = document.documentElement.dataset.theme;
const darkBefore = document.documentElement.classList.contains('dark');
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;

vi.mock('@/api/auth', async (original) => ({ ...await original<typeof Auth>(), meQuery: {
  queryKey: ['section-nav-reader'], initialData: () => admin, queryFn: () => Promise.resolve(admin), staleTime: Infinity,
} }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof Nodes>(), orgBySlugQuery: () => ({
  queryKey: ['section-nav-org'], initialData: () => department, queryFn: () => Promise.resolve(department), staleTime: Infinity,
}) }));
vi.mock('@/features/access/AdminMode', () => ({ useAdminMode: () => ({ data: { available: true, as_admin: true }, isPending: false, isError: false }) }));
vi.mock('@/realtime/live', async (original) => ({ ...await original<typeof Live>(), useOrgTreeFeed: () => undefined }));
vi.mock('@/shell/screen', async (original) => ({ ...await original<typeof Screen>(), useScreenHeading: () => undefined }));
vi.mock('@/shell/org', () => ({ selectOrg: () => undefined }));
vi.mock('@/features/navigation/OrgSwitcher', () => ({ orgInitial: () => 'R', orgColor: () => 0 }));
vi.mock('@/features/settings/org/General', () => ({ General: () => <p data-section-body>General content</p> }));
vi.mock('@/features/admin/Overview', () => ({ Overview: () => <p data-section-body>Overview content</p> }));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<typeof Router>(),
  Link: ({ to, params, children, activeOptions: _activeOptions, ...props }: HTMLAttributes<HTMLAnchorElement> & { to: string; params?: Record<string, string>; children?: ReactNode; activeOptions?: unknown }) => {
    return <a {...props} href={to.replace(/\$(\w+)/g, (_match: string, key: string) => params?.[key] ?? '')}>{children}</a>;
  },
}));

afterEach(async () => {
  cleanup(); useAppearance.setState(appearance);
  document.documentElement.classList.toggle('dark', darkBefore);
  if (themeBefore === undefined) delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = themeBefore;
  await setLocale('en');
});

it.each(modes)('keeps complete section links and the compact picker on the same destinations in %s/%s', async (locale, theme) => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
  const frames = [
    { body: <PersonalFrame page="profile" lead={null}><p>Profile content</p></PersonalFrame>, label: i18n.t('settings.personal.nav'), current: '/settings/profile', count: 9 },
    { body: <OrgSettings slug="acme.research" section="general" />, label: i18n.t('settings.org.sections', { name: department.name }), current: '/org/acme.research/settings/general', count: sectionsOf(department).length },
    { body: <Management section="overview" search={{}} onSearch={() => undefined} onSection={() => undefined} />, label: i18n.t('admin.nav.label'), current: '/admin/overview', count: LISTED.length },
  ];
  for (const frame of frames) {
    const view = show(frame.body);
    const nav = await screen.findByRole('navigation', { name: frame.label });
    expect(within(nav).getAllByRole('link')).toHaveLength(frame.count);
    const current = within(nav).getAllByRole('link').filter((link) => link.getAttribute('aria-current') === 'page');
    expect(current).toHaveLength(1); expect(current[0]).toHaveAttribute('href', frame.current);
    // jsdom has no container-query layout; the browser proof checks which presentation is visible.
    expect(within(nav).getByRole('combobox', { name: frame.label })).toHaveValue(current[0]?.textContent ?? '');
    expect(within(nav).queryByRole('button', { name: /More|更多/ })).not.toBeInTheDocument();
    expect(screen.getAllByRole('heading', { level: 1 })).toHaveLength(1);
    expect(screen.queryByRole('link', { name: /Back to|返回/ })).not.toBeInTheDocument();
    view.unmount();
  }
});
