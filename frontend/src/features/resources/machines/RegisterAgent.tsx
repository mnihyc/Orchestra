/** The dialog that registers an agent profile for a harness found on a machine: which harness, what the profile is called and which org it belongs to.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { useEffect,useRef,useState } from 'react';
import { useTranslation } from 'react-i18next';

import { harnessKindsQuery, registerAgent,registerProvider,machineQuery,machineKey,agentKey,providerKey } from '@/api/resources';
import { readMachineFacts, registrableHarnesses,harnessPolicyDelegated } from '@/features/resources/machines/facts';
import { LocalHarnessCommand, useMachineRefresh } from '@/features/resources/machines/LocalHarnessCommand';
import { HomePicker } from '@/features/resources/HomePicker';
import { holds } from '@/lib/permissions';
import { useActKey, useForm } from '@/lib/forms';
import type { Machine,Agent,Provider } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert,ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { ResourceName } from '@/features/resource-facts/ResourceFacts';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { defaultProviderReader } from '@/api/defaultsOpening';
import { changeMachinePrograms } from '@/api/machinePrograms';
import { Input } from '@/ui/Input';
import { ChoiceLocator } from '@/ui/ChoiceLocator';
import { toast } from '@/ui/Toast';

export function RegisterAgentDialog({ machine, open, onClose }: { machine: Machine; open: boolean; onClose: () => void }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const kinds = useQuery({ ...harnessKindsQuery, enabled: open });
  const harnesses = registrableHarnesses(readMachineFacts(machine.facts));
  const [kind, setKind] = useState(harnesses[0]?.kind ?? '');
  const [home, setHome] = useState('');
  const key = useActKey();
  // Whether the form took the server's complaint to one of its fields; if not, it is said as a whole.
  const [placed, setPlaced] = useState(false);
  const titleOf = (value: string) => harnesses.find((entry)=>entry.kind===value)?.title ?? kinds.data?.find((entry) => entry.kind === value)?.title ?? value;
  const form = useForm({ name: '' }, { name: (value) => (value.trim() === '' && kind === '' ? t('machines.agent.nameEmpty') : null) });
  const proposed = kind === '' ? '' : t('machines.agent.proposed', { harness: titleOf(kind), machine: machine.name });

  const register = useMutation({
    mutationFn: (name: string) => {
      const body = { home_id: home, harness_kind: kind, name: name.trim() === '' ? proposed : name.trim() };
      return registerAgent(machine.id, body, key(body));
    },
    onSuccess: (agent) => {
      toast({ tone: 'success', title: t('machines.agent.done', { name: agent.name }) });
      onClose();
      void navigate({ to: '/resources/agents/$id', params: { id: agent.id } });
    },
    onError: (error) => setPlaced(form.refuse(error)),
  });

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && !register.isPending && onClose()}
      title={t('machines.agent.title', { machine: machine.name })}
      description={t('machines.agent.lead')}
      dismissible={!register.isPending}
      footer={
        <>
          <Button disabled={register.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" type="submit" form="register-agent" pending={register.isPending} blocked={kind === '' || home === ''}>
            {t('machines.agent.submit')}
          </Button>
        </>
      }
    >
      <form id="register-agent" className="flex flex-col gap-4 py-2" noValidate onSubmit={form.submit((values) => kind !== '' && home !== '' && register.mutate(values.name))}>
        <Input label={t('machines.agent.name')} optional placeholder={proposed} {...form.field('name')} autoComplete="off" />
        <ChoiceLocator
          label={t('machines.agent.harness')}
          hint={harnesses.length === 0 ? t('machines.agent.noHarness') : t('machines.agent.harnessHint')}
          value={kind}
          onValueChange={setKind}
          disabled={harnesses.length === 0 || register.isPending}
          placeholder={harnesses.length === 0 ? t('machines.agent.noHarness') : undefined}
          options={harnesses.map((harness) => ({
            value: harness.kind,
            label: t('machines.agent.harnessOption', { harness: titleOf(harness.kind), version: harness.version }),
            description: harness.path,
          }))}
        />
        <HomePicker value={home} onChange={setHome} />
        {register.error !== null && !placed && <ErrorAlert error={register.error} />}
      </form>
    </Dialog>
  );
}

/** One registration act uses the confirmed native path and keeps a committed profile across retries. */
export function RegisterInstalledSignInDialog({machineId,initialHome='',onClose,onDone}:{machineId:string;initialHome?:string;onClose:()=>void;onDone?:(agent:Agent,provider:Provider)=>void}) {
  const {t}=useTranslation();
  const reader=useStore(client.session,(state)=>`${state.userId??''}:${state.readerVersion??0}`);
  const machine=useQuery(machineQuery(machineId));
  if(machine.isSuccess)return <InstalledSignIn key={`${reader}:${machine.data.id}`} machine={machine.data} initialHome={initialHome} onClose={onClose} onDone={onDone}/>;
  return <Dialog open title={t('machineSignup.title')} onOpenChange={(open)=>!open && onClose()} footer={<Button onClick={onClose}>{t('common.close')}</Button>}>
    {machine.isPending?<SkeletonRows read={machine} rows={3}/>:<ErrorState compact error={machine.error} onRetry={()=>void machine.refetch()}/>}
  </Dialog>;
}

function InstalledSignIn({machine,initialHome,onClose,onDone}:{machine:Machine;initialHome:string;onClose:()=>void;onDone:((agent:Agent,provider:Provider)=>void)|undefined}) {
  const {t}=useTranslation();const navigate=useNavigate();const cache=useQueryClient();
  const kinds=useQuery(harnessKindsQuery);const facts=readMachineFacts(machine.facts);
  const allowed=new Set(facts.adapters.map((adapter)=>adapter.kind));
  const confirmed=registrableHarnesses(facts);
  const found=[...confirmed,...(facts.discoveredHarnesses??[]).filter((row)=>allowed.has(row.kind) && !confirmed.some((before)=>before.kind===row.kind && before.path===row.path))];
  const idOf=(row:{kind:string;path:string})=>JSON.stringify([row.kind,row.path]);
  const [selected,setSelected]=useState('');
  const [home,setHome]=useState(initialHome!==''?initialHome:machine.home?.id ?? '');
  const [name,setName]=useState('');const [created,setCreated]=useState<Agent|null>(null);
  const createdRef=useRef<Agent|null>(null);const mounted=useRef(true);
  useEffect(()=>{mounted.current=true;return()=>{mounted.current=false;};},[]);
  const [step,setStep]=useState<'profile'|'provider'|null>(null);
  const key=useActKey();
  const chosen=found.find((row)=>idOf(row)===selected) ?? (selected==='' && found.length===1?found[0]:undefined);
  const titleOf=(kind:string)=>facts.adapters.find((row)=>row.kind===kind)?.title ?? kinds.data?.find((row)=>row.kind===kind)?.title ?? kind;
  const proposed=chosen===undefined?'':t('machines.agent.proposed',{harness:titleOf(chosen.kind),machine:machine.name});
  const isConfirmed=chosen!==undefined && confirmed.some((row)=>row.kind===chosen.kind && row.path===chosen.path);
  const roleReason = !machine.roles.includes('provider') ? t('machineSignup.providerNeeded')
    : !machine.roles.includes('agent') ? t('machineSignup.agentNeeded') : null;
  const localSelection = machine.online && roleReason === null && created === null && chosen !== undefined
    && !isConfirmed && holds(machine.access, 'machine.manage') && !harnessPolicyDelegated(facts);
  const reason = !machine.online ? t('machines.registered.offline') : roleReason ?? (
    created !== null ? null
      : chosen === undefined ? t('machineSignup.chooseProgram')
      : !isConfirmed && !holds(machine.access, 'machine.manage') ? t('machineSignup.manageNeeded')
      : !isConfirmed && !harnessPolicyDelegated(facts) ? t('machinePrograms.nativeKept')
      : home === '' ? t('machineSignup.chooseHome') : null
  );
  const register=useMutation({mutationFn:async()=>{
    if(chosen===undefined || reason!==null)throw new Error('The registration selection is unavailable.');
    const reader=defaultProviderReader();const current=()=>{if(defaultProviderReader()!==reader)throw new DOMException('The reader changed.','AbortError');};
    const actualName=name.trim()!==''?name.trim():createdRef.current?.name ?? proposed;
    let agent=createdRef.current;
    if(agent===null) {
      setStep('profile');current();
      if(!isConfirmed) {
        const answer=await changeMachinePrograms(machine.id,{action:'confirm_harness',kind:chosen.kind,path:chosen.path});current();
        if(answer.machine_view?.id===machine.id)cache.setQueryData(machineKey(machine.id),answer.machine_view);
      }
      current();
      const body={home_id:home,harness_kind:chosen.kind,name:actualName,...(chosen.path===''?{}:{profile:{attach:{binary:chosen.path}}})};
      agent=await registerAgent(machine.id,body,key({stage:'profile',machine:machine.id,...body}));
      createdRef.current=agent;setCreated(agent);
    }
    current();setStep('provider');
    const body={kind:'subscription' as const,home_id:home,agent_id:agent.id,name:actualName};
    const provider=await registerProvider(machine.id,body,key({stage:'provider',machine:machine.id,...body}));current();
    return {agent,provider,reader};
  },onSuccess:({agent,provider,reader})=>{
    if(!mounted.current || defaultProviderReader()!==reader)return;
    cache.setQueryData(agentKey(agent.id),agent);cache.setQueryData(providerKey(provider.id),provider);
    void cache.invalidateQueries({queryKey:machineKey(machine.id)});
    if(onDone!==undefined)onDone(agent,provider);
    else {onClose();void navigate({to:'/resources/providers/$id',params:{id:provider.id}});}
  },onSettled:()=>setStep(null)});
  const refresh=useMachineRefresh(machine.id);
  const locked=register.isPending || created!==null;
  return <Dialog open title={t('machineSignup.title')} dismissible={!register.isPending} onOpenChange={(open)=>!open && !register.isPending && onClose()}
    footer={<><span className="min-w-0 text-sm text-text-muted">{reason}</span><Button disabled={register.isPending} onClick={onClose}>{t('common.cancel')}</Button>
      <Button variant="primary" blocked={reason!==null} pending={register.isPending} onClick={()=>register.mutate()}>{register.isPending?t(step==='provider'?'machineSignup.makingProvider':'machineSignup.makingProfile'):register.isError?t('common.tryAgain'):t('machineSignup.register')}</Button></>}>
    <div className="flex min-w-0 flex-col gap-3">
      {created===null?<><Input label={t('machines.agent.name')} optional placeholder={proposed} value={name} disabled={locked} onChange={(event)=>setName(event.target.value)}/>
        <ChoiceLocator label={t('machines.agent.harness')} value={chosen===undefined?'':idOf(chosen)} disabled={locked} onValueChange={setSelected}
          options={found.map((row)=>({value:idOf(row),label:t('machines.agent.harnessOption',{harness:titleOf(row.kind),version:row.version}),description:row.path}))}/>
        <HomePicker value={home} onChange={setHome} disabled={locked}/>
      </>:<div className="flex min-w-0 flex-wrap items-center gap-2"><ResourceName subject={{kind:'agent',agent:created}} hint={false}/><span className="text-sm text-text-muted">{t('machineSignup.profileReady')}</span></div>}
      {localSelection && chosen!==undefined && <LocalHarnessCommand machine={machine} program={chosen} accept/>}
      {found.length===0 && <StateLine compact message={t('machineSignup.notFound')} action={<Button size="sm" pending={refresh.isPending} onClick={()=>refresh.mutate()}>{t('machinePrograms.nativeRefresh')}</Button>}/>}
      {register.error!==null && <ErrorAlert error={register.error}/>}
      {refresh.error!==null && <ErrorAlert error={refresh.error}/>}
    </div>
  </Dialog>;
}
