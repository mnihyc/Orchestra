/** The single screen/inline workflow for registering and testing machine-local agent and provider programs. */
import { useMutation,useQuery,useQueryClient } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { changeMachinePrograms,machineProgramsKey,machineProgramsQuery } from '@/api/machinePrograms';
import { machinePolicyQuery,machineQuery,probeMachine } from '@/api/resources';
import { ApiError } from '@/api/client';
import type { Machine,MachineProgram,MachineProgramKind,MachinePrograms,MachineProgramsCommand } from '@/generated/api-types';
import { TaskGuideLink } from '@/features/help/TaskGuideLink';
import { useHelpOpener } from '@/features/help/open';
import { CommandBox } from '@/features/resources/Facts';
import { readMachineFacts } from '@/features/resources/machines/facts';
import { FACT_MEMBERS } from '@/features/resources/providers/details';
import { machineProgramName } from '@/features/resources/machines/programNames';
import { useHarnessTitle } from '@/features/resources/harnesses';
import { AddMachineDialog } from '@/features/resources/machines/Enrollment';
import { commandArguments } from '@/lib/commandLine';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert,ErrorState } from '@/ui/ErrorState';
import { HelpHint } from '@/ui/HelpHint';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { CommandValue, PathValue } from '@/features/resources/TechnicalValue';

export { machineProgramName };

function object(value:unknown):Record<string,unknown> {return value!==null && typeof value==='object' && !Array.isArray(value)?value as Record<string,unknown>:{};}
function isList(value:unknown):value is readonly unknown[] {return Array.isArray(value);}
function words(value:unknown):string[] {return Array.isArray(value)?value.filter((item):item is string=>typeof item==='string'):[];}
function text(value:unknown):string|null {return typeof value==='string' && value!==''?value:null;}
/** Native registration refusals are validated field facts, including the deciding policy key. */
function programFailure(error:unknown):string|undefined {
  if(!(error instanceof ApiError))return undefined;
  const faults=error.faults();
  return faults.length===0?undefined:faults.map((fault)=>fault.message).join(' ');
}
function ProgramError({error}:{error:unknown}) {
  const message=programFailure(error);
  return message===undefined?<ErrorAlert error={error}/>:<StateLine role="alert" tone="danger" compact message={message}/>;
}


function ProgramsHelp() {
  return <TaskGuideLink guide="register-your-program" />;
}

/** Local discoveries are facts; changing the delegated policy is the authorization to use one. */
function LocalHarnessPrograms({machine,canManage}:{machine:Pick<Machine,'id'|'facts'>;canManage:boolean}) {
  const {t}=useTranslation();
  const client=useQueryClient();
  const openHelp=useHelpOpener();
  const [fresh,setFresh]=useState<{previous:unknown;facts:unknown}|null>(null);
  const facts=readMachineFacts(fresh!==null && fresh.previous===machine.facts?fresh.facts:machine.facts);
  const title=useHarnessTitle();
  const confirmed=facts.harnesses;
  const found=facts.discoveredHarnesses ?? [];
  const reported=facts.discoveredHarnesses!==null || confirmed.length>0;
  const policy=useQuery({...machinePolicyQuery(machine.id),enabled:reported && canManage});
  const key=policy.data?.keys.find((entry)=>entry.key==='agent.harnesses');
  const change=useMutation({mutationFn:async ({kind,path,allow}:{kind:string;path:string;allow:boolean})=>{
    if(key?.writable!==true)throw new Error(t('machinePrograms.managerRequired'));
    const saved=await changeMachinePrograms(machine.id,{action:allow?'confirm_harness':'remove_harness',kind,path});
    client.setQueryData(machineProgramsKey(machine.id),saved);
    void client.invalidateQueries({queryKey:machinePolicyQuery(machine.id).queryKey});
    return saved.machine_view ?? probeMachine(machine.id);
  },onSuccess:(updated)=>{
    setFresh({previous:machine.facts,facts:updated.facts});
    client.setQueryData(machineQuery(machine.id).queryKey,updated);
  }});
  const search=useMutation({mutationFn:()=>probeMachine(machine.id),onSuccess:(updated)=>{
    setFresh({previous:machine.facts,facts:updated.facts});
    client.setQueryData(machineQuery(machine.id).queryKey,updated);
  }});
  if(!reported)return null;
  const entries=[...confirmed.map((entry)=>({entry,confirmed:true})),...found.map((entry)=>({entry,confirmed:false}))];
  const missing=(facts.searchedHarnessKinds ?? []).filter((kind)=>!entries.some(({entry})=>entry.kind===kind));
  return <section aria-labelledby="machine-local-programs"><div className="mb-2 flex flex-wrap items-center gap-2">
    <h3 id="machine-local-programs" className="font-medium">{t('machinePrograms.installedHarnesses')}</h3>
    <HelpHint term={t('machinePrograms.installedHarnesses')} what={t('machinePrograms.harnessHelp')} guide={{article:'connect-a-machine',heading:'confirm-an-agent-program'}}
      more={{label:t('help.hint.more'),onOpen:()=>openHelp('connect-a-machine','confirm-an-agent-program')}}/>
    {canManage && <Button size="sm" pending={search.isPending} onClick={()=>search.mutate()}>{t('machinePrograms.searchAgain')}</Button>}
    {canManage && key!==undefined && !key.writable && <Button size="sm" asChild><Link to="/resources/machines/$id" params={{id:machine.id}} search={{tab:'policy'}}>{t('machinePrograms.openPolicy')}</Link></Button>}
  </div>
    {entries.length===0 && missing.length===0?<StateLine compact message={t('machinePrograms.noProgramsFound')}/>:<ul className="divide-y divide-border rounded-lg border border-border">
      {entries.map(({entry,confirmed:allowed})=><li key={`${entry.kind}:${entry.path}`} className="flex flex-wrap items-center gap-x-3 gap-y-1 px-3 py-2">
        <span>{title(entry.kind)}</span><span className="text-sm text-text-muted">{entry.version==='' || entry.version==='unknown'?t('machinePrograms.unknownVersion'):entry.version}</span>
        <PathValue path={entry.path} full/><span className="text-sm text-text-muted">{t(allowed?'machinePrograms.confirmed':'machinePrograms.found')}</span>
        {key?.writable===true && <Button size="sm" pending={change.isPending && change.variables.path===entry.path}
          disabled={change.isPending || search.isPending} onClick={()=>change.mutate({kind:entry.kind,path:entry.path,allow:!allowed})}>{t(allowed?'machinePrograms.removeConfirmation':'machinePrograms.use')}</Button>}
      </li>)}
      {missing.map((kind)=><li key={kind} className="flex flex-wrap items-center gap-x-3 gap-y-1 px-3 py-2"><span>{title(kind)}</span><span className="text-sm text-text-muted">{t('machinePrograms.noProgramFound')}</span></li>)}
    </ul>}
    {canManage && policy.isError && <ErrorAlert error={policy.error}/>}
    {change.isError && <ErrorAlert error={change.error}/>}
    {search.isError && <ErrorAlert error={search.error}/>}
  </section>;
}

/** Exact local operator entry; argv is serialized as TOML strings without a shell command or invented path. */
export function programConfiguration(kind:MachineProgramKind,name:string,command:readonly string[]):string {
  return `[[plugin]]\nkind = ${JSON.stringify(kind==='agent'?'adapter':'provider')}\nname = ${JSON.stringify(name)}\ncommand = ${JSON.stringify(command)}\n`;
}

function Declaration({entry}:{entry:MachineProgram}) {
  const {t}=useTranslation();
  const descriptor=object(entry.descriptor);
  const sandbox=words(descriptor.sandbox_modes).flatMap((mode)=>mode==='read_only'||mode==='workspace'||mode==='open'?[t(`machines.policy.sandbox.${mode}`)]:[]);
  const approval=words(descriptor.approval_modes).flatMap((mode)=>mode==='ask'||mode==='on_request'||mode==='auto'||mode==='bypass'?[t(`machinePrograms.approval.${mode}`)]:[]);
  const report=object(entry.models);
  const listed=isList(report.models)?report.models:[];
  const models=listed.flatMap((model)=>{
    const row=object(model);const name=text(row.display_name) ?? text(row.model_name) ?? text(row.name);return name===null?[]:[name];
  });
  const methods=object(descriptor.native_provider).login_methods;
  const signIns=isList(methods)?methods.flatMap((method)=>{const title=text(object(method).title);return title===null?[]:[title];}):[];
  const reports=words(object(descriptor.facts).reports).flatMap((name)=>{
    const member=FACT_MEMBERS.find((member)=>member===name);return member===undefined?[]:[t(`providerDetails.members.${member}`)];
  });
  const summary=[sandbox.length===0?null:t('machinePrograms.sandbox',{modes:sandbox.join(', ')}),
    approval.length===0?null:t('machinePrograms.approvalModes',{modes:approval.join(', ')}),
    entry.kind==='agent' && (entry.ready || Object.hasOwn(descriptor,'native_provider'))?t('machinePrograms.signIns',{methods:signIns.length===0?t('machinePrograms.noSignIn'):[...new Set(signIns)].join(', ')}):null,
    models.length===0 || models.length>2?null:t('machinePrograms.models',{models:models.join(', ')}),
    entry.kind==='provider_source' && models.length===0 && isList(report.models)?t('machinePrograms.noModels'):null,
    entry.kind==='provider_source' && models.length===0 && !isList(report.models) && descriptor.models!==null && descriptor.models!==undefined?t('machinePrograms.suppliesModels'):null,
    reports.length>0?t('providerSources.reports',{members:reports.join(', ')}):
      entry.kind==='provider_source' && descriptor.facts!==null && descriptor.facts!==undefined?t('machinePrograms.suppliesFacts'):null].filter((part)=>part!==null);
  return <>{summary.length>0 && <ul className="text-sm wrap-anywhere text-text-muted">{summary.map((line)=><li key={line}>{line}</li>)}</ul>}
    {models.length>2 && <span className="inline-flex items-center gap-1 text-sm text-text-muted">{t('machinePrograms.modelCount',{count:models.length})}
      <HelpHint term={t('machinePrograms.modelCount',{count:models.length})} what={<ul className="max-h-64 overflow-y-auto">{models.map((name,index)=><li key={`${index}:${name}`} className="wrap-anywhere">{name}</li>)}</ul>}/></span>}
    {entry.detail!==null && <p className="text-sm wrap-anywhere text-text-muted">{entry.detail}</p>}</>;
}

/** Shared inline entrance used by Add agent profile and the provider source fields as well as this page. */
export function RegisterMachineProgram({machineId,kind,onRegistered,onClose}:{machineId:string;kind:MachineProgramKind;
  onRegistered:(program:MachineProgram)=>void;onClose:()=>void}) {
  const {t}=useTranslation();
  const client=useQueryClient();
  const query=useQuery(machineProgramsQuery(machineId));
  const [name,setName]=useState('');
  const [command,setCommand]=useState('');
  const [tested,setTested]=useState<MachineProgram|null>(null);
  const argv=commandArguments(command);
  const valid=name.trim()!=='' && argv!==null && argv.length>0 && argv[0]!=='';
  const capability=query.data?.capabilities.find((capability)=>capability.kind===kind);
  const remote=query.data?.can_manage===true && capability?.remote===true;
  const supported=capability?.supported===true;
  const kept=capability?.delegated!==true;
  const apply=useMutation({mutationFn:(body:MachineProgramsCommand)=>changeMachinePrograms(machineId,body),onSuccess:(answer)=>{
    client.setQueryData(machineProgramsKey(machineId),answer);
    const entry=answer.entries.find((entry)=>entry.kind===kind && entry.name===name.trim());
    if(entry!==undefined) {setTested(entry);if(entry.ready) {onRegistered(entry);onClose();}}
  }});
  return <Dialog open onOpenChange={(open)=>!open && !apply.isPending && onClose()} title={t('machinePrograms.addTitle')}
    footer={<><Button disabled={apply.isPending} onClick={onClose}>{t('common.close')}</Button>
      {remote && tested===null && <Button variant="primary" pending={apply.isPending} disabled={!valid}
        onClick={()=>{if(valid) apply.mutate({action:'register',kind,name:name.trim(),command:argv});}}>{t('machinePrograms.register')}</Button>}
    </>}>
    <div className="flex flex-col gap-3 py-2">
      {query.isPending?<SkeletonRows rows={3}/>:query.isError?<ErrorState compact title={t('machinePrograms.readFailed')} error={query.error} retrying={query.isRefetching} onRetry={()=>void query.refetch()}/>:<>
        <p className="flex items-center gap-2 text-sm text-text-muted">{t(`machinePrograms.kinds.${kind}.name`)}<ProgramsHelp/></p>
        <Input label={t('machinePrograms.name')} value={name} disabled={apply.isPending}
          onChange={(event)=>{setName(event.target.value);setTested(null);apply.reset();}} autoComplete="off"/>
        <Input label={t('machinePrograms.command')} error={argv===null?t('machinePrograms.invalidCommand'):undefined} value={command} disabled={apply.isPending}
          onChange={(event)=>{setCommand(event.target.value);setTested(null);apply.reset();}} autoComplete="off" spellCheck={false}/>
        {!supported?<p className="text-sm text-text-muted">{t('machinePrograms.roleNeeded')}</p>:!remote && !kept && query.data.can_manage?<>
          <p className="text-sm text-text-muted">{t('machinePrograms.switchedOff')}</p>
          <Button asChild><Link to="/resources/machines/$id" params={{id:machineId}} search={{tab:'policy'}}>{t('machinePrograms.openPolicy')}</Link></Button>
        </>:!remote?<div role="group" aria-label={t('machinePrograms.localConfiguration')} className="flex min-w-0 flex-col gap-2">
          <p className="text-sm wrap-anywhere text-text-muted">{query.data.can_manage?t('machinePrograms.kept',{file:query.data.config_file ?? t('machinePrograms.localConfiguration')}):t('machinePrograms.managerRequired')}</p>
          {valid && kept && query.data.can_manage && <CommandBox command={programConfiguration(kind,name.trim(),argv)} label={t('machinePrograms.copyConfiguration')}/>}
          {kept && query.data.can_manage && <Button size="sm" className="self-start" pending={apply.isPending} onClick={()=>apply.mutate({action:'reload'})}>{t('machinePrograms.reload')}</Button>}
        </div>:null}
        {tested!==null && <div role="status" className="rounded-lg border border-control-border p-3">
          <p className="font-medium">{t('machinePrograms.described',{name:machineProgramName(tested)})}</p><Declaration entry={tested}/>
        </div>}
      </>}
      {apply.isError && <ProgramError error={apply.error}/>}</div>
  </Dialog>;
}

/** This tab alone subscribes to program data; hidden tabs create no query or native request. */
export function MachineProgramsTab({machine}:{machine:Pick<Machine,'id'|'facts'|'roles'|'role_management'>}) {
  const {t}=useTranslation();
  const client=useQueryClient();
  const query=useQuery(machineProgramsQuery(machine.id));
  const machineFacts=readMachineFacts(machine.facts);
  const showLocalHarnesses=machineFacts.discoveredHarnesses!==null || (machineFacts.policy!==null && machineFacts.harnesses.length>0);
  const harnesses=showLocalHarnesses?[]:machineFacts.harnesses;
  const harnessTitle=useHarnessTitle(harnesses.length>0);
  const [enrolling,setEnrolling]=useState(false);
  const [kind,setKind]=useState<MachineProgramKind>('agent');
  const [adding,setAdding]=useState(false);
  const [removing,setRemoving]=useState<MachineProgram|null>(null);
  const apply=useMutation({mutationFn:(body:MachineProgramsCommand)=>changeMachinePrograms(machine.id,body),onSuccess:(answer)=>{
    client.setQueryData(machineProgramsKey(machine.id),answer);setRemoving(null);
  }});
  if(query.isPending) return <SkeletonRows rows={3}/>;
  if(query.isError) return <ErrorState title={t('machinePrograms.readFailed')} error={query.error} retrying={query.isRefetching} onRetry={()=>void query.refetch()}/>;
  const programs:MachinePrograms=query.data;
  const roles=machine.role_management;
  const canChangeRoles=roles?.supported===true && roles.delegated && roles.enabled;
  const kinds=programs.capabilities.filter((capability)=>capability.supported);
  const selected=kinds.some((capability)=>capability.kind===kind)?kind:kinds[0]?.kind ?? kind;
  return <div className="flex flex-col gap-4">
    <div className="flex flex-wrap items-center gap-2">
      {programs.can_manage && kinds.length>0 && <>
        <Select label={t('machinePrograms.contract')} value={selected} onValueChange={setKind}
          options={kinds.map((capability)=>({value:capability.kind,name:t(`machinePrograms.kinds.${capability.kind}.name`),description:t(`machinePrograms.kinds.${capability.kind}.description`)}))}/>
        <Button onClick={()=>setAdding(true)}>{t('machinePrograms.add')}</Button>
        <Button pending={apply.isPending && apply.variables.action==='reload'} onClick={()=>apply.mutate({action:'reload'})}>{t('machinePrograms.reload')}</Button>
      </>}
      <ProgramsHelp/>
    </div>
    {kinds.length===0 && <EmptyState compact title={t(programs.capabilities.length===0 && !machine.roles.includes('node')?'machinePrograms.notReported':'machinePrograms.roleNeeded')}
      action={programs.can_manage?canChangeRoles?<Button asChild><Link to="/resources/machines/$id" params={{id:machine.id}} search={{tab:'overview'}}>{t('machineRoles.edit')}</Link></Button>:<Button onClick={()=>setEnrolling(true)}>{t('machinePrograms.addConnector')}</Button>:undefined}/>}
    {programs.entries.length===0?(kinds.length===0?null:<EmptyState compact title={t('machinePrograms.empty')} />):
      <ul aria-label={t('machinePrograms.title')} className="divide-y divide-border rounded-lg border border-border">
        {programs.entries.map((entry)=>{
          const name=machineProgramName(entry);
          const contract=t(`machinePrograms.kinds.${entry.kind}.name`);
          const capability=programs.capabilities.find((capability)=>capability.kind===entry.kind);
          return <li key={`${entry.kind}:${entry.name}`} className="flex flex-wrap items-start gap-3 px-3 py-2">
            <div className="min-w-0 flex-1"><p className="flex min-h-(--control-sm) flex-wrap items-center gap-x-2"><span className="font-medium">{name}</span>
              {entry.version!==null && <span className="text-sm text-text-muted">{entry.version}</span>}
              {name!==contract && <span className="text-sm text-text-muted">{contract}</span>}
              {!entry.ready && <span className="text-sm text-warning-text">{t('machinePrograms.needsAttention')}</span>}</p>
              {entry.command.length>0 && <CommandValue command={entry.command} />}
              <Declaration entry={entry}/>
              {entry.origin==='local' && programs.entries.some((program)=>program.origin==='orch') && <span className="text-sm text-text-muted">{t('machinePrograms.local')}</span>}
            </div>
            {programs.can_manage && <div className="flex items-center gap-1">
              <Button size="sm" pending={apply.isPending && apply.variables.action==='test' && apply.variables.name===entry.name}
                onClick={()=>apply.mutate({action:'test',kind:entry.kind,name:entry.name})}>{t('machinePrograms.test')}</Button>
              {entry.origin==='orch' && capability?.remote===true && <Button size="sm" variant="danger-ghost" onClick={()=>setRemoving(entry)}>{t('machinePrograms.remove')}</Button>}
            </div>}
          </li>;
        })}
      </ul>}
    {programs.entries.some((entry)=>entry.origin==='local') && <p className="text-sm text-text-muted">{t('machinePrograms.localEntry',{file:programs.config_file ?? t('machinePrograms.localConfiguration')})}</p>}
    {showLocalHarnesses && <LocalHarnessPrograms machine={machine} canManage={programs.can_manage}/>}
    {harnesses.length>0 && <section aria-labelledby="machine-installed-harnesses"><h3 id="machine-installed-harnesses" className="mb-2 font-medium">{t('machinePrograms.installedHarnesses')}</h3>
      <ul className="divide-y divide-border rounded-lg border border-border">{harnesses.map((harness)=><li key={`${harness.kind}:${harness.path}`} className="flex flex-wrap items-baseline gap-x-3 gap-y-1 px-3 py-2">
        <span>{harnessTitle(harness.kind)}</span>{harness.version!=='' && harness.version!=='unknown' && <span className="text-sm text-text-muted">{harness.version}</span>}<PathValue path={harness.path}/>
      </li>)}</ul></section>}
    {apply.isError && removing===null && <ProgramError error={apply.error}/>}
    {adding && <RegisterMachineProgram machineId={machine.id} kind={selected} onRegistered={()=>setAdding(false)} onClose={()=>setAdding(false)}/>}
    {enrolling && <AddMachineDialog onClose={()=>setEnrolling(false)}/>}
    {removing!==null && <ConfirmDialog open onOpenChange={(open)=>!open && setRemoving(null)} title={t('machinePrograms.removeTitle',{name:machineProgramName(removing)})}
      confirmLabel={t('machinePrograms.remove')} danger pending={apply.isPending} error={apply.isError && programFailure(apply.error)===undefined?apply.error:undefined}
      onConfirm={()=>apply.mutate({action:'remove',kind:removing.kind,name:removing.name})}>{t('machinePrograms.removeBody')}
        {apply.isError && programFailure(apply.error)!==undefined && <ProgramError error={apply.error}/>}</ConfirmDialog>}
  </div>;
}
