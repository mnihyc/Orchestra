/** A restart is reviewed before submission and chooses the promised grace or explicit interruption. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { restartEnvironment } from '@/api/environments';
import { RestartEnvironmentDialog } from '@/features/resources/environments/Restart';

import { show } from './support';

vi.mock('@/api/environments',()=>({restartEnvironment:vi.fn(()=>Promise.reject(new Error('The machine is unavailable.')))}));

describe('restarting an environment',()=>{
  beforeEach(()=>vi.clearAllMocks());
  it('reviews every ending category and file preservation, then waits by default',async()=>{
    const user=userEvent.setup();
    show(<RestartEnvironmentDialog environment={{id:'environment',name:'Build'}} onClose={vi.fn()} />);
    expect(screen.getByRole('dialog',{name:'Restart Build'})).toBeVisible();
    expect(screen.getByText(/Running turns, shells, browsers, displays and open port connections/)).toBeVisible();
    expect(screen.getByText(/Every file is kept, including installations and files outside the volumes/)).toBeVisible();
    expect(screen.getByText(/Queued inputs keep their authors and order/)).toBeVisible();
    expect(screen.getByRole('checkbox',{name:'Interrupt now'})).not.toBeChecked();
    expect(restartEnvironment).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button',{name:'Restart'}));
    expect(restartEnvironment).toHaveBeenCalledWith('environment','wait');
  });
  it('interrupts only after that choice is explicitly selected',async()=>{
    const user=userEvent.setup();
    show(<RestartEnvironmentDialog environment={{id:'environment',name:'Build'}} onClose={vi.fn()} />);
    await user.click(screen.getByRole('checkbox',{name:'Interrupt now'}));
    expect(screen.getByText('Interrupt running turns as soon as the restart begins.')).toBeVisible();
    await user.click(screen.getByRole('button',{name:'Restart'}));
    expect(restartEnvironment).toHaveBeenCalledWith('environment','interrupt');
  });
});
