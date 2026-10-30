/** Domain members and inherited access are distinguished without hiding people from the department. */
import { cleanup, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Access from '@/api/access';
import { InheritedMembers } from '@/features/settings/org/InheritedMembers';
import type { NodeRef, RoleHeld } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { org } from '../doubles/fixtures';
import { ADA, BEA } from '../unit/general-access-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ roles: vi.fn() }));
const root: NodeRef = { id: 'acme', kind: 'org', name: 'Acme', redacted: false };
const department = org('dep2', 'dep2', 'Research', ADA, { parent_id: 'acme', org_id: 'acme', depth: 1, member_count: 0 });
const held = (id: string, role: RoleHeld['role']): RoleHeld => ({ id, role, subject: { kind: 'user', id: BEA.id, user: { ...BEA, address: 'acme.bea' } },
  node: root, domain: true, revision: 1, outside: false, created_at: '2026-10-15T00:00:00Z', created_by: ADA, expires_at: null, note: '' });
vi.mock('@/api/access', async (original) => ({ ...await original<typeof Access>(), rolesQuery: (id: string) => ({ queryKey: ['inherited-roles', id], queryFn: () => {
  calls.roles(id); return Promise.resolve({ own: [], above: [held('viewer', 'viewer'), held('developer', 'developer')], can_give: [] });
} }) }));
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const before = useAppearance.getState();
const themeBefore = document.documentElement.dataset.theme;
const darkBefore = document.documentElement.classList.contains('dark');
afterEach(async () => {
  cleanup(); vi.clearAllMocks(); useAppearance.setState(before); document.documentElement.classList.toggle('dark', darkBefore);
  if (themeBefore === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = themeBefore;
  await setLocale('en');
});
it.each(modes)('shows the actual inherited person once, with the declared level and source, in %s/%s', async (locale, theme) => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
  const view = show(<InheritedMembers org={department} members={[]} query="" />);
  expect(await screen.findByText('Bea')).toBeVisible();
  expect(screen.getAllByText('Bea')).toHaveLength(1);
  expect(screen.getByText(i18n.t('roles.developer'))).toBeVisible();
  expect(screen.getByText(i18n.t('access.roster.inheritedFrom', { place: 'Acme' }))).toBeVisible();
  expect(screen.getByText('Ada')).toBeVisible();
  expect(screen.getByText(i18n.t('access.roster.owner'))).toBeVisible();
  expect(view.container).not.toHaveTextContent('acme.bea');
  expect(calls.roles).toHaveBeenCalledTimes(1);
  expect(calls.roles).toHaveBeenCalledWith('dep2');
  view.rerender(<InheritedMembers org={department} members={[]} query="unrelated" />);
  expect(screen.queryByText('Bea')).not.toBeInTheDocument();
  expect(screen.queryByText('Ada')).not.toBeInTheDocument();
  expect(calls.roles).toHaveBeenCalledTimes(1);
});
