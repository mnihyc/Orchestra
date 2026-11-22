/** A runtime's declared lifecycle and independent keeper decide its page, regardless of display placement. */
import {cleanup,screen,waitFor,within} from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type {ReactNode} from 'react';
import {afterEach,beforeEach,expect,it,vi} from 'vitest';
import {startRuntime,stopRuntime} from '@/api/provider-runtime';
import {QueryClient} from '@tanstack/react-query';
import {ReceiptAcknowledged} from '@/api/client';
import {providerQuery} from '@/api/resources';
import {i18n,setLocale} from '@/i18n';
import type {Provider} from '@/generated/api-types';
import {ProviderRuntime} from '@/features/resources/providers/Runtime';
import {planProvider} from '../unit/provider-plan-fixtures';
import {show} from './support';
import {detailAgent,detailUnitProvider,detailEnvironment} from './resource-detail-fixtures';
import {useAppearance} from '@/lib/appearance';

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
  expect(screen.getByRole('button',{name:'Detach container'})).toBeDisabled();
  expect(screen.queryByRole('button',{name:'Start container'})).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button',{name:'Attach container'}));
  await waitFor(()=>expect(startRuntime).toHaveBeenCalledWith('plan-provider'));
});

afterEach(async () => { cleanup(); useAppearance.setState({locale:'en'}); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) {
  it(`names the acts that restart a stopped shared runtime and the actual retry delay in ${locale}`, async () => {
    await setLocale(locale); useAppearance.setState({locale});
    if (detailUnitProvider.runtime === null) throw new Error('The unit fixture has a runtime.');
    const row = {...detailUnitProvider, kind:'subscription' as const, runtime: {...detailUnitProvider.runtime, state: 'stopped',renew_at:'2030-01-01T00:00:00Z'},
      runtime_support: { lifecycle: 'unit' as const, commands_after_start: null, after_up: null, retry_after_secs: 420,sign_in:{home_owned:true,policy_allowed:null,allowed:true} }};
    const view = show(<ProviderRuntime provider={row} configuration={false} />);
    expect(screen.getByText(locale === 'en' ? /Stopped · starts for a model call, Refresh, sign-in, Start or renewal/ : /已停止 · 模型请求、刷新、登录、启动或续期可启动/)).toBeVisible();
    view.rerender(<ProviderRuntime provider={{...row, runtime: {...row.runtime, state: 'failed'}}} configuration={false} />);
    expect(screen.getByText(locale === 'en' ? /Automatic retry after 7m · Start retries now/ : /自动重试等待 7(?:分|分钟) · 启动可立即重试/)).toBeVisible();
  });
  it(`keeps unknown expiry and a policy refusal beside the due renewal in ${locale}`, async () => {
    await setLocale(locale); useAppearance.setState({locale});
    if (detailUnitProvider.runtime === null) throw new Error('The unit fixture has a runtime.');
    show(<ProviderRuntime provider={{...detailUnitProvider, kind:'subscription', facts: {...detailUnitProvider.facts, credential: {...detailUnitProvider.facts.credential, method:null, yields: 'login', expires_at: null, issued_at: null}},
      runtime: {...detailUnitProvider.runtime, renew_at: '2030-01-01T00:00:00Z'},
      runtime_support: {lifecycle: 'unit', commands_after_start: null, after_up: null, sign_in: {home_owned: false, policy_allowed: false, allowed: false}}}} configuration={false} />);
    expect(screen.getByText(locale === 'en' ? /expiry unknown · since last renewal or creation · blocked by machine sign-in policy/ : /到期未知 · 按上次续期或创建时间 · 机器登录策略阻止续期/)).toBeVisible();
  });
  it(`shows the reported execution and keeper homes rather than a later setting in ${locale}`, async () => {
    await setLocale(locale); useAppearance.setState({locale});
    const machine = {id:'runner', name:'Runner', online:true, placement:'connected' as const, platform_os:'linux' as const};
    const keeper = {...machine, id:'keeper', name:'Keeper'};
    const report = {agent:{...detailAgent,id:'agent',name:'Program'}, status:'online' as const, status_detail:null, harness_version:'1', sessions_open:0, last_active_at:null};
    show(<ProviderRuntime configuration={false} provider={planProvider({machine, credential_keeper:keeper,
      runtime_support:{lifecycle:'attach',commands_after_start:null,after_up:null,home:{kind:'existing_directory',path:'/later-setting'}},
      harnesses:[{...report,machine,home:{path:'/actual-runner',source:'runtime'}},{...report,machine:keeper,home:{path:null,source:'native'}}]})}/>);
    expect(screen.getByText(locale === 'en' ? /Execution home: \/actual-runner · Runtime choice/ : /执行目录：\/actual-runner · 运行位置设置/)).toBeVisible();
    expect(screen.getByText(locale === 'en' ? /Credential home: Native home · preserved · path unreported/ : /凭据目录：原生目录 · 保持原样 · 未报告路径/)).toBeVisible();
    expect(screen.queryByText('/later-setting')).not.toBeInTheDocument();
  });
}

it.each(['en','zh-CN'] as const)('withholds inapplicable native wake acts from a key connection in %s',async(locale)=>{
  await setLocale(locale); useAppearance.setState({locale});
  if (detailUnitProvider.runtime === null) throw new Error('The unit fixture has a runtime.');
  show(<ProviderRuntime configuration={false} provider={{...detailUnitProvider,kind:'api',runtime:{...detailUnitProvider.runtime,state:'stopped'}}}/>);
  expect(screen.getByText(locale==='en'?/starts for a model call, Refresh or Start/:/模型请求、刷新或启动可启动/)).toBeVisible();
  expect(screen.queryByText(locale==='en'?/starts for.*sign-in/:/登录.*可启动/)).not.toBeInTheDocument();
});

it.each(['en','zh-CN'] as const)('keeps an explicitly reported native home path in %s',async(locale)=>{
  await setLocale(locale); useAppearance.setState({locale});
  const machine={id:'native',name:'Native machine',online:true,placement:'connected' as const,platform_os:'linux' as const};
  show(<ProviderRuntime configuration={false} provider={planProvider({machine,
    runtime_support:{lifecycle:'attach',commands_after_start:null,after_up:null},
    harnesses:[{agent:detailAgent,machine,status:'online',status_detail:null,harness_version:'1',sessions_open:0,last_active_at:null,home:{path:'/reported/native',source:'native'}}]})}/>);
  expect(screen.getByText(locale==='en'?/Runtime and credential home: \/reported\/native · Native home · preserved/:/运行及凭据目录：\/reported\/native · 原生目录 · 保持原样/)).toBeVisible();
});

function unitProvider(state:string):Provider {
  return planProvider({harness_kind:null,harness:null,runtime_support:{lifecycle:'unit',commands_after_start:{supported:true,reason:null},after_up:false},
    runtime:{detail:null,driver:'docker',generation:1,host:null,idle_stop_at:null,job:null,last_active_at:null,network:detailEnvironment.network,renew_at:null,state,
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
    await waitFor(()=>expect(stopRuntime).toHaveBeenCalledExactlyOnceWith(provider.id,true,false));
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
    await waitFor(()=>expect(stopRuntime).toHaveBeenCalledExactlyOnceWith('plan-provider',true,false));
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
  await userEvent.click(screen.getByRole('button',{name:i18n.t('providerRuntime.stop')}));
  await userEvent.click(within(screen.getByRole('alertdialog')).getByRole('button',{name:i18n.t('providerRuntime.stop')}));
  await waitFor(()=>expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument());
  expect(screen.getByText(i18n.t('ui.receipt.accepted'))).toBeVisible();
  expect(cache.getQueryData(providerQuery(provider.id).queryKey)).toEqual(provider);
  expect(stopRuntime).toHaveBeenCalledExactlyOnceWith(provider.id,true,false);
});

it('blocks the reviewed Stop when current Manage is withdrawn before confirmation',async()=>{
  const provider=unitProvider('running');
  const view=show(<ProviderRuntime provider={provider} configuration={false} />);
  await userEvent.click(screen.getByRole('button',{name:i18n.t('providerRuntime.stop')}));
  view.rerender(<ProviderRuntime provider={{...provider,access:{...provider.access,permissions:['provider.view']}}} configuration={false} />);
  const confirm=within(screen.getByRole('alertdialog')).getByRole('button',{name:i18n.t('providerRuntime.stop')});
  expect(confirm).toBeDisabled();
  await userEvent.click(confirm);
  expect(stopRuntime).not.toHaveBeenCalled();
});

it.each(['running','stopping','stopped','failed'])('keeps runtime controls disabled with their permission reason for a viewer of %s',async(state)=>{
  const provider=unitProvider(state);
  show(<ProviderRuntime provider={{...provider,access:{...provider.access,permissions:['provider.view']}}} configuration={false} />);
  const stop=screen.getByRole('button',{name:i18n.t(state==='stopping'?'providerRuntime.retryStop':'providerRuntime.stop')});
  expect(stop).toHaveAttribute('aria-disabled','true');
  expect(stop).toHaveAttribute('aria-description');
  await userEvent.click(stop);
  expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
  const start=screen.getByRole('button',{name:i18n.t('providerRuntime.start')});
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
