/** A runtime's declared lifecycle and independent keeper decide its page, regardless of display placement. */
import {screen,waitFor} from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type {ReactNode} from 'react';
import {beforeEach,expect,it,vi} from 'vitest';
import {startRuntime} from '@/api/provider-runtime';
import {ProviderRuntime} from '@/features/resources/providers/Runtime';
import {planProvider} from '../unit/provider-plan-fixtures';
import {show} from './support';

vi.mock('@tanstack/react-router',()=>({Link:({children}:{children:ReactNode})=><a href="#machine">{children}</a>}));
vi.mock('@/features/resources/machines/Policy',()=>({MachineLimits:()=>null}));
vi.mock('@/features/resources/providers/RuntimeConfiguration',()=>({RuntimeConfiguration:()=> <section aria-label="Runtime settings">Runtime variables and secrets</section>}));
vi.mock('@/api/provider-runtime',()=>({startRuntime:vi.fn(),stopRuntime:vi.fn()}));
beforeEach(()=>{vi.clearAllMocks();vi.mocked(startRuntime).mockResolvedValue(planProvider());});

it.each(['managed','connected'] as const)('shows a directory runtime and its separate credential keeper for %s display placement',async(placement)=>{
  const machine={id:'runtime-machine',name:'Runner',online:true,placement:'connected' as const,platform_os:'linux' as const};
  show(<ProviderRuntime provider={planProvider({placement,machine,credential_keeper:{...machine,id:'keeper',name:'Account keeper'},
    runtime_support:{lifecycle:'attach',commands_after_start:{supported:true,reason:null},after_up:false}})} />);
  expect(screen.getByText(/Credential keeper: Account keeper/)).toBeVisible();
  expect(screen.getByRole('region',{name:'Runtime settings'})).toBeVisible();
  expect(screen.getByRole('button',{name:'Detach runtime'})).toBeDisabled();
  expect(screen.queryByRole('button',{name:'Start runtime'})).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button',{name:'Attach runtime'}));
  await waitFor(()=>expect(startRuntime).toHaveBeenCalledWith('plan-provider'));
});
