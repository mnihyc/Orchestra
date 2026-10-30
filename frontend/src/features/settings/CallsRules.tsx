/** Compact ordered recovery rows inside the existing inherited setting editor. */
import { useQuery } from '@tanstack/react-query';
import { Ellipsis, Plus } from 'lucide-react';
import { useId, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { agentsQuery, providerModelsQuery, providerQuery } from '@/api/resources';
import type { CallRecovery } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { formatSpan } from '@/lib/time';
import { ProviderDefaultChoice } from './ProviderChoice';
import type { ValueControlProps } from './ValueControl';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { Input } from '@/ui/Input';
import { IconButton } from '@/ui/IconButton';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { Select } from '@/ui/Select';
import { Table } from '@/ui/Table';
import { Textarea } from '@/ui/Textarea';
import { ErrorAlert } from '@/ui/ErrorState';
import { Combobox } from '@/ui/Combobox';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';
import { profilesFor } from '@/features/change/RunsOn';
import { holds } from '@/lib/permissions';
import { CallModel } from '@/features/calls/Model';
import { CallHelp } from '@/features/calls/Help';
import { resourceChoice } from '@/features/resource-facts/ResourceFacts';
import { modelResource } from '@/features/resource-facts/facts';

type Step = NonNullable<CallRecovery['step']>;
interface Rule { id: string; when: { kind?: string; reason?: string; purpose?: 'agent' | 'summary' | 'embedding' | 'review'; provider_id?: string; model?: string; status?: number; contains?: string; regex?: string }; steps: Step[] }
const KINDS = ['any', 'provider_busy', 'provider_unreachable', 'model_refused', 'model_unavailable', 'provider_exhausted', 'provider_unauthorized', 'context_overflow', 'harness_error'] as const;
const record = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === 'object' && !Array.isArray(value);
function rulesOf(value: unknown): Rule[] { return Array.isArray(value) ? value.filter((row): row is Rule => record(row) && typeof row.id === 'string' && record(row.when) && Array.isArray(row.steps)) : []; }

function defaultStep(type: Step['type']): Step {
  const rule = SETTINGS_REGISTRY.find((setting) => setting.key === 'calls.rules');
  const held = rulesOf(rule?.default).flatMap((rule) => rule.steps).find((step) => step.type === type);
  return held === undefined ? type === 'failover' ? { type, candidate: { provider_id: '', model: '', params: {} } } : { type: 'wait', until: 'person' } : structuredClone(held);
}

export function ruleSummary(rule: Rule, t: TFunction, locale: string): string {
  const when = t(`calls.kinds.${rule.when.reason === 'access' ? 'access' : rule.when.kind ?? 'any'}`, { defaultValue: rule.when.kind ?? t('calls.kinds.any') });
  return [when, ...rule.steps.map((step) => stepSummary(step, t, locale))].join(' → ');
}

function stepSummary(step: Step, t: TFunction, locale: string): string {
  return step.type === 'retry' ? t('calls.ruleRetry', { delay: formatSpan(step.delay_secs * 1_000, { locale }, 'short'), times: formatNumber(step.times, locale) })
    : step.type === 'continue' ? t('calls.ruleContinue', { times: formatNumber(step.times, locale) })
      : step.type === 'failover' ? step.candidate.model : t(step.until === 'reset' ? 'calls.waitReset' : 'calls.waitPerson');
}

export function CallsSummary({ value }: { value: unknown }) {
  const { t } = useTranslation(); const format = useFormatContext();
  return <span className="inline-flex min-w-0 flex-col gap-1">{rulesOf(value).map((rule) => <span key={rule.id} className="flex min-w-0 flex-wrap items-center gap-1 text-sm" title={ruleSummary(rule, t, format.locale)}><span>{ruleSummary({ ...rule, steps: [] }, t, format.locale)}</span><RuleSteps rule={rule} /></span>)}</span>;
}

function RuleSteps({ rule }: { rule: Rule }) {
  const { t } = useTranslation(); const format = useFormatContext();
  return rule.steps.map((step, index) => <span key={index} className="inline-flex min-w-0 max-w-full items-center gap-1"><span aria-hidden>→</span>{step.type === 'failover' ? <CallModel providerId={step.candidate.provider_id} model={step.candidate.model} /> : <span>{stepSummary(step, t, format.locale)}</span>}</span>);
}

function CandidateEditor({ step, onChange }: { step: Extract<Step, { type: 'failover' }>; onChange: (step: Step) => void }) {
  const { t } = useTranslation();
  const candidate = step.candidate;
  const [modelsOpen,setModelsOpen]=useState(false);
  const models = useQuery({ ...providerModelsQuery(candidate.provider_id), enabled: candidate.provider_id !== '' && modelsOpen, retry: false });
  const provider=useQuery({...providerQuery(candidate.provider_id),enabled:candidate.provider_id!=='',retry:false});
  const agents=useQuery({...agentsQuery(),enabled:candidate.provider_id!=='',retry:false});
  const profiles=profilesFor(provider.data,agents.data??[]);
  const parameterText=(value:unknown)=>typeof value==='string'?value:JSON.stringify(value===undefined?{}:value,null,2);
  const [params, setParams] = useState(()=>parameterText(candidate.params));
  const [seenParams,setSeenParams]=useState<unknown>(()=>candidate.params);
  const [invalid, setInvalid] = useState(!record(candidate.params===undefined?{}:candidate.params));
  if(seenParams!==candidate.params) {
    setSeenParams(candidate.params);setParams(parameterText(candidate.params));setInvalid(!record(candidate.params===undefined?{}:candidate.params));
  }
  const failure=models.error??provider.error??agents.error;
  const retry=models.error!==null?models.refetch:provider.error!==null?provider.refetch:agents.refetch;
  return <div className="flex min-w-0 flex-col gap-2">
    <ProviderDefaultChoice showLabel shape={{ type: 'text', maxLength: null, pattern: null }} value={candidate.provider_id} label={t('calls.provider')} onChange={(value) => onChange({ ...step, candidate: { ...candidate, provider_id: typeof value === 'string' ? value : '', model: '' } })} />
    <Select label={t('calls.profile')} value={candidate.agent_id??'\u0000current'} valueLabel={typeof candidate.agent_id==='string' && !profiles.some((agent)=>agent.id===candidate.agent_id)?t(agents.isFetching?'common.loading':'calls.managed'):undefined} options={[{value:'\u0000current',label:t('calls.keepProfile')},...profiles.map((agent)=>({value:agent.id,label:agent.name,...resourceChoice({kind:'agent',agent,provider:provider.data}),disabled:!holds(agent.access,'agent.use'),...(!holds(agent.access,'agent.use')?{description:t('calls.profileUseRequired')}:{})}))]} onValueChange={(agent_id)=>{const {agent_id:_old,...rest}=candidate;onChange({...step,candidate:{...rest,...(agent_id==='\u0000current'?{}:{agent_id})}});}} />
    <Combobox label={t('calls.model')} onOpenChange={setModelsOpen} query={candidate.model} onQueryChange={(model) => onChange({ ...step, candidate: { ...candidate, model } })} loading={models.isFetching} groups={[{ id: 'models', label: t('calls.model'), options: (models.data ?? []).filter((model) => model.enabled).map((model) => ({ value: model.model_name, label: model.display_name || model.model_name, text: model.display_name || model.model_name, ...resourceChoice(modelResource(model,candidate.provider_id,{usage:provider.data?.choice_usage})) })) }]} onSelect={({ value: model }) => onChange({ ...step, candidate: { ...candidate, model } })} />
    <Textarea label={t('calls.parameters')} help={<CallHelp section="candidates" term={t('calls.parameters')} />} value={params} error={invalid ? t('calls.invalidParameters') : undefined} onChange={(event) => { const text = event.target.value; setParams(text); try { const value: unknown = JSON.parse(text); if (!record(value)) throw new Error(); setInvalid(false); setSeenParams(value); onChange({ ...step, candidate: { ...candidate, params: value } }); } catch { setInvalid(true); setSeenParams(text); onChange({ ...step, candidate: { ...candidate, params: text } }); } }} />
    {failure!==null && <ErrorAlert error={failure} action={<Button size="sm" onClick={()=>void retry()}>{t('common.tryAgain')}</Button>} />}
    <Input type="number" min={1} label={t('calls.dimensions')} value={candidate.dimensions ?? ''} onChange={(event) => { const dimensions = event.target.value === '' ? undefined : Number(event.target.value); const { dimensions: _old, ...rest } = candidate; onChange({ ...step, candidate: { ...rest, ...(dimensions === undefined ? {} : { dimensions }) } }); }} />
  </div>;
}

function StepEditor({ step, onChange }: { step: Step; onChange: (step: Step) => void }) {
  const { t } = useTranslation();
  return <div className="flex min-w-0 flex-col gap-2">
    <Select size="sm" label={t('calls.action')} value={step.type} options={(['retry', 'continue', 'failover', 'wait'] as const).map((value) => ({ value, label: t(`calls.actions.${value}`) }))} onValueChange={(value) => onChange(defaultStep(value))} />
    {(step.type === 'retry' || step.type === 'continue') && <Input label={t('calls.times')} type="number" min={1} value={step.times} onChange={(event) => onChange({ ...step, times: Number(event.target.value) })} />}
    {step.type === 'retry' && <div className="grid min-w-0 grid-cols-2 gap-2"><Input label={t('calls.delay')} type="number" min={0} value={step.delay_secs} onChange={(event) => onChange({ ...step, delay_secs: Number(event.target.value) })} /><Input label={t('calls.backoff')} type="number" min={1} value={step.backoff} onChange={(event) => onChange({ ...step, backoff: Number(event.target.value) })} /></div>}
    {step.type === 'wait' && <Select label={t('calls.until')} value={step.until} options={(['person', 'reset'] as const).map((value) => ({ value, label: t(value === 'person' ? 'calls.waitPerson' : 'calls.waitReset') }))} onValueChange={(until) => onChange({ ...step, until })} />}
    {step.type === 'failover' && <CandidateEditor key={step.candidate.provider_id} step={step} onChange={onChange} />}
  </div>;
}

interface RuleRow {rule:Rule;index:number}
interface StepRow {step:Step;index:number;key:string}
interface Draft {index:number;rule:Rule;keys:string[];next:number}

export function CallsRules({ value, onChange, onEdited, disabled, error }: ValueControlProps) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const identity=useId();
  const rules = rulesOf(value);
  const [editing, setEditing] = useState<Draft | null>(null);
  const [open, setOpen] = useState(false);
  const edit = (index: number, rule: Rule) => {
    setEditing((held) => held?.index === index ? held : { index, rule: structuredClone(rule),keys:rule.steps.map((_,at)=>`${identity}:${index}:${at}`),next:rule.steps.length });
    setOpen(true);
  };
  const invalid = editing === null || editing.rule.id.trim() === '' || rules.some((rule, index) => index !== editing.index && rule.id === editing.rule.id)
    || editing.rule.steps.some((step) => step.type === 'failover' ? step.candidate.provider_id === '' || step.candidate.model.trim() === '' || !record(step.candidate.params===undefined?{}:step.candidate.params) || step.candidate.dimensions!==undefined && step.candidate.dimensions!==null && (!Number.isInteger(step.candidate.dimensions) || step.candidate.dimensions<=0)
      : step.type === 'retry' ? !Number.isInteger(step.times) || !Number.isInteger(step.delay_secs) || !Number.isInteger(step.backoff) || step.times <= 0 || step.delay_secs < 0 || step.backoff <= 0 : step.type === 'continue' && (!Number.isInteger(step.times) || step.times <= 0));
  const update = (rule: Rule) => { if (editing !== null) { onEdited?.(); setEditing({ ...editing, rule }); } };
  const move = (index: number, delta: number) => {
    const next = [...rules], other = next[index + delta], held = next[index];
    if (held === undefined || other === undefined) return;
    next[index] = other; next[index + delta] = held; onChange(next);
  };
  const changeSteps=(steps:Step[],keys:string[],next:number)=>{
    if(editing===null)return;
    onEdited?.();setEditing({...editing,rule:{...editing.rule,steps},keys,next});
  };
  const moveStep=(index:number,delta:number)=>{
    if(editing===null)return;
    const steps=[...editing.rule.steps],keys=[...editing.keys];
    const held=steps[index],other=steps[index+delta],key=keys[index],otherKey=keys[index+delta];
    if(held===undefined||other===undefined||key===undefined||otherKey===undefined)return;
    steps[index]=other;steps[index+delta]=held;keys[index]=otherKey;keys[index+delta]=key;
    changeSteps(steps,keys,editing.next);
  };
  const ruleLabel=({rule}:RuleRow)=>t('calls.menuOf',{name:rule.id});
  const ruleEntries=({rule,index}:RuleRow):readonly MenuEntry[]=>[
    {id:'edit',label:t('calls.edit'),disabled:disabled?t('common.loading'):false,onSelect:()=>edit(index,rule)},
    {id:'up',label:t('calls.moveUp'),disabled:disabled?t('common.loading'):index===0?t('calls.firstRule'):false,onSelect:()=>move(index,-1)},
    {id:'down',label:t('calls.moveDown'),disabled:disabled?t('common.loading'):index===rules.length-1?t('calls.lastRule'):false,onSelect:()=>move(index,1)},
    {id:'remove',label:t('calls.remove'),danger:true,disabled:disabled?t('common.loading'):false,onSelect:()=>onChange(rules.filter((_,at)=>at!==index))},
  ];
  const stepLabel=({index}:StepRow)=>t('calls.menuOf',{name:t('calls.stepNumber',{step:formatNumber(index+1,format.locale)})});
  const stepEntries=({index}:StepRow):readonly MenuEntry[]=>{
    const held=editing;
    if(held===null)return [];
    return [
      {id:'up',label:t('calls.moveUp'),disabled:index===0?t('calls.firstStep'):false,onSelect:()=>moveStep(index,-1)},
      {id:'down',label:t('calls.moveDown'),disabled:index===held.rule.steps.length-1?t('calls.lastStep'):false,onSelect:()=>moveStep(index,1)},
      {id:'remove',label:t('calls.remove'),disabled:held.rule.steps.length===1?t('calls.oneStep'):false,onSelect:()=>changeSteps(held.rule.steps.filter((_,at)=>at!==index),held.keys.filter((_,at)=>at!==index),held.next)},
    ];
  };
  return <div className="flex min-w-0 max-w-full flex-col gap-2">
    <Table boundedIdentity label={t('calls.rules')} rows={rules.map((rule,index)=>({rule,index}))} rowKey={({rule})=>rule.id} rowMenu={{label:ruleLabel,entries:ruleEntries}} columns={[
      {id:'name',header:t('calls.rules'),hideHeader:true,cell:({rule,index})=><span className="flex min-w-0 flex-wrap items-center gap-1"><Button variant="ghost" size="sm" className="max-w-full justify-start" aria-label={ruleSummary(rule,t,format.locale)} disabled={disabled} onClick={()=>edit(index,rule)}><span className="truncate">{ruleSummary({...rule,steps:[]},t,format.locale)}</span></Button><RuleSteps rule={rule} /></span>},
      {id:'actions',header:t('calls.action'),hideHeader:true,align:'end',cell:(row)=><Menu label={ruleLabel(row)} entries={ruleEntries(row)} trigger={<IconButton size="sm" label={ruleLabel(row)} icon={<Ellipsis aria-hidden />} />} />},
    ]} />
    <Button size="sm" variant="ghost" disabled={disabled} icon={<Plus aria-hidden />} onClick={()=>edit(rules.length,{id:t('calls.newRule',{ordinal:formatNumber(rules.length+1,format.locale)}),when:{},steps:[{type:'wait',until:'person'}]})}>{t('calls.addRule')}</Button>
    {error!==undefined && <ErrorAlert error={new Error(error)} />}
    {editing!==null && <Dialog open={open} title={t('calls.editRule')} size="sm" onOpenChange={setOpen} footer={<><Button size="sm" onClick={()=>{setEditing(null);setOpen(false);}}>{t('common.cancel')}</Button><Button size="sm" variant="primary" disabled={invalid} onClick={()=>{const next=[...rules];next[editing.index]=editing.rule;onChange(next);setEditing(null);setOpen(false);}}>{t('common.save')}</Button></>}>
      <div className="flex min-w-0 flex-col gap-3">
        <Input label={t('calls.name')} help={<CallHelp section="rules" />} value={editing.rule.id} onChange={(event)=>update({...editing.rule,id:event.target.value})} />
        <Select hideLabel={false} label={t('calls.reason')} value={editing.rule.when.reason??'any'} options={(['any','cyber','safety','access','other'] as const).map((reason)=>({value:reason,label:t(reason==='any'?'calls.anyReason':`calls.reasons.${reason}`)}))} onValueChange={(reason)=>{const {reason:_old,...when}=editing.rule.when;update({...editing.rule,when:{...when,...(reason==='any'?{}:{reason})}});}} />
        <ProviderDefaultChoice shape={{type:'text',maxLength:null,pattern:null}} showLabel label={t('calls.provider')} placeholder={t('calls.anyProvider')} value={editing.rule.when.provider_id??''} onChange={(value)=>{const {provider_id:_old,...when}=editing.rule.when;update({...editing.rule,when:{...when,...(typeof value==='string'&&value!==''?{provider_id:value}:{})}});}} />
        {editing.rule.when.provider_id!==undefined && <Button size="sm" variant="ghost" onClick={()=>{const {provider_id:_old,...when}=editing.rule.when;update({...editing.rule,when});}}>{t('calls.anyProvider')}</Button>}
        <Select label={t('calls.when')} value={editing.rule.when.kind??'any'} options={KINDS.map((kind)=>({value:kind,label:t(`calls.kinds.${kind}`)}))} onValueChange={(kind)=>update({...editing.rule,when:{...editing.rule.when,kind}})} />
        <Select label={t('calls.purpose')} value={editing.rule.when.purpose??'any'} options={(['any','agent','summary','embedding','review'] as const).map((purpose)=>({value:purpose,label:t(`calls.purposes.${purpose}`)}))} onValueChange={(purpose)=>{const {purpose:_old,...when}=editing.rule.when;update({...editing.rule,when:{...when,...(purpose==='any'?{}:{purpose})}});}} />
        <Input label={t('calls.model')} placeholder={t('calls.anyModel')} value={editing.rule.when.model??''} onChange={(event)=>{const {model:_old,...when}=editing.rule.when;update({...editing.rule,when:{...when,...(event.target.value===''?{}:{model:event.target.value})}});}} />
        <Input label={t('calls.statusCode')} type="number" placeholder={t('calls.anyStatus')} value={editing.rule.when.status??''} onChange={(event)=>{const {status:_old,...when}=editing.rule.when;update({...editing.rule,when:{...when,...(event.target.value===''?{}:{status:Number(event.target.value)})}});}} />
        <Input label={t('calls.contains')} placeholder={t('calls.anyWords')} value={editing.rule.when.contains??''} onChange={(event)=>{const {contains:_old,...when}=editing.rule.when;update({...editing.rule,when:{...when,...(event.target.value===''?{}:{contains:event.target.value})}});}} />
        <Input label={t('calls.regex')} placeholder={t('calls.anyPattern')} value={editing.rule.when.regex??''} onChange={(event)=>{const {regex:_old,...when}=editing.rule.when;update({...editing.rule,when:{...when,...(event.target.value===''?{}:{regex:event.target.value})}});}} />
        <Table boundedIdentity label={t('calls.steps')} rows={editing.rule.steps.map((step,index)=>({step,index,key:editing.keys[index]??`${identity}:missing:${index}`}))} rowKey={({key})=>key} rowMenu={{label:stepLabel,entries:stepEntries}} columns={[
          {id:'name',header:t('calls.steps'),cell:(row)=><div className="flex min-w-0 flex-col gap-2"><div className="flex items-center justify-between gap-2"><span className="text-sm text-text-muted">{t('calls.stepNumber',{step:formatNumber(row.index+1,format.locale)})}</span><Menu label={stepLabel(row)} entries={stepEntries(row)} trigger={<IconButton size="sm" label={stepLabel(row)} title={stepLabel(row)} icon={<Ellipsis aria-hidden />} />} /></div><StepEditor step={row.step} onChange={(next)=>update({...editing.rule,steps:editing.rule.steps.map((held,at)=>at===row.index?next:held)})} /></div>},
        ]} />
        <Button size="sm" variant="ghost" icon={<Plus aria-hidden />} onClick={()=>changeSteps([...editing.rule.steps,{type:'wait',until:'person'}],[...editing.keys,`${identity}:${editing.index}:${editing.next}`],editing.next+1)}>{t('calls.addStep')}</Button>
      </div>
    </Dialog>}
  </div>;
}
