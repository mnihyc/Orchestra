/** Management names every served locked sign-in proof in the person's language, including recovery codes. */
import { screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';

import type * as AdminApi from '@/api/admin';
import { Users } from '@/features/admin/Users';
import type { AdminUser, AdminUserLock, AdminUserPage } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { formatTimeOfDay } from '@/lib/time';

import { show } from './support';

vi.mock('@/api/admin', async (load) => ({ ...await load<typeof AdminApi>(),
  usersQuery: () => ({ queryKey: ['admin-user-locks'], initialPageParam: null,
    queryFn: (): Promise<AdminUserPage> => Promise.resolve({ items: [person], next_cursor: null }), getNextPageParam: () => undefined }),
  systemSettingsQuery: { queryKey: ['admin-user-lock-settings'], queryFn: () => Promise.resolve([]) },
}));
vi.mock('@/features/trace/Person', () => ({
  TracePerson: ({ user }: { user: AdminUser }) => <span>{user.display_name}</span>,
  useTraceEntry: () => () => [],
}));

const ways: AdminUserLock['way'][] = ['password', 'email_code', 'totp', 'recovery_code'];
const until = '2026-10-16T12:30:00Z';
const person: AdminUser = { id: 'locked-person', username: 'reader', display_name: 'A reader', kind: 'human', avatar: { text: 'R', color: 0 },
  email: 'reader@example.test', email_verified: true, status: 'active', is_system_admin: false, totp_enabled: true, email_code_second_step: false,
  locked: ways.map((way) => ({ way, until })), org_invited_at: null, created_at: '2026-10-01T00:00:00Z', last_seen_at: null };

afterEach(async () => { await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`keeps every lock and the one recovery explanation readable (${locale}, ${theme})`, async () => {
    await setLocale(locale);
    show(<div data-theme={theme}><Users search={{}} onSearch={vi.fn()} /></div>);
    const name = await screen.findByText(person.display_name, { exact: true });
    const row = name.closest('tr');
    if (row === null) throw new Error('The locked person lost their row');
    const { locale: clockLocale, timeZone } = useAppearance.getState();
    const time = formatTimeOfDay(until, { locale: clockLocale, timeZone });
    for (const way of ways) expect(within(row).getAllByText(i18n.t(`admin.users.locked.${way}`, { time }), { exact: true })).toHaveLength(1);
    expect(within(row).getAllByText(i18n.t('admin.users.locked.advice'), { exact: true })).toHaveLength(1);
    expect(row).not.toHaveTextContent('recovery_code');
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });
}
