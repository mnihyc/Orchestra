/** A runtime's declared lifecycle and independent keeper decide its page, regardless of display placement. */
import {screen,waitFor,within} from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type {ReactNode} from 'react';
import {beforeEach,expect,it,vi} from 'vitest';
import {startRuntime,stopRuntime} from '@/api/provider-runtime';
import {QueryClient} from '@tanstack/react-query';
import {ReceiptAcknowledged} from '@/api/client';
import {providerQuery} from '@/api/resources';
import {i18n,setLocale} from '@/i18n';
import type {Provider} from '@/generated/api-types';
import {ProviderRuntime} from '@/features/resources/providers/Runtime';
import {planProvider} from '../unit/provider-plan-fixtures';
import {show} from './support';
import {detailEnvironment} from './resource-detail-fixtures';

vi.mock('@tanstack/react-router',async (loadOriginal) => ({ ...await loadOriginal<object>(),Link:({children}:{children:ReactNode})=><a href="#machine">{children}</a>}));
vi.mock('@/features/resources/machines/Policy',()=>({MachineLimits:()=>null}));
vi.mock('@/features/resources/providers/RuntimeConfiguration',()=>({RuntimeConfiguration:()=> <section aria-label="Runtime settings">Runtime variables and secrets</section>}));
vi.mock('@/api/provider-runtime',()=>({startRuntime:vi.fn(),stopRuntime:vi.fn()}));
beforeEach(async()=>{await setLocale('en');vi.clearAllMocks();vi.mocked(startRuntime).mockResolvedValue(planProvider());vi.mocked(stopRuntime).mockResolvedValue(unitProvider('stopped'));});

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


function unitProvider(state:string):Provider {
  return planProvider({harness_kind:null,harness:null,runtime_support:{lifecycle:'unit',commands_after_start:{supported:true,reason:null},after_up:false},
    runtime:{driver:'docker',generation:1,host:null,idle_stop_at:null,job:null,last_active_at:null,network:detailEnvironment.network,renew_at:null,state,
      template:{id:'template',name:'Provider template',owner:null},update_required:false,updates:[]}});
}

for (const state of ['queued','running'] as const) it.each(['en','zh-CN'] as const)(`lets a manager stop its own ${state} birth or Start through the existing reviewed control in %s`,async(locale)=>{
  await setLocale(locale);
  try {
    const provider=unitProvider('creating');
    if (provider.runtime===null) throw new Error('The fixture has an allocated native runtime');
    show(<ProviderRuntime provider={{...provider,runtime:{...provider.runtime,job:{id:'original-provision',kind:'provider_provision',state,phase:'Creating the unit',percent:30}}}} configuration={false} />);
    const label=i18n.t('providerRuntime.stop');
    const button=screen.getByRole('button',{name:label});
    expect(button).toBeEnabled();
    expect(button).not.toHaveAttribute('aria-disabled','true');
    await userEvent.click(button);
    await userEvent.click(within(screen.getByRole('alertdialog')).getByRole('button',{name:label}));
    await waitFor(()=>expect(stopRuntime).toHaveBeenCalledExactlyOnceWith(provider.id,true));
    expect(startRuntime).not.toHaveBeenCalled();
  } finally {await setLocale('en');}
});

it('does not borrow an unrelated replacement job from its Creating state',async()=>{
  const provider=unitProvider('creating');
  if (provider.runtime===null) throw new Error('The fixture has an allocated native runtime');
  show(<ProviderRuntime provider={{...provider,runtime:{...provider.runtime,job:{id:'replacement',kind:'provider_recreate',state:'running',phase:'Copying data',percent:30}}}} configuration={false} />);
  const stop=screen.getByRole('button',{name:i18n.t('providerRuntime.stop')});
  expect(stop).toBeDisabled();
  await userEvent.click(stop);
  expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
  expect(stopRuntime).not.toHaveBeenCalled();
});

it.each(['en','zh-CN'] as const)('lets a manager retry an unconfirmed Stop through the existing control in %s',async(locale)=>{
  await setLocale(locale);
  try {
    const retry=locale==='en'?'Retry stop':'重试停止';
    show(<ProviderRuntime provider={unitProvider('stopping')} configuration={false} />);
    const button=screen.getByRole('button',{name:retry});
    expect(button).not.toBeDisabled();
    expect(button).not.toHaveAttribute('aria-disabled','true');
    await userEvent.click(button);
    await userEvent.click(within(screen.getByRole('alertdialog')).getByRole('button',{name:retry}));
    await waitFor(()=>expect(stopRuntime).toHaveBeenCalledExactlyOnceWith('plan-provider',true));
    await waitFor(()=>expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument());
    expect(startRuntime).not.toHaveBeenCalled();
  } finally {await setLocale('en');}
});

it('closes an accepted Stop receipt without offering to resend or replacing the full cached provider',async()=>{
  const provider=unitProvider('running');
  const cache=new QueryClient({defaultOptions:{queries:{retry:false},mutations:{retry:false}}});
  cache.setQueryData(providerQuery(provider.id).queryKey,provider);
  vi.mocked(stopRuntime).mockRejectedValueOnce(new ReceiptAcknowledged({projection:'receipt',provider_id:provider.id},200,'accepted-stop'));
  show(<ProviderRuntime provider={provider} configuration={false} />,cache);
  await userEvent.click(screen.getByRole('button',{name:'Stop runtime'}));
  await userEvent.click(within(screen.getByRole('alertdialog')).getByRole('button',{name:'Stop runtime'}));
  await waitFor(()=>expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument());
  expect(screen.getByText(i18n.t('ui.receipt.accepted'))).toBeVisible();
  expect(cache.getQueryData(providerQuery(provider.id).queryKey)).toEqual(provider);
  expect(stopRuntime).toHaveBeenCalledExactlyOnceWith(provider.id,true);
});

it('blocks the reviewed Stop when current Manage is withdrawn before confirmation',async()=>{
  const provider=unitProvider('running');
  const view=show(<ProviderRuntime provider={provider} configuration={false} />);
  await userEvent.click(screen.getByRole('button',{name:'Stop runtime'}));
  view.rerender(<ProviderRuntime provider={{...provider,access:{...provider.access,permissions:['provider.view']}}} configuration={false} />);
  const confirm=within(screen.getByRole('alertdialog')).getByRole('button',{name:'Stop runtime'});
  expect(confirm).toBeDisabled();
  await userEvent.click(confirm);
  expect(stopRuntime).not.toHaveBeenCalled();
});

it.each(['running','stopping','stopped','failed'])('keeps runtime controls disabled with their permission reason for a viewer of %s',async(state)=>{
  const provider=unitProvider(state);
  show(<ProviderRuntime provider={{...provider,access:{...provider.access,permissions:['provider.view']}}} configuration={false} />);
  const stop=screen.getByRole('button',{name:state==='stopping'?'Retry stop':'Stop runtime'});
  expect(stop).toHaveAttribute('aria-disabled','true');
  expect(stop).toHaveAttribute('aria-description');
  await userEvent.click(stop);
  expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
  const start=screen.getByRole('button',{name:'Start runtime'});
  expect(start).toHaveAttribute('aria-disabled','true');
  await userEvent.click(start);
  expect(stopRuntime).not.toHaveBeenCalled();
  expect(startRuntime).not.toHaveBeenCalled();
});


it.each(['en','zh-CN'] as const)('keeps an owned Creating runtime Stop blocked for a viewer in %s',async(locale)=>{
  await setLocale(locale);
  try {
    const provider=unitProvider('creating');
    if (provider.runtime===null) throw new Error('The fixture has an allocated native runtime');
    show(<ProviderRuntime provider={{...provider,access:{...provider.access,permissions:['provider.view']},runtime:{...provider.runtime,job:{id:'original-provision',kind:'provider_provision',state:'running',phase:'Creating',percent:30}}}} configuration={false} />);
    const stop=screen.getByRole('button',{name:i18n.t('providerRuntime.stop')});
    expect(stop).toHaveAttribute('aria-disabled','true');
    expect(stop).toHaveAttribute('aria-description');
    await userEvent.click(stop);
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
    expect(stopRuntime).not.toHaveBeenCalled();
  } finally {await setLocale('en');}
});
