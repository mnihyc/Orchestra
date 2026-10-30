/** Invitation role facts use the actual read error and recover through that same read. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import { InvitationRoleLine } from '@/features/onboarding/RoleSummary';
import type { FirstSteps } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ACME } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn<() => Promise<FirstSteps>>() }));
vi.mock('@/api/firstSteps', () => ({ firstStepsQuery: (org: string) => ({ queryKey: ['first-steps', org], queryFn: calls.read }) }));
function projection(): FirstSteps {
  return { org: { id: ACME.org, kind: 'org', name: 'Acme', redacted: false }, work_mode: 'team', start_here: null,
    steps: [], as_of: '2026-10-16T02:37:00Z', invitation_roles: [{ role: 'viewer', start_topics: true, start_threads: false,
      send_inputs: false, answer_requests: false, model_access_required: false }] };
}
afterEach(async () => { cleanup(); calls.read.mockReset(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`shows the failed read's cause with Retry, then the current role facts in ${locale}`, async () => {
    await setLocale(locale);
    calls.read.mockRejectedValueOnce(new ApiError({ type: 'orch:error:internal', code: 'internal', title: 'Read failed', detail: 'The synthetic invitation-role read failed.', status: 500, request_id: 'roles' }, null)).mockResolvedValue(projection());
    show(<InvitationRoleLine orgId={ACME.org} role="viewer" />);
    expect(await screen.findByRole('alert')).toHaveTextContent(i18n.t('errors.internal'));
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    expect(await screen.findByText(i18n.t('firstSteps.roles.defaults', { acts: i18n.t('firstSteps.roles.startTopics') }))).toBeVisible();
    expect(calls.read).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it(`treats an absent selected role as a failed response rather than claiming a missing feature in ${locale}`, async () => {
    await setLocale(locale); calls.read.mockResolvedValueOnce({ ...projection(), invitation_roles: [] }).mockResolvedValue(projection());
    show(<InvitationRoleLine orgId={ACME.org} role="viewer" />);
    expect(await screen.findByRole('alert')).toHaveTextContent(i18n.t('firstSteps.roles.missing'));
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    expect(await screen.findByText(i18n.t('firstSteps.roles.defaults', { acts: i18n.t('firstSteps.roles.startTopics') }))).toBeVisible();
    expect(calls.read).toHaveBeenCalledTimes(2);
  });
}
