/** Operation counts use the server's complete units and reveal only permitted names on focus or tap. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ActivityCount, BindingCount } from '@/features/storage/Impact';
import type { ActivityGroup } from '@/generated/api-types';
import { setLocale } from '@/i18n';
import { Checkbox } from '@/ui/Checkbox';
import { PeopleCount } from '@/ui/PeopleList';
import { me, userRef } from '../doubles/fixtures';
import { show } from './support';

vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
const group: ActivityGroup = { count: 6, threads: 4, hidden: 3, since: '2026-10-01T00:00:00Z', visible: [
  { object: { kind: 'thread', id: '00000000-0000-4000-8000-000000000101' }, name: 'Repair redirects', relation: 'binding', until: null },
] };
afterEach(async () => { cleanup(); await setLocale('en'); });

it('waiting counts preserve six messages in four threads and open visible names with a private remainder', async () => {
  const user = userEvent.setup();
  show(<ActivityCount group={group} kind="waiting" />);
  expect(screen.getByText('6 messages in 4 threads')).toBeVisible();
  expect(screen.getByText(/Waiting since/)).toBeVisible();
  await user.click(screen.getByRole('button', { name: '6 messages in 4 threads' }));
  expect(await screen.findByText('Repair redirects')).toBeVisible();
  expect(screen.getByText('and 3 more')).toBeVisible();
  await user.keyboard('{Escape}');
  expect(screen.queryByText('Repair redirects')).not.toBeInTheDocument();
});

it('keyboard focus exposes the same bounded current turn names without inventing private identities', async () => {
  const user = userEvent.setup();
  show(<ActivityCount group={{ ...group, count: 4 }} kind="running" />);
  await user.tab();
  expect(screen.getByRole('button', { name: '4 turns stop now' })).toHaveFocus();
  expect(await screen.findByText('Repair redirects')).toBeVisible();
  expect(screen.getByText('4 turns stop now')).toBeVisible();
  expect(screen.getByText('and 3 you cannot see')).toBeVisible();
});

it('zero groups show no figure or inspection control', () => {
  show(<><ActivityCount group={{ count: 0, threads: 0, hidden: 0, since: null, visible: [] }} kind="running" /><BindingCount group={undefined} /></>);
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
  expect(screen.queryByText(/turns stop now/)).not.toBeInTheDocument();
});

it('a notification count opens the shared people list without changing its independent checkbox', async () => {
  const user = userEvent.setup(), changed = vi.fn();
  show(<Checkbox checked onCheckedChange={changed} label={<PeopleCount users={[userRef(me(1, 'ada', 'Ada'))]} total={3} label="Notify 3 people" />} />);
  const choice = screen.getByRole('checkbox', { name: 'Notify 3 people' });
  await user.click(screen.getByRole('button', { name: 'Notify 3 people' }));
  expect(await screen.findByText('Ada')).toBeVisible();
  expect(screen.getByText('and 2 more')).toBeVisible();
  expect(choice).toBeChecked();
  expect(changed).not.toHaveBeenCalled();
});
