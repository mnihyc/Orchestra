/** Managed sharing and reader-filtered homes/roles use the existing forms and directory facts. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as AccessApi from '@/api/access';
import type * as NodesApi from '@/api/nodes';
import type * as ProfileApi from '@/api/profile';
import { CardSharing } from '@/features/access/CardSharing';
import { cardSummary } from '@/features/access/cardSummary';
import { CardHome } from '@/features/navigation/CardHome';
import { Memberships } from '@/features/profile/Memberships';
import type { Member, NodeRef, PermissionPage } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ADA, CATALOGUE, TOPIC, pageFor } from '../unit/general-access-fixtures';
import { show } from './support';

const ORG: NodeRef = { ...TOPIC, kind: 'org', name: 'Acme' };
const member: Member = { terms_hash: null, terms_accepted_at: null, user: ADA, domain: ORG, role: 'developer', is_owner: false, address: '@ada', also: 0,
  since: null, last_seen_at: null, suspended: false, second_step: null, second_step_required: null };
const target = { kind: 'document' as const, id: 'document-1', name: 'Rollout plan' };
const calls = vi.hoisted(() => ({
  page: vi.fn<() => Promise<PermissionPage>>(), role: vi.fn<() => Promise<Member | null>>(),
  home: vi.fn<() => Promise<{ path: NodeRef[] }>>(), open: vi.fn(),
}));
vi.mock('@/api/access', async (original) => ({ ...await original<typeof AccessApi>(),
  pageQuery: (at: { id: string }) => ({ queryKey: ['card-page', at.id], queryFn: calls.page }),
  permissionsQuery: { queryKey: ['card-catalogue'], queryFn: () => Promise.resolve(CATALOGUE.reply) },
}));
vi.mock('@/api/profile', async (original) => ({ ...await original<typeof ProfileApi>(),
  profileMembershipQuery: (_user: unknown, domain: string) => ({ queryKey: ['card-role', domain], queryFn: calls.role }),
}));
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof NodesApi>(),
  nodeQuery: (id: string) => ({ queryKey: ['card-home', id], queryFn: calls.home }),
}));
vi.mock('@/features/navigation/places', () => ({ PlaceLink: ({ place, children, className }: { place: NodeRef; children: ReactNode; className?: string }) =>
  <a href={`/org/${place.id}`} className={className}>{children}</a> }));
vi.mock('@/features/access/AccessDialog', () => ({ openAccess: (...args: unknown[]) => calls.open(...args) }));

const previousTheme = document.documentElement.dataset.theme;
const wasDark = document.documentElement.classList.contains('dark');
beforeEach(() => {
  vi.clearAllMocks();
  vi.stubGlobal('IntersectionObserver', undefined);
  calls.page.mockResolvedValue(pageFor('document'));
  calls.role.mockResolvedValue(member);
  calls.home.mockResolvedValue({ path: [ORG, { ...TOPIC, id: 'topic-home' }] });
});
afterEach(async () => {
  cleanup(); vi.unstubAllGlobals();
  if (previousTheme === undefined) delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = previousTheme;
  document.documentElement.classList.toggle('dark', wasDark);
  await setLocale('en');
});

it.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)(
  'opens the same short form from one sharing line in %s/%s', async (locale, theme) => {
    await setLocale(locale);
    document.documentElement.dataset.theme = theme;
    document.documentElement.classList.toggle('dark', theme === 'dark');
    show(<CardSharing target={target} />);
    const button = await screen.findByRole('button', { name: i18n.t('linkCards.share') });
    expect(screen.getByText(i18n.t('linkCards.sharing', { access: i18n.t('access.general.defaults', { place: TOPIC.name }) }))).toBeInTheDocument();
    await userEvent.click(button);
    expect(calls.open).toHaveBeenCalledOnce();
    expect(calls.open).toHaveBeenCalledWith(target, { share: true });
  },
);

it('offers no sharing line from another holder’s Manage entry when this reader cannot change the page', async () => {
  const page = pageFor('document');
  const readonly: PermissionPage = { ...page, owners_writable: false, rows: page.rows.map((row) => ({ ...row, writable: false })) };
  expect(cardSummary(readonly, CATALOGUE, 'document')).toBeNull();
  calls.page.mockResolvedValue(readonly);
  show(<CardSharing target={target} />);
  await waitFor(() => expect(calls.page).toHaveBeenCalledOnce());
  expect(screen.queryByRole('button', { name: i18n.t('linkCards.share') })).toBeNull();
});

it.each(['en', 'zh-CN'] as const)('keeps the whole readable home path and no redacted name in %s', async (locale) => {
  await setLocale(locale);
  calls.home.mockResolvedValue({ path: [{ ...ORG, id: 'hidden', name: 'Private parent', redacted: true }, ORG, TOPIC] });
  show(<CardHome place={TOPIC} />);
  expect(await screen.findByRole('link', { name: 'Acme' })).toBeInTheDocument();
  expect(screen.getByRole('link', { name: 'Fix login' })).toBeInTheDocument();
  expect(screen.getByText(i18n.t('ui.nameUnknown'))).toBeInTheDocument();
  expect(document.body).not.toHaveTextContent('Private parent');
});

it.each(['en', 'zh-CN'] as const)('shows the directory role beside its own org in %s', async (locale) => {
  await setLocale(locale);
  show(<Memberships user={ADA} orgs={[ORG]} />);
  expect(await screen.findByText(i18n.t('roles.developer'))).toBeInTheDocument();
  expect(screen.getByRole('link', { name: 'Acme' })).toBeInTheDocument();
});

it('keeps a visible org without inventing a role when the directory cannot disclose membership', async () => {
  calls.role.mockResolvedValue(null);
  show(<Memberships user={ADA} orgs={[ORG]} />);
  await waitFor(() => expect(calls.role).toHaveBeenCalledOnce());
  expect(screen.getByRole('link', { name: 'Acme' })).toBeInTheDocument();
  expect(screen.queryByText(i18n.t('roles.developer'))).toBeNull();
});
