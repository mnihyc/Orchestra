/** The roles offered by this actual program, changed in place only after the local handover. */
import { READ_NOW } from '@/api/freshness';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ApiError } from '@/api/client';
import { machineQuery, setMachineRoles } from '@/api/resources';
import type { Machine } from '@/generated/api-types';
import { CommandBox } from '@/features/resources/Facts';
import { CONNECTOR_ROLES,type ConnectorRole,roleName } from '@/features/resources/machines/EnrollmentRoles';
import { installationOf } from '@/features/resources/machines/Installation';
import { holdsMachineManagement, needsMachineManagementOverride, useMachineManagement } from '@/features/resources/machines/Management';
import { useHelpOpener } from '@/features/help/open';
import { Button } from '@/ui/Button';
import { CheckboxList } from '@/ui/Checkbox';
import { ErrorAlert } from '@/ui/ErrorState';
import { HelpHint } from '@/ui/HelpHint';

export function MachineRoles({machine}:{machine:Machine}) {
  const {t}=useTranslation();
  const openHelp=useHelpOpener();
  const [editing,setEditing]=useState(false);
  const [draft,setDraft]=useState<ConnectorRole[]>([]);
  const [revision,setRevision]=useState(machine.revision);
  const cache=useQueryClient();
  const management=useMachineManagement();
  const capability=machine.role_management;
  const canManage=holdsMachineManagement(machine) || needsMachineManagementOverride(machine);
  const node=machine.roles.includes('node');
  const installation=machine.installation ?? installationOf(machine.facts);
  const command=installation?.commands.roles ?? 'orch-connector roles';
  const localRoles=CONNECTOR_ROLES.filter((role)=>machine.roles.includes(role));
  const canEdit=canManage && machine.online && capability?.supported===true && capability.delegated && capability.enabled;
  // An old active-role snapshot in the command answer must never overwrite a newer live hello.
  // The acknowledgement supplies the pending selection; only the live machine row activates it.
  const apply=useMutation({mutationFn:(roles:readonly ConnectorRole[])=>setMachineRoles(machine.id,roles,revision),onSuccess:()=>setEditing(false)});
  const refresh=useMutation({mutationFn:()=>cache.query({...machineQuery(machine.id),...READ_NOW}),onSuccess:(current)=>{
    setDraft(CONNECTOR_ROLES.filter((role)=>current.roles.includes(role)));setRevision(current.revision);apply.reset();
  }});
  const waiting=apply.data?.reconnecting===true && !sameRoles(machine.roles,apply.data.requested_roles);
  const names=(roles:readonly string[])=>roles.length===0?t('machines.overview.noRoles'):roles.map((role)=>roleName(t,role)).join(' · ');
  const failure=apply.error instanceof ApiError?apply.error.faults().find((error)=>error.path==='roles')?.message:undefined;
  return <div className="flex min-w-0 flex-col items-start gap-2">
    {editing?<div role="group" aria-label={t('machineRoles.edit')} className="flex min-w-0 flex-col gap-2">
      <CheckboxList label={t('machines.overview.roles')} hideLabel value={draft} onValueChange={setDraft}
        disabled={apply.isPending} className="flex flex-wrap gap-x-4 gap-y-2"
        options={CONNECTOR_ROLES.map((role)=>({value:role,name:t(`machines.add.roleChoices.${role}.name`),description:t(`machines.add.roleChoices.${role}.description`)}))}/>
      <div className="flex items-center gap-2"><Button size="sm" variant="primary" pending={apply.isPending} disabled={draft.length===0 || sameRoles(machine.roles,draft)}
        onClick={()=>apply.mutate(draft)}>{t('common.save')}</Button><Button size="sm" disabled={apply.isPending} onClick={()=>{setEditing(false);apply.reset();}}>{t('common.cancel')}</Button></div>
    </div>:<div className="flex flex-wrap items-center gap-2">
      {canEdit && !waiting?<Button size="sm" aria-label={t('machineRoles.edit')} onClick={()=>management.run(machine,()=>{setDraft(localRoles);setRevision(machine.revision);apply.reset();setEditing(true);})}>{names(machine.roles)}</Button>:<span>{names(waiting && apply.data!==undefined?apply.data.requested_roles:machine.roles)}</span>}
      {waiting && <span role="status" className="text-sm text-text-muted">{t('machineRoles.reconnecting')}</span>}
      <HelpHint term={t('machines.overview.roles')} what={t('machineRoles.help')}
        more={{label:t('help.hint.more'),onOpen:()=>openHelp('resources','enroll-by-role')}} guide={{article:'resources',heading:'enroll-by-role'}}/>
    </div>}
    {!editing && !waiting && !canEdit && <div className="flex min-w-0 flex-wrap items-center gap-2 text-sm text-text-muted">
      <span>{t(node?'machineRoles.node':capability?.supported!==true?'machineRoles.update':!capability.delegated?'machineRoles.kept':!canManage?'machineRoles.manager':!machine.online?'machineRoles.offline':'machineRoles.off')}</span>
      {!node && capability?.delegated!==true && <CommandBox command={`${command} set ${localRoles.join(',') || 'environment,agent'}`} label={t('machines.add.copyRoles')}/>}
      {canManage && !node && capability?.delegated===true && !capability.enabled && <Link to="/resources/machines/$id" params={{id:machine.id}} search={{tab:'policy'}} className="text-link underline">{t('machinePrograms.openPolicy')}</Link>}
    </div>}
    {apply.isError && (failure===undefined?<ErrorAlert error={apply.error}/>:<p role="alert" className="text-sm text-danger-text">{failure}</p>)}
    {editing && apply.error instanceof ApiError && apply.error.problem.code==='conflict.stale_revision' && <Button size="sm" pending={refresh.isPending} onClick={()=>refresh.mutate()}>{t('common.refresh')}</Button>}
    {refresh.isError && <ErrorAlert error={refresh.error}/>}
    {management.dialog}
  </div>;
}

function sameRoles(left:readonly string[],right:readonly string[]):boolean {return left.length===right.length && left.every((role)=>right.includes(role));}
