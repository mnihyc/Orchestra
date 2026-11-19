/** A custom adapter is registered and used without leaving Add agent. */
import { screen,waitFor,within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach,beforeEach,describe,expect,it,vi } from 'vitest';
import type * as Resources from '@/api/resources';
import { registerAgent } from '@/api/resources';
import { changeMachinePrograms } from '@/api/machinePrograms';
import { AddAgentDialog } from '@/features/resources/agents/AddAgent';
import type { HomePickerProps } from '@/features/resources/HomePicker';
import type { Agent,Machine,MachineProgram } from '@/generated/api-types';
import { i18n,setLocale } from '@/i18n';
import { show } from './support';
import { harnessName } from '../doubles/harness-names';

const help=vi.hoisted(()=>vi.fn());
vi.mock('@/features/help/open',()=>({useHelpOpener:()=>help}));
vi.mock('@tanstack/react-router',()=>({useNavigate:()=>vi.fn()}));
vi.mock('@/features/resources/HomePicker',()=>({HomePicker:({value,onChange}:Pick<HomePickerProps,'value'|'onChange'>)=><select aria-label="Home" value={value} onChange={(event)=>onChange(event.target.value)}><option value="">Choose</option><option value="org">Acme</option></select>}));
const machine:Pick<Machine,'id'|'name'|'roles'|'facts'|'online'>={id:'gpu',name:'GPU computer',roles:['agent'],facts:{adapters:[],harnesses:[]},online:true};
vi.mock('@/api/resources',async(load)=>({...await load<typeof Resources>(),registerAgent:vi.fn(),
  harnessKindsQuery:{queryKey:['agent-kinds'],queryFn:()=>Promise.resolve([])},
  machinesQuery:{queryKey:['agent-machines'],queryFn:()=>Promise.resolve([machine])},
  machineQuery:(id:string)=>({queryKey:['agent-machine',id],queryFn:()=>Promise.resolve(machine)}),
}));
vi.mock('@/api/machinePrograms',()=>({machineProgramsKey:(id:string)=>['programs',id],changeMachinePrograms:vi.fn(),
  machineProgramsQuery:(id:string)=>({queryKey:['programs',id],queryFn:()=>Promise.resolve({entries:[],can_manage:true,config_file:null,
    capabilities:[{kind:'agent',supported:true,remote:true,delegated:true,enabled:true,policy_key:'agent.programs',reason:null}]})})}));
const program:MachineProgram={kind:'agent',name:'research',command:['python','/opt/agent.py'],origin:'orch',ready:true,version:'1',
  descriptor:{kind:'research-agent',harness:{name:'Research agent'}},models:null,detail:null};
const agent:Agent={id:'profile',name:'My research agent',address:'research',harness_kind:'research-agent', harness: harnessName('research-agent'),machine:{id:'gpu',name:machine.name,online:true,placement:'connected',platform_os:'linux'},
  profile:{},descriptor:program.descriptor,descriptor_overridden:[],home:null,org_id:null,owner:null,disabled:false,revision:1,
  access:{permissions:['agent.view','agent.manage'],admin_only:[],participating:true,administers:false,through:[]},created_at:'2026-10-15T17:00:00Z',updated_at:'2026-10-15T17:00:00Z'};
beforeEach(()=>{machine.facts={adapters:[],harnesses:[]};vi.clearAllMocks();vi.mocked(registerAgent).mockResolvedValue(agent);vi.mocked(changeMachinePrograms).mockResolvedValue({entries:[program],can_manage:true,capabilities:[],config_file:null});});
afterEach(async()=>{document.documentElement.removeAttribute('data-theme');await setLocale('en');});

for(const locale of ['en','zh-CN'] as const)for(const theme of ['light','dark'])describe(`${locale}/${theme}`,()=>{
  it('keeps the new reported adapter selected and creates its profile on that same machine',async()=>{
    await setLocale(locale);document.documentElement.dataset.theme=theme;const t=i18n.getFixedT(locale);const user=userEvent.setup();const created=vi.fn();
    show(<AddAgentDialog onClose={vi.fn()} onCreated={created}/>);
    await user.click(screen.getByRole('combobox',{name:t('agentCreation.mode')}));
    await user.click(screen.getByRole('option',{name:t('agentCreation.onMachine')}));
    await user.click(await screen.findByRole('combobox',{name:t('agentCreation.machine')}));
    await user.click(screen.getByRole('option',{name:machine.name}));
    await user.click(screen.getByRole('button',{name:t('agentCreation.register')}));
    const dialog=await screen.findByRole('dialog',{name:t('machinePrograms.addTitle')});
    await user.click(within(dialog).getByRole('textbox',{name:t('machinePrograms.name')}));await user.paste('research');
    await user.click(within(dialog).getByRole('textbox',{name:t('machinePrograms.command')}));await user.paste('python /opt/agent.py');
    await user.click(within(dialog).getByRole('button',{name:t('machinePrograms.register')}));
    await waitFor(()=>expect(screen.getByRole('combobox',{name:t('agents.add.harness')})).toHaveValue('Research agent'));
    expect(changeMachinePrograms).toHaveBeenCalledExactlyOnceWith('gpu',{action:'register',kind:'agent',name:'research',command:['python','/opt/agent.py']});
    await user.selectOptions(screen.getByRole('combobox',{name:'Home'}),'org');
    await user.click(screen.getByLabelText(t('agents.add.name')));await user.paste(agent.name);
    await user.click(screen.getByRole('button',{name:t('agents.add.submit')}));
    await waitFor(()=>expect(created).toHaveBeenCalledExactlyOnceWith(agent));
    expect(registerAgent).toHaveBeenCalledWith('gpu',{home_id:'org',harness_kind:'research-agent',name:agent.name},expect.any(String));
  });
  it('offers the previously registered program by its declared name after leaving Programs',async()=>{
    await setLocale(locale);document.documentElement.dataset.theme=theme;const t=i18n.getFixedT(locale);const user=userEvent.setup();
    machine.facts={adapters:[{kind:'research-agent',builtin:false,needs_binary:false}],harnesses:[],programs:{entries:[program]}};
    show(<AddAgentDialog onClose={vi.fn()}/>);
    await user.click(screen.getByRole('combobox',{name:t('agentCreation.mode')}));
    await user.click(screen.getByRole('option',{name:t('agentCreation.onMachine')}));
    await user.click(await screen.findByRole('combobox',{name:t('agentCreation.machine')}));
    await user.click(screen.getByRole('option',{name:machine.name}));
    await user.click(await screen.findByRole('combobox',{name:t('agents.add.harness')}));
    await user.click(screen.getByRole('option',{name:'Research agent'}));
    expect(screen.queryByText('research-agent')).not.toBeInTheDocument();
    await user.selectOptions(screen.getByRole('combobox',{name:'Home'}),'org');
    await user.click(screen.getByLabelText(t('agents.add.name')));await user.paste(agent.name);
    await user.click(screen.getByRole('button',{name:t('agents.add.submit')}));
    await waitFor(()=>expect(registerAgent).toHaveBeenCalledWith('gpu',{home_id:'org',harness_kind:'research-agent',name:agent.name},expect.any(String)));
    expect(changeMachinePrograms).not.toHaveBeenCalled();
  });
  it('explains a custom agent through its own guide without changing a machine',async()=>{
    await setLocale(locale);const t=i18n.getFixedT(locale);const user=userEvent.setup();
    show(<AddAgentDialog onClose={vi.fn()}/>);
    await user.click(screen.getByRole('button',{name:t('taskGuides.open',{title:t('taskGuides.registerProgram')})}));
    expect(help).toHaveBeenCalledExactlyOnceWith('register-your-program');expect(changeMachinePrograms).not.toHaveBeenCalled();
  });
});
