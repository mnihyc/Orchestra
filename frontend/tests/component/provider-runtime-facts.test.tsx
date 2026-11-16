/** A runtime's declared lifecycle and independent keeper decide its page, regardless of display placement. */
import {cleanup,screen,waitFor} from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type {ReactNode} from 'react';
import {afterEach,beforeEach,expect,it,vi} from 'vitest';
import {startRuntime} from '@/api/provider-runtime';
import {ProviderRuntime} from '@/features/resources/providers/Runtime';
import {planProvider} from '../unit/provider-plan-fixtures';
import {show} from './support';
import {setLocale} from '@/i18n';
import {useAppearance} from '@/lib/appearance';
import {detailAgent,detailUnitProvider} from './resource-detail-fixtures';

vi.mock('@tanstack/react-router',async (loadOriginal) => ({ ...await loadOriginal<object>(),Link:({children}:{children:ReactNode})=><a href="#machine">{children}</a>}));
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
