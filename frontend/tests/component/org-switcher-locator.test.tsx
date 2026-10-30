/** A large organization list uses the same input for its selected name and search. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import type * as Router from '@tanstack/react-router';
import { afterEach, expect, it, vi } from 'vitest';
import { LeaveDrawer, OrgSwitcher } from '@/features/navigation/OrgSwitcher';
import { org, userRef, me } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ navigate: vi.fn(), leave: vi.fn() }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(),
  useNavigate: () => calls.navigate,
  Link: ({ to, params, children, ...props }: { to: string; params: { slug: string }; children: ReactNode }) => <a {...props} href={to.replace('$slug', params.slug)}>{children}</a>,
}));
afterEach(() => { cleanup(); vi.clearAllMocks(); });

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
