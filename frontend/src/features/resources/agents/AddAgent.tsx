/** A profile uses an available adapter or the same machine program registry offered on Machines. */
import { useMutation, useQuery } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { createAgent, harnessKindsQuery, machineQuery, machinesQuery, registerAgent } from '@/api/resources';
import type { Agent, MachineProgram } from '@/generated/api-types';
import { HomePicker } from '@/features/resources/HomePicker';
import { readMachineFacts, registrableHarnesses } from '@/features/resources/machines/facts';
import { RegisterMachineProgram, machineProgramName } from '@/features/resources/machines/Programs';
import { AgentProgramHelp } from './AgentProgramHelp';
import { useActKey, useForm } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { ChoiceLocator } from '@/ui/ChoiceLocator';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { toast } from '@/ui/Toast';

type Mode = 'available' | 'machine';
export function AddAgentDialog({ onClose, onCreated, initialHome = '' }: { onClose: () => void; onCreated?: (agent: Agent) => void; initialHome?:string }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const kinds = useQuery(harnessKindsQuery);
  const [mode,setMode]=useState<Mode>('available');
  const [machineId,setMachineId]=useState('');
  const [registering,setRegistering]=useState(false);
  const [registered,setRegistered]=useState<{machine:string;kind:string;title:string}[]>([]);
  const machines=useQuery({...machinesQuery,enabled:mode==='machine'});
  const machine=useQuery({...machineQuery(machineId),enabled:mode==='machine' && machineId!==''});
  const key = useActKey();
  const [kind, setKind] = useState('');
  const [home, setHome] = useState(initialHome);
  const [placed, setPlaced] = useState(false);
  const form = useForm({ name: '' }, { name: (value) => (value.trim() === '' ? t('agents.add.nameEmpty') : null) });
  const installed=registrableHarnesses(readMachineFacts(machine.data?.facts));
  const choices=mode==='available'?(kinds.data ?? []).map((row)=>({value:row.kind,label:row.title}))
    : [...new Map([
      ...installed.map((row)=>[row.kind,{value:row.kind,label:row.title ?? kinds.data?.find((entry)=>entry.kind===row.kind)?.title ?? row.kind}] as const),
      ...registered.filter((row)=>row.machine===machineId).map((row)=>[row.kind,{value:row.kind,label:row.title}] as const),
    ]).values()];
  const selected=choices.some((choice)=>choice.value===kind);
  const machineOptions=machines.data?.filter((row)=>row.roles.includes('agent')).map((row)=>({value:row.id,label:row.name,disabled:!row.online,description:row.online?undefined:t('agentCreation.offline')})) ?? [];
  const made = useMutation({
    mutationFn: (name: string) => {
      const body = { name: name.trim(), harness_kind: kind };
      return mode==='available' ? createAgent(home, body, key({ ...body, home }))
        : registerAgent(machineId,{...body,home_id:home},key({...body,home,machineId}));
    },
    onSuccess: (agent) => {
      toast({ tone: 'success', title: t('agents.add.done', { name: agent.name }) });
      onClose();
      if (onCreated !== undefined) onCreated(agent);
      else void navigate({ to: '/resources/agents/$id', params: { id: agent.id } });
    },
    onError: (error) => setPlaced(form.refuse(error)),
  });
  const useProgram=(program:MachineProgram)=>{
    const descriptor=program.descriptor;
    if(!program.ready || program.kind!=='agent' || descriptor===null || typeof descriptor!=='object' || Array.isArray(descriptor)
      || !('kind' in descriptor) || typeof descriptor.kind!=='string' || descriptor.kind==='')return;
    const reportedKind=descriptor.kind;
    setRegistered((before)=>[...before.filter((row)=>row.machine!==machineId || row.kind!==reportedKind),
      {machine:machineId,kind:reportedKind,title:machineProgramName(program)}]);
    setKind(reportedKind);setRegistering(false);
  };
  const blocked=!selected || home==='' || mode==='machine' && (machineId==='' || machine.data?.online!==true);
  return <Dialog open onOpenChange={(next)=>!next && !made.isPending && onClose()} title={t('agents.add.title')}
    dismissible={!made.isPending} footer={<>
      <Button disabled={made.isPending} onClick={onClose}>{t('common.cancel')}</Button>
      <Button variant="primary" type="submit" form="add-agent" pending={made.isPending} blocked={blocked}>{t('agents.add.submit')}</Button>
    </>}>
    <form id="add-agent" className="flex min-w-0 flex-col gap-3 py-2" noValidate onSubmit={form.submit((values)=>!blocked && made.mutate(values.name))}>
      <div className="flex items-center gap-1 text-sm font-medium"><span>{t('agentCreation.mode')}</span><AgentProgramHelp/></div>
      <Select<Mode> hideLabel label={t('agentCreation.mode')} value={mode} onValueChange={(next)=>{setMode(next);setKind('');}}
        options={[{value:'available',label:t('agentCreation.available')},{value:'machine',label:t('agentCreation.onMachine')}]}/>
      {mode==='machine' && <>
        {machines.isPending?<SkeletonRows rows={1}/>:machines.isError?<ErrorState compact error={machines.error} onRetry={()=>void machines.refetch()}/>:
          <ChoiceLocator label={t('agentCreation.machine')} value={machineId} onValueChange={(id)=>{setMachineId(id);setKind('');}}
            options={machineOptions}/>}
        {machineId!=='' && <Button size="sm" className="self-start" onClick={()=>setRegistering(true)}>{t('agentCreation.register')}</Button>}
        {machineId!=='' && machine.isError && <ErrorState compact error={machine.error} onRetry={()=>void machine.refetch()}/>}
      </>}
      {kinds.isPending || mode==='machine' && machineId!=='' && machine.isPending ? <SkeletonRows rows={1}/>
        : kinds.isError?<ErrorState compact error={kinds.error} onRetry={()=>void kinds.refetch()}/>
          : <ChoiceLocator label={t('agents.add.harness')} value={selected?kind:''} placeholder={t('agents.add.harnessChoose')} onValueChange={setKind} options={choices}/>}
      <Input label={t('agents.add.name')} {...form.field('name')} autoComplete="off"/>
      <HomePicker value={home} onChange={setHome}/>
      {made.error!==null && !placed && <ErrorAlert error={made.error}/>}
    </form>
    {registering && machineId!=='' && <RegisterMachineProgram machineId={machineId} kind="agent" onRegistered={useProgram} onClose={()=>setRegistering(false)}/>}
  </Dialog>;
}
