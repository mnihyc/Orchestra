/** Both unit pools show applicable values, provenance, apply timing and guarded editors, including commands after start. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ApiError } from '@/api/client';
import { environmentKey, environmentSettingsKey } from '@/api/environments';
import { jobQuery } from '@/api/forks';
import { providerKey, providerSettingsKey, templateQuery } from '@/api/resources';
import { putSetting, targetSettingsKey, targetSettingsQuery, type SettingsTarget } from '@/api/settings';
import { SettingsTable } from '@/features/resources/SettingsTable';
import { DetailSections, ResourceDetails } from '@/features/resources/DetailSections';
import { CommandList, CommandListEditor, TemplatePrograms, commandMap, record, templateCommands, validCommands } from '@/features/resources/UnitCommands';
import { SettingsEditor } from '@/features/settings/Editor';
import type { RowPlace } from '@/features/settings/SettingRow';
import { runtimeEffect } from '@/features/settings/runtimeEffects';
import type { ResolvedSetting, RuntimeChanges } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Preview } from '@/ui/Preview';
import { SkeletonRows } from '@/ui/Skeleton';
import { useErrorText } from '@/ui/errorText';

export interface UnitConfigurationProps {
  pool: 'environment' | 'provider'; id: string; placement: 'managed' | 'connected'; place: RowPlace;
  owns?: boolean; templateId?: string | null; jobId?: string | null; failureDetail?: string | null; runtime?: RuntimeChanges | null;
  lifecycle?: 'unit' | 'attach' | null;
  commands?: { supported: boolean; reason: string | null } | null;
  afterUp?: boolean | null;
  resourceSupport?:unknown;
  compact?: boolean;
  wantedKey?: string;
}

/** Variables, secrets and after-start commands belong to every provider runtime; a recipe needs unit services. */
export function unitKeys(pool: 'environment' | 'provider', managed: boolean, rows: readonly ResolvedSetting[]): string[] {
  if (!managed) return pool === 'provider' ? rows.filter((row) => ['runtime.variables','runtime.secret_names','runtime.after_up','runtime.home','runtime.storage','runtime.resources'].includes(row.key)).map((row) => row.key) : rows.filter((row) => ['env.variables', 'env.secret_names', 'env.mounts','env.after_up','env.resources','env.data_sources'].includes(row.key) || /^(exec|mcp|browser|vnc|terminal|ports|capability|workspace)\./.test(row.key)).map((row) => row.key);
  return rows.filter((row) => row.key.startsWith(pool === 'provider' ? 'runtime.' : 'env.') || row.key === 'network.allowed_hosts' || pool === 'environment' && row.key === 'agents.harness_versions' ||
    (pool === 'environment' && /^(exec|mcp|browser|vnc|terminal|ports|capability|workspace)\./.test(row.key))).map((row) => row.key);
}

function AfterStart({ target, setting, spec, editable, templateReadable, templateApplies = true, blockedReason = null, compact = false }: { target: SettingsTarget; setting: ResolvedSetting; spec: unknown; editable: boolean; templateReadable: boolean; templateApplies?: boolean; blockedReason?: string | null; compact?: boolean }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const [commands, setCommands] = useState(() => commandMap(setting.own?.value));
  const [removed, setRemoved] = useState(() => Object.entries(record(setting.own?.value)).filter(([, value]) => value === null).map(([name]) => name));
  const [revision, setRevision] = useState(setting.own?.revision ?? null);
  const inherited = commandMap(setting.value).filter((command) => !(command.name in record(setting.own?.value)));
  const save = useMutation({ mutationFn: () => putSetting(target, setting.key, {
    value: { ...Object.fromEntries(removed.map((name) => [name, null])), ...Object.fromEntries(commands.map((command) => [command.name, { command: command.command, timeout_secs: command.timeout_secs }])) },
    expectedRevision: revision,
  }), onSuccess: async (next) => {
    setRevision(next.own?.revision ?? null);
    if (compact) {
      setCommands(commandMap(next.own?.value));
      setRemoved(Object.entries(record(next.own?.value)).filter(([, value]) => value === null).map(([name]) => name));
      client.setQueryData<ResolvedSetting[]>(targetSettingsKey(target), (before) => before?.map((row) => row.key === next.key ? next : row));
      return;
    }
    await Promise.all([
      client.invalidateQueries({ queryKey: targetSettingsKey(target) }),
      ...(target.kind === 'environment' ? [client.invalidateQueries({ queryKey: environmentKey(target.id) }), client.invalidateQueries({ queryKey: environmentSettingsKey(target.id) })] :
        target.kind === 'provider' ? [client.invalidateQueries({ queryKey: providerKey(target.id) }), client.invalidateQueries({ queryKey: providerSettingsKey(target.id) })] : []),
    ]);
  }, onError: (error) => {
    if (error instanceof ApiError && error.is('conflict.stale_revision')) void client.invalidateQueries({ queryKey: targetSettingsKey(target) });
  } });
  return <section id="unit-after-up" className="flex flex-col gap-4" aria-label={t('unitConfiguration.commands')}>
    {!compact && <h2 className="text-base font-semibold">{t('unitConfiguration.commands')}</h2>}
    <p className="text-sm text-text-muted">{t(templateApplies ? 'unitConfiguration.commandTiming' : 'unitConfiguration.commandAttachTiming')}</p>
    {target.kind === 'provider' && <InlineAlert tone="warning">{t('unitConfiguration.credentialWarning')}</InlineAlert>}
    {templateApplies && (templateReadable ? <CommandList label={t('unitConfiguration.templateCommands')} commands={templateCommands(spec)} /> : <p className="text-sm text-text-muted">{t('unitConfiguration.templateUnread')}</p>)}
    <CommandList label={t('unitConfiguration.inheritedCommands')} commands={inherited.filter((command) => !removed.includes(command.name))} />
    {editable && inherited.filter((command) => !removed.includes(command.name)).map((command) => <Button key={command.name} size="sm" variant="ghost" onClick={() => setRemoved((before) => [...before, command.name])}>{t('unitConfiguration.removeInherited', { name: command.name })}</Button>)}
    {removed.length > 0 && <div><h3 className="text-sm font-medium">{t('unitConfiguration.removedCommands')}</h3><ul>{removed.map((name) => <li key={name}>{editable ?
      <Button size="sm" variant="ghost" onClick={() => setRemoved((before) => before.filter((command) => command !== name))}>{t('unitConfiguration.restoreInherited', { name })}</Button> : <code>{name}</code>}</li>)}</ul></div>}
    {editable ? <><CommandListEditor value={commands} onChange={setCommands} disabled={save.isPending} />
      {revision !== (setting.own?.revision ?? null) && <InlineAlert tone="warning">{t('unitConfiguration.changed')}<Button size="sm" disabled={save.isPending} onClick={() => {
        setCommands(commandMap(setting.own?.value)); setRemoved(Object.entries(record(setting.own?.value)).filter(([, value]) => value === null).map(([name]) => name)); setRevision(setting.own?.revision ?? null); save.reset();
      }}>{t('unitConfiguration.readLatest')}</Button></InlineAlert>}
      <Button disabled={!validCommands(commands)} pending={save.isPending} onClick={() => save.mutate()}>{t('unitConfiguration.saveCommands')}</Button>
      {save.error && <ErrorAlert error={save.error} />}{save.isSuccess && <InlineAlert tone="info">{t('unitConfiguration.saved')}</InlineAlert>}
    </> : <><CommandList label={t('unitConfiguration.ownCommands')} commands={commandMap(setting.own?.value)} />
      <p className="text-sm text-text-muted">{blockedReason ?? t(target.kind === 'provider' ? 'unitConfiguration.ownerOnly' : 'unitConfiguration.readOnly')}</p></>}
  </section>;
}

// The startup-failure preview has always shown its first ten lines; the full saved diagnostic
// stays reachable through the shared preview, outside the ordinary translated status sentence.
const COMMAND_FAILURE_PREVIEW_LINES = 10;
const COMMAND_FAILURE_CODE = 'after_up_failed';
export function UnitCommandFailure({ jobId, detail, href = '#unit-after-up', onOpen }: { jobId?: string | null; detail?: string | null; href?: string; onOpen?: () => void }) {
  const { t } = useTranslation();
  const failureText = useErrorText();
  const job = useQuery({ ...jobQuery(jobId ?? ''), enabled: Boolean(jobId) });
  const error: Record<string, unknown> = job.data?.state === 'failed' ? record(job.data.error) : {};
  // Provider provisioning wraps the saved command failure with its phase after the active job clears.
  const status = detail ?? '';
  const commandAt = status.indexOf('after-start command ');
  if (error.code !== COMMAND_FAILURE_CODE && commandAt < 0) return null;
  const diagnostic=typeof error.message==='string'?error.message:typeof error.detail==='string'?error.detail:commandAt<0?'':status.slice(commandAt);
  const lines=diagnostic.split('\n');
  return <><InlineAlert tone="warning" action={
    <a href={href} className="underline" onClick={(event) => { if (onOpen !== undefined && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey && event.button === 0) { event.preventDefault(); onOpen(); } }}>{t('unitConfiguration.changeCommands')}</a>
  }>{failureText({ code: COMMAND_FAILURE_CODE }).message}</InlineAlert>
    {diagnostic!=='' && <ResourceDetails label={t('unitConfiguration.commandOutput')}><Preview more={lines.length>COMMAND_FAILURE_PREVIEW_LINES}>{(expanded)=><pre className="text-sm wrap-anywhere whitespace-pre-wrap">{(expanded?lines:lines.slice(0,COMMAND_FAILURE_PREVIEW_LINES)).join('\n')}</pre>}</Preview></ResourceDetails>}
  </>;
}

const RESOURCES=['cpu','memory','pids','disk'] as const;
function resourceState(value:unknown):'enforced'|'reported'|'none'|'unknown' {
  return value==='enforced'||value==='reported'||value==='none'?value:'unknown';
}

export function ResourceSupport({support}:{support:unknown}) {
  const {t}=useTranslation();
  const facts=record(support);
  const reasons=record(facts.reasons);
  return <section aria-label={t('unitConfiguration.resources.title')} className="flex flex-col gap-3">
    <h2 className="font-semibold">{t('unitConfiguration.resources.title')}</h2>
    <p className="text-sm text-text-muted">{t('unitConfiguration.resources.about')}</p>
    <dl className="divide-y divide-border rounded-lg border border-border bg-surface px-3">
      {RESOURCES.map((resource)=>{
        const value=facts[resource];
        const state=resourceState(value);
        return <div key={resource} className="grid gap-1 py-3 sm:grid-cols-2">
          <dt className="text-sm font-medium">{t(`unitConfiguration.resources.${resource}.name`)}<p className="font-normal text-text-muted">{t(`unitConfiguration.resources.${resource}.description`)}</p></dt>
          <dd className="text-sm">{t(`unitConfiguration.resources.states.${state}.name`)}<p className="text-text-muted">{typeof reasons[resource]==='string'?reasons[resource]:t(`unitConfiguration.resources.states.${state}.description`)}</p></dd>
        </div>;
      })}
    </dl>
  </section>;
}

export function UnitConfiguration({ pool, id, place, owns = false, templateId, jobId, failureDetail, runtime, lifecycle, commands, afterUp,resourceSupport, compact = false, wantedKey }: UnitConfigurationProps) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const target: SettingsTarget = pool === 'environment' ? { kind: 'environment', id } : { kind: 'provider', id };
  const list = useQuery(targetSettingsQuery(target));
  const managed = lifecycle === 'unit' || ((lifecycle === null || lifecycle === undefined) && Boolean(templateId));
  const resolvedTemplate = list.data?.find((row) => row.key === (pool === 'provider' ? 'runtime.template_id' : 'env.template_id'))?.value;
  const selectedTemplate = templateId ?? (typeof resolvedTemplate === 'string' ? resolvedTemplate : undefined);
  const [section, setSection] = useState(wantedKey === (pool === 'provider' ? 'runtime.after_up' : 'env.after_up') ? 'commands' : 'values');
  const template = useQuery({ ...templateQuery(selectedTemplate ?? ''), enabled: managed && Boolean(selectedTemplate) && (!compact || section === 'commands') });
  const keys = unitKeys(pool, managed, list.data ?? []);
  const commandKey = pool === 'provider' ? 'runtime.after_up' : 'env.after_up';
  const command = list.data?.find((row) => row.key === commandKey);
  const storageKey=pool==='provider'?'runtime.storage':'env.storage';
  const storage=list.data?.find((row)=>row.key===storageKey);
  // Dedicated command/home editors are their one entrance, including their stricter authority.
  const rows = (list.data ?? []).filter((row) => keys.includes(row.key)&&row.key!==storageKey && row.key!==commandKey && (pool !== 'provider' || row.key !== 'runtime.home'));
  const editorKeys = rows.map((row) => row.key);
  const when = (key: string) => {
    if (!managed && key === 'env.mounts') return t('unitConfiguration.effects.attach');
    if (!managed && (key==='env.resources'||key==='runtime.resources')) return t('unitConfiguration.effects.attach');
    if (key === 'env.variables' || key === 'env.secret_names') return t('unitConfiguration.effects.attach');
    if (key === commandKey) return t(managed ? 'unitConfiguration.effects.start' : 'unitConfiguration.effects.attach');
    if (!managed && (key === 'runtime.variables' || key === 'runtime.secret_names')) return t('unitConfiguration.effects.attach');
    if (key === 'runtime.home') return t(managed ? 'unitConfiguration.effects.update' : 'unitConfiguration.effects.attach');
    if (['runtime.template_id', 'runtime.host_id', 'runtime.variables', 'runtime.secret_names', 'runtime.storage', 'runtime.needs', 'env.template_id', 'env.runtime', 'env.storage', 'env.needs', 'env.host_id', 'network.allowed_hosts'].includes(key)) return t('unitConfiguration.effects.update');
    const effect = runtimeEffect(key, runtime);
    if (effect !== null) return <span>{t(`runtimeChanges.${effect}`)}{effect === 'pendingResources' && <span className="block text-warning-text">{runtime?.pending_reason}</span>}</span>;
    if (/^(exec|mcp)\./.test(key)) return t('unitConfiguration.effects.turn');
    return t('unitConfiguration.effects.policy');
  };
  if (compact) {
    if (list.isPending) return <SkeletonRows rows={4} />;
    if (list.isError) return <ErrorState compact error={list.error} onRetry={() => void list.refetch()} />;
    return <div className="flex min-w-0 flex-col gap-4">
      <UnitCommandFailure jobId={jobId} detail={failureDetail} href={`?tab=settings&setting=${commandKey}#unit-after-up`} onOpen={() => setSection('commands')} />
      <DetailSections label={t('resourceDetails.layout.configuration')} value={section} onValueChange={setSection} sections={[
        { value: 'values', label: t('resourceDetails.layout.values'), panel: <SettingsEditor compact target={target} scope={pool} only={[...editorKeys, ...(storage === undefined ? [] : [storageKey])]} place={place} onChanged={(next) => {
          client.setQueryData<ResolvedSetting[]>(targetSettingsKey(target), (before) => before?.map((row) => row.key === next.key ? next : row));
        }} /> },
        ...(command === undefined ? [] : [{ value: 'commands', label: t('unitConfiguration.commands'), panel: <>
          {afterUp === false && <InlineAlert tone="info">{t('unitConfiguration.commandsPolicyOff')}</InlineAlert>}
          {managed && template.isError ? <ErrorState compact error={template.error} onRetry={() => void template.refetch()} /> : managed && selectedTemplate !== undefined && template.isPending ? <SkeletonRows rows={2} /> : <AfterStart compact key={`${pool}:${id}`} target={target} setting={command} spec={template.data?.spec}
            templateReadable={selectedTemplate === undefined || template.data !== undefined} templateApplies={managed}
            blockedReason={commands?.supported === false ? commands.reason ?? t('unitConfiguration.commandsUnsupported') : null}
            editable={command.writable && (pool === 'environment' || owns) && commands?.supported !== false} />}
        </> }]),
      ]} />
      <ResourceDetails><ResourceSupport support={resourceSupport} /></ResourceDetails>
    </div>;
  }
  return <div className="flex flex-col gap-6">
    <UnitCommandFailure jobId={jobId} detail={failureDetail} />
    <ResourceSupport support={resourceSupport} />
    {storage!==undefined&&<section id="unit-storage" aria-label={t('machineStorage.title')} className="flex flex-col gap-3">
      <SettingsTable settings={[storage]} label={t('machineStorage.title')} target={target} place={place} runtime={runtime} effectFor={when} />
    </section>}
    {list.isPending ? <SkeletonRows rows={4} /> : list.isError ? <ErrorState compact error={list.error} onRetry={() => void list.refetch()} /> : <>
      <SettingsTable settings={rows} label={t('unitConfiguration.values')} target={target} place={place} runtime={runtime} effectFor={when} />
      {managed && template.isError && <ErrorState compact error={template.error} onRetry={() => void template.refetch()} />}
      {pool === 'provider' && commands?.supported === false && <InlineAlert tone="info">{commands.reason ?? t('unitConfiguration.commandsUnsupported')}</InlineAlert>}
      {afterUp === false && <InlineAlert tone="info">{t('unitConfiguration.commandsPolicyOff')}</InlineAlert>}
      {managed && selectedTemplate !== undefined && template.isPending ? <SkeletonRows rows={2} /> : command !== undefined && <AfterStart key={`${pool}:${id}`} target={target} setting={command} spec={template.data?.spec}
        templateReadable={selectedTemplate === undefined || template.data !== undefined}
        templateApplies={managed} blockedReason={commands?.supported === false ? commands.reason ?? t('unitConfiguration.commandsUnsupported') : null}
        editable={command.writable && (pool === 'environment' || owns) && commands?.supported !== false} />}
      {managed && pool === 'provider' && template.data !== undefined && <TemplatePrograms spec={template.data.spec} />}
    </>}
  </div>;
}
