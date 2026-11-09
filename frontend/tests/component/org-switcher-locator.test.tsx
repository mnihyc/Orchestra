/** A large organization list uses the same input for its selected name and search. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import type * as Router from '@tanstack/react-router';
import { afterEach, expect, it, vi } from 'vitest';
import { LeaveDrawer, OrgSwitcher } from '@/features/navigation/OrgSwitcher';
import { i18n, setLocale } from '@/i18n';
import { org, userRef, me } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ navigate: vi.fn(), leave: vi.fn() }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(),
  useNavigate: () => calls.navigate,
  Link: ({ to, params, children, ...props }: { to: string; params: { slug: string }; children: ReactNode }) => <a {...props} href={to.replace('$slug', params.slug)}>{children}</a>,
}));
afterEach(async () => { cleanup(); vi.clearAllMocks(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`distinguishes loading organizations from an empty list and keeps a loaded selection in ${locale}`, async () => {
    await setLocale(locale);
    const owner = userRef(me(1, 'ada', 'Ada'));
    const current = org('acme', 'acme', 'Acme', owner);
    const view = show(<OrgSwitcher orgs={[]} current={null} loading />);
    expect(screen.getByRole('combobox')).toHaveValue(i18n.t('common.loading'));
    expect(screen.getByRole('combobox')).toHaveAttribute('placeholder', i18n.t('common.loading'));
    view.rerender(<OrgSwitcher orgs={[current]} current={current} loading />);
    expect(screen.getByRole('combobox')).toHaveValue('Acme');
    expect(screen.getByRole('link', { name: /Acme/ })).toHaveAttribute('href', '/org/acme');
    view.rerender(<OrgSwitcher orgs={[]} current={null} />);
    expect(screen.getByRole('combobox')).toHaveValue('');
    expect(screen.getByRole('combobox')).toHaveAttribute('placeholder', i18n.t('navigation.chooseOrg'));
  });
}

it('finds one of twenty organizations in one field and keeps a direct home link', async () => {
  const owner = userRef(me(1, 'ada', 'Ada'));
  const orgs = Array.from({ length: 20 }, (_, index) => org(`team-${index}`, `team-${index}`, `Team ${index}`, owner));
  const current = orgs[0]; if (current === undefined) throw new Error('A current organization is required');
  show(<LeaveDrawer.Provider value={calls.leave}><OrgSwitcher orgs={orgs} current={current} /></LeaveDrawer.Provider>);
  const input = screen.getByRole('combobox');
  expect(input).toHaveValue('Team 0');
  expect(screen.getByRole('link', { name: /Team 0/ })).toHaveAttribute('href', '/org/team-0');
  const user = userEvent.setup(); await user.click(input);
  expect(screen.getAllByRole('option')).toHaveLength(20);
  await user.type(input, 'Team 19');
  expect(screen.getAllByRole('option')).toHaveLength(1);
  await user.keyboard('{Enter}');
  expect(calls.navigate).toHaveBeenCalledWith({ to: '/org/$slug', params: { slug: 'team-19' } });
  expect(calls.leave).toHaveBeenCalledTimes(1);
  expect(screen.getAllByRole('combobox')).toHaveLength(1);
});
