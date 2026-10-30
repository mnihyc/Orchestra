/** Nested person cards retain their parent; dangerous quick actions use the shared confirmation. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';

import { DetailCard } from '@/ui/DetailCard';
import { UserChip } from '@/ui/UserChip';
import ResourceCard from '@/features/resource-facts/ResourceCard';
import type { Environment, UserRef } from '@/generated/api-types';
import { detailEnvironment } from './resource-detail-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ stop: vi.fn<(id: string, interrupt: boolean) => Promise<Environment>>() }));
vi.mock('@/api/environments', async (load) => ({ ...await load<object>(), stopEnvironment: (...args: [string, boolean]) => calls.stop(...args) }));

it('keeps the resource card while a UserChip opens and dismisses its person card', async () => {
  const person: UserRef = { id: 'ops', username: 'ops', display_name: 'Operations', kind: 'service', avatar: { text: 'O', color: 1 } };
  const user = userEvent.setup();
  show(<DetailCard label="Environment" content={() => <UserChip user={person} />}>Environment</DetailCard>);
  await user.click(screen.getByRole('button', { name: 'Environment' }));
  const parent = screen.getByRole('dialog', { name: 'Environment' });
  await user.click(within(parent).getByRole('button', { name: 'Operations' }));
  expect(await screen.findByRole('dialog', { name: 'Operations' })).toBeVisible();
  expect(parent).toBeVisible();
  await user.keyboard('{Escape}');
  await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Operations' })).not.toBeInTheDocument());
  expect(parent).toBeVisible();
});

it('asks once before stopping and preserves the non-interrupting permission boundary', async () => {
  calls.stop.mockReset();
  const environment = { ...detailEnvironment, machine: null };
  calls.stop.mockResolvedValue({ ...environment, status: 'stopped' });
  const user = userEvent.setup();
  show(<DetailCard label={environment.name} content={() => <ResourceCard subject={{ kind: 'environment', environment }} />}>{environment.name}</DetailCard>);
  await user.click(screen.getByRole('button', { name: environment.name }));
  await user.click(await screen.findByRole('button', { name: 'Stop' }));
  const confirmation = await screen.findByRole('alertdialog', { name: `Stop ${environment.name}` });
  expect(confirmation).toHaveTextContent(`Stops open shells and processes in ${environment.name}`);
  expect(calls.stop).not.toHaveBeenCalled();
  await user.click(within(confirmation).getByRole('button', { name: 'Cancel' }));
  expect(calls.stop).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Stop' }));
  await user.click(within(await screen.findByRole('alertdialog', { name: `Stop ${environment.name}` })).getByRole('button', { name: `Stop ${environment.name}` }));
  await waitFor(() => expect(calls.stop).toHaveBeenCalledExactlyOnceWith(environment.id, false));
});
