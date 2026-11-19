/** Share stays on its access list until typing; ownership is native metadata, never a Viewer inference. */
import { infiniteQueryOptions } from '@tanstack/react-query';
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as AccessApi from '@/api/access';
import type * as AuthApi from '@/api/auth';
import type * as NodesApi from '@/api/nodes';
import type * as SettingsApi from '@/api/settings';
import type * as CandidatesApi from '@/features/access/candidates';
import type { CandidateOptions } from '@/features/access/candidates';
import type { Changes } from '@/features/access/changes';
import { HolderPeople } from '@/features/access/AccessRoster';
import { Roles } from '@/features/access/Roles';
import { ShortForm } from '@/features/access/ShortForm';
import type { Holder, NodeRoleListReply, PermissionPeopleListReply, UserRef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { me, org } from '../doubles/fixtures';
import { ADA, BEA, CATALOGUE, pageFor, TOPIC } from '../unit/general-access-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ search: vi.fn<(query: string, options: CandidateOptions) => void>(), people: vi.fn() }));
const group: Holder = { kind: 'group', group: { id: 'owners', org_id: 'acme', place: 'Acme', name: 'Reviewers', description: '', member_count: 1 } };
const regular: UserRef = { ...BEA, id: 'regular', username: 'vera', display_name: 'Vera' };
const place = org('acme', 'acme', 'Acme', ADA);
const changes: Changes = { busy: false, doing: null, error: null, stale: null, forget: vi.fn(), again: () => Promise.resolve(false), run: vi.fn(() => Promise.resolve(true)) };

vi.mock('@/api/access', async (load) => ({ ...await load<typeof AccessApi>(),
  permissionsQuery: { queryKey: ['people-permissions'], queryFn: () => Promise.resolve(CATALOGUE.reply) },
  defaultsQuery: (id: string) => ({ queryKey: ['people-defaults', id], queryFn: () => Promise.resolve(pageFor('org')) }),
  rolesQuery: (id: string) => ({ queryKey: ['people-roles', id], queryFn: (): Promise<NodeRoleListReply> => Promise.resolve({ owners: [{ kind: 'user', user: ADA }, group], own: [], above: [], can_give: [] }) }),
  waitingRequestsQuery: (id: string) => ({ queryKey: ['people-waiting', id], queryFn: () => Promise.resolve([]) }),
  peopleQuery: (target: AccessApi.AccessTarget, param: string) => infiniteQueryOptions({
    queryKey: ['people-roster', target.id, param], initialPageParam: null as string | null,
    queryFn: (): Promise<PermissionPeopleListReply> => { calls.people(); return Promise.resolve({ items: [ADA, BEA, regular], owners: [ADA.id, BEA.id], next_cursor: null }); },
    getNextPageParam: (page: PermissionPeopleListReply) => page.next_cursor,
  }),
}));
vi.mock('@/api/auth', async (load) => ({ ...await load<typeof AuthApi>(), meQuery: { queryKey: ['people-me'], queryFn: () => Promise.resolve(me(1, 'ada', 'Ada')) } }));
vi.mock('@/api/nodes', async (load) => ({ ...await load<typeof NodesApi>(), nodeQuery: (id: string) => ({ queryKey: ['people-node', id], queryFn: () => Promise.resolve(place) }) }));
vi.mock('@/api/settings', async (load) => ({ ...await load<typeof SettingsApi>(), settingsQuery: (id: string) => ({ queryKey: ['people-settings', id], queryFn: () => Promise.resolve([]) }) }));
vi.mock('@/features/navigation/placeLabels', () => ({ usePlaceLabel: () => (node: { name?: string | null } | null | undefined) => node?.name ?? null }));
vi.mock('@/features/trace/Person', () => ({ useTraceEntry: () => () => [], TracePerson: ({ user }: { user: UserRef }) => <span>{user.display_name}</span> }));
vi.mock('@/features/access/candidates', async (load) => ({ ...await load<typeof CandidatesApi>(),
  useTargetOrg: () => 'acme',
  useCandidates: (query: string, _org: string | null, options: CandidateOptions) => {
    calls.search(query, options);
    return { loading: false, placePicker: null,
      byValue: new Map([['bea', { input: { kind: 'user', id: BEA.id }, holder: { kind: 'user', user: BEA }, name: BEA.display_name, outside: false, user: BEA }]]),
      groups: [{ id: 'people', label: 'People', options: [{ value: 'bea', label: BEA.display_name, text: BEA.display_name }] }],
    };
  },
}));

const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const before = useAppearance.getState();
const oldTheme = document.documentElement.dataset.theme;
const oldDark = document.documentElement.classList.contains('dark');
beforeEach(() => { vi.clearAllMocks(); });
afterEach(async () => {
  cleanup(); useAppearance.setState(before); document.documentElement.classList.toggle('dark', oldDark);
  if (oldTheme === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = oldTheme;
  await setLocale('en');
});

for (const [locale, theme] of modes) {
  it(`opens Share on its owners and access list, searching only while typing in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme;
    const page = pageFor();
    const target = { kind: 'thread', id: page.object.id, name: 'Release' } as const;
    const user = userEvent.setup();
    show(<ShortForm target={target} page={page} catalogue={CATALOGUE} source={{ type: 'page', target }} changes={changes} ask={vi.fn()} onFull={vi.fn()} focusAdd />);
    const field = screen.getByRole('combobox', { name: i18n.t('access.add.label') });
    await waitFor(() => expect(field).toHaveFocus());
    expect(field).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
    expect(screen.getByRole('group', { name: i18n.t('access.owners.label') })).toHaveTextContent(ADA.display_name);
    expect(screen.getByRole('region', { name: i18n.t('access.roster.title') })).toBeVisible();
    expect(calls.search.mock.calls.filter(([, options]) => options.kinds.includes('members')).at(-1)?.[1].enabled).toBe(false);
    await user.type(field, 'be');
    expect(await screen.findByRole('option', { name: BEA.display_name })).toBeVisible();
    expect(calls.search.mock.calls.filter(([, options]) => options.kinds.includes('members')).at(-1)?.[1].enabled).toBe(true);
    await user.clear(field);
    expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
    expect(calls.search.mock.calls.filter(([, options]) => options.kinds.includes('members')).at(-1)?.[1].enabled).toBe(false);
    expect(changes.run).not.toHaveBeenCalled();
  });

  it(`labels the native owners as Owner when expanding Everyone, without changing ordinary attribution in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme;
    const level = i18n.t('access.choice.view');
    const via = i18n.t('access.holder.everyoneIn', { place: 'Release' });
    show(<HolderPeople target={{ kind: 'thread', id: 'release', name: 'Release' }} param={`role:${TOPIC.id}:viewer`} level={level} via={via} />);
    await screen.findByText(ADA.display_name);
    for (const person of [ADA, BEA]) {
      const row = screen.getByText(person.display_name).closest('li');
      if (row === null) throw new Error('The owner has no roster row');
      expect(within(row).getByText(i18n.t('access.roster.owner'))).toBeVisible();
      expect(row).not.toHaveTextContent(level);
    }
    const row = screen.getByText(regular.display_name).closest('li');
    if (row === null) throw new Error('The ordinary viewer has no roster row');
    expect(row).toHaveTextContent(i18n.t('access.roster.personVia', { level, holder: via }));
    expect(within(row).queryByText(i18n.t('access.roster.owner'))).not.toBeInTheDocument();
    expect(calls.people).toHaveBeenCalledOnce();
  });

  it(`includes the primary and group owners in Roles without inventing editable Admin assignments in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme;
    show(<Roles place={{ kind: 'org', id: place.id, name: place.name }} />);
    const owners = await screen.findByRole('region', { name: i18n.t('access.owners.label') });
    expect(within(owners).getByText(ADA.display_name)).toBeVisible();
    expect(within(owners).getByText('Reviewers, Acme')).toBeVisible();
    expect(await within(owners).findByText(i18n.t('access.roster.primaryOwner'))).toBeVisible();
    expect(within(owners).getByText(i18n.t('access.roster.owner'))).toBeVisible();
    expect(within(owners).queryByRole('combobox')).not.toBeInTheDocument();
    expect(within(owners).queryByRole('button', { name: i18n.t('access.roles.take') })).not.toBeInTheDocument();
    expect(screen.queryByText(i18n.t('access.roles.none'))).not.toBeInTheDocument();
    expect(calls.people).not.toHaveBeenCalled();
  });
}
