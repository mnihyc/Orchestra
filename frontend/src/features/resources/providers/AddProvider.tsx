/** Add the provider interface: model access, credential, machine, and visible model/fact sources; examples fill those same fields. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Link, useNavigate } from '@tanstack/react-router';
import { lazy, Suspense, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { createProvider, createModel, createTemplate, templatesQuery, providerQuery, providerSetupQuery } from '@/api/resources';
import { ApiError } from '@/api/client';
import { TaskGuideLink } from '@/features/help/TaskGuideLink';
import { HomePicker, useHomeChoices } from '@/features/resources/HomePicker';
import { NewAccess } from '@/features/resources/NewAccess';
import { RegisterMachineProgram } from '@/features/resources/machines/Programs';
import { draftOf, endpointFault, EndpointsEditor, newEndpoint, upstreamOf, type EndpointDraft } from './Endpoints';
import { emptySourceDraft, SourceFields, sourceFault, type SourceFieldsProps } from './SourceFields';
import { defaultOffer, programOffers, selectedProgram, sourceChoiceSet } from './sourceOptions';
import { signInChoices } from './signInChoices';
import type { ArrangementType, CreateProvider, MachineProgram, NewProviderSources, NewResourceAccess, Provider, ProviderSetupProtocol, ProviderSetupSource, SourceDescriptor } from '@/generated/api-types';
import { useActKey } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { Dialog } from '@/ui/Dialog';
import { DialogBoundary } from '@/ui/DialogBoundary';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { toast } from '@/ui/Toast';

type Reach = 'address' | 'native';
type Credential = 'key' | 'none';
const AddAgentDialog=lazy(()=>import('../agents/AddAgent').then((module)=>({default:module.AddAgentDialog})));
export function AddProviderDialog({ onClose, initialRuntimeMachine, initialHome, initialPreset, onCreated }: {
  onClose: () => void; initialRuntimeMachine?: string; initialHome?: string; initialPreset?: string; onCreated?: (provider: Provider) => void;
}) {
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
  const actKey = useActKey();
  const cache = useQueryClient();
  const submitting = useRef(false);
  const [created, setCreated] = useState(false);
  const [partial, setPartial] = useState<Awaited<ReturnType<typeof createProvider>> | null>(null);
  const [serverFaults, setServerFaults] = useState<Record<string, string>>({});
  const [unplaced, setUnplaced] = useState(false);
  const [unplacedMessages,setUnplacedMessages]=useState<string[]>([]);
  const fields=useRef<HTMLDivElement>(null);
  const [step, setStep] = useState<1 | 2>(1);
  const [home, setHome] = useState(initialHome ?? '');
  const [reach, setReach] = useState<Reach>('address');
  const [credential, setCredential] = useState<Credential>('key');
  const [nativeAgent, setNativeAgent] = useState('');
  const [example, setExample] = useState('custom');
  const [model, setModel] = useState('');
  const [dimensions, setDimensions] = useState<number | undefined>();
  const [presetChosen,setPresetChosen] = useState(false);
  const templateAct = useActKey();
  const createdReply = useRef<Awaited<ReturnType<typeof createProvider>> | null>(null);
  const [endpoints, setEndpoints] = useState<EndpointDraft[]>(() => [newEndpoint(undefined)]);
  const [faults, setFaults] = useState<Record<string, string>>({});
  const [keyMachine, setKeyMachine] = useState('');
  const [runtimeMachine, setRuntimeMachine] = useState(initialRuntimeMachine ?? '');
  const [name, setName] = useState('');
  const [nameFault, setNameFault] = useState<string | undefined>();
  const [access, setAccess] = useState<NewResourceAccess>('defaults');
  const [ticked, setTicked] = useState<ArrangementType[] | null>(null);
  const [details, setDetails] = useState(emptySourceDraft);
  const [listing, setListing] = useState(emptySourceDraft);
  const [registering, setRegistering] = useState<string | null>(null);
  const [registered, setRegistered] = useState<ProviderSetupSource[]>([]);
  const [addingAgent,setAddingAgent]=useState(false);

  const setup = useQuery({ ...providerSetupQuery(home), enabled: home !== '' });
  const homes = useHomeChoices(home).choices;
  const homeName = homes.find((choice) => choice.id === home)?.label ?? '';
  const data = setup.data;
  const signIns=signInChoices(data,t,i18n.resolvedLanguage ?? i18n.language);
  const exampleOptions=[{value:'custom',label:t('providerCreation.custom')},
    ...(data?.presets ?? []).map((row)=>({value:`preset:${row.name}`,label:row.name})),
    ...signIns.map((row)=>({...row,value:row.disabled?row.value:`subscription:${row.value}`}))];
  const protocols: readonly ProviderSetupProtocol[] = useMemo(() => data?.protocols ?? [], [data]);
  const native = reach === 'native';
  const keeperLabel=t(native?'providerCreation.signInMachine':credential==='key'?'providerCreation.keyMachine':'providerCreation.machine');
  const subscription = native ? data?.subscriptions.find((row) => row.agent.id === nativeAgent) : undefined;
  const fixedKeeper = subscription?.agent.machine ?? null;
  const machines = data?.machines ?? [];
  const runtimeChoices = machines.filter((choice) => choice.directory_runtime && choice.roles.includes(native ? 'agent' : 'provider')
    && (!native || subscription !== undefined && choice.harnesses.includes(subscription.harness.kind)));
  const keeperChoices = machines.filter((choice) => native
    ? choice.roles.includes('agent') && subscription !== undefined && choice.harnesses.includes(subscription.harness.kind) && (fixedKeeper === null || choice.machine.id === fixedKeeper.id)
    : choice.roles.includes('provider'));
  const keepers = machines.length === 0 && !native ? data?.key_machines ?? [] : keeperChoices.map((choice) => choice.machine);
  // This default is made only by the creation form; the stored keeper and runtime stay independent.
  const keepingMachine = keyMachine !== '' ? keyMachine : runtimeMachine !== '' ? runtimeMachine : null;
  const connectionPolicy = machines.find((choice) => choice.machine.id === keepingMachine)?.upstreams;
  const deniedAddress = !native && keepingMachine !== null && connectionPolicy !== undefined && connectionPolicy !== null
    ? endpoints.find((row) => !connectionPolicy.some((url) => url === '*' || url === row.baseUrl.trim()))?.baseUrl : undefined;
  const policyError = native || keepingMachine === null ? undefined : connectionPolicy === undefined || connectionPolicy === null
    ? t('providerCreation.machinePolicyUnread') : deniedAddress === undefined ? undefined : t('providerCreation.machineAddressRefused', { address: deniedAddress });
  const offers = [...(data?.sources ?? []).filter((offer) => !registered.some((added) => added.name === offer.name && added.machine?.id === offer.machine?.id)), ...registered];
  const factsPrograms = programOffers(offers, 'facts', keepingMachine, null, keepingMachine === null);
  const modelsPrograms = programOffers(offers, 'models', keepingMachine, null, keepingMachine === null);
  const addresses = native ? [] : endpoints.map((row) => ({ base_url: row.baseUrl }));
  const factsDefault = defaultOffer(offers, 'facts', addresses, native);
  const modelsDefault = defaultOffer(offers, 'models', addresses, native);
  const factsProgram = selectedProgram(details, factsPrograms);
  const modelsProgram = selectedProgram(listing, modelsPrograms);
  const templates = [...new Set([factsProgram?.templateId, modelsProgram?.templateId].filter((id): id is string => typeof id === 'string'))];
  const templateConflict = templates.length > 1;
  const credentialKind: SourceFieldsProps['credentialKind'] = native ? 'native' : credential;
  const sourceContext = (programs: typeof factsPrograms) => ({ programs, compact:true, httpAllowed: data?.http_allowed ?? false, canSendCredential: !native && credential === 'key', credentialKind, savedAddress: null, authorizationSet: false, secretFieldsSet: [] });
  const sourcesInvalid = sourceFault(details, sourceContext(factsPrograms)) !== null || sourceFault(listing, sourceContext(modelsPrograms)) !== null || templateConflict;
  const machineArrangements = machines.find((choice) => choice.machine.id === keepingMachine)?.arrangements;
  const consents = (subscription?.arrangements ?? data?.api_arrangements ?? []).map((consent)=>({ ...consent,
    available:consent.available && (machineArrangements === undefined || machineArrangements === null || machineArrangements.includes(consent.type)) }));
  const arrangements: ArrangementType[] = (ticked ?? consents.filter((consent) => consent.allowed).map((consent) => consent.type))
    .filter((kind)=>consents.some((consent)=>consent.type===kind && consent.available));
  const registeredSource = (program: MachineProgram) => {
    const machine = machines.find((choice) => choice.machine.id === registering)?.machine;
    if (machine === undefined || program.kind !== 'provider_source' || !program.ready) return;
    // MachinePrograms returns the connector's validated descriptor for this declared contract.
    // The wire keeps it JSON because agent and provider descriptors share that registry.
    const descriptor = program.descriptor as SourceDescriptor;
    setRegistered((before) => [...before.filter((offer) => offer.name !== program.name || offer.machine?.id !== machine.id),
      { by: 'program', name: program.name, machine, template: null, command: program.command, default_for: [], descriptor }]);
    if (descriptor.facts !== null) setDetails({ ...emptySourceDraft(), by: 'program', program: program.name });
    if (descriptor.models !== null) setListing({ ...emptySourceDraft(), by: 'program', program: program.name });
    setRegistering(null);
  };

  const create = useMutation({
    mutationFn: async (given: CreateProvider) => {
      const preset = data?.presets.find((row) => example === `preset:${row.name}`);
      let body = given;
      const presetTemplate = preset?.template;
      if (presetTemplate !== undefined && presetTemplate !== null && given.runtime_machine_id === undefined) {
        const choices = await cache.query(templatesQuery('provider', home));
        const existing = choices.find((row) => row.name === presetTemplate.name && row.home?.id === home)
          ?? choices.find((row) => row.name === presetTemplate.name);
        const recipe = { ...presetTemplate, home_id: home };
        const template = existing ?? await createTemplate(recipe, templateAct(recipe));
        body = { ...given, runtime_config: { ...given.runtime_config as object, 'runtime.template_id': template.id } };
      }
      const reply = createdReply.current ?? await createProvider(body, actKey(body));
      createdReply.current = reply;
      const presetModel = preset?.model;
      if (presetModel !== undefined && presetModel !== null && model.trim() !== '') {
        await createModel(reply.id, { model_name: model.trim(),
          ...((presetModel.display_name === null || presetModel.display_name === undefined) ? {} : {display_name:presetModel.display_name}),
          ...((presetModel.groups === null || presetModel.groups === undefined) ? {} : {groups:presetModel.groups}),
          ...((presetModel.parameters === null || presetModel.parameters === undefined) ? {} : {parameters:presetModel.parameters}) });
        return {...await cache.query({...providerQuery(reply.id), staleTime: 0}), warning: reply.warning};
      }
      return reply;
    },
    onSuccess: (reply) => {
      const { warning, ...provider }=reply;
      setCreated(true);
      cache.setQueryData(providerQuery(provider.id).queryKey,provider);
      toast({ tone: 'success', title: t('providers.add.done', { name: provider.name }), ...(warning === undefined || warning === null ? {} : {description:warning}) });
      onClose();
      if (onCreated !== undefined) onCreated(provider);
      else void navigate({ to: '/resources/providers/$id', params: { id: provider.id }, search: { tab: 'key' } });
    },
    onError: (error) => {
      submitting.current=false;
      if (createdReply.current !== null) {
        setPartial(createdReply.current);
        return;
      }
      const placed:Record<string,string>={};const rowFaults:Record<string,string>={};const rest:string[]=[];let first=false;
      for(const fault of error instanceof ApiError ? error.faults() : []) {
        const endpoint=/^upstreams\[(\d+)\](?:\.(.*))?$/.exec(fault.path);
        const row=endpoint===null?undefined:endpoints[Number(endpoint[1])];
        if(row!==undefined){rowFaults[endpoint?.[2]===undefined?row.id:row.id+'.'+endpoint[2]]=fault.message;first=true;continue;}
        const field=fault.path==='home_id'?'home':fault.path==='agent_id'||fault.path==='kind'?'native'
          :fault.path==='name'?'name':fault.path==='keeper_machine_id'||fault.path==='key_machine_id'?'keeper'
            :fault.path.startsWith('runtime')?'runtime':/^(?:sources\.)?facts(?:\.|$)/.test(fault.path)?'facts'
              :/^(?:sources\.)?models(?:\.|$)/.test(fault.path)?'models':fault.path==='arrangements'?'arrangements':null;
        if(field===null){rest.push(fault.message);continue;}
        placed[field==='facts'||field==='models'?fault.path.replace(/^sources\./,''):field]=fault.message;if(field==='home'||field==='native')first=true;
      }
      setServerFaults(placed);setFaults(rowFaults);setUnplacedMessages(rest);setUnplaced(rest.length>0||Object.keys(placed).length+Object.keys(rowFaults).length===0);
      setStep(first?1:2);
    },
  });
  const pending = create.isPending;
  useEffect(()=>{if(!pending)fields.current?.querySelector<HTMLElement>('[aria-invalid="true"]')?.focus();},[pending,step,serverFaults,faults]);
  const sourceErrors=(element:'facts'|'models')=>Object.fromEntries<string>(Object.entries(serverFaults).flatMap(([key,message]):[string,string][]=>key.startsWith(element+'.')?[[key.slice(element.length+1),message]]:[]));
  const factsErrors=sourceErrors('facts'),modelsErrors=sourceErrors('models');
  const credentialRows = (rows: readonly EndpointDraft[], mode = credential): EndpointDraft[] => rows.map((row) => {
    const options = { ...row.options };
    if (mode === 'none') options.auth = 'none';
    else if (options.auth === 'none') delete options.auth;
    return { ...row, options };
  });
  const stopSendingCredential = () => {
    setDetails((before) => ({ ...before, sendCredential: false }));
    setListing((before) => ({ ...before, sendCredential: false }));
  };
  const changeCredential = (next: Credential) => {
    setCredential(next); setEndpoints((rows) => credentialRows(rows, next));
    if (next === 'none') stopSendingCredential();
  };
  const changeReach = (value: Reach) => {
    setReach(value); setTicked(null);
    if (value === 'native') stopSendingCredential();
  };
  const chooseNative = (id: string) => {
    const chosen=data?.subscriptions.find((row)=>row.agent.id===id);
    if(chosen===undefined)return;
    setNativeAgent(id); setTicked(null);
    setKeyMachine(chosen.agent.machine?.id ?? '');
  };
  const chooseExample = (value: string) => {
    if(value.startsWith('unavailable:') || value.startsWith('withheld:'))return;
    setExample(value); setTicked(null); setDetails(emptySourceDraft()); setListing(emptySourceDraft());
    if (value.startsWith('subscription:')) {
      setReach('native'); chooseNative(value.slice('subscription:'.length));
      return;
    }
    setReach('address'); setNativeAgent(''); setKeyMachine('');
    const preset = data?.presets.find((row) => value === `preset:${row.name}`);
    const rows = preset === undefined ? [newEndpoint(protocols[0])] : preset.upstreams.map((row) => draftOf(row, protocols.find((protocol) => protocol.name === row.protocol)));
    const mode = preset !== undefined && preset.upstreams.length > 0 && preset.upstreams.every((row) => typeof row.options === 'object' && row.options !== null && !Array.isArray(row.options) && 'auth' in row.options && row.options.auth === 'none') ? 'none' : 'key';
    setCredential(mode); setEndpoints(credentialRows(rows, mode));
    if (name === '' && preset !== undefined) setName(preset.name);
    setModel(preset?.model?.model_name ?? '');
    setDimensions(preset?.dimensions ?? undefined);
  };
  if (initialPreset !== undefined && data !== undefined && !presetChosen && data.presets.some((row)=>row.name===initialPreset)) {
    setPresetChosen(true);
    chooseExample(`preset:${initialPreset}`);
  }

  const next = () => {
    if (pending || created || submitting.current) return;
    if (data === undefined || native && subscription === undefined) return;
    const found: Record<string, string> = {};
    for (const row of native ? [] : endpoints) {
      const fault = endpointFault(row, data.http_allowed, t);
      if (fault !== null) found[row.protocol===''?row.id+'.protocol':row.id] = fault;
    }
    setFaults(found);
    if (Object.keys(found).length === 0) setStep(2);
  };
  const finish = () => {
    if (pending || created || submitting.current) return;
    const trimmed = name.trim();
    setNameFault(trimmed === '' ? t('providers.add.nameEmpty') : undefined);
    if (trimmed === '' || data === undefined || home === '' || sourcesInvalid || policyError !== undefined || arrangements.length === 0 || native && subscription === undefined) return;
    const built = (native ? [] : credentialRows(endpoints)).map((row) => {
      const one = upstreamOf(row, protocols.find((protocol) => protocol.name === row.protocol));
      // No credential is an explicit access choice even if an older setup reply omits auth's form field.
      if (credential === 'none') {
        const options = one.upstream.options;
        one.upstream.options = { ...(typeof options === 'object' && options !== null && !Array.isArray(options) ? options : {}), auth: 'none' };
      }
      return one;
    });
    const sources: NewProviderSources = {};
    if (details.by !== 'default') sources.facts = sourceChoiceSet(details, factsProgram?.fields ?? [], factsPrograms);
    if (listing.by !== 'default') sources.models = sourceChoiceSet(listing, modelsProgram?.fields ?? [], modelsPrograms);
    const selectedTemplate = runtimeMachine === '' ? templates[0] ?? subscription?.template.id : undefined;
    const common = {
      name: trimmed, home_id: home, access, arrangements,
      ...(runtimeMachine === '' ? {} : { runtime_machine_id: runtimeMachine }),
      ...(keyMachine === '' ? {} : { keeper_machine_id: keyMachine }),
      ...(Object.keys(sources).length === 0 ? {} : { sources }),
      ...(selectedTemplate === undefined ? {} : { runtime_config: { 'runtime.template_id': selectedTemplate } }),
    };
    submitting.current=true;setUnplaced(false);setServerFaults({});
    if (native && subscription !== undefined) create.mutate({ kind: 'subscription', ...common, agent_id: subscription.agent.id });
    else create.mutate({ kind: 'api', ...common, upstreams: built.map((row) => row.upstream) });
  };

  if (partial !== null) return <Dialog open title={t('providerCreation.partialTitle')} onOpenChange={(open) => { if (!open) onClose(); }} footer={<>
    <Button onClick={onClose}>{t('common.close')}</Button>
    <Button variant="primary" asChild><Link to="/resources/providers/$id" params={{ id: partial.id }} search={{ tab: 'models' }} onClick={onClose}>{t('providerCreation.openCreated')}</Link></Button>
  </>}>
    <div className="flex flex-col gap-3">
      <InlineAlert tone="info">{t('providerCreation.partial', { name: partial.name })}</InlineAlert>
      <ErrorAlert error={create.error} />
    </div>
  </Dialog>;

  return <Dialog open dismissible={!pending} title={t('providers.add.title')} onOpenChange={(open) => { if (!open && !pending) onClose(); }} footer={<>
    <TaskGuideLink guide="add-a-provider" heading={example === 'preset:Local embeddings' ? 'local-embeddings' : null} />
    <Button disabled={pending} onClick={step === 1 ? onClose : () => setStep(1)}>{t(step === 1 ? 'common.cancel' : 'common.back')}</Button>
    <Button variant="primary" pending={pending} blocked={created || data === undefined || home === '' || native && subscription === undefined || step === 2 && (sourcesInvalid || policyError !== undefined || arrangements.length === 0)} onClick={step === 1 ? next : finish}>
      {t(step === 1 ? 'common.next' : 'providerCreation.create')}
    </Button>
  </>}>
    <div ref={fields} className="flex min-w-0 flex-col gap-3 py-2" data-tutorial-target="provider-form">
      {step === 1 ? <>
        <Select label={t('providerCreation.examples')} value={example} onValueChange={chooseExample} options={exampleOptions}/>
        <div className="text-sm font-medium">{t('providerCreation.reach')}</div>
        <Select<Reach> hideLabel error={native?undefined:serverFaults.native} label={t('providerCreation.reach')} value={reach} onValueChange={changeReach} options={[
          { value: 'address', label: t('providerCreation.address') }, { value: 'native', label: t('providerCreation.native') },
        ]} />
        {setup.isPending && home !== '' ? <SkeletonRows rows={2} /> : setup.isError ? <ErrorState compact error={setup.error} onRetry={() => void setup.refetch()} /> : native ? <>
          <Select label={t('providerCreation.program')} error={serverFaults.native} value={nativeAgent} onValueChange={chooseNative} placeholder={t('providerCreation.chooseProgram')} options={signIns} />
          {data!==undefined && signIns.length===0 && <p className="text-sm text-text-muted">{t('providerCreation.noProgram')}</p>}
          <Button size="sm" className="self-start" onClick={()=>setAddingAgent(true)}>{t('resourceCreation.agents')}</Button>
        </> : <EndpointsEditor compact value={endpoints} onChange={(rows) => setEndpoints(credentialRows(rows))} protocols={protocols} httpAllowed={data?.http_allowed ?? false} errors={faults} disabled={pending} />}
        {!native && <><div className="text-sm font-medium">{t('providerCreation.credential')}</div>
        <Select<Credential> hideLabel label={t('providerCreation.credential')} value={credential} onValueChange={changeCredential} options={[
          { value: 'key', label: t('providerCreation.key') }, { value: 'none', label: t('providerCreation.none') },
        ]} /></>}
        {model !== '' && <Input label={t('admin.semantic.fields.model')} value={model} disabled={pending} onChange={(event) => setModel(event.target.value)} />}
        {dimensions !== undefined && <Input label={t('admin.semantic.fields.dimensions')} value={dimensions} readOnly />}
        <HomePicker compact value={home} onChange={setHome} error={serverFaults.home} />
      </> : <>
        <div className="text-sm font-medium">{keeperLabel}</div>
        <Select hideLabel label={keeperLabel} error={serverFaults.keeper ?? policyError} value={keyMachine === '' ? 'runtime' : keyMachine} onValueChange={(value) => {setKeyMachine(value === 'runtime' ? '' : value);setServerFaults({});}} disabled={pending || fixedKeeper !== null}
          options={[...(fixedKeeper === null ? [{ value: 'runtime', label:t('providerCreation.whereConnectionRuns') }] : []), ...keepers.map((machine) => ({ value: machine.id, label: machine.name }))]} />
        {fixedKeeper !== null && <p className="text-xs text-text-muted">{t('providers.add.nativeKeeperFixed', { machine: fixedKeeper.name })}</p>}
        <Select label={t('providerCreation.runtime')} error={serverFaults.runtime} value={runtimeMachine === '' ? 'unit' : runtimeMachine} onValueChange={(value) => {setRuntimeMachine(value === 'unit' ? '' : value);setServerFaults({});}} disabled={pending}
          options={[{ value: 'unit', label:t('providerCreation.newContainer') }, ...runtimeChoices.map((choice) => ({ value: choice.machine.id, label: choice.machine.name }))]} />
        <fieldset className="flex min-w-0 flex-col gap-3 border-t border-border pt-3">
          <legend className="text-sm font-medium">{t('providerCreation.sources')}</legend>
          <SourceFields element="facts" value={details} serverError={serverFaults.facts} serverErrors={factsErrors} onChange={(value)=>{setDetails(value);setServerFaults({});}} {...sourceContext(factsPrograms)} defaultTitle={native?t('providerCreation.signedInProgram'):factsDefault?.descriptor?.title ?? t('providerSources.nothing')} defaultSummary={factsDefault?.descriptor?.summary ?? t('providerSources.nothingAbout')} disabled={pending} />
          <SourceFields element="models" value={listing} serverError={serverFaults.models} serverErrors={modelsErrors} onChange={(value)=>{setListing(value);setServerFaults({});}} {...sourceContext(modelsPrograms)} defaultTitle={native?t('providerCreation.signedInProgram'):modelsDefault?.name==='endpoints'?t('providerCreation.endpointModels'):modelsDefault?.descriptor?.title ?? t('providerSources.nothing')} defaultSummary={modelsDefault?.descriptor?.summary ?? t('providerSources.defaultModels')} disabled={pending} />
          {templateConflict && <InlineAlert tone="warning">{t('providerSources.templateConflict')}</InlineAlert>}
          {keepingMachine !== null && <Button size="sm" className="self-start" onClick={() => setRegistering(keepingMachine)}>{t('providerCreation.registerSource')}</Button>}
        </fieldset>
        <Input label={t('providers.add.name')} value={name} error={nameFault ?? serverFaults.name} disabled={pending} autoComplete="off" onChange={(event) => {setName(event.target.value);setNameFault(undefined);setServerFaults({});}} />
        {arrangements.length === 0 && <InlineAlert tone="warning">{t('providerCreation.noAllowedArrangement')}</InlineAlert>}
        <details open={serverFaults.arrangements === undefined ? undefined : true}><summary className="cursor-pointer text-sm text-text-muted">{t('providers.add.arrangements')}</summary><div className="flex flex-col gap-2 pt-2">
          {serverFaults.arrangements !== undefined && <InlineAlert tone="warning">{serverFaults.arrangements}</InlineAlert>}
          <NewAccess value={access} onChange={setAccess} home={homeName} disabled={pending} />
          {consents.filter((consent) => consent.allowed).map((consent) => <Checkbox key={consent.type} label={t(`providerCreation.arrangements.${consent.type}`)} checked={arrangements.includes(consent.type)} disabled={pending || !consent.available}
            onCheckedChange={(checked) => setTicked(checked ? [...arrangements, consent.type] : arrangements.filter((value) => value !== consent.type))} />)}
        </div></details>
        <p className="text-xs text-text-muted">{t('providerCreation.later')}</p>
      </>}
      {create.error !== null && unplaced && <ErrorAlert error={create.error} message={unplacedMessages.length>0?unplacedMessages.join(' '):create.error instanceof ApiError ? create.error.message : undefined} />}
      {registering !== null && <RegisterMachineProgram machineId={registering} kind="provider_source" onRegistered={registeredSource} onClose={() => setRegistering(null)} />}
      {addingAgent && <DialogBoundary resetKey="provider-add-agent" title={t('agents.add.title')} onClose={()=>setAddingAgent(false)}><Suspense fallback={<SkeletonRows rows={2}/>}>
        <AddAgentDialog initialHome={home} onClose={()=>setAddingAgent(false)} onCreated={(agent)=>{
          setAddingAgent(false);setNativeAgent(agent.id);setKeyMachine(agent.machine?.id ?? '');setTicked(null);setExample('custom');void setup.refetch();
        }}/>
      </Suspense></DialogBoundary>}
    </div>
  </Dialog>;
}
