import { unplacedGrouping } from '../doubles/resource-grouping';
/** Move choices follow reported services and machine policy, with one exclusive selection. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { HostChoices } from '@/features/change/Where';
import type { MoveMachine } from '@/generated/api-types';
import { show } from './support';

const machine: MoveMachine = { ...unplacedGrouping(), machine: { id: 'personal', name: 'Team workstation', online: true, placement: 'connected', platform_os: 'linux' }, host: null, usable: true, reasons: [] };

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

it('shows the current machine capacity in the place picker when its reported total changes', async () => {
  const choice = (memory_bytes:number) => ({...machine, capacity:{cpus:4,memory_bytes,data_volume_bytes: 128 * 1024 ** 3,capacity_refresh_secs:600}});
  const props = {hosts:[],value:null,onChange:vi.fn(),loading:false,onMachine:vi.fn()};
  const page = show(<HostChoices {...props} machines={[choice(40 * 1024 ** 3)]}/>);
  await userEvent.click(screen.getByRole('combobox',{name:'Place'}));
  expect(screen.getByText(/40 GiB/)).toBeVisible();
  page.rerender(<HostChoices {...props} machines={[choice(36 * 1024 ** 3)]}/>);
  expect(await screen.findByText(/36 GiB/)).toBeVisible();
  expect(screen.queryByText(/40 GiB/)).not.toBeInTheDocument();
  // A disk reads in the whole decimal gigabytes its storage report gives, as on the machine's Storage tab: 128 GiB is 137 GB.
  expect(screen.getByText(/137 GB data disk/)).toBeVisible();
});
