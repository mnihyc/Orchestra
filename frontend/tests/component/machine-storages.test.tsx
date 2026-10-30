/** Storage names and capacity are facts of any machine, independent of its resource roles. */
import { screen } from '@testing-library/react';
import { expect,it } from 'vitest';
import { MachineStorages } from '@/features/resources/machines/Storages';
import { show } from './support';

it('shows a provider-only machine storage with its default and free capacity',()=>{
  show(<MachineStorages facts={{roles:['provider'],storages:[{name:'fast',default:true,capacity_gb:100,free_gb:40,private_views:false}]}} />);
  expect(screen.getByText('fast (default)')).toBeInTheDocument();
  expect(screen.getByText('40 GB free of 100 GB')).toBeInTheDocument();
  expect(screen.getByText(/Choose these names/)).toBeInTheDocument();
});

it('does not invent a default when a machine has not reported storage',()=>{
  show(<MachineStorages facts={{}} />);
  expect(screen.getByText('This machine has not reported an available storage.')).toBeInTheDocument();
  expect(screen.queryByText(/GB free/)).not.toBeInTheDocument();
});
