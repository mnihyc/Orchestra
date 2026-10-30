/** Profile edits use accepted answers, pin searches wait for typing, and the two saves never race. */
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { AnchorHTMLAttributes, ReactNode } from 'react';
import type * as Router from '@tanstack/react-router';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { ApiError } from '@/api/client';
import type * as ProfileApi from '@/api/profile';
import type * as AuthApi from '@/api/auth';
import type * as PersonalApi from '@/api/personal';
import type * as SearchApi from '@/api/search';
import type { NameSearch } from '@/api/search';
import { ProfileSettingsPage } from '@/features/profile/Settings';
import { ProfilePage } from '@/features/profile/Page';
import type { Favorite, NameHit, UserProfile } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ADA } from '../doubles/chat-rows';
import { MADE_AT, me } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn<() => Promise<UserProfile>>(), save: vi.fn<typeof ProfileApi.updateProfile>(), pins: vi.fn<typeof ProfileApi.replaceProfilePins>(), search: vi.fn<(q: string) => Promise<NameHit[]>>(), favorites: vi.fn<() => Promise<Favorite[]>>() }));
vi.mock('@/api/profile', async (original) => {
  const actual = await original<typeof ProfileApi>();
  return { ...actual, profileQuery: (username: string) => ({ queryKey: actual.profileKey(username), queryFn: calls.read, retry: false }), updateProfile: calls.save, replaceProfilePins: calls.pins };
});
vi.mock('@/api/auth', async (original) => ({ ...await original<typeof AuthApi>(), meQuery: { queryKey: ['editing-me'], queryFn: () => Promise.resolve(me(1, 'ada', 'Ada Lovelace')) } }));
vi.mock('@/api/personal', async (original) => ({ ...await original<typeof PersonalApi>(), favoritesQuery: { queryKey: ['editing-favorites'], queryFn: calls.favorites } }));
vi.mock('@/api/search', async (original) => ({ ...await original<typeof SearchApi>(), namesQuery: (search: NameSearch) => ({ queryKey: ['editing-names', search.q], queryFn: () => calls.search(search.q), placeholderData: (previous: NameHit[] | undefined) => previous }) }));
vi.mock('@/shell/screen', () => ({ useScreenHeading: vi.fn() }));
vi.mock('@/shell/Page', () => ({ Page: ({ children }: { children: ReactNode }) => <>{children}</> }));
vi.mock('@/features/settings/PersonalFrame', () => ({ PersonalFrame: ({ children }: { children: ReactNode }) => <>{children}</> }));
vi.mock('@/features/access/AdminReachNotice', () => ({ AdminReachNotice: () => null }));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<typeof Router>(), Link: ({ to, children, className }: AnchorHTMLAttributes<HTMLAnchorElement> & { to: string }) => <a href={to} className={className}>{children}</a> }));

const PROFILE: UserProfile = { user: ADA, bio: 'Initial biography', pronouns: '', links: [], local_time: null, member_of: [], personal_org: null, pins: [], activity: false, is_me: true, can_message: false, inspected: null, joined_at: MADE_AT, settings: { audience: 'system', show_time: false, show_activity: false } };
function name(id: string, title: string, root = 'Acme'): NameHit {
  return { id, kind: 'topic', name: title, path: [{ id: root, name: root, kind: 'org', redacted: false }], address: null, archived: false, avatar: null, color: null, custom_color: null, owner: null, recent: false, tag: null, thread_attention: null, username: null };
}

beforeEach(() => { vi.clearAllMocks(); calls.read.mockReset().mockResolvedValue(PROFILE); calls.save.mockReset(); calls.pins.mockReset(); calls.search.mockReset().mockResolvedValue([]); calls.favorites.mockReset().mockResolvedValue([]); });
afterEach(async () => { cleanup(); await setLocale('en'); });

describe('editing a profile through its own controls', () => {
  it.each(['en', 'zh-CN'] as const)('saves the changed field automatically, then uses the accepted value without a profile refetch in %s', async (locale) => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale), user = userEvent.setup();
    let accept: (profile: UserProfile) => void = () => undefined;
    calls.save.mockImplementation(() => new Promise((resolve) => { accept = resolve; }));
    calls.search.mockResolvedValue([name('topic-one', 'Selected topic')]);
    show(<ProfileSettingsPage />);
    const bio = await screen.findByRole('textbox', { name: t('profile.page.bio') });
    expect(screen.getByText(t('profile.page.pinHelp'))).toBeVisible();
    await user.clear(bio); await user.type(bio, '  Written biography  ');
    await waitFor(() => expect(calls.save).toHaveBeenCalledExactlyOnceWith({ bio: '  Written biography  ' }));
    expect(bio).toBeEnabled();
    expect(screen.getByRole('combobox', { name: t('profile.page.audience') })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Selected topic' })).toBeDisabled();
    expect(screen.queryByRole('button', { name: t('common.save') })).not.toBeInTheDocument();
    await act(async () => { accept({ ...PROFILE, bio: 'Written biography' }); await Promise.resolve(); });
    await waitFor(() => expect(bio).toBeEnabled());
    expect(bio).toHaveValue('Written biography');
    expect(screen.getByText(t('settings.compact.saved'), { exact: true })).toBeVisible();
    expect(screen.getByRole('button', { name: t('settings.compact.undoNamed', { name: t('profile.page.settingsTitle') }) })).toBeVisible();
    expect(calls.save).toHaveBeenCalledTimes(1); expect(calls.read).toHaveBeenCalledTimes(1); expect(calls.pins).not.toHaveBeenCalled();
  });

  it('keeps a refused draft editable and makes no follow-up profile read', async () => {
    calls.save.mockRejectedValue(new ApiError({ type: 'orch:error:request.value_refused', title: 'Refused', status: 422, code: 'request.value_refused', detail: 'Choose an HTTPS address.', request_id: 'profile-refusal' }, null));
    show(<ProfileSettingsPage />);
    const bio = await screen.findByRole('textbox', { name: 'About you' });
    await userEvent.clear(bio); await userEvent.type(bio, 'Keep these words after refusal.');
    await waitFor(() => expect(screen.getByRole('alert')).toBeVisible());
    expect(bio).toBeEnabled(); expect(bio).toHaveValue('Keep these words after refusal.');
    expect(calls.read).toHaveBeenCalledTimes(1);
  });

  it('keeps recents visible initially, then asks once for the finished name and hides stale choices', async () => {
    calls.search.mockImplementation((q) => Promise.resolve(q === '' ? [name('recent', 'Recent item')] : [name('wanted', 'Fix login')]));
    show(<ProfileSettingsPage />);
    await screen.findByRole('textbox', { name: 'Find something to pin' });
    await screen.findByRole('button', { name: 'Recent item' });
    const field = screen.getByRole('textbox', { name: 'Find something to pin' });
    await userEvent.type(field, 'Fix login');
    expect(screen.queryByRole('button', { name: 'Recent item' })).not.toBeInTheDocument();
    await screen.findByRole('button', { name: 'Fix login' });
    expect(calls.search.mock.calls.map(([q]) => q)).toEqual(['', 'Fix login']);
  });

  it('uses the common shortest place qualifier to distinguish equal candidate names', async () => {
    calls.search.mockResolvedValue([name('one', 'Same topic', 'Acme'), name('two', 'Same topic', 'Beta')]);
    show(<ProfileSettingsPage />);
    await screen.findByRole('button', { name: 'Same topic · Acme' });
    expect(screen.getByRole('button', { name: 'Same topic · Beta' })).toBeVisible();
    expect(calls.search).toHaveBeenCalledTimes(1);
  });

  it('offers a deliberate retry for a failed pin lookup without reloading the profile', async () => {
    calls.search.mockRejectedValueOnce(new Error('Names could not be loaded.')).mockResolvedValue([]);
    show(<ProfileSettingsPage />);
    const retry = await screen.findByRole('button', { name: 'Try again' });
    await userEvent.click(retry);
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Try again' })).not.toBeInTheDocument());
    expect(calls.search).toHaveBeenCalledTimes(2);
    expect(calls.read).toHaveBeenCalledTimes(1);
  });

  it('serializes pin saving with the other profile fields and replaces selection from the accepted profile', async () => {
    calls.search.mockResolvedValue([name('topic-one', 'Selected topic')]);
    let accept: (profile: UserProfile) => void = () => undefined;
    calls.pins.mockImplementation(() => new Promise((resolve) => { accept = resolve; }));
    show(<ProfileSettingsPage />);
    const candidate = await screen.findByRole('button', { name: 'Selected topic' });
    expect(candidate).toBeEnabled();
    await userEvent.click(candidate);
    const pins = screen.getByRole('region', { name: 'Pinned' });
    // Adding the pin starts its automatic write; the selected row cannot start an overlapping inverse.
    expect(within(pins).getByRole('button', { name: 'Unpin Selected topic' })).toBeDisabled();
    await waitFor(() => expect(calls.pins).toHaveBeenCalledTimes(1));
    expect(screen.getByRole('textbox', { name: 'About you' })).toBeDisabled();
    expect(within(pins).getByRole('button', { name: 'Unpin Selected topic' })).toBeDisabled();
    await userEvent.click(within(pins).getByRole('button', { name: 'Unpin Selected topic' }));
    expect(calls.pins).toHaveBeenCalledTimes(1);
    await act(async () => { accept(PROFILE); await Promise.resolve(); });
    await waitFor(() => expect(within(pins).queryByRole('button', { name: 'Unpin Selected topic' })).not.toBeInTheDocument());
    expect(screen.getByRole('textbox', { name: 'About you' })).toBeEnabled();
    expect(screen.queryByRole('button', { name: 'Save pins' })).not.toBeInTheDocument();
    expect(calls.pins).toHaveBeenCalledExactlyOnceWith([{ node_id: 'topic-one' }]);
    expect(calls.read).toHaveBeenCalledTimes(1); expect(calls.save).not.toHaveBeenCalled();
  });

  it.each(['en', 'zh-CN'] as const)('gives an unavailable private/missing profile one heading and a way home in %s', async (locale) => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale);
    calls.read.mockRejectedValue(new ApiError({ type: 'orch:error:not_found', title: 'Not found', status: 404, code: 'not_found', detail: 'There is no page.', request_id: 'profile-hidden' }, null));
    show(<ProfilePage username="unavailable" />);
    expect(await screen.findByRole('heading', { level: 1, name: t('profile.page.missing') })).toBeVisible();
    expect(screen.getAllByRole('heading', { level: 1 })).toHaveLength(1);
    expect(screen.getByRole('link', { name: t('actions.home.label') })).toHaveAttribute('href', '/');
    expect(calls.read).toHaveBeenCalledTimes(1);
  });
});
