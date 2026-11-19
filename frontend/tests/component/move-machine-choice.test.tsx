/** Move choices follow reported services and machine policy, with one exclusive selection. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { HostChoices } from '@/features/change/Where';
import type { MoveMachine } from '@/generated/api-types';
import { show } from './support';

const machine: MoveMachine = { machine: { id: 'personal', name: 'Team workstation', online: true, placement: 'connected', platform_os: 'linux' }, host: null, usable: true, reasons: [] };

it('offers an enabled directory destination through the same move choice', async () => {
  const onMachine = vi.fn();
  show(<HostChoices hosts={[]} value={null} onChange={vi.fn()} loading={false} machines={[machine]} onMachine={onMachine} />);
  expect(screen.queryByText(/under its storage and write policy/)).toBeNull();
  await userEvent.click(screen.getByRole('combobox', { name: 'Place' }));
  const choice = screen.getByRole('option', { name: 'Team workstation' });
  expect(choice).not.toHaveAttribute('aria-disabled','true');
  expect(screen.getByText(/under its storage and write policy/)).toBeVisible();
  await userEvent.click(choice);
  expect(onMachine).toHaveBeenCalledWith('personal');
});

it('shows a declared policy refusal and does not permit that choice', async () => {
  const onMachine = vi.fn();
  const refused = { ...machine, usable: false, reasons: ['The machine policy has remote writes off.'] };
  show(<HostChoices hosts={[]} value={null} onChange={vi.fn()} loading={false} machines={[refused]} onMachine={onMachine} />);
  expect(screen.queryByText(/remote writes off/)).toBeNull();
  await userEvent.click(screen.getByRole('combobox', { name: 'Place' }));
  const choice = screen.getByRole('option', { name: 'Team workstation' });
  expect(choice).toHaveAttribute('aria-disabled','true');
  expect(choice).toHaveAccessibleDescription(/remote writes off/);
  await userEvent.click(choice);
  expect(onMachine).not.toHaveBeenCalled();
});
