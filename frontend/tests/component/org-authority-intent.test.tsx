/** Same-user org reads may pause revealed content, but cannot discard a person's open invitation draft. */
import type * as Nodes from '@/api/nodes';
import type * as Auth from '@/api/auth';
import type * as Directory from '@/api/directory';
import type * as Users from '@/api/users';
import type * as Settings from '@/api/settings';
import type * as Live from '@/realtime/live';
import type * as Router from '@tanstack/react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { applyChanges, people } from '@/api/acts';
import { client, ApiError } from '@/api/client';
import { orgBySlugQuery } from '@/api/nodes';
import { OrgHome } from '@/features/org/Home';
import { OrgSettings } from '@/features/settings/org/OrgSettings';
import type { Org } from '@/generated/api-types';
import { setLocale } from '@/i18n';
import { TooltipProvider } from '@/ui/Tooltip';
import { Toaster } from '@/ui/Toast';
import { me, memberAccess, meta, org, userRef } from '../doubles/fixtures';

const calls = vi.hoisted(() => ({ org: vi.fn<(slug: string) => Promise<Org>>(), create: vi.fn() }));
vi.mock('@/api/nodes', async (original) => {
  const api = await original<typeof Nodes>();
  return { ...api, orgBySlugQuery: (slug: string) => ({ ...api.orgBySlugQuery(slug), queryFn: () => calls.org(slug), staleTime: Infinity }) };
});
vi.mock('@/api/auth', async (original) => ({ ...await original<typeof Auth>(), metaQuery: { queryKey: ['meta'], queryFn: () => Promise.resolve(meta({ mail: false })), staleTime: Infinity } }));
vi.mock('@/api/directory', async (original) => {
  const api = await original<typeof Directory>();
  return { ...api,
    membersQuery: (...args: Parameters<typeof api.membersQuery>) => ({ ...api.membersQuery(...args), queryFn: () => Promise.resolve({ items: [], next_cursor: null }) }),
    suspensionsQuery: (id: string) => ({ ...api.suspensionsQuery(id), queryFn: () => Promise.resolve([]) }),
  };
});
vi.mock('@/api/users', async (original) => {
  const api = await original<typeof Users>();
  return { ...api, createInvitation: calls.create,
    invitationsQuery: (id: string) => ({ ...api.invitationsQuery(id), queryFn: () => Promise.resolve([]) }),
  };
});
vi.mock('@/api/settings', async (original) => {
  const api = await original<typeof Settings>();
  return { ...api,
    settingsQuery: (id: string) => ({ ...api.settingsQuery(id), queryFn: () => Promise.resolve([]) }),
    targetSettingsQuery: (...args: Parameters<typeof api.targetSettingsQuery>) => ({ ...api.targetSettingsQuery(...args), queryFn: () => Promise.resolve([]) }),
  };
});
vi.mock('@/realtime/live', async (original) => ({ ...await original<typeof Live>(), useOrgTreeFeed: () => undefined }));
vi.mock('@/shell/screen', () => ({ useScreenHeading: () => undefined }));
vi.mock('@/shell/org', () => ({ selectOrg: () => undefined }));
vi.mock('@/features/workbench/settingsTabTitle', () => ({ useSettingsTabTitle: (title: string) => title }));
vi.mock('@/features/settings/Frame', () => ({ SettingsFrame: ({ children }: { children: ReactNode }) => <section>{children}</section> }));
vi.mock('@/features/settings/org/Outsiders', () => ({ Outsiders: () => null }));
vi.mock('@/features/onboarding/RoleSummary', () => ({ InvitationRoleLine: () => null }));
vi.mock('@/features/org/Activity', () => ({ OrgActivity: () => null, ActivityFeed: () => null }));
vi.mock('@/features/org/HeaderMenu', () => ({ OrgHeaderMenu: () => null }));
vi.mock('@/features/navigation/favorites', () => ({ StarButton: () => null }));
vi.mock('@/features/navigation/OrgSwitcher', () => ({ orgInitial: () => 'O', orgColor: () => 0 }));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<typeof Router>(),
  useRouter: () => ({ history: { push: vi.fn() } }), Link: ({ children }: { children: ReactNode }) => <a href="#local">{children}</a>,
}));

const owner = userRef(me(1, 'owner', 'Owner'));
const place = org('org-one', 'one', 'One', owner, { access: { ...memberAccess, owns: true,
  permissions: ['org.view', 'org.interact', 'org.manage', 'members.manage'] } });
const other = { ...place, id: 'org-two', slug: 'two', address: 'two', name: 'Two' };
const before = client.session.getState();
let cache: QueryClient;
function Host({ kind, slug = 'one' }: { kind: 'home' | 'settings'; slug?: string }) {
  return <QueryClientProvider client={cache}><TooltipProvider><Toaster>
    {kind === 'home' ? <OrgHome slug={slug} tab="members" onTab={() => undefined} /> : <OrgSettings slug={slug} section="members" />}
  </Toaster></TooltipProvider></QueryClientProvider>;
}
function deferred() {
  let resolve!: (value: Org) => void, reject!: (error: Error) => void;
  const promise = new Promise<Org>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
async function openDraft() {
  await userEvent.click(await screen.findByRole('button', { name: 'Invite…' }));
  const field = await screen.findByRole('spinbutton', { name: 'How many people' });
  fireEvent.change(field, { target: { value: '2' } });
  expect(field).toHaveValue(2);
  return field;
}
beforeEach(async () => {
  await setLocale('en'); calls.org.mockReset().mockImplementation((slug) => Promise.resolve(slug === 'two' ? other : place)); calls.create.mockReset();
  cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity }, mutations: { retry: false } } });
  client.session.setState({ ...before, userId: owner.id });
});
afterEach(() => { cleanup(); cache.clear(); client.session.setState(before); });

for (const kind of ['home', 'settings'] as const) {
  it(`${kind} keeps the actual invitation fields across a pending same-org authority refresh`, async () => {
    render(<Host kind={kind} />); await openDraft();
    const pending = deferred(); calls.org.mockReturnValue(pending.promise);
    let refreshing!: Promise<void>;
    await act(async () => { refreshing = applyChanges(people(), cache); await Promise.resolve(); });
    await waitFor(() => expect(cache.getQueryData(orgBySlugQuery('one').queryKey)).toBeUndefined());
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Create invitation' })).not.toBeInTheDocument();
    await act(async () => { pending.resolve({ ...place, member_count: place.member_count + 1 }); await refreshing; });
    expect(await screen.findByRole('spinbutton', { name: 'How many people' })).toHaveValue(2);
    expect(screen.getByRole('dialog')).toBeVisible();
    expect(calls.create).not.toHaveBeenCalled();
  });

  it(`${kind} discards the old dialog on a confirmed permission refusal instead of retaining private hidden controls`, async () => {
    render(<Host kind={kind} />); const old = await openDraft();
    const pending = deferred(); calls.org.mockReturnValue(pending.promise);
    let refreshing!: Promise<void>;
    await act(async () => { refreshing = applyChanges(people(), cache); await Promise.resolve(); });
    await act(async () => {
      pending.reject(new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', title: '', detail: '', status: 403, request_id: '' }, null));
      await refreshing;
    });
    await waitFor(() => expect(old.isConnected).toBe(false));
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    expect(screen.queryByRole('spinbutton', { name: 'How many people', hidden: true })).not.toBeInTheDocument();
    expect(calls.create).not.toHaveBeenCalled();
  });

  it(`${kind} discards another user's draft and a different org's draft`, async () => {
    const view = render(<Host kind={kind} />); const first = await openDraft();
    await act(async () => { client.session.setState({ userId: 'another-reader' }); await Promise.resolve(); });
    await waitFor(() => expect(first.isConnected).toBe(false));
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    await openDraft();
    cache.setQueryData(orgBySlugQuery('two').queryKey, other);
    view.rerender(<Host kind={kind} slug="two" />);
    await waitFor(() => expect(document.querySelector('[role="dialog"]')).toBeNull());
    expect(calls.create).not.toHaveBeenCalled();
  });
}


for (const kind of ['home', 'settings'] as const) {
  it(`${kind} retires an open invitation when current org access loses Members Manage without losing org visibility`, async () => {
    const permitted: Org = { ...place, access: { ...place.access, owns: false } };
    const withdrawn: Org = { ...permitted, access: { ...permitted.access,
      permissions: permitted.access.permissions.filter((permission) => permission !== 'members.manage') } };
    calls.org.mockResolvedValue(permitted);
    render(<Host kind={kind} />);
    const field = await openDraft();
    const submit = screen.getByRole('button', { name: 'Create invitation' });
    const pending = deferred(); calls.org.mockReturnValue(pending.promise);
    let refreshing!: Promise<void>;
    await act(async () => { refreshing = applyChanges(people(), cache); await Promise.resolve(); });
    await waitFor(() => expect(cache.getQueryData(orgBySlugQuery('one').queryKey)).toBeUndefined());
    await act(async () => { pending.resolve(withdrawn); await refreshing; });
    await waitFor(() => expect(cache.getQueryData(orgBySlugQuery('one').queryKey)).toEqual(withdrawn));
    const invite = await screen.findByRole('button', { name: 'Invite…' });
    expect(invite).toHaveAttribute('aria-disabled', 'true');
    expect(field.isConnected).toBe(false); expect(submit.isConnected).toBe(false);
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    expect(screen.queryByRole('spinbutton', { name: 'How many people', hidden: true })).not.toBeInTheDocument();
    await userEvent.click(invite);
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    expect(calls.create).not.toHaveBeenCalled();

    // A later permission grant does not resurrect the retired intent or its entered values.
    calls.org.mockResolvedValue(permitted);
    await act(async () => { await applyChanges(people(), cache); });
    await waitFor(() => expect(screen.getByRole('button', { name: 'Invite…' })).not.toHaveAttribute('aria-disabled', 'true'));
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: 'Invite…' }));
    expect(await screen.findByRole('spinbutton', { name: 'How many people' })).toHaveValue(1);
    expect(calls.create).not.toHaveBeenCalled();
  });
}
