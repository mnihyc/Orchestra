/** A discovered Codex is shown as a fact and only machine managers may confirm it. */
import { screen,waitFor,within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { beforeEach,expect,it,vi } from 'vitest';
import { MachineProgramsTab } from '@/features/resources/machines/Programs';
import type { Machine,MachinePolicy,MachinePrograms,MachineProgramsCommand } from '@/generated/api-types';
import { setLocale } from '@/i18n';
import { show } from './support';

const {programs,policy,change,probe}=vi.hoisted(()=>({
  programs:vi.fn<()=>Promise<MachinePrograms>>(),policy:vi.fn<()=>Promise<MachinePolicy>>(),
  change:vi.fn<(id:string,command:MachineProgramsCommand)=>Promise<MachinePrograms>>(),
  probe:vi.fn<(id:string)=>Promise<Machine>>(),
}));
vi.mock('@/api/machinePrograms',()=>({machineProgramsKey:(id:string)=>['machine-programs',id],machineProgramsQuery:(id:string)=>({queryKey:['machine-programs',id],queryFn:programs}),changeMachinePrograms:change}));
vi.mock('@/api/resources',()=>({machinePolicyQuery:(id:string)=>({queryKey:['machine-policy',id],queryFn:policy}),machineQuery:(id:string)=>({queryKey:['machine',id]}),probeMachine:probe}));
vi.mock('@/features/help/open',()=>({useHelpOpener:()=>vi.fn()}));
vi.mock('@/features/resources/harnesses',()=>({useHarnessTitle:()=>((kind:string)=>kind==='codex'?'Codex':kind==='claude-code'?'Claude Code':kind)}));
vi.mock('@/features/resources/machines/Enrollment',()=>({AddMachineDialog:()=>null}));
vi.mock('@tanstack/react-router',()=>({Link:({children}:{children:ReactNode})=><a href="/policy">{children}</a>}));

const found={kind:'codex',path:'/usr/local/bin/codex',version:'0.164.0',daemon:false};
const machine={id:'machine',facts:{harnesses:[],discovered_harnesses:[found]},roles:['environment']};
function currentPolicy(writable=true):MachinePolicy {
  return {machine:{id:'machine',name:'Workstation',online:true,placement:'connected',platform_os:'linux'},reported_at:null,
    keys:[{key:'agent.harnesses',value:[{kind:'other',path:'/opt/other'}],bounds:'machine',delegated:writable,writable,
      definition:{key:'agent.harnesses',title:'Harnesses',meaning:'Harnesses',sections:['machine'],managed:[],cautious:[],delegable:true}}]};
}
beforeEach(()=>{
  vi.clearAllMocks();
  programs.mockResolvedValue({entries:[],capabilities:[],config_file:null,can_manage:true});
  policy.mockResolvedValue(currentPolicy());
  change.mockResolvedValue({entries:[],capabilities:[],config_file:null,can_manage:true,machine_view:{ ...machine,facts:{harnesses:[found],discovered_harnesses:[]} } as Machine});
  probe.mockResolvedValue({ ...machine,facts:{harnesses:[found],discovered_harnesses:[]} } as Machine);
});

for(const [locale,foundLabel,useLabel,confirmed] of [['en','Found','Use','Confirmed'],['zh-CN','已发现','使用','已确认']] as const) for(const theme of ['light','dark'] as const) {
  it(`shows and confirms a found Codex without changing other harnesses (${locale}, ${theme})`,async()=>{
    await setLocale(locale);document.documentElement.dataset.theme=theme;
    try {
      const user=userEvent.setup();show(<MachineProgramsTab machine={machine}/>);
      const state=await screen.findByText(foundLabel);
      const row=state.closest('li');expect(row).not.toBeNull();
      expect(within(row as HTMLElement).getByText('0.164.0')).toBeVisible();
      expect(within(row as HTMLElement).getByText('/usr/local/bin/codex')).toBeVisible();
      await user.click(await within(row as HTMLElement).findByRole('button',{name:useLabel}));
      await waitFor(()=>expect(change).toHaveBeenCalledWith('machine',{action:'confirm_harness',kind:'codex',path:'/usr/local/bin/codex'}));
      expect(probe).not.toHaveBeenCalled();
      expect(await screen.findByText(confirmed)).toBeVisible();
    } finally {await setLocale('en');delete document.documentElement.dataset.theme;}
  });
}

it('keeps a found program visible to readers without offering confirmation',async()=>{
  programs.mockResolvedValue({entries:[],capabilities:[],config_file:null,can_manage:false});
  policy.mockResolvedValue(currentPolicy(false));
  show(<MachineProgramsTab machine={machine}/>);
  expect(await screen.findByText('Found')).toBeVisible();
  expect(screen.queryByRole('button',{name:'Use'})).not.toBeInTheDocument();
  expect(change).not.toHaveBeenCalled();
});

it('shows an unknown version and permits a machine manager to remove confirmation',async()=>{
  const unknown={...found,version:'unknown'};
  const confirmedMachine={...machine,facts:{harnesses:[unknown],discovered_harnesses:[]}};
  policy.mockResolvedValue({...currentPolicy(),keys:[{...currentPolicy().keys[0]!,value:[{kind:'codex',path:found.path}]}]});
  change.mockResolvedValue({entries:[],capabilities:[],config_file:null,can_manage:true,machine_view:{...machine,facts:{harnesses:[],discovered_harnesses:[unknown]}} as Machine});
  const user=userEvent.setup();show(<MachineProgramsTab machine={confirmedMachine}/>);
  expect(await screen.findByText('Version unknown')).toBeVisible();
  expect(screen.getByText('Confirmed')).toBeVisible();
  await user.click(await screen.findByRole('button',{name:'Remove confirmation'}));
  await waitFor(()=>expect(change).toHaveBeenCalledWith('machine',{action:'remove_harness',kind:'codex',path:found.path}));
  expect(await screen.findByText('Found')).toBeVisible();
});

it('says when an actual search found no program',async()=>{
  show(<MachineProgramsTab machine={{...machine,facts:{harnesses:[],discovered_harnesses:[]}}}/>);
  expect(await screen.findByText('No agent programs found')).toBeVisible();
});

for(const [locale,useLabel,foundLabel] of [['en','Use','Found'],['zh-CN','使用','已发现']] as const) for(const theme of ['light','dark'] as const) {
  it(`confirms a found Claude without changing the Codex policy (${locale}, ${theme})`,async()=>{
    await setLocale(locale);document.documentElement.dataset.theme=theme;
    try {
      const claude={kind:'claude-code',path:'/usr/local/bin/claude',version:'2.1.309',daemon:false};
      const both={...machine,facts:{harnesses:[found],searched_harness_kinds:['codex','claude-code'],discovered_harnesses:[claude]}};
      policy.mockResolvedValue({...currentPolicy(),keys:[{...currentPolicy().keys[0]!,value:[{kind:'codex',path:found.path}]}]});
      change.mockResolvedValue({entries:[],capabilities:[],config_file:null,can_manage:true,machine_view:{...both,facts:{harnesses:[found,claude],searched_harness_kinds:['codex','claude-code'],discovered_harnesses:[]}} as Machine});
      const user=userEvent.setup();show(<MachineProgramsTab machine={both}/>);
      const row=(await screen.findByText('Claude Code')).closest('li');expect(row).not.toBeNull();
      expect(within(row as HTMLElement).getByText('2.1.309')).toBeVisible();
      expect(within(row as HTMLElement).getByText(foundLabel)).toBeVisible();
      await user.click(await within(row as HTMLElement).findByRole('button',{name:useLabel}));
      await waitFor(()=>expect(change).toHaveBeenCalledWith('machine',{action:'confirm_harness',kind:'claude-code',path:claude.path}));
      expect(screen.getByText('Codex')).toBeVisible();
    } finally {await setLocale('en');delete document.documentElement.dataset.theme;}
  });
}

it('names each searched program for which nothing was found',async()=>{
  const empty={...machine,facts:{harnesses:[],searched_harness_kinds:['codex','claude-code'],discovered_harnesses:[]}};
  show(<MachineProgramsTab machine={empty}/>);
  const codex=(await screen.findByText('Codex')).closest('li');
  const claude=screen.getByText('Claude Code').closest('li');
  expect(within(codex as HTMLElement).getByText('No program found')).toBeVisible();
  expect(within(claude as HTMLElement).getByText('No program found')).toBeVisible();
});
