/** Native sign-in actions follow the selected home and reported keeper policy on every machine. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach,describe,expect,it,vi } from 'vitest';
import { cancelProviderLogin } from '@/api/provider-plan';
import { ProviderLogin } from '@/features/resources/providers/Login';
import type { Provider } from '@/generated/api-types';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';
import { harnessName } from '../doubles/harness-names';

const server=vi.hoisted((): {state:'idle'|'awaiting_user'}=>({state:'idle'}));
beforeEach(()=>{server.state='idle';vi.clearAllMocks();});

vi.mock('@/api/resources',()=>({providerSetupQuery:()=>({queryKey:['signin-setup'],queryFn:()=>Promise.resolve({subscriptions:[{
  agent:{id:'profile'},methods:[{id:'import',title:'Import this sign-in',summary:'Native import',kind:'import',yields:'token',
    fields:[],requires:null,recommended:true,lifetime_secs:null}],
}]})})}));
vi.mock('@/api/provider-plan',()=>({
  loginStateQuery:()=>({queryKey:['signin-progress'],queryFn:()=>Promise.resolve({state:server.state,method:null,shows:[],asks:server.state==='idle'?null:[{name:'answer',label:'Vendor answer',type:'text',required:true,help:null,choices:null,default:null,max_bytes:null}],expires_at:null,error:null})}),
  answerProviderLogin:vi.fn(),cancelProviderLogin:vi.fn(()=>{server.state='idle';return Promise.resolve({state:'idle',method:null,shows:[],asks:null,expires_at:null,error:null});}),retryProvider:vi.fn(),startProviderLogin:vi.fn(),
}));

function account(placement:Provider['placement'],home_owned:boolean,allowed:boolean):Provider {
  return planProvider({placement,agent:{id:'profile',name:'Native harness',harness_kind:'fake', harness: harnessName('fake'),machine:null,address:'acme.native',disabled:false},
    runtime_support:{lifecycle:'attach',commands_after_start:{supported:true,reason:null},after_up:false,
      sign_in:{home_owned,allowed,policy_allowed:allowed && !home_owned}}});
}

describe('native sign-in permission',()=>{
  it.each(['connected','managed'] as const)('offers the same native methods in an owned home with %s display placement',async (placement)=>{
    show(<ProviderLogin provider={account(placement,true,true)} />);
    expect(screen.getByText(/uses a home made by Orch/)).toBeVisible();
    expect(await screen.findByRole('button',{name:'Continue'})).toBeVisible();
    expect(screen.getByRole('combobox',{name:'Sign-in method'})).toHaveValue('Import this sign-in');
  });
  it('explains a borrowed home and removes its native acts when the machine takes permission back',async ()=>{
    const {rerender}=show(<ProviderLogin provider={account('connected',false,true)} />);
    expect(await screen.findByRole('button',{name:'Continue'})).toBeVisible();
    expect(screen.getByText(/Its machine permits Orch to change and renew the sign-in there/)).toBeVisible();
    rerender(<ProviderLogin provider={account('connected',false,false)} />);
    await waitFor(()=>expect(screen.queryByRole('button',{name:'Continue'})).not.toBeInTheDocument());
    expect(screen.getByText(/Its machine does not allow Orch to manage that sign-in/)).toBeVisible();
    expect(screen.getByRole('button',{name:'Check sign-in'})).toBeVisible();
  });
  it('keeps cancellation available after revocation while withholding the pending answer form',async ()=>{
    server.state='awaiting_user';
    show(<ProviderLogin provider={account('connected',false,false)} />);
    const cancel=await screen.findByRole('button',{name:'Cancel'});
    expect(screen.queryByLabelText('Vendor answer')).not.toBeInTheDocument();
    expect(screen.queryByRole('button',{name:'Continue'})).not.toBeInTheDocument();
    await userEvent.click(cancel);
    expect(cancelProviderLogin).toHaveBeenCalledWith('plan-provider');
    expect(await screen.findByText('No sign-in is running')).toBeVisible();
  });
});
