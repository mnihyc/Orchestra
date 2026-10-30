/** Page states have page headings, approval lists respect sections, and profile previews have unique landmarks. */
import { cleanup, screen } from '@testing-library/react';
import type { AnchorHTMLAttributes, ReactNode } from 'react';
import type * as Router from '@tanstack/react-router';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type * as AdminModeApi from '@/api/adminMode';
import { ApprovalHeadings, CardFrame } from '@/features/approvals/Card';
import { Management } from '@/features/admin/Management';
import { ProfileContents } from '@/features/profile/Page';
import { ProfileSettingsPage } from '@/features/profile/Settings';
import type { UserProfile } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { NotFound } from '@/routes/not-found';
import { EmptyState } from '@/ui/EmptyState';
import { ADA } from '../doubles/chat-rows';
import { MADE_AT, me } from '../doubles/fixtures';
import { ALLOW_DENY, harnessApproval } from '../doubles/thread-rows';
import { show } from './support';

vi.mock('@tanstack/react-router', async (original) => ({
  ...await original<typeof Router>(),
  Link: ({ to, children, className }: AnchorHTMLAttributes<HTMLAnchorElement> & { to: string }) => <a href={to} className={className}>{children}</a>,
}));
vi.mock('@/shell/screen', () => ({ useScreenHeading: vi.fn() }));
vi.mock('@/features/help/GuideLink', () => ({ GuideLink: () => null }));
vi.mock('@/features/access/AdminReachNotice', () => ({ AdminReachNotice: () => null }));
vi.mock('@/features/settings/PersonalFrame', () => ({ PersonalFrame: ({ children }: { children: ReactNode }) => <>{children}</> }));
vi.mock('@/api/auth', () => ({ meQuery: { queryKey: ['structure-me'], queryFn: () => Promise.resolve(me(1, 'ada', 'Ada Lovelace', { is_system_admin: true })) } }));
vi.mock('@/api/adminMode', async (original) => {
  const actual = await original<typeof AdminModeApi>();
  const mode = { available: true, as_admin: false };
  return { ...actual, adminModeQuery: { ...actual.adminModeQuery, initialData: mode, queryFn: () => Promise.resolve(mode) } };
});
vi.mock('@/api/profile', () => ({
  profileKey: (username: string) => ['profile', username],
  profileQuery: (username: string) => ({ queryKey: ['structure-profile', username], queryFn: () => Promise.resolve(PROFILE) }),
  updateProfile: vi.fn(), replaceProfilePins: vi.fn(),
}));
vi.mock('@/api/personal', () => ({ favoritesQuery: { queryKey: ['structure-favorites'], queryFn: () => Promise.resolve([]) } }));
vi.mock('@/api/search', () => ({ namesQuery: () => ({ queryKey: ['structure-names'], queryFn: () => Promise.resolve([]) }) }));

const PROFILE: UserProfile = {
  user: ADA, bio: '', pronouns: '', links: [], local_time: null, member_of: [], personal_org: null,
  pins: [], activity: false, is_me: true, can_message: false, inspected: null, joined_at: MADE_AT,
  settings: { audience: 'system', show_time: false, show_activity: false },
};
const APPROVAL = harnessApproval('structure-approval', {
  title: 'Run the checks', threadId: 'thread', itemId: 'item', requester: ADA,
  request: { request_kind: 'command', detail: { command: 'check', cwd: '/work', reason: null }, options: ALLOW_DENY },
});

afterEach(async () => { cleanup(); await setLocale('en'); });

describe('headings that follow their page', () => {
  it.each(['en', 'zh-CN'] as const)('gives the actual not-found page an h1 and a way home in %s', async (locale) => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale);
    show(<NotFound />);
    expect(screen.getByRole('heading', { level: 1, name: t('shell.notFound.title') })).toBeVisible();
    expect(screen.getAllByRole('heading', { level: 1 })).toHaveLength(1);
    expect(screen.getByRole('link', { name: t('actions.home.label') })).toHaveAttribute('href', '/');
  });

  it('does not make a nested empty view another page heading', () => {
    show(<><h1>Approvals</h1><EmptyState icon={<span />} title="No waiting requests" compact /></>);
    expect(screen.getAllByRole('heading', { level: 1 })).toHaveLength(1);
    expect(screen.getByText('No waiting requests')).toBeVisible();
  });

  it.each(['en', 'zh-CN'] as const)('gives the actual Management off-state one h1 in %s', async (locale) => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale);
    show(<Management section="overview" search={{}} onSearch={vi.fn()} onSection={vi.fn()} />);
    expect(await screen.findByRole('heading', { level: 1, name: t('admin.title') })).toBeVisible();
    expect(screen.getAllByRole('heading', { level: 1 })).toHaveLength(1);
    expect(screen.getByRole('button', { name: t('adminReach.mode.enable') })).toBeVisible();
  });

  it('uses h2 immediately below a page heading and h3 inside a named section', () => {
    const view = show(<><h1>Approvals</h1><ApprovalHeadings level={2}><CardFrame approval={APPROVAL}><p>The exact request</p></CardFrame></ApprovalHeadings></>);
    expect(screen.getByRole('heading', { level: 2, name: APPROVAL.title })).toBeVisible();
    expect(screen.queryByRole('heading', { level: 3 })).not.toBeInTheDocument();
    view.rerender(<><h1>Approvals</h1><section><h2>Requested by you</h2><CardFrame approval={APPROVAL}><p>The exact request</p></CardFrame></section></>);
    expect(screen.getByRole('heading', { level: 3, name: APPROVAL.title })).toBeVisible();
  });
});

describe('landmarks of your page and its preview', () => {
  it.each(['en', 'zh-CN'] as const)('distinguishes the real pin editor from the preview landmarks in %s', async (locale) => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale);
    show(<ProfileSettingsPage />);
    expect(await screen.findByRole('region', { name: t('profile.page.pinned') })).toBeVisible();
    expect(screen.getAllByRole('region', { name: t('profile.page.pinned') })).toHaveLength(1);
    expect(screen.getByRole('region', { name: t('profile.page.previewPinned') })).toBeVisible();
    expect(screen.getByRole('complementary', { name: t('profile.page.previewAbout') })).toBeVisible();
    expect(screen.getByRole('region', { name: t('profile.page.preview') })).toBeVisible();
    expect(screen.getAllByRole('heading', { name: t('profile.page.pinned') })).toHaveLength(1);
    expect(screen.getByRole('heading', { level: 3, name: t('profile.page.previewPinned') })).toBeVisible();
  });

  it('keeps the public profile’s familiar landmark names', () => {
    show(<ProfileContents profile={PROFILE} />);
    expect(screen.getByRole('region', { name: 'Pinned' })).toBeVisible();
    expect(screen.getByRole('complementary', { name: 'About this person' })).toBeVisible();
    expect(screen.queryByRole('region', { name: 'Pin preview' })).not.toBeInTheDocument();
    expect(screen.getByRole('heading', { level: 1, name: PROFILE.user.display_name })).toBeVisible();
    expect(screen.getAllByText(PROFILE.user.display_name, { exact: true })).toHaveLength(1);
  });
});
