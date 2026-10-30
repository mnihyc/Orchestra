/** Delegation is explicit, roles change on the page, and the live hello alone names active roles. */
import { screen,waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useQuery } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { afterEach,beforeEach,expect,it,vi } from 'vitest';
import type * as Resources from '@/api/resources';
import { machineKey,setMachineRoles } from '@/api/resources';
import { ApiError } from '@/api/client';
import { MachineRoles } from '@/features/resources/machines/Roles';
import type { Machine } from '@/generated/api-types';
import { i18n,setLocale } from '@/i18n';
import { show } from './support';

vi.mock('@/features/help/open',()=>({useHelpOpener:()=>vi.fn()}));
vi.mock('@/api/resources',async(load)=>({...await load<typeof Resources>(),setMachineRoles:vi.fn()}));
vi.mock('@tanstack/react-router',()=>({Link:({children}:{children:ReactNode})=><a href="/resources/machines/machine?tab=policy">{children}</a>}));

function machine(delegated=true):Machine {return {id:'machine',name:'world-box',online:true,placement:'connected',platform_os:'linux',
  platform_arch:'x86_64',connector_version:'1.0.0',protocol_version:1,owner:null,roles:['environment','agent'],status:'active',
  last_seen_at:null,last_ip:null,cert_not_after:'2027-01-01T00:00:00Z',providers:[],agents:[],environments:[],environment_sandboxes:[],
  access:{permissions:['machine.view','machine.manage'],admin_only:[],administers:false,participating:true,through:[]},
  role_management:{supported:true,delegated,enabled:delegated},
  facts:{installation:{program:'orch-connector',executable:'/opt/orch-connector',data_dir:'/srv/instances/work',config_file:'/srv/instances/work/config.toml',
    user:'operator',system:false,service:'orch-connector-work',commands:{roles:'/opt/orch-connector --data-dir /srv/instances/work roles',policy:'/opt/orch-connector --data-dir /srv/instances/work policy',start:'start',stop:'stop',update:'update',uninstall:'uninstall'}}}};}

beforeEach(()=>vi.clearAllMocks());
afterEach(async()=>{delete document.documentElement.dataset.theme;await setLocale('en');});

function Page({initial}:{initial:Machine}) {
  const query=useQuery({queryKey:machineKey(initial.id),queryFn:()=>Promise.resolve(initial),initialData:initial,enabled:false});
  return <MachineRoles machine={query.data}/>;
}

for(const locale of ['en','zh-CN'] as const)for(const theme of ['light','dark'])it(`changes delegated roles in place with one native act (${locale}, ${theme})`,async()=>{
  await setLocale(locale);document.documentElement.dataset.theme=theme;const t=i18n.getFixedT(locale);
  const original=machine();const updated={...original,roles:['environment','agent','provider']};
  vi.mocked(setMachineRoles).mockResolvedValue({machine_view:original,requested_roles:updated.roles,reconnecting:true});
  const user=userEvent.setup();show(<Page initial={original}/>);
  await user.click(screen.getByRole('button',{name:t('machineRoles.edit')}));
  expect(screen.queryByRole('checkbox',{name:t('machines.add.roleChoices.node.name')})).not.toBeInTheDocument();
  await user.click(screen.getByRole('checkbox',{name:t('machines.add.roleChoices.provider.name')}));
  await user.click(screen.getByRole('button',{name:t('common.save')}));
  await waitFor(()=>expect(screen.queryByRole('group',{name:t('machineRoles.edit')})).not.toBeInTheDocument());
  expect(setMachineRoles).toHaveBeenCalledExactlyOnceWith('machine',['environment','agent','provider']);
  expect(screen.getAllByText(['environment','agent','provider'].map((role)=>role==='environment'?t('machines.add.roleChoices.environment.name'):role==='agent'?t('machines.add.roleChoices.agent.name'):t('machines.add.roleChoices.provider.name')).join(' · '))).toHaveLength(1);
  expect(screen.queryByText(/roles set/u)).not.toBeInTheDocument();
});

it('keeps the precise named-instance command beside its one local refusal',()=>{
  const t=i18n.getFixedT('en');show(<MachineRoles machine={machine(false)}/>);
  expect(screen.getAllByText(t('machineRoles.kept'))).toHaveLength(1);
  expect(screen.getByText('/opt/orch-connector --data-dir /srv/instances/work roles set environment,agent')).toBeVisible();
  expect(screen.getByRole('button',{name:t('machines.add.copyRoles')})).toBeEnabled();
  expect(screen.queryByRole('button',{name:t('machineRoles.edit')})).not.toBeInTheDocument();expect(setMachineRoles).not.toHaveBeenCalled();
});

it('shows the accepted request while reconnecting, and takes active roles from the subsequent machine row',async()=>{
  const original=machine();const roles=['environment','agent','provider'];
  vi.mocked(setMachineRoles).mockResolvedValue({machine_view:original,requested_roles:roles,reconnecting:true});
  const user=userEvent.setup();const view=show(<MachineRoles machine={original}/>);
  await user.click(screen.getByRole('button',{name:'Change roles'}));await user.click(screen.getByRole('checkbox',{name:'Provider'}));
  await user.click(screen.getByRole('button',{name:'Save'}));
  expect(await screen.findByRole('status')).toHaveTextContent('Reconnecting to apply roles');
  expect(screen.getAllByText('Environment · Agent · Provider')).toHaveLength(1);
  expect(screen.queryByRole('button',{name:'Change roles'})).not.toBeInTheDocument();
  view.rerender(<MachineRoles machine={{...original,roles}}/>);
  expect(screen.queryByRole('status')).not.toBeInTheDocument();expect(screen.getByRole('button',{name:'Change roles'})).toHaveTextContent('Environment · Agent · Provider');
});

it('keeps a refused draft with the deciding cause and allows a retry',async()=>{
  const words='Remove its registered environments before turning off Environment.';
  vi.mocked(setMachineRoles).mockRejectedValueOnce(new ApiError({type:'orch:error:request.invalid',code:'request.invalid',status:400,title:'Invalid request',detail:words,request_id:'roles',errors:[{path:'roles',message:words}]},null));
  const user=userEvent.setup();show(<MachineRoles machine={machine()}/>);
  await user.click(screen.getByRole('button',{name:'Change roles'}));await user.click(screen.getByRole('checkbox',{name:'Environment'}));await user.click(screen.getByRole('button',{name:'Save'}));
  expect(await screen.findByRole('alert')).toHaveTextContent(words);expect(screen.getByRole('checkbox',{name:'Environment'})).not.toBeChecked();
  expect(screen.getByRole('button',{name:'Save'})).toBeEnabled();expect(setMachineRoles).toHaveBeenCalledTimes(1);
});

it('does not offer a Node factory, widen a kept switch, or give readers a role editor',()=>{
  const node={...machine(),roles:['node'],role_management:{supported:false,delegated:true,enabled:true}};
  const view=show(<MachineRoles machine={node}/>);expect(screen.getByText(i18n.t('machineRoles.node'))).toBeVisible();
  expect(screen.queryByRole('button',{name:'Change roles'})).not.toBeInTheDocument();
  view.rerender(<MachineRoles machine={{...machine(),access:{permissions:['machine.view'],admin_only:[],administers:false,participating:true,through:[]}}}/>);
  expect(screen.getByText(i18n.t('machineRoles.manager'))).toBeVisible();expect(setMachineRoles).not.toHaveBeenCalled();
  view.rerender(<MachineRoles machine={{...machine(),role_management:{supported:true,delegated:true,enabled:false}}}/>);
  expect(screen.getByText(i18n.t('machineRoles.off'))).toBeVisible();expect(screen.getByRole('link',{name:i18n.t('machinePrograms.openPolicy')})).toBeVisible();
  expect(screen.queryByText(/roles set/u)).not.toBeInTheDocument();
});
