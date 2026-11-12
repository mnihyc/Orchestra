/** A visible membership never turns a failed role read into an absent role or a private disclosure. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import type * as ProfileApi from '@/api/profile';
import { Memberships } from '@/features/profile/Memberships';
import type { Member, NodeRef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ADA, TOPIC } from '../unit/general-access-fixtures';
import { show } from './support';

const org: NodeRef = { ...TOPIC, kind: 'org', name: 'Visible org' };
const member: Member = { terms_hash: null, terms_accepted_at: null, user: ADA, domain: org, role: 'developer', is_owner: false, address: '@ada', also: 0,
  since: null, last_seen_at: null, suspended: false, second_step: null, second_step_required: null };
const read = vi.hoisted(() => vi.fn<() => Promise<Member | null>>());
vi.mock('@/api/profile', async (original) => ({ ...await original<typeof ProfileApi>(), profileMembershipQuery: () => ({ queryKey: ['profile-role'], queryFn: read, retry: false }) }));
beforeEach(() => { read.mockReset(); vi.stubGlobal('IntersectionObserver', undefined); });
afterEach(async () => { cleanup(); vi.unstubAllGlobals(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`keeps a failed membership role explicit and retries the same ordinary read (${locale})`, async () => {
    await setLocale(locale);
    read.mockRejectedValueOnce(new Error('temporary network failure')).mockResolvedValue(member);
    const user = userEvent.setup();
    show(<Memberships user={ADA} orgs={[org]} />);
    expect(await screen.findByText(i18n.t('implicitAvailability.roleFailed'))).toBeVisible();
    expect(screen.getByRole('link', { name: org.name ?? '' })).toBeVisible();
    await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    expect(await screen.findByText(i18n.t('roles.developer'))).toBeVisible();
    expect(read).toHaveBeenCalledTimes(2);
  });

  it(`keeps refusal separate from unreported data and exposes no private error details (${locale})`, async () => {
    await setLocale(locale);
    read.mockRejectedValue(new ApiError({ type: '', title: 'Denied', status: 403, code: 'authz.denied', detail: 'Hidden team and role', request_id: 'role-read' }, null));
    show(<Memberships user={ADA} orgs={[org]} />);
    expect(await screen.findByText(i18n.t('implicitAvailability.roleUnavailable'))).toBeVisible();
    expect(screen.queryByText('Hidden team and role')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: i18n.t('common.tryAgain') })).not.toBeInTheDocument();
  });

  it(`keeps a successful missing role distinct and never loads a redacted organization (${locale})`, async () => {
    await setLocale(locale); read.mockResolvedValue(null);
    const view = show(<Memberships user={ADA} orgs={[org]} />);
    expect(await screen.findByText(i18n.t('implicitAvailability.roleUnreported'))).toBeVisible();
    view.rerender(<Memberships user={ADA} orgs={[{ ...org, name: 'Hidden org name', redacted: true }]} />);
    expect(screen.queryByText('Hidden org name')).not.toBeInTheDocument();
    expect(screen.queryByText(i18n.t('roles.developer'))).not.toBeInTheDocument();
    expect(read).toHaveBeenCalledTimes(1);
  });
}
