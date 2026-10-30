/** A host's one confirmation reviews cascades, notification consent and each selected volume's exact name. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { RemoveHostDialog } from '@/features/resources/hosts/Remove';
import type { Host, HostDeletePlan, HostDeletePreflightBody } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { show } from './support';
const calls=vi.hoisted(()=>({plan:vi.fn<(id:string,body:HostDeletePreflightBody)=>Promise<HostDeletePlan>>(),start:vi.fn()}));
const query=(key:string,value:unknown)=>({queryKey:[key],queryFn:()=>Promise.resolve(value)});
vi.mock('@/api/loss',async(original)=>({...await original<object>(),planHostDelete:calls.plan,startHostDelete:calls.start}));
vi.mock('@/api/resources',async(original)=>({...await original<object>(),hostsQuery:()=>query('cascade-hosts',[])}));
const host={id:'host',name:'host-2',machine:{id:'machine',name:'host-2'}} as unknown as Host;
beforeEach(()=>{
  vi.clearAllMocks();calls.plan.mockImplementation((_id:string,body:HostDeletePreflightBody)=>Promise.resolve<HostDeletePlan>({expires_at:new Date(Date.now()+900_000).toISOString(),job:null,preflight_id:`delete-${body.mounts}`,name:'host-2',online:true,rows:[{kind:'environments',count:2,blockers:[]},{kind:'mount_locations',count:1,blockers:[]},{kind:'mcp_servers',count:1,blockers:[]},{kind:'provider_runtimes',count:1,blockers:[]}],recovery:[],others:0,volumes:[],kept_until:null,data:[{id:'mount:mount',resource:{id:'mount',name:'team-data'},label:null,kind:'mount',bytes:8192,until:null,selected:body.data.find((row)=>row.id==='mount:mount')?.fate??(body.mounts==='delete_data'?'delete':'leave'),choices:['leave','move','delete'],host:null,copy:null,blockers:[]}],objects:[],erase:body.erase??true,erase_blockers:[],inventory_measurement:null,executable:true,admin_grants:[],notification:{people:2,default_on:true}}));
  calls.start.mockImplementation(()=>new Promise(()=>undefined));
});
it('starts the reviewed cascades once and honors an unticked Notify choice',async()=>{
  const user=userEvent.setup();show(<RemoveHostDialog host={host} onClose={vi.fn()}/>);

  const name=await screen.findByRole('textbox',{name:i18n.t('hosts.remove.type',{name:'host-2'})});
  
  expect(calls.plan.mock.calls[0]?.[1]).toEqual({environments:'recover',mounts:'forget',providers:'leave',data:[]});
  const consent=screen.getByRole('checkbox',{name:i18n.t('operation.notify',{count:2,number:'2'})});expect(consent).toBeChecked();await user.click(consent);
  await user.click(name);await user.paste('host-2');const confirm=screen.getByRole('button',{name:i18n.t('hosts.remove.confirm',{name:'host-2'})});await user.click(confirm);
  await waitFor(()=>expect(calls.start).toHaveBeenCalledOnce());
  expect(calls.start.mock.calls[0]?.slice(0,2)).toEqual(['host',{preflight_id:'delete-forget',confirm:'host-2',notify:false,volumes:[],data_confirmations:[]}]);expect(confirm).toBeDisabled();
});
it('requires a volume mounts exact name only when deleting its data was selected',async()=>{
  const user=userEvent.setup();show(<RemoveHostDialog host={host} onClose={vi.fn()}/>);
  await user.click(await screen.findByRole('combobox',{name:i18n.t('hostCascade.mount_locations')}));
  await user.click(await screen.findByRole('option',{name:i18n.t('hostCascade.deleteData')}));
  const volume=await screen.findByRole('textbox',{name:i18n.t('hostCascade.typeMount',{name:'team-data'})});
  await user.click(screen.getByRole('textbox',{name:i18n.t('hosts.remove.type',{name:'host-2'})}));await user.paste('host-2');
  const confirm=screen.getByRole('button',{name:i18n.t('hosts.remove.confirm',{name:'host-2'})});expect(confirm).toBeDisabled();
  await user.click(volume);await user.paste('Team-data');expect(confirm).toBeDisabled();await user.clear(volume);await user.paste('team-data');expect(confirm).toBeEnabled();
  await user.click(confirm);await waitFor(()=>expect(calls.start).toHaveBeenCalledOnce());
  expect(calls.start.mock.calls[0]?.[1]).toEqual({preflight_id:'delete-delete_data',confirm:'host-2',notify:true,volumes:[],data_confirmations:[{id:'mount:mount',confirm:'team-data'}]});
});

it('keeps mount data by default even with erasure on and offers no fake MCP decision',async()=>{
  const user=userEvent.setup();show(<RemoveHostDialog host={host} onClose={vi.fn()}/>);
  const fate=await screen.findByRole('combobox',{name:i18n.t('hostCustody.fateFor',{name:'team-data'})});
  expect(fate).toHaveTextContent(i18n.t('hostCustody.fates.leave'));
  expect(screen.getByRole('checkbox',{name:i18n.t('hostCustody.erase')})).toBeChecked();
  expect(screen.queryByRole('combobox',{name:i18n.t('hostCascade.mcp_servers')})).toBeNull();
  expect(screen.getByText(i18n.t('hostCascade.stopOffering'))).toBeVisible();
  expect(screen.queryByRole('textbox',{name:i18n.t('hostCascade.typeMount',{name:'team-data'})})).toBeNull();
  await user.click(fate);await user.click(await screen.findByRole('option',{name:i18n.t('hostCustody.fates.delete')}));
  expect(await screen.findByRole('textbox',{name:i18n.t('hostCascade.typeMount',{name:'team-data'})})).toBeVisible();
  expect(calls.plan.mock.calls.at(-1)?.[1].data).toEqual([{id:'mount:mount',fate:'delete'}]);
});

it('other managers environments are a count without a recovery choice or an unused destination',async()=>{
  const base=calls.plan.getMockImplementation();
  if(base===undefined)throw new Error('The host plan fixture must be configured');
  calls.plan.mockImplementation(async(id,body)=>({...await base(id,body),others:2}));
  show(<RemoveHostDialog host={host} onClose={vi.fn()}/>);
  expect(await screen.findByText(i18n.t('loss.others',{count:2,number:'2'}))).toBeVisible();
  expect(screen.queryByRole('combobox',{name:i18n.t('hostCascade.environments')})).not.toBeInTheDocument();
  expect(screen.queryByRole('combobox',{name:i18n.t('hostCascade.host')})).not.toBeInTheDocument();
  expect(screen.queryByText(i18n.t('hostCascade.recoveryCount',{count:0,number:'0'}))).not.toBeInTheDocument();
});
