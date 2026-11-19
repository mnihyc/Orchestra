/** One reviewed cascade removes the host after independent work and the ordinary kept-data window finish. */
import { Hint } from '@/features/help/Hint';
import { keepPreviousData, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { cancelJob, isOver, jobQuery } from '@/api/forks';
import { record } from '@/api/cache';
import { planHostDelete, startHostDelete } from '@/api/loss';
import { hostsQuery } from '@/api/resources';
import { reasonWords } from '@/features/change/Result';
import { blockerWords } from '@/features/change/plan';
import { Frame } from '@/features/change/Frame';
import { JobProgress } from '@/features/change/Progress';
import { Measurements, MeasurementReason, type PlanCounts } from '@/features/change/Measurements';
import { NotifyPeople } from '@/features/notifications/NotifyPeople';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { CommandBox } from '@/features/resources/Facts';
import type { Host, HostDataFate, HostDataRow, HostDeletePreflightBody, HostEnvironmentCascade, HostMountCascade, HostProviderCascade, Job, PlanBlocker } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { useDebounced } from '@/lib/debounce';
import { formatBytes, formatNumber } from '@/lib/format';
import { formatExact } from '@/lib/time';
import { useActKey } from '@/lib/forms';
import { permissionLabel } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { InlineAlert } from '@/ui/InlineAlert';
import { RelativeTime } from '@/ui/RelativeTime';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { Table } from '@/ui/Table';

export function RemoveHostDialog({ host, onClose, afterDelete }: { host: Host; onClose: () => void; afterDelete?: () => void }) {
  const [started,setStarted]=useState<Job|null>(null);
  return started===null ? <HostDeletionPlan host={host} onClose={onClose} onStarted={setStarted} /> : <HostDeletionProgress host={host} initial={started} onClose={onClose} afterDelete={afterDelete} />;
}
function HostDeletionPlan({host,onClose,onStarted}:{host:Host;onClose:()=>void;onStarted:(job:Job)=>void}){
  const {t}=useTranslation();const format=useFormatContext();const {locale}=format;const key=useActKey();
  const [body,setBody]=useState<HostDeletePreflightBody>({environments:'recover',mounts:'forget',providers:'leave',data:[]});
  const asked=JSON.stringify(body);const settled=useDebounced(asked);
  const plan=useQuery({queryKey:['host-delete-plan',host.id,settled],queryFn:({signal})=>planHostDelete(host.id,JSON.parse(settled) as HostDeletePreflightBody,signal),staleTime:Number.POSITIVE_INFINITY,gcTime:0,retry:false,placeholderData:keepPreviousData});
  const hosts=useQuery(hostsQuery(null));
  const [typed,setTyped]=useState('');const [volumes,setVolumes]=useState<Record<string,string>>({});const [notify,setNotify]=useState<boolean|undefined>();
  const start=useMutation({mutationFn:()=>{if(plan.data===undefined)throw new Error('The deletion plan is unavailable');return startHostDelete(host.id,{preflight_id:plan.data.preflight_id,confirm:typed,notify:notify??plan.data.notification.default_on,volumes:[],data_confirmations:plan.data.data.filter((row)=>row.selected==='delete'||row.selected==='purge').map((row)=>({id:row.id,confirm:volumes[row.id]??''}))},key({plan:plan.data.preflight_id}));},onSuccess:onStarted,onError:()=>{void plan.refetch();}});
  const data=plan.data;const accepted=data?.job;const stale=asked!==settled||plan.isFetching;
  const options=(kind:string)=>kind==='environments'?[{value:'recover',name:t('hostCascade.recover')},{value:'remove',name:t('hostCascade.remove')}]:kind==='mount_locations'?[{value:'forget',name:t('hostCascade.forget')},...(data?.online===true?[{value:'delete_data',name:t('hostCascade.deleteData')}]:[])]:kind==='provider_runtimes'?[{value:'leave',name:t('hostCascade.leave')},{value:'recreate',name:t('loss.recreate')}]:[{value:'stop',name:t('hostCascade.stopOffering')}];
  const value=(kind:string)=>kind==='environments'?body.environments:kind==='mount_locations'?body.mounts:kind==='provider_runtimes'?body.providers:'stop';
  const change=(kind:string,next:string)=>setBody((before)=>({...before,...(kind==='environments'?{environments:next as HostEnvironmentCascade}:kind==='mount_locations'?{mounts:next as HostMountCascade}:kind==='provider_runtimes'?{providers:next as HostProviderCascade}:{})}));
  const kindLabel=(kind:string)=>t(`hostCascade.${kind}`,{defaultValue:t('hostCascade.kind')});
  const words=(blocker:PlanBlocker)=>t(`hostCustody.blockers.${blocker.code.replaceAll('.','_')}`,{defaultValue:blockerWords(blocker,null,t).text});
  const onlyOthers=data?.rows.some((row)=>row.kind==='environments'&&row.count===data.others)??false;
  const choiceFor=(kind:string,count:number)=>count===0||kind==='environments'&&onlyOthers?null:kind==='mcp_servers'?<span className="text-sm text-text-muted">{t('hostCascade.stopOffering')}</span>:<Select label={kindLabel(kind)} hideLabel size="sm" fieldClassName="min-w-0 flex-1" disabled={start.isPending} value={value(kind)} onValueChange={(next)=>change(kind,next)} options={options(kind)}/>;
  const effectFor=(kind:string,blockers:readonly PlanBlocker[])=>blockers.length>0?<StateLine compact tone={blockers.every((blocker)=>blocker.code==='measurement_pending')?'neutral':'warning'} message={blockers.map(words).join(' · ')}/>:kind==='environments'&&(onlyOthers||body.environments==='recover'&&data?.recovery.length===0)?null:<span className="text-sm text-text-muted">{kind==='environments'?body.environments==='remove'?t(data?.online===true?'hostCascade.keptWindow':'hostCascade.dataUnavailable'):t('hostCascade.recoveryCount',{count:data?.recovery.length??0,number:formatNumber(data?.recovery.length??0,locale)}):t('hostCascade.definitionKept')}</span>;
  const countPlans:PlanCounts[]=data?.recovery.flatMap((row)=>[{files:row.files},...(row.move_plan===null?[]:[{ ...row.move_plan, homeFallback: true }]),...(row.switch_plan?.plans??[])])??[];
  countPlans.push(...(data?.data.map((row)=>({mounts:row.copy===null?[]:[{copy:row.copy}]}))??[]));
  if(data?.inventory_measurement!==undefined&&data.inventory_measurement!==null)countPlans.push({measurements:[data.inventory_measurement]});
  const waiting=(data?.recovery.some((row)=>row.blockers.some((blocker)=>blocker.code==='measurement_pending'))??false)||(data?.data.some((row)=>row.blockers.some((blocker)=>blocker.code==='measurement_pending'))??false)||(data?.erase_blockers.some((blocker)=>blocker.code==='measurement_pending')??false);
  const destinations=(hosts.data??[]).filter((row)=>row.id!==host.id&&row.machine.online&&row.schedulable&&row.lost_at===undefined);
  const selectData=(row:HostDataRow,fate:HostDataFate,hostId?:string)=>setBody((before)=>({...before,data:[...before.data.filter((choice)=>choice.id!==row.id),{id:row.id,fate,...(hostId===undefined?{}:{host_id:hostId})}]}));
  const migrates=data!==undefined&&(data.recovery.length>0||body.providers==='recreate'&&data.rows.some((row)=>row.kind==='provider_runtimes'&&row.count>0)||data.data.some((row)=>row.selected==='move'));
  const fateFor=(row:HostDataRow)=><div className="flex min-w-0 flex-col gap-2"><Select size="sm" hideLabel label={t('hostCustody.fateFor',{name:row.label??row.resource.name??t('loss.hiddenResource')})} value={body.data.find((choice)=>choice.id===row.id)?.fate??row.selected} options={row.choices.map((fate)=>({value:fate,name:fate==='wait'&&row.until!==null?t('hostCustody.waitUntil',{date:formatExact(row.until,format)}):t(`hostCustody.fates.${fate}`)}))} disabled={start.isPending} onValueChange={(value)=>selectData(row,value)}/>{row.selected==='move'&&<Select size="sm" hideLabel label={t('hostCustody.destinationFor',{name:row.resource.name??t('loss.hiddenResource')})} value={body.data.find((choice)=>choice.id===row.id)?.host_id??body.host_id??'choose'} options={[{value:'choose',name:t('hostCustody.chooseHost')},...destinations.map((host)=>({value:host.id,name:host.name}))]} onValueChange={(value)=>selectData(row,'move',value==='choose'?undefined:value)} disabled={start.isPending}/>}</div>;
  return <Measurements plan={{plans:countPlans}} active onReady={()=>void plan.refetch()}><Frame open onClose={onClose} aside={<Hint popover id="loss-delete-host" />} at={start.isPending?'confirm':'plan'} dismissible={!start.isPending} title={t('hosts.remove.title',{name:host.name})} footer={<>
    <div className="me-auto min-w-0">{waiting?<span role="status" data-change-reason><MeasurementReason waiting planning={stale} fallback={null}/></span>:<NotifyPeople plan={data?.notification} value={notify} onChange={setNotify} disabled={start.isPending||stale} />}</div>
    <Button disabled={start.isPending} onClick={onClose}>{t('common.cancel')}</Button>
    <Button variant="danger" pending={start.isPending} disabled={start.isPending||data===undefined||stale||!data.executable||typed!==data.name||data.data.some((row)=>(row.selected==='delete'||row.selected==='purge')&&volumes[row.id]!== (row.label??row.resource.name))} onClick={()=>start.mutate()}>{t('hosts.remove.confirm',{name:host.name})}</Button>
  </>}>
    {plan.isPending?<SkeletonRows rows={4}/>:plan.isError?<ErrorState compact error={plan.error} retrying={plan.isRefetching} onRetry={()=>void plan.refetch()}/>:data!==undefined&&<div className="flex min-w-0 flex-col gap-3">
      {accepted!==undefined&&accepted!==null&&<Button onClick={()=>onStarted(accepted)}>{t('operation.showProgress')}</Button>}
      {migrates&&<Select label={t('hostCascade.host')} value={body.host_id??'automatic'} onValueChange={(next)=>setBody((before)=>{const copy={...before};if(next==='automatic')delete copy.host_id;else copy.host_id=next;return copy;})} options={[{value:'automatic',name:t('changeCompact.automatic')},...(hosts.data??[]).filter((row)=>row.id!==host.id&&row.machine.online&&row.schedulable&&row.lost_at===undefined).map((row)=>({value:row.id,name:row.name}))]}/>}
      <Table label={t('hostCascade.title')} rows={data.rows} rowKey={(row)=>row.kind} columns={[
        {id:'kind',header:t('hostCascade.kind'),cell:(row,shown)=><div className="flex min-w-0 flex-col gap-1"><div className="flex min-w-0 flex-wrap items-center gap-2"><span>{kindLabel(row.kind)} · {formatNumber(row.count,locale)}</span>{!shown.has('choice')&&choiceFor(row.kind,row.count)}</div>{!shown.has('effect')&&effectFor(row.kind,row.blockers)}</div>},
        {id:'choice',header:t('loss.action'),from:'sm',cell:(row)=>choiceFor(row.kind,row.count)},
        {id:'effect',header:t('loss.carries'),from:'md',cell:(row)=>effectFor(row.kind,row.blockers)},
      ]}/>
      {data.others>0&&<span className="text-sm text-text-muted">{t('loss.others',{count:data.others,number:formatNumber(data.others,locale)})}</span>}
      {data.kept_until!==null&&<span className="text-sm text-text-muted">{t('hostCascade.keepingUntil')} <RelativeTime at={data.kept_until}/></span>}
      {data.data.length>0&&<Table label={t('hostCustody.data')} boundedIdentity rows={data.data} rowKey={(row)=>row.id} columns={[
        {id:'name',header:t('storage.name'),grow:true,cell:(row,shown)=><div className="flex min-w-0 flex-col gap-1"><HostDataName row={row}/><span className="text-sm text-text-muted">{row.bytes===null?'–':formatBytes(row.bytes,locale)}{row.until!==null&&<> · <RelativeTime at={row.until}/></>}</span>{row.blockers.length>0&&<StateLine compact tone={row.blockers.every((blocker)=>blocker.code==='measurement_pending')?'neutral':'warning'} message={row.blockers.map(words).join(' · ')}/>}{!shown.has('fate')&&fateFor(row)}</div>},
        {id:'fate',header:t('storage.cleanup.fate'),from:'sm',cell:fateFor},
      ]}/>}
      <Checkbox label={t('hostCustody.erase')} checked={body.erase??data.erase} disabled={start.isPending||host.lost_at!==undefined} onCheckedChange={(erase)=>setBody((before)=>({...before,erase}))}/>
      {data.erase_blockers.map((blocker)=><StateLine key={blocker.code} compact tone={blocker.code==='measurement_pending'?'neutral':'warning'} message={words(blocker)}/>)}
      {data.objects.length>0&&<Table label={t('hostCustody.objects')} boundedIdentity rows={data.objects} rowKey={(row)=>row.id} columns={[{id:'name',header:t('storage.name'),grow:true,cell:(row)=><span className="truncate" title={row.name}>{row.name}</span>},{id:'bytes',header:t('storage.size'),cell:(row)=>row.bytes===null?'–':formatBytes(row.bytes,locale)}]}/>}
      {data.data.filter((row)=>row.selected==='delete'||row.selected==='purge').map((row)=><Input key={row.id} label={t('hostCascade.typeMount',{name:row.label??row.resource.name??t('loss.hiddenResource')})} value={volumes[row.id]??''} onChange={(event)=>setVolumes((before)=>({...before,[row.id]:event.target.value}))} autoComplete="off" disabled={start.isPending}/>)}
      <Input label={t('hosts.remove.type',{name:data.name})} value={typed} onChange={(event)=>setTyped(event.target.value)} autoComplete="off" disabled={start.isPending}/>
      {data.admin_grants.length>0&&<InlineAlert tone="warning"><span>{t('loss.adminGrant')}</span><ul>{data.admin_grants.map((grant)=><li key={`${grant.object.kind}:${grant.object.id}:${grant.permission}`}>{grant.name} · {permissionLabel(grant.permission)}</li>)}</ul></InlineAlert>}
    </div>}
    {start.error!==null&&<ErrorAlert error={start.error}/>}
  </Frame></Measurements>;
}
function HostDeletionProgress({host,initial,onClose,afterDelete}:{host:Host;initial:Job;onClose:()=>void;afterDelete?:()=>void}){
  const {t}=useTranslation();const {locale}=useFormatContext();const client=useQueryClient();const [cancelling,setCancelling]=useState(false);
  const query=useQuery({...jobQuery(initial.id),initialData:initial});const job=query.data;const over=isOver(job.state);
  const cancel=useMutation({mutationFn:()=>cancelJob(job.id),onSuccess:(accepted)=>{client.setQueryData(['job',job.id],accepted);setCancelling(false);}});
  const result=record(job.result)?job.result:null;const rows=leftObjects(result);const data=leftData(result);const unreachable=result?.unreachable===true;const confirmed=result?.left_confirmed===true;const command=typeof result?.uninstall_command==='string'?result.uninstall_command:null;
  return <Frame open onClose={onClose} at={over?'done':'progress'} failed={job.state==='failed'||job.state==='lost'} title={t('hosts.remove.title',{name:host.name})} footer={<><Button onClick={()=>{if(job.state==='succeeded')afterDelete?.();onClose();}}>{t(over?'common.close':'change.progress.hide')}</Button>{!over&&<Button variant="danger" onClick={()=>setCancelling(true)}>{t('change.progress.cancel')}</Button>}</>}>
    <JobProgress job={{...job,progress:{...job.progress,message:t(`hostCascade.phase.${job.progress.phase}`,{defaultValue:t('source.job.running')})}}} heading={t('hostCascade.title')} phaseLabel={(name)=>t(`hostCustody.phases.${name}`,{defaultValue:name})} reconnecting={query.isError}/>
    {query.error!==null&&<ErrorAlert error={query.error}/>}{job.error!==null&&<StateLine compact tone="warning" message={t(`hostCustody.blockers.${job.error.code.replaceAll('.','_')}`,{defaultValue:reasonWords(job,t,10)})}/>}
    {over&&<><StateLine title={t(`source.job.${job.state}`)}/>{unreachable&&<StateLine compact message={t('hostCustody.unreachable')}/>}{rows.length>0&&<><h3 className="text-base font-medium">{t(confirmed?'hostCustody.left':'hostCustody.lastObserved')}</h3>{!confirmed&&<StateLine compact tone="warning" message={t('hostCustody.unconfirmed')}/>}<Table label={t(confirmed?'hostCustody.left':'hostCustody.lastObserved')} boundedIdentity rows={rows} rowKey={(row)=>row.id} columns={[{id:'name',header:t('storage.name'),grow:true,cell:(row,shown)=><div className="flex min-w-0 flex-col gap-1"><span className="truncate" title={row.name}>{row.name}</span>{!shown.has('observed')&&row.observed_at!==null&&<RelativeTime at={row.observed_at} className="text-sm text-text-muted"/>}</div>},{id:'bytes',header:t('storage.size'),cell:(row)=>row.bytes===null?'–':formatBytes(row.bytes,locale)},{id:'observed',header:t('hostCustody.observed'),from:'md',cell:(row)=>row.observed_at===null?'–':<RelativeTime at={row.observed_at}/>} ]}/></>}{data.length>0&&<Table label={t('hostCustody.left')} boundedIdentity rows={data} rowKey={(row)=>row.id} columns={[{id:'name',header:t('storage.name'),grow:true,cell:(row)=><HostDataName row={row}/>},{id:'bytes',header:t('storage.size'),cell:(row)=>row.bytes===null?'–':formatBytes(row.bytes,locale)}]}/>}{(unreachable||rows.length>0||data.length>0)&&(command===null?<StateLine compact message={t('machineInstallation.uninstallUnknown')}/>:<CommandBox command={command} label={t('hostCustody.copyUninstall')}/>)}</>}
    <ConfirmDialog open={cancelling} onOpenChange={setCancelling} title={t('hostCascade.cancelTitle')} confirmLabel={t('change.progress.cancel')} danger pending={cancel.isPending} error={cancel.error} onConfirm={()=>cancel.mutate()}>{t('hostCascade.cancelEffect')}</ConfirmDialog>
  </Frame>;
}

type DataIdentity=Pick<HostDataRow,'id'|'resource'|'label'|'kind'|'bytes'>;
function HostDataName({row}:{row:DataIdentity}){
  const {t}=useTranslation();const resource=row.resource;
  if(row.label!==null)return <span className="truncate" title={row.label}>{row.label}</span>;
  if(resource.name===null)return <span className="text-sm text-text-muted">{t('loss.hiddenResource')}</span>;
  const reference={id:resource.id,name:resource.name,address:''};
  return <ResourceChip subject={row.kind==='mount'?{kind:'mount',mount:reference}:{kind:'environment',environment:reference}} hint={false}/>;
}

function leftObjects(result:unknown):{id:string;name:string;bytes:number|null;observed_at:string|null}[]{
  if(typeof result!=='object'||result===null||!('left' in result)||!Array.isArray(result.left))return [];
  return result.left.flatMap((row)=>record(row)&&typeof row.id==='string'&&typeof row.name==='string'&&(row.bytes===null||typeof row.bytes==='number')?[{id:row.id,name:row.name,bytes:row.bytes,observed_at:typeof row.observed_at==='string'&&Number.isFinite(Date.parse(row.observed_at))?row.observed_at:null}]:[]);
}

function leftData(result:unknown):DataIdentity[]{
  if(!record(result)||!Array.isArray(result.left_data))return [];
  return result.left_data.flatMap((row):DataIdentity[]=>record(row)&&typeof row.id==='string'&&record(row.resource)&&typeof row.resource.id==='string'&&(row.resource.name===null||typeof row.resource.name==='string')&&(row.label===null||typeof row.label==='string')&&typeof row.kind==='string'&&(row.bytes===null||typeof row.bytes==='number')?[{id:row.id,resource:{id:row.resource.id,name:row.resource.name},label:row.label,kind:row.kind,bytes:row.bytes}]:[]);
}
