/** UA-28: a domain-membership count never claims that an inherited department has no people. */
import { cleanup, screen, within } from '@testing-library/react';
import type * as Router from '@tanstack/react-router';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import type * as NodesApi from '@/api/nodes';
import type * as Live from '@/realtime/live';
import type * as Screen from '@/shell/screen';
import { OrgHome } from '@/features/org/Home';
import type { Org } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { memberAccess, org } from '../doubles/fixtures';
import { ADA } from '../unit/general-access-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn<(slug: string) => Promise<Org>>(), firstSteps: vi.fn() }));
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const before = useAppearance.getState();
const previousTheme = document.documentElement.dataset.theme;
const wasDark = document.documentElement.classList.contains('dark');

vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof NodesApi>(),
  orgBySlugQuery: (slug: string) => ({ queryKey: ['header-org', slug], queryFn: () => calls.read(slug), staleTime: Infinity }),
}));
vi.mock('@/api/firstSteps', () => ({ firstStepsQuery: () => ({ queryKey: ['header-first-steps'], queryFn: () => {
  calls.firstSteps();
  return Promise.reject(new Error('the Activity tab does not read First steps'));
} }) }));
vi.mock('@/realtime/live', async (original) => ({ ...await original<typeof Live>(), useOrgTreeFeed: () => undefined }));
vi.mock('@/shell/screen', async (original) => ({ ...await original<typeof Screen>(), useScreenHeading: () => undefined }));
vi.mock('@/shell/org', () => ({ selectOrg: () => undefined }));
vi.mock('@tanstack/react-router', async (original) => {
  const router = await original<typeof Router>();
  return { ...router, Link: ({ children }: { children: ReactNode }) => <a href="#settings">{children}</a> };
});
vi.mock('@/features/org/Activity', () => ({ ActivityFeed: () => null, OrgActivity: () => null }));
vi.mock('@/features/navigation/NodeMenu', () => ({ NodeMenuButton: () => null }));
vi.mock('@/features/navigation/OrgSwitcher', () => ({ orgInitial: () => 'R', orgColor: () => 0 }));
vi.mock('@/features/topic/TagPicker', () => ({ TagPicker: () => null }));
vi.mock('@/features/announcements/Composer', () => ({ PlaceAnnouncements: () => null }));

beforeEach(() => { vi.clearAllMocks(); });
afterEach(async () => {
  cleanup();
  useAppearance.setState(before);
  document.documentElement.classList.toggle('dark', wasDark);
  if (previousTheme === undefined) delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = previousTheme;
  await setLocale('en');
});

it.each(modes)('keeps domain membership out of the compact header for every org kind in %s/%s', async (locale, theme) => {
  await setLocale(locale);
  useAppearance.setState({ theme, resolvedTheme: theme });
  document.documentElement.dataset.theme = theme;
  document.documentElement.classList.toggle('dark', theme === 'dark');
  const department = org('research', 'research', 'Research', ADA, {
    org_id: 'acme', parent_id: 'acme', depth: 1, address: 'acme.research', member_count: 0,
    // Inherited administrative reach does not turn domain membership into a people/role count.
    access: { ...memberAccess, role: 'admin', role_from: { id: 'acme', kind: 'org', name: 'Acme', redacted: false }, administers: true, permissions: ['org.view', 'org.interact'] },
  });
  const cases: Org[] = [
    department,
    { ...department, member_count: 2 },
    { ...department, id: 'root', slug: 'acme', address: 'acme', name: 'Acme', parent_id: null, depth: 0, member_count: 3 },
    { ...department, id: 'personal', slug: 'ada', address: 'ada', name: 'Ada', parent_id: null, depth: 0, org_kind: 'personal', member_count: 1 },
  ];
  for (const data of cases) {
    calls.read.mockResolvedValueOnce(data);
    const view = show(<OrgHome slug={data.address} tab="activity" onTab={() => undefined} />);
    const heading = await screen.findByRole('heading', { name: data.org_kind === 'personal' ? i18n.t('places.org.personalTitle', { name: data.name }) : data.name, level: 1 });
    const header = heading.closest('header');
    if (header === null) throw new Error('the place heading belongs to its one header');
    const facts = within(header);
    const empty = locale === 'en' ? 'No members of its own' : '没有归属此处的成员';
    expect(facts.queryByText(empty)).not.toBeInTheDocument();
    expect(facts.queryByText(i18n.t('places.cards.peopleWorking', { count: data.member_count }))).not.toBeInTheDocument();
    expect(facts.getAllByRole('button', { name: i18n.t('places.org.newProject') })).toHaveLength(1);
    view.unmount();
  }
  expect(calls.read.mock.calls.map(([slug]) => slug)).toEqual(cases.map((data) => data.address));
  expect(calls.firstSteps).not.toHaveBeenCalled();
});
