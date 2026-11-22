/** The single screen/inline workflow for registering and testing machine-local agent and provider programs. */
import { useMutation,useQuery,useQueryClient } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { CompleteCollection } from '@/ui/CompleteCollection';
import { EmptyState } from '@/ui/EmptyState';
import { changeMachinePrograms,machineProgramsKey,machineProgramsQuery } from '@/api/machinePrograms';
import { machineKey } from '@/api/resources';
import { machineProgramsProjection, record } from '@/api/cache';
import { ApiError } from '@/api/client';
import type { Machine,MachineProgram,MachineProgramKind,MachinePrograms,MachineProgramsCommand } from '@/generated/api-types';
import { TaskGuideLink } from '@/features/help/TaskGuideLink';
import { CommandBox } from '@/features/resources/Facts';
import { harnessPolicyDelegated, readMachineFacts, type FoundHarness } from '@/features/resources/machines/facts';
import { LocalHarnessCommand } from '@/features/resources/machines/LocalHarnessCommand';
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
import { ExecutionApproval } from '@/features/resources/ExecutionApproval';

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
      <HelpHint term={t('machinePrograms.modelCount',{count:models.length})} what={<CompleteCollection label={t('machinePrograms.modelCount',{count:models.length})} items={models} textOf={(name)=>name} scroll>{(page)=><ul>{page.map((name,index)=><li key={`${index}:${name}`} className="wrap-anywhere">{name}</li>)}</ul>}</CompleteCollection>}/></span>}
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
      {query.isPending?<SkeletonRows read={query} rows={3}/>:query.isError?<ErrorState compact title={t('machinePrograms.readFailed')} error={query.error} retrying={query.isRefetching} onRetry={()=>void query.refetch()}/>:<>
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
export function MachineProgramsTab({machine}:{machine:Pick<Machine,'id'|'facts'|'roles'|'role_management'> & Partial<Pick<Machine,'installation'|'platform_os'|'access'|'execution_approvals'>>}) {
  const {t}=useTranslation();
  const client=useQueryClient();
  const held=client.getQueryState<Machine>(machineKey(machine.id));
  const projected=held?.data===machine?machineProgramsProjection(machine.facts,machine.access,machine.execution_approvals):null;
  const query=useQuery({...machineProgramsQuery(machine.id),...(projected===null?{}:{initialData:projected,initialDataUpdatedAt:held?.dataUpdatedAt})});
  const facts=readMachineFacts(machine.facts);
  const harnesses=facts.harnesses;
  const discovered=facts.discoveredHarnesses ?? [];
  const nativeReported=facts.discoveredHarnesses!==null || facts.searchedHarnessKinds!==null || harnesses.length>0;
  const nativeRole=machine.roles.some((role)=>role==='environment' || role==='agent');
  const nativeRelevant=nativeReported || nativeRole;
  const nativeFetch=nativeRole && object(object(machine.facts).build_fetch).supported===true;
  const fetchPolicy=facts.policy?.['agent.programs']!==true?'agent.programs':facts.policy['tools.fetch']!==true?'tools.fetch':null;
  const harnessTitle=useHarnessTitle(harnesses.length+discovered.length>0);
  const [enrolling,setEnrolling]=useState(false);
  const [kind,setKind]=useState<MachineProgramKind>('agent');
  const [adding,setAdding]=useState(false);
  const [removing,setRemoving]=useState<MachineProgram|null>(null);
  const [unconfirming,setUnconfirming]=useState<FoundHarness|null>(null);
  // The program whose local command is shown, on a machine that keeps its agent programs in its own policy, and the act it
  // was opened for. A box opened to accept a program closes once the machine reports it confirmed (and one opened to
  // withdraw it, once it is gone): its command would now do the opposite, under the hand of someone copying it again.
  const [local,setLocal]=useState<{id:string;accept:boolean}|null>(null);
  const apply=useMutation({mutationFn:(body:MachineProgramsCommand)=>changeMachinePrograms(machine.id,body),onSuccess:(answer)=>{
    client.setQueryData(machineProgramsKey(machine.id),answer);
    if(answer.machine_view!==null && answer.machine_view!==undefined)client.setQueryData(machineKey(machine.id),answer.machine_view);
    setRemoving(null);setUnconfirming(null);
  },onError:(error,body)=>{
    if((body.action==='confirm_harness' || body.action==='remove_harness') && error instanceof ApiError && error.is('policy.not_delegated')) {
      setUnconfirming(null);setLocal({id:`${body.kind}:${body.path}`,accept:body.action==='confirm_harness'});
    }
  }});
  if(query.isPending) return <SkeletonRows read={query} rows={3}/>;
  if(query.isError) return <ErrorState title={t('machinePrograms.readFailed')} error={query.error} retrying={query.isRefetching} onRetry={()=>void query.refetch()}/>;
  const programs:MachinePrograms=query.data;
  const roles=machine.role_management;
  const canChangeRoles=roles?.supported===true && roles.delegated && roles.enabled;
  const kinds=programs.capabilities.filter((capability)=>capability.supported);
  const selected=kinds.some((capability)=>capability.kind===kind)?kind:kinds[0]?.kind ?? kind;
  const nativeAction=apply.variables?.action==='confirm_harness' || apply.variables?.action==='remove_harness';
  const nativeKept=!(harnessPolicyDelegated(facts)) || (nativeAction && apply.error instanceof ApiError && apply.error.is('policy.not_delegated'));
  const nativeManage=programs.can_manage && nativeRole && !nativeKept;
  const nativeLocal=programs.can_manage && nativeRole && nativeKept && facts.policy!==null;
  const nativeRows=[...harnesses.map((harness)=>({harness,confirmed:true})),...discovered.map((harness)=>({harness,confirmed:false}))];
  const refresh=programs.can_manage && (kinds.length>0 || nativeRelevant);
  return <div className="flex flex-col gap-4">
    {record(machine.facts) && record(machine.facts.runtime_platform) &&
      <Link className="w-fit text-sm text-accent-text hover:underline" to="/resources/machines/$id" params={{ id: machine.id }} search={{ tab: 'policy' }}>{t('machines.policy.reviewSystem')}</Link>}
    <div className="flex flex-wrap items-end gap-2">
      {programs.can_manage && kinds.length>0 && <Select label={t('machinePrograms.contract')} value={selected} onValueChange={setKind}
          options={kinds.map((capability)=>({value:capability.kind,name:t(`machinePrograms.kinds.${capability.kind}.name`),description:t(`machinePrograms.kinds.${capability.kind}.description`)}))}/>}
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        {programs.can_manage && kinds.length>0 && <Button onClick={()=>setAdding(true)}>{t('machinePrograms.add')}</Button>}
        {refresh && <Button pending={apply.isPending && apply.variables.action==='reload'} onClick={()=>apply.mutate({action:'reload'})}>{t(nativeReported || kinds.length===0?'machinePrograms.nativeRefresh':'machinePrograms.reload')}</Button>}
        <ProgramsHelp/>
      </div>
    </div>
    {nativeRelevant && <section aria-labelledby="machine-native-harnesses" className="flex flex-col gap-2">
      <h3 id="machine-native-harnesses" className="font-medium">{t('machinePrograms.nativeTitle')}</h3>
      {nativeRows.length===0?<StateLine compact message={nativeFetch?t(fetchPolicy===null?'machinePrograms.nativeFetch':'machinePrograms.nativeFetchBlocked',{key:fetchPolicy}):t(facts.searchedHarnessKinds===null?'machinePrograms.nativeNotReported':facts.searchedHarnessKinds.length===0?'machinePrograms.nativeNotSearched':'machinePrograms.nativeEmpty')}
        action={nativeFetch && fetchPolicy!==null && programs.can_manage?<Button size="sm" asChild><Link to="/resources/machines/$id" params={{id:machine.id}} search={{tab:'policy'}}>{t('machinePrograms.nativeFetchPolicy')}</Link></Button>:undefined}/>:
        <CompleteCollection label={t('machinePrograms.nativeTitle')} items={nativeRows} textOf={({harness})=>`${harnessTitle(harness.kind)} ${harness.kind} ${harness.version} ${harness.path}`} scroll>{(page)=><ul className="divide-y divide-border rounded-lg border border-border">{page.map(({harness,confirmed})=>{
          const approval=confirmed?programs.approvals.find((approval)=>approval.subject.kind === 'program' && approval.subject.harness_kind===harness.kind && approval.subject.path===harness.path):undefined;
          const id=`${harness.kind}:${harness.path}`;
          const shown=nativeLocal && local!==null && local.id===id && local.accept!==confirmed;
          return <li key={id} className="flex flex-wrap items-start gap-3 px-3 py-2">
          <div className="min-w-0 flex-1"><p className="flex min-h-(--control-sm) flex-wrap items-baseline gap-x-3 gap-y-1"><span className="font-medium">{harnessTitle(harness.kind)}</span>
            <span className="text-sm text-text-muted">{harness.version==='' || harness.version==='unknown'?t('machinePrograms.nativeUnknownVersion'):harness.version}</span>
            <span className="text-sm text-text-muted">{t(confirmed?'machinePrograms.nativeConfirmed':'machinePrograms.nativeFound')}</span></p><PathValue path={harness.path}/>
            {approval!==undefined && <ExecutionApproval approval={approval}/>}</div>
          {nativeManage && harness.path!=='' && <Button size="sm" disabled={apply.isPending} onClick={()=>confirmed?setUnconfirming(harness):apply.mutate({action:'confirm_harness',kind:harness.kind,path:harness.path})}>{t(confirmed?'machinePrograms.nativeRemove':'machinePrograms.nativeUse')}</Button>}
          {nativeLocal && harness.path!=='' && <Button size="sm" aria-expanded={shown} onClick={()=>setLocal(shown?null:{id,accept:!confirmed})}>{t(confirmed?'machinePrograms.nativeRemove':'machinePrograms.nativeUse')}</Button>}
          {shown && <div className="w-full min-w-0"><LocalHarnessCommand machine={machine} program={harness} accept={!confirmed}/></div>}
        </li>;})}</ul>}</CompleteCollection>}
      {programs.can_manage && !nativeRole && <p className="text-sm text-text-muted">{t('machinePrograms.nativeRoleNeeded')}</p>}
      {programs.can_manage && nativeKept && (!nativeFetch || nativeRows.length>0) && <p className="text-sm text-text-muted">{t(facts.policy===null?'machinePrograms.nativeCommandNotReported':'machinePrograms.nativeKept')}</p>}
    </section>}
    {kinds.length===0 && !nativeRelevant && <EmptyState compact title={t(programs.capabilities.length===0 && !machine.roles.includes('node')?'machinePrograms.notReported':'machinePrograms.roleNeeded')}
      action={programs.can_manage?canChangeRoles?<Button asChild><Link to="/resources/machines/$id" params={{id:machine.id}} search={{tab:'overview'}}>{t('machineRoles.edit')}</Link></Button>:<Button onClick={()=>setEnrolling(true)}>{t('machinePrograms.addConnector')}</Button>:undefined}/>}
    {programs.entries.length===0?(kinds.length===0?null:<EmptyState compact title={t('machinePrograms.empty')} />):
      <CompleteCollection label={t('machinePrograms.title')} items={programs.entries} textOf={(entry)=>`${machineProgramName(entry)} ${entry.name} ${entry.version??''} ${entry.command.join(' ')}`} scroll>{(page)=><ul aria-label={t('machinePrograms.title')} className="divide-y divide-border rounded-lg border border-border">
        {page.map((entry)=>{
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
      </ul>}</CompleteCollection>}
    {programs.entries.some((entry)=>entry.origin==='local') && <p className="text-sm text-text-muted">{t('machinePrograms.localEntry',{file:programs.config_file ?? t('machinePrograms.localConfiguration')})}</p>}
    {apply.isError && removing===null && unconfirming===null && <ProgramError error={apply.error}/>}
    {adding && <RegisterMachineProgram machineId={machine.id} kind={selected} onRegistered={()=>setAdding(false)} onClose={()=>setAdding(false)}/>}
    {enrolling && <AddMachineDialog onClose={()=>setEnrolling(false)}/>}
    {removing!==null && <ConfirmDialog open onOpenChange={(open)=>!open && setRemoving(null)} title={t('machinePrograms.removeTitle',{name:machineProgramName(removing)})}
      confirmLabel={t('machinePrograms.remove')} danger pending={apply.isPending} error={apply.isError && programFailure(apply.error)===undefined?apply.error:undefined}
      onConfirm={()=>apply.mutate({action:'remove',kind:removing.kind,name:removing.name})}>{t('machinePrograms.removeBody')}
        {apply.isError && programFailure(apply.error)!==undefined && <ProgramError error={apply.error}/>}</ConfirmDialog>}
    {unconfirming!==null && <ConfirmDialog open onOpenChange={(open)=>!open && !apply.isPending && setUnconfirming(null)} title={t('machinePrograms.nativeRemoveTitle',{name:harnessTitle(unconfirming.kind)})}
      confirmLabel={t('machinePrograms.nativeRemove')} danger pending={apply.isPending} error={apply.isError?apply.error:undefined}
      onConfirm={()=>apply.mutate({action:'remove_harness',kind:unconfirming.kind,path:unconfirming.path})}>{t('machinePrograms.nativeRemoveBody')}</ConfirmDialog>}
  </div>;
}
