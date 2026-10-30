/** The machine's local policy as it is enforced there, before every page: shown to whoever sees the machine, changed on the machine, and from here only for the keys its owner handed over, by whoever manages what each bounds; the arrangement types the machine allows and what its exec node may offer among them.
 *
 * One row for each key: what it means, its value as the machine reports it, what it bounds, and where it
 * is managed. A key kept on the machine is read-only for everybody, with the command that changes it
 * there and the means to copy it. A key its owner handed over is changed here by whoever manages what it
 * bounds, with the value shown as it was until the machine has answered; for everybody else it says who
 * does. The row for the handing over itself always says that it is changed on the machine alone.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { type ReactNode, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';

import { ApiError, type FieldFault } from '@/api/client';
import { machinePolicyQuery, machineQuery, updateMachinePolicy } from '@/api/resources';
import { useHelpOpener } from '@/features/help/open';
import { machineExecutorsQuery } from '@/api/machineExecutors';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';
import { CommandBox } from '@/features/resources/Facts';
import { DetailPages, DetailSections, ResourceDetails } from '@/features/resources/DetailSections';
import { PortPolicyValue } from '@/features/resources/machines/PortPolicyValue';
import { canonicalExecutorOffers, ExecutorOffers, executorOfferIndex } from '@/features/resources/machines/ExecutorOffers';
import type { Machine, MachinePolicy, MachinePolicyKey, MachinePolicySection } from '@/generated/api-types';
import { Section } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { CheckboxList } from '@/ui/Checkbox';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { RelativeTime } from '@/ui/RelativeTime';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { Switch } from '@/ui/Switch';
import { SettingRow } from '@/ui/SettingRow';
import { SettingEditor } from '@/ui/SettingEditor';
import { HelpHint } from '@/ui/HelpHint';
import { Textarea } from '@/ui/Textarea';
import { toast } from '@/ui/Toast';

/** How a key's value is changed: a switch, a choice, a set of known words, or lines of text. */
type PolicyChoice = 'read_only' | 'workspace' | 'open' | 'separated' | 'key_separated' | 'credential_shared' | 'handed_over';
type Control =
  | { kind: 'switch' }
  | { kind: 'choice'; options: readonly PolicyChoice[] }
  | { kind: 'set'; options: readonly PolicyChoice[] }
  | { kind: 'executors' }
  | { kind: 'lines' }
  | { kind: 'harnesses' }
  | { kind: 'json' }
  | { kind: 'fixed' };

/** The keys of a machine's local policy, as the connector has them: the name each has in the catalogue (its dots would nest), and how it is changed. */
const KEYS = {
  drivers: { id: 'drivers', control: { kind: 'json' } },
  storages: { id: 'storages', control: { kind: 'json' } },
  'resources.max': { id: 'resources_max', control: { kind: 'json' } },
  hooks: { id: 'hooks', control: { kind: 'json' } },
  ports: { id: 'ports', control: { kind: 'json' } },
  'environment.roots': { id: 'environment_roots', control: { kind: 'lines' } },
  'environment.remote_write': { id: 'environment_remote_write', control: { kind: 'switch' } },
  'environment.terminal': { id: 'environment_terminal', control: { kind: 'switch' } },
  'environment.after_up': { id: 'environment_after_up', control: { kind: 'switch' } },
  'environment.browser': { id: 'environment_browser', control: { kind: 'switch' } },
  'environment.vnc': { id: 'environment_vnc', control: { kind: 'switch' } },
  'mcp.import': { id: 'mcp_import', control: { kind: 'switch' } },
  'mcp.add': { id: 'mcp_add', control: { kind: 'switch' } },
  'exec.offers': { id: 'exec_offers', control: { kind: 'executors' } },
  'environment.deliver_files': { id: 'environment_deliver_files', control: { kind: 'switch' } },
  'agent.max_sandbox': { id: 'agent_max_sandbox', control: { kind: 'choice', options: ['read_only', 'workspace', 'open'] } },
  'provider.upstreams': { id: 'provider_upstreams', control: { kind: 'lines' } },
  'provider.signin': { id: 'provider_signin', control: { kind: 'switch' } },
  'provider.arrangements': { id: 'provider_arrangements', control: { kind: 'set', options: ['separated', 'key_separated', 'credential_shared', 'handed_over'] } },
  'agent.harnesses': { id: 'agent_harnesses', control: { kind: 'harnesses' } },
  'agent.programs': { id: 'agent_programs', control: { kind: 'switch' } },
  'provider.programs': { id: 'provider_programs', control: { kind: 'switch' } },
  'machine.roles': { id: 'machine_roles', control: { kind: 'switch' } },
  'tools.fetch': { id: 'tools_fetch', control: { kind: 'switch' } },
  'update.auto': { id: 'update_auto', control: { kind: 'switch' } },
  'policy.delegated': { id: 'policy_delegated', control: { kind: 'fixed' } },
} as const satisfies Record<string, { id: string; control: Control }>;

type KnownKey = keyof typeof KEYS;

const isKnown = (key: string): key is KnownKey => key in KEYS;

const asLines = (value: unknown): string[] => (Array.isArray(value) ? value.filter((entry): entry is string => typeof entry === 'string') : []);

function harnessLines(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((entry) => {
    if (typeof entry !== 'object' || entry === null) return [];
    const { kind, path } = entry as { kind?: unknown; path?: unknown };
    return typeof kind === 'string' ? [typeof path === 'string' ? `${kind} ${path}` : kind] : [];
  });
}

function parseHarnesses(text: string): { kind: string; path: string }[] {
  return text
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line !== '')
    .map((line) => {
      const at = line.search(/\s/);
      return at === -1 ? { kind: line, path: '' } : { kind: line.slice(0, at), path: line.slice(at).trim() };
    });
}

/** A value as the command that sets it writes it: words as they are, anything else as JSON in single quotes. */
export function commandValue(value: unknown,windows=false): string {
  if (typeof value === 'boolean' || typeof value === 'number') return String(value);
  if (typeof value === 'string' && /^[\w./-]+$/.test(value)) return value;
  return `'${JSON.stringify(value).replaceAll("'", windows?"''":"'\\''")}'`;
}

/** The command that explicitly keeps the displayed value on this machine. */
export function policyCommand(key: string, value: unknown, program='orch-connector',windows=false): string {
  const prefix=program.endsWith(' policy')?program:`${program} policy`;
  return `${prefix} set ${key} ${commandValue(value,windows)}`;
}

function policyProgram(policy:MachinePolicy):string {
  return policy.command ?? 'orch-connector policy';
}

const SECTIONS: readonly MachinePolicySection[] = ['environment', 'node', 'agent', 'provider', 'machine'];

function keyWords(entry: MachinePolicyKey, t: TFunction) {
  const spec = isKnown(entry.key) ? KEYS[entry.key] : null;
  return spec === null
    ? { name: entry.definition.title, description: entry.definition.meaning }
    : { name: t(`machines.policy.keys.${spec.id}.title`), description: t(`machines.policy.keys.${spec.id}.meaning`) };
}

function ValueText({ policyKey, value }: { policyKey: string; value: unknown }) {
  const { t } = useTranslation();
  if (policyKey === 'ports') return <PortPolicyValue value={value} />;
  if (typeof value === 'boolean') return <>{value ? t('machines.policy.on') : t('machines.policy.off')}</>;
  if (policyKey === 'agent.max_sandbox' && (value === 'read_only' || value === 'workspace' || value === 'open')) return <>{t(`machines.policy.sandbox.${value}`)}</>;
  const lines = policyKey === 'agent.harnesses' ? harnessLines(value) : Array.isArray(value) && value.every((entry: unknown) => typeof entry === 'string') ? asLines(value) : null;
  if (lines !== null) {
    if (lines.length === 0) return <span className="text-text-muted">{t('machines.policy.none')}</span>;
    return (
      <ul className="flex flex-col gap-1">
        {lines.map((line) => (
          <li key={line}>
            <code className="wrap-anywhere">{line}</code>
          </li>
        ))}
      </ul>
    );
  }
  if (typeof value === 'string') return <code className="wrap-anywhere">{value}</code>;
  return <ResourceDetails><code className="wrap-anywhere">{JSON.stringify(value)}</code></ResourceDetails>;
}

/** The server's schema checks use fixed, value-free reasons. */
function driverFaultReason(message: string, t: TFunction): string {
  const keys = {
    'the value has the wrong type': 'type',
    'a required member is missing': 'required',
    'the object contains a member this configuration does not accept': 'extra',
    'the value is not an allowed choice': 'choice',
    'the number is outside the allowed range': 'range',
    'the text has a length outside the allowed range': 'length',
    'the text does not have the required format': 'format',
    'the list does not meet the required length or uniqueness': 'list',
    'the driver reported an invalid configuration schema': 'schema',
  } as const;
  const key = Object.hasOwn(keys, message) ? keys[message as keyof typeof keys] : 'rule';
  return t(`machines.policy.driverFaults.${key}`);
}

/** Put only driver configuration faults beside this editor; keep other failures in the alert. */
function driverFaults(error: unknown, key: string): { placed: FieldFault[]; allPlaced: boolean } {
  if (key !== 'drivers' || !(error instanceof ApiError)) return { placed: [], allPlaced: false };
  const faults = error.faults();
  const placed = faults.filter((fault) => fault.path.startsWith('drivers.configurations.'));
  return { placed, allPlaced: faults.length > 0 && placed.length === faults.length };
}

/** Who changes a key that was handed over, by what it bounds. */
function EditKey({ machineId, entry, onClose }: { machineId: string; entry: MachinePolicyKey; onClose: () => void }) {
  const { t } = useTranslation();
  const client=useQueryClient();
  const spec = isKnown(entry.key) ? KEYS[entry.key] : null;
  const control: Control = spec?.control ?? { kind: 'json' };
  const words = keyWords(entry, t);
  const [flag, setFlag] = useState(entry.value === true);
  const [choice, setChoice] = useState(typeof entry.value === 'string' ? entry.value : '');
  const [chosen, setChosen] = useState<string[]>(asLines(entry.value));
  const executors = useQuery({ ...machineExecutorsQuery(machineId), enabled: control.kind === 'executors' });
  const offerIndex = useMemo(() => executorOfferIndex(executors.data ?? []), [executors.data]);
  const executorChosen = control.kind === 'executors' ? canonicalExecutorOffers(chosen, offerIndex) : chosen;
  const executorSaved = control.kind === 'executors' ? canonicalExecutorOffers(asLines(entry.value), offerIndex) : [];
  const executorsUnavailable = control.kind === 'executors' && !executors.isSuccess;
  const [text, setText] = useState(control.kind === 'json' ? JSON.stringify(entry.value, null, 2) : (control.kind === 'harnesses' ? harnessLines(entry.value) : asLines(entry.value)).join('\n'));
  let validJson = true;
  if (control.kind === 'json') { try { JSON.parse(text); } catch { validJson = false; } }

  const save = useMutation({
    mutationFn: () => {
      const value: unknown =
        control.kind === 'json'
          ? JSON.parse(text) as unknown
          : control.kind === 'switch'
          ? flag
          : control.kind === 'choice'
            ? choice
            : control.kind === 'set'
              ? chosen
              : control.kind === 'executors'
                ? executorChosen
              : control.kind === 'harnesses'
                ? parseHarnesses(text)
                : text
                    .split('\n')
                    .map((line) => line.trim())
                    .filter((line) => line !== '');
      return updateMachinePolicy(machineId, { [entry.key]: value });
    },
    onSuccess: (policy) => {
      client.setQueryData(machinePolicyQuery(machineId).queryKey,policy);
      if(policy.machine_view!==null && policy.machine_view!==undefined) client.setQueryData(machineQuery(machineId).queryKey,policy.machine_view);
      toast({ tone: 'success', title: t('machines.policy.changed', { key: words.name }) });
      onClose();
    },
  });

  const faults = driverFaults(save.error, entry.key);

  const cancel=()=>{
    setFlag(entry.value===true);setChoice(typeof entry.value==='string'?entry.value:'');setChosen(asLines(entry.value));
    setText(control.kind==='json'?JSON.stringify(entry.value,null,2):(control.kind==='harnesses'?harnessLines(entry.value):asLines(entry.value)).join('\n'));
    save.reset();onClose();
  };
  const jsonEditor = <Textarea label={t('machines.policy.value')} hint={t('machines.policy.jsonHint')}
    error={!validJson ? t('machines.policy.jsonInvalid') : faults.placed.length === 0 ? undefined : <>{faults.placed.map((fault, index) =>
      <span className="block" key={`${fault.path}-${index}`}><code className="wrap-anywhere">{fault.path}</code>: {driverFaultReason(fault.message, t)}</span>)}</>}
    rows={4} value={text} disabled={save.isPending} onChange={(event) => { setText(event.target.value); if (entry.key === 'drivers') save.reset(); }} className="font-mono" />;
  return <div className="flex flex-col gap-3 py-2">
        {control.kind === 'switch' && <Switch name={t('machines.policy.allowedChoice.name')} description={t('machines.policy.allowedChoice.description')} checked={flag} onCheckedChange={setFlag} disabled={save.isPending} />}
        {control.kind === 'choice' && (
          <Select
            label={t('machines.policy.value')}
            value={choice}
            onValueChange={setChoice}
            disabled={save.isPending}
            options={control.options.map((option) => ({ value: option, name: t(`machines.policy.choices.${option}.name`, { defaultValue: option }), description: t(`machines.policy.choices.${option}.description`, { defaultValue: words.description }) }))}
          />
        )}
        {control.kind === 'set' && (
          <CheckboxList label={t('machines.policy.value')} value={chosen} onValueChange={setChosen} disabled={save.isPending}
            options={control.options.map((option) => ({ value: option, name: t(`machines.policy.choices.${option}.name`, { defaultValue: option }), description: t(`machines.policy.choices.${option}.description`, { defaultValue: words.description }) }))} />
        )}
        {control.kind === 'executors' && (executors.isPending
          ? <StateLine compact message={t('machines.policy.loadingExecutors')} />
          : executors.isError
            ? <ErrorState compact error={executors.error} message={t('machines.policy.executorReadFailed')} retrying={executors.isRefetching} onRetry={() => void executors.refetch()} />
            : <ExecutorOffers index={offerIndex} selected={executorChosen} saved={executorSaved} onValueChange={setChosen} disabled={save.isPending} />)}
        {(control.kind === 'lines' || control.kind === 'harnesses') && (
          <Textarea
            label={t('machines.policy.value')}
            hint={control.kind === 'harnesses' ? t('machines.policy.harnessesHint') : t('machines.policy.linesHint')}
            rows={4}
            value={text}
            disabled={save.isPending}
            onChange={(event) => setText(event.target.value)}
            className="font-mono"
          />
        )}
        {control.kind === 'json' && (entry.key === 'drivers' ? jsonEditor : <ResourceDetails keepMounted pauseEffects={false}>{jsonEditor}</ResourceDetails>)}
        {save.error !== null && !faults.allPlaced && <ErrorAlert error={save.error} />}
        <div className="flex justify-end gap-2"><Button disabled={save.isPending} onClick={cancel}>{t('common.cancel')}</Button>
          <Button variant="primary" pending={save.isPending} blocked={!validJson || executorsUnavailable} onClick={()=>save.mutate()}>{t('machines.policy.save')}</Button></div>
    </div>;
}

function PolicySummary({entry}:{entry:MachinePolicyKey}) {
  const {t}=useTranslation();
  if(entry.key==='policy.delegated' && Array.isArray(entry.value)) {
    const selectors=asLines(entry.value);
    return <>{selectors.length===0?t('machines.policy.none'):selectors.map((selector)=>{
      if(selector==='*') return t('machines.policy.allRoles');
      const section=SECTIONS.find((section)=>selector===`${section}.*`);
      if(section!==undefined) return t(`machines.policy.sections.${section}.name`);
      return isKnown(selector)?t(`machines.policy.keys.${KEYS[selector].id}.title`):t('machines.policy.otherSetting');
    }).join(', ')}</>;
  }
  if(Array.isArray(entry.value) && entry.value.length>1) return <>{t('machines.policy.items',{count:entry.value.length})}</>;
  if(entry.value!==null && typeof entry.value==='object' && !Array.isArray(entry.value)) {
    const count=Object.keys(entry.value).length;
    return <>{count===0?t('machines.policy.none'):t('machines.policy.items',{count})}</>;
  }
  return <ValueText policyKey={entry.key} value={entry.value}/>;
}

function DriverPolicyHelp({ term, what }: { term: string; what: ReactNode }) {
  const { t } = useTranslation();
  const open = useHelpOpener();
  return <HelpHint term={term} what={what} guide={{article:'connect-a-machine',heading:'change-driver-settings'}}
    more={{label:t('help.hint.more'),onOpen:()=>open('connect-a-machine','change-driver-settings')}} />;
}

function PolicyRow({ machineId, entry, program,windows=false }: { machineId: string; entry: MachinePolicyKey; program:string;windows?:boolean }) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState(false);
  const words = keyWords(entry, t);
  const fixed = entry.key === 'policy.delegated';
  const helpContent=<div className="flex flex-col gap-2"><p>{words.description}</p>
      {entry.key === 'exec.offers' && <ArticleGuideLink article="resources" heading="profiles-hosts-and-computers" title={t('help.hint.more')} />}
      <dl className="grid grid-cols-2 gap-2 text-sm">
        <div><dt>{t('machines.policy.defaults.managed')}</dt><dd><ValueText policyKey={entry.key} value={entry.definition.managed} /></dd></div>
        <div><dt>{t('machines.policy.defaults.cautious')}</dt><dd><ValueText policyKey={entry.key} value={entry.definition.cautious} /></dd></div>
      </dl>
      {fixed?<><p>{t('machines.policy.onlyOnMachine')}</p><CommandBox command={`${program} delegate '*'`} label={t('machines.policy.copy')}/></>:
        !entry.delegated?<CommandBox command={policyCommand(entry.key,entry.value,program,windows)} label={t('machines.policy.copy')}/>:null}
    </div>;
  const help=entry.key==='drivers'?<DriverPolicyHelp term={words.name} what={helpContent}/>:<HelpHint term={words.name} what={helpContent}/>;
  const reason=t(entry.writable?'machines.policy.source.orch':entry.delegated?'machines.policy.source.manager':'machines.policy.source.local');
  const complex=entry.value!==null && typeof entry.value==='object' && (Array.isArray(entry.value)?entry.value.length>0:Object.keys(entry.value).length>0);
  if(!entry.writable && !complex) return <SettingRow name={words.name} settingKey={entry.key} value={<PolicySummary entry={entry}/>} help={help} reason={reason} readOnly/>;
  return <SettingEditor name={words.name} settingKey={entry.key} help={help} reason={reason} readOnly={!entry.writable}
    summary={editing?t('common.close'):<PolicySummary entry={entry}/>} label={t(entry.writable?'machines.policy.editTitle':'machines.policy.viewTitle',{key:words.name})}
    open={editing} onOpenChange={setEditing}>
    {entry.writable?<EditKey machineId={machineId} entry={entry} onClose={()=>setEditing(false)}/>:<ValueText policyKey={entry.key} value={entry.value}/>}
  </SettingEditor>;
}

export function MachinePolicyTab({ machine }: { machine: Pick<Machine, 'id'> }) {
  const { t } = useTranslation();
  const [handover, setHandover] = useState<MachinePolicySection | null>(null);
  const policy = useQuery(machinePolicyQuery(machine.id));
  if (policy.isPending) return <SkeletonRows rows={4} />;
  if (policy.isError) return <ErrorState compact error={policy.error} retrying={policy.isRefetching} onRetry={() => void policy.refetch()} />;
  const { keys, reported_at: reportedAt } = policy.data;
  const program = policyProgram(policy.data);
  const handed = keys.find((entry) => entry.key === 'policy.delegated')?.value;
  const fullAccess = Array.isArray(handed) && handed.includes('*') && keys.filter((entry)=>entry.definition.delegable).every((entry)=>entry.delegated);
  return (
    <div className="flex flex-col gap-4" data-tutorial-target="machine-policy">
      <div className="flex flex-col gap-1">
        {fullAccess && <p className="font-medium text-text">{t('machines.policy.fullAccess')}</p>}
        <p className="text-sm text-text-muted">
          {reportedAt === null ? t('machines.policy.notReported') : <>{t('machines.policy.reported')} <RelativeTime at={reportedAt} /></>}
        </p>
      </div>
      {keys.length === 0 ? (
        <EmptyState compact title={t('machines.policy.noKeys')} />
      ) : (
        <DetailSections label={t('resourceDetails.layout.sections')} sections={SECTIONS.flatMap((section) => {
          // Shared keys (for example directory roots) keep one row and one editor on this page.
          const rows = keys.filter((entry) => SECTIONS.find((candidate)=>entry.definition.sections.includes(candidate))===section);
          if (rows.length === 0) return [];
          const full = rows.filter((entry) => entry.definition.delegable).every((entry) => entry.delegated);
          return [{ value: section, label: t(`machines.policy.sections.${section}.name`), panel: <section aria-label={t(`machines.policy.sections.${section}.name`)} className="flex min-w-0 flex-col gap-2">
            <div className="flex flex-wrap items-start justify-between gap-2">
              <HelpHint term={t(`machines.policy.sections.${section}.name`)} what={t(`machines.policy.sections.${section}.description`)}/>
              <Button size="sm" onClick={() => setHandover(section)}>{t('machines.policy.fullAccessAction')}</Button>
            </div>
            {full && !fullAccess && <p className="text-sm text-text-muted">{t('machines.policy.roleFullAccess')}</p>}
            <DetailPages items={rows}>{(page) => <ul className="flex flex-col divide-y divide-border rounded-lg border border-border bg-surface">
              {page.map((entry) => <PolicyRow key={entry.key} machineId={machine.id} entry={entry} program={program} windows={policy.data.machine.platform_os==='windows'} />)}
            </ul>}</DetailPages>
          </section> }];
        })} />
      )}
      {handover !== null && <Dialog open onOpenChange={(open) => !open && setHandover(null)} title={t('machines.policy.fullAccessFor', { role: t(`machines.policy.sections.${handover}.name`) })}
        description={t('machines.policy.handoverLead')} footer={<Button onClick={() => setHandover(null)}>{t('common.close')}</Button>}>
        <div className="flex flex-col gap-3 py-2">
          <p>{t('machines.policy.localValuesHold')}</p>
          <CommandBox command={`${program} delegate '${handover === 'machine' ? '*' : `${handover}.*`}'`} label={t('machines.policy.copy')} />
        </div>
      </Dialog>}
    </div>
  );
}

/**
 * The same rows, cut down to the keys that bound one kind of thing: what a machine allows its
 * environments or its providers, which is what whoever manages one of those reads and, where the owner
 * handed a key over, changes. Whoever may not read the machine's policy is shown nothing of it.
 */
export function MachineLimits({ machineId, bounds, id, title, lead, compact = false }: { machineId: string; bounds: 'environment' | 'provider'; id: string; title: string; lead: string; compact?: boolean }) {
  const policy = useQuery({ ...machinePolicyQuery(machineId), retry: false });
  if (policy.isPending) return <SkeletonRows rows={3}/>;
  if (policy.isError) return <ErrorState compact error={policy.error} retrying={policy.isRefetching} onRetry={()=>void policy.refetch()}/>;
  const sections:readonly MachinePolicySection[]=bounds==='environment'?['environment','node']:['environment','node','provider'];
  const keys = policy.data.keys.filter((entry) => entry.definition.sections.some((section)=>sections.includes(section)));
  if (keys.length === 0) return null;
  const rows = (entries: readonly MachinePolicyKey[]) => <ul aria-label={title} className="flex flex-col divide-y divide-border rounded-lg border border-border bg-surface">
    {entries.map((entry) => <PolicyRow key={entry.key} machineId={machineId} entry={entry} program={policyProgram(policy.data)} windows={policy.data.machine.platform_os==='windows'} />)}
  </ul>;
  return (
    <Section id={id} title={title} lead={lead}>
      {compact ? <DetailPages items={keys}>{rows}</DetailPages> : rows(keys)}
    </Section>
  );
}
