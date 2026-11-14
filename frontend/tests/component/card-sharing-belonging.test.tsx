/** Managed sharing and reader-filtered homes/roles use the existing forms and directory facts. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { ApiError } from '@/api/client';
import { NetworkError } from '@/lib/requests';
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
  page: vi.fn<() => Promise<PermissionPage>>(), catalogue: vi.fn<() => Promise<typeof CATALOGUE.reply>>(), role: vi.fn<() => Promise<Member | null>>(),
  home: vi.fn<() => Promise<{ path: NodeRef[] }>>(), open: vi.fn(),
}));
vi.mock('@/api/access', async (original) => ({ ...await original<typeof AccessApi>(),
  pageQuery: (at: { id: string }) => ({ queryKey: ['card-page', at.id], queryFn: calls.page }),
  permissionsQuery: { queryKey: ['card-catalogue'], queryFn: calls.catalogue },
}));
vi.mock('@/api/profile', async (original) => ({ ...await original<typeof ProfileApi>(),
  profileMembershipQuery: (_user: unknown, domain: string) => ({ queryKey: ['card-role', domain], queryFn: calls.role }),
}));
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof NodesApi>(),
  nodeQuery: (id: string) => ({ queryKey: ['card-home', id], queryFn: calls.home }),
}));
vi.mock('@/features/navigation/places', async (loadOriginal) => ({ ...await loadOriginal<object>(), PlaceLink: ({ place, children, className }: { place: NodeRef; children: ReactNode; className?: string }) =>
  <a href={`/org/${place.id}`} className={className}>{children}</a> }));
vi.mock('@/features/access/AccessDialog', async (loadOriginal) => ({ ...await loadOriginal<object>(), openAccess: (...args: unknown[]) => { calls.open(...args); } }));

const previousTheme = document.documentElement.dataset.theme;
const wasDark = document.documentElement.classList.contains('dark');
beforeEach(() => {
  vi.clearAllMocks();
  vi.stubGlobal('IntersectionObserver', undefined);
  calls.page.mockResolvedValue(pageFor('document'));
  calls.catalogue.mockResolvedValue(CATALOGUE.reply);
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
    await screen.findByText(i18n.t('linkCards.sharing', { access: i18n.t('access.general.defaults', { place: TOPIC.name }) }));
    const button = screen.getByRole('button', { name: i18n.t('linkCards.share') });
    expect(screen.getByText(i18n.t('linkCards.sharing', { access: i18n.t('access.general.defaults', { place: TOPIC.name }) }))).toBeInTheDocument();
    await userEvent.click(button);
    expect(calls.open).toHaveBeenCalledOnce();
    expect(calls.open).toHaveBeenCalledWith(target, { share: true });
  },
);

it('keeps a read-only line without claiming another holder’s Manage entry as this reader’s authority', async () => {
  const page = pageFor('document');
  const readonly: PermissionPage = { ...page, owners_writable: false, rows: page.rows.map((row) => ({ ...row, writable: false })) };
  expect(cardSummary(readonly, CATALOGUE, 'document')).toBeNull();
  calls.page.mockResolvedValue(readonly);
  show(<CardSharing target={target} />);
  await screen.findByText(i18n.t('linkCards.sharingReadOnly'));
  expect(screen.getByRole('button', { name: i18n.t('linkCards.share') })).toHaveAttribute('aria-disabled', 'true');
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

for (const locale of ['en', 'zh-CN'] as const) {
  it(`AI-13 keeps checking sharing visible without inventing an audience (${locale})`, async () => {
    await setLocale(locale); let finish: ((page: PermissionPage) => void) | undefined;
    calls.page.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
    show(<CardSharing target={target} />);
    expect(screen.getByText(i18n.t('linkCards.sharingChecking'))).toBeInTheDocument();
    expect(screen.queryByText(i18n.t('linkCards.sharing', { access: i18n.t('access.general.private') }))).not.toBeInTheDocument();
    expect(document.body).not.toHaveTextContent(i18n.t('choiceCatalog.resourceAudience.owner.name'));
    const share = screen.getByRole('button', { name: i18n.t('linkCards.share') }); expect(share).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(share); expect(calls.open).not.toHaveBeenCalled();
    await act(async () => { finish?.(pageFor('document')); await Promise.resolve(); });
    await waitFor(() => expect(screen.getByRole('button', { name: i18n.t('linkCards.share') })).not.toHaveAttribute('aria-disabled', 'true'));
    await userEvent.click(screen.getByRole('button', { name: i18n.t('linkCards.share') })); expect(calls.open).toHaveBeenCalledWith(target, { share: true });
  });
  it(`AI-13 retries failed sharing reads without an Only me substitute (${locale})`, async () => {
    await setLocale(locale); calls.page.mockRejectedValueOnce(new NetworkError('failed'));
    show(<CardSharing target={target} />);
    await screen.findByText(i18n.t('linkCards.sharingUnavailable'));
    expect(screen.queryByText(i18n.t('linkCards.sharing', { access: i18n.t('access.general.private') }))).not.toBeInTheDocument();
    expect(document.body).not.toHaveTextContent(i18n.t('choiceCatalog.resourceAudience.owner.name'));
    expect(screen.getByRole('button', { name: i18n.t('linkCards.share') })).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    await screen.findByText(i18n.t('linkCards.sharing', { access: i18n.t('access.general.defaults', { place: TOPIC.name }) }));
    expect(calls.page).toHaveBeenCalledTimes(2);
  });
  it.each([403, 404, 501])(`AI-13 keeps a native %s sharing refusal private and distinct (${locale})`, async (status) => {
    await setLocale(locale); calls.page.mockRejectedValue(new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', title: 'Hidden audience', detail: 'Secret source and hidden people', status, request_id: 'sharing-read' }, null));
    show(<CardSharing target={target} />);
    await screen.findByText(i18n.t(status === 403 ? 'linkCards.sharingRestricted' : status === 501 ? 'linkCards.sharingUnsupported' : 'linkCards.sharingUnavailable'));
    expect(document.body).not.toHaveTextContent('Hidden audience'); expect(document.body).not.toHaveTextContent('Secret source and hidden people');
    const share = screen.getByRole('button', { name: i18n.t('linkCards.share') }); expect(share).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(share); expect(calls.open).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: i18n.t('common.tryAgain') })).toBeInTheDocument();
  });
  it(`AI-13 retains an authorized Share entrance when only its catalogue read fails (${locale})`, async () => {
    await setLocale(locale); calls.catalogue.mockRejectedValue(new NetworkError('failed'));
    show(<CardSharing target={target} />); await screen.findByText(i18n.t('linkCards.sharingUnavailable'));
    await waitFor(() => expect(screen.getByRole('button', { name: i18n.t('linkCards.share') })).not.toHaveAttribute('aria-disabled', 'true'));
    await userEvent.click(screen.getByRole('button', { name: i18n.t('linkCards.share') })); expect(calls.open).toHaveBeenCalledWith(target, { share: true });
  });
  it(`AI-13 never borrows a cached writable page after its current read fails (${locale})`, async () => {
    await setLocale(locale); const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    show(<QueryClientProvider client={cache}><CardSharing target={target} /></QueryClientProvider>);
    await screen.findByText(i18n.t('linkCards.sharing', { access: i18n.t('access.general.defaults', { place: TOPIC.name }) }));
    calls.page.mockRejectedValue(new NetworkError('failed'));
    await act(async () => { await cache.refetchQueries({ queryKey: ['card-page', target.id], exact: true }); });
    await screen.findByText(i18n.t('linkCards.sharingUnavailable'));
    const share = screen.getByRole('button', { name: i18n.t('linkCards.share') }); expect(share).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(share); expect(calls.open).not.toHaveBeenCalled(); cache.clear();
  });
}
