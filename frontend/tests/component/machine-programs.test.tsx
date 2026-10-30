/** The machine screen registers both contracts through the same native workflow and applies its answer. */
import { screen,waitFor,within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { beforeEach,expect,it,vi } from 'vitest';
import { MachineProgramsTab,RegisterMachineProgram,programConfiguration } from '@/features/resources/machines/Programs';
import { ApiError } from '@/api/client';
import type { MachineProgram,MachineProgramKind,MachinePrograms,MachineProgramsCommand } from '@/generated/api-types';
import { i18n,setLocale } from '@/i18n';
import { show } from './support';
const {read,write}=vi.hoisted(()=>({read:vi.fn<()=>Promise<MachinePrograms>>(),write:vi.fn<(id:string,body:MachineProgramsCommand)=>Promise<MachinePrograms>>()}));
vi.mock('@/api/machinePrograms',()=>({machineProgramsKey:(id:string)=>['machine-programs',id],machineProgramsQuery:(id:string)=>({queryKey:['machine-programs',id],queryFn:read}),changeMachinePrograms:write}));
vi.mock('@/features/help/open',()=>({useHelpOpener:()=>vi.fn()}));
vi.mock('@/features/resources/harnesses',()=>({useHarnessTitle:()=>((kind:string)=>kind)}));
vi.mock('@/features/resources/machines/Enrollment',()=>({AddMachineDialog:()=>null}));
vi.mock('@tanstack/react-router',()=>({Link:({children}:{children:ReactNode})=><a href="/resources/machines/machine?tab=policy">{children}</a>}));
const machine={id:'machine',facts:{},roles:['agent','provider']};
function snapshot(remote=true):MachinePrograms {return {entries:[],can_manage:true,config_file:'/var/user/orch/config.toml',capabilities:(['agent','provider_source'] as const).map((kind)=>({kind,supported:true,remote,delegated:remote,enabled:remote,policy_key:kind==='agent'?'agent.programs':'provider.programs',reason:remote?null:'Kept by the operator.'}))};}
function program(kind:MachineProgramKind):MachineProgram {return {kind,name:'local-program',command:['/opt/my program','--model','local model'],origin:'orch',ready:true,version:'1.0.0',descriptor:kind==='agent'?{kind:'local-agent',harness:{name:'Local agent'},sandbox_modes:['open'],approval_modes:['on_request']}:{title:'GPU service',models:{}},models:null,detail:null};}
beforeEach(()=>{vi.clearAllMocks();read.mockResolvedValue(snapshot());write.mockResolvedValue(snapshot());});

for(const [locale,theme] of [['en','light'],['en','dark'],['zh-CN','light'],['zh-CN','dark']] as const) for(const kind of ['agent','provider_source'] as const) it(`registers, describes, tests and removes a ${kind} (${locale}, ${theme})`,async()=>{
  await setLocale(locale);document.documentElement.dataset.theme=theme;
  try {
    let state=snapshot();const entry=program(kind);
    read.mockImplementation(()=>Promise.resolve(state));
    write.mockImplementation((_id,body)=>{
      if(body.action==='register')state={...state,entries:[entry]};
      else if(body.action==='test')state={...state,entries:[{...entry,models:kind==='provider_source'?{models:[{model_name:'gpu-model',display_name:'GPU model'}]}:null}]};
      else if(body.action==='remove')state={...state,entries:[]};
      return Promise.resolve(state);
    });
    const user=userEvent.setup();show(<MachineProgramsTab machine={machine}/>);
    await screen.findByText(i18n.t('machinePrograms.empty'));
    if(kind==='provider_source') {await user.click(screen.getByRole('combobox',{name:i18n.t('machinePrograms.contract')}));await user.click(screen.getByRole('option',{name:new RegExp(i18n.t('machinePrograms.kinds.provider_source.name'))}));}
    await user.click(screen.getByRole('button',{name:i18n.t('machinePrograms.add')}));
    const dialog=screen.getByRole('dialog',{name:i18n.t('machinePrograms.addTitle')});
    await user.type(within(dialog).getByRole('textbox',{name:i18n.t('machinePrograms.name')}),'local-program');
    await user.type(within(dialog).getByRole('textbox',{name:i18n.t('machinePrograms.command')}),'"/opt/my program" --model "local model"');
    await user.click(within(dialog).getByRole('button',{name:i18n.t('machinePrograms.register')}));
    await waitFor(()=>expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(write).toHaveBeenLastCalledWith('machine',{action:'register',kind,name:'local-program',command:['/opt/my program','--model','local model']});
    expect(screen.getAllByText(kind==='agent'?'Local agent':'GPU service')).toHaveLength(1);
    expect(screen.getByText('1.0.0')).toBeVisible();
    if(kind==='agent')expect(screen.getByText(i18n.t('machinePrograms.signIns',{methods:i18n.t('machinePrograms.noSignIn')}))).toBeVisible();
    const reads=read.mock.calls.length;
    await user.click(screen.getByRole('button',{name:i18n.t('machinePrograms.test')}));
    if(kind==='provider_source')expect(await screen.findByText(/GPU model/u)).toBeVisible();
    else await waitFor(()=>expect(write).toHaveBeenLastCalledWith('machine',{action:'test',kind,name:'local-program'}));
    expect(read).toHaveBeenCalledTimes(reads);
    await user.click(screen.getByRole('button',{name:i18n.t('machinePrograms.remove')}));
    const confirm=await screen.findByRole('alertdialog',{name:i18n.t('machinePrograms.removeTitle',{name:kind==='agent'?'Local agent':'GPU service'})});
    await user.click(within(confirm).getByRole('button',{name:i18n.t('machinePrograms.remove')}));
    expect(await screen.findByText(i18n.t('machinePrograms.empty'))).toBeVisible();
    expect(write).toHaveBeenLastCalledWith('machine',{action:'remove',kind,name:'local-program'});
    expect(read).toHaveBeenCalledTimes(reads);
  }finally{await setLocale('en');delete document.documentElement.dataset.theme;}
});

it('preserves the command and name after a failed describe and lets the person correct it',async()=>{
  write.mockRejectedValueOnce(new ApiError({type:'orch:error:request.invalid',code:'request.invalid',status:400,title:'The program was refused',detail:'The program could not start.',request_id:'registration',errors:[{path:'program',message:'The program could not start.'}]},null));
  const done=vi.fn();const user=userEvent.setup();show(<RegisterMachineProgram machineId="machine" kind="agent" onClose={vi.fn()} onRegistered={done}/>);
  const name=await screen.findByRole('textbox',{name:'Name'});const command=screen.getByRole('textbox',{name:'Command'});
  await user.type(name,'my-agent');await user.type(command,'/missing-program');
  await user.click(screen.getByRole('button',{name:'Register and describe'}));
  expect(await screen.findByText('The program could not start.')).toBeVisible();
  expect(name).toHaveValue('my-agent');expect(command).toHaveValue('/missing-program');expect(done).not.toHaveBeenCalled();
});

for(const [locale,theme] of [['en','light'],['en','dark'],['zh-CN','light'],['zh-CN','dark']] as const)it(`shows the source's models and the adapter's modes and sign-ins compactly (${locale}, ${theme})`,async()=>{
  await setLocale(locale);document.documentElement.dataset.theme=theme;
  try {
    const source={...program('provider_source'),name:'gpu-box',descriptor:{title:'GPU box',models:{},facts:{reports:['plan','windows','limit_reached','spend']}},
      models:{models:[{model_name:'gpu-small',display_name:'GPU Small'},{model_name:'gpu-large',display_name:'GPU Large'}]}};
    const agent={...program('agent'),name:'echo-agent',descriptor:{kind:'echo-agent',harness:{name:'Echo agent'},sandbox_modes:['workspace','open'],approval_modes:['ask','on_request','bypass'],
      native_provider:{login_methods:[{id:'device',title:'Device code'},{id:'token',title:'Token import'}]}}};
    read.mockResolvedValue({...snapshot(),entries:[source,agent]});
    show(<MachineProgramsTab machine={machine}/>);
    const rows=await screen.findByRole('list',{name:i18n.t('machinePrograms.title')});
    expect(within(rows).getByText(i18n.t('machinePrograms.models',{models:'GPU Small, GPU Large'}))).toBeVisible();
    expect(within(rows).getByText(i18n.t('machinePrograms.sandbox',{modes:[i18n.t('machines.policy.sandbox.workspace'),i18n.t('machines.policy.sandbox.open')].join(', ')}))).toBeVisible();
    expect(within(rows).getByText(i18n.t('machinePrograms.approvalModes',{modes:[i18n.t('machinePrograms.approval.ask'),i18n.t('machinePrograms.approval.on_request'),i18n.t('machinePrograms.approval.bypass')].join(', ')}))).toBeVisible();
    expect(within(rows).getByText(i18n.t('machinePrograms.signIns',{methods:'Device code, Token import'}))).toBeVisible();
    expect(within(rows).getByText(i18n.t('providerSources.reports',{members:[i18n.t('providerDetails.members.plan'),i18n.t('providerDetails.members.windows'),i18n.t('providerDetails.members.limit_reached'),i18n.t('providerDetails.members.spend')].join(', ')}))).toBeVisible();
    expect(within(rows).queryByText(i18n.t('machinePrograms.suppliesModels'))).not.toBeInTheDocument();
    expect(within(rows).queryByText(i18n.t('machinePrograms.suppliesFacts'))).not.toBeInTheDocument();
    expect(write).not.toHaveBeenCalled();expect(read).toHaveBeenCalledTimes(1);
  }finally{await setLocale('en');delete document.documentElement.dataset.theme;}
});

for(const [locale,theme] of [['en','light'],['en','dark'],['zh-CN','light'],['zh-CN','dark']] as const)it(`gives exact configuration and Reload in one kept block (${locale}, ${theme})`,async()=>{
  await setLocale(locale);document.documentElement.dataset.theme=theme;const t=i18n.getFixedT(locale);
  try {
  read.mockResolvedValue(snapshot(false));
  const local={...program('provider_source'),name:'gpu',origin:'local' as const};
  write.mockResolvedValue({...snapshot(false),entries:[local]});
  const done=vi.fn();const user=userEvent.setup();show(<RegisterMachineProgram machineId="machine" kind="provider_source" onClose={vi.fn()} onRegistered={done}/>);
  await user.type(await screen.findByRole('textbox',{name:t('machinePrograms.name')}),'gpu');
  await user.type(screen.getByRole('textbox',{name:t('machinePrograms.command')}),'"/opt/GPU source" --device 0');
  expect(screen.queryByRole('button',{name:t('machinePrograms.register')})).not.toBeInTheDocument();
  const block=screen.getByRole('group',{name:i18n.t('machinePrograms.localConfiguration')});
  expect(within(block).getByText((_text,element)=>element?.tagName==='CODE' && element.textContent===programConfiguration('provider_source','gpu',['/opt/GPU source','--device','0']))).toBeVisible();
  expect(within(block).getAllByText(/\/var\/user\/orch\/config.toml/u)).toHaveLength(1);
  expect(within(block).getByRole('button',{name:i18n.t('machinePrograms.copyConfiguration')})).toBeEnabled();
  expect(screen.getAllByRole('button',{name:t('machinePrograms.reload')})).toHaveLength(1);
  await user.click(within(block).getByRole('button',{name:t('machinePrograms.reload')}));
  await waitFor(()=>expect(done).toHaveBeenCalledWith(local));expect(write).toHaveBeenCalledWith('machine',{action:'reload'});
  }finally{await setLocale('en');delete document.documentElement.dataset.theme;}
});

it('keeps the error visible with Retry and does not offer mutations to a reader',async()=>{
  read.mockRejectedValueOnce(new Error('Registry snapshot could not be read.')).mockResolvedValue({...snapshot(),can_manage:false,entries:[program('agent')]});
  const user=userEvent.setup();show(<MachineProgramsTab machine={machine}/>);
  expect(await screen.findByText(i18n.t('machinePrograms.readFailed'))).toBeVisible();
  await user.click(screen.getByRole('button',{name:i18n.t('common.tryAgain')}));expect(await screen.findByText('Local agent')).toBeVisible();
  for(const name of ['Add','Test','Delete','Reload'])expect(screen.queryByRole('button',{name})).not.toBeInTheDocument();
  expect(write).not.toHaveBeenCalled();
});

it('distinguishes an Orch-controlled off switch from a key kept on the machine',async()=>{
  const off=snapshot(false);off.capabilities=off.capabilities.map((rule)=>({...rule,delegated:true}));read.mockResolvedValue(off);
  show(<RegisterMachineProgram machineId="machine" kind="agent" onClose={vi.fn()} onRegistered={vi.fn()}/>);
  expect(await screen.findByText(i18n.t('machinePrograms.switchedOff'))).toBeVisible();
  expect(screen.getByRole('link',{name:'Open policy'})).toHaveAttribute('href','/resources/machines/machine?tab=policy');
  expect(screen.queryByRole('button',{name:'Reload'})).not.toBeInTheDocument();
  expect(screen.queryByText(/Kept on this machine/u)).not.toBeInTheDocument();
});
