import { ResourcePickerLookup } from '@/ui/ResourcePicker';
/** Template and host settings use authorized names; the containing native row owns every write. */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { hostQuery, hostsQuery, templateQuery, templatesQuery } from '@/api/resources';
import type { Host, Template } from '@/generated/api-types';
import type { ValueControlProps } from '@/features/settings/ValueControl';
import { holds } from '@/lib/permissions';
import { ErrorState } from '@/ui/ErrorState';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { hostChoice, templateChoice } from '@/features/resources/choiceAvailability';
import { usePoolTargetChoices } from '@/features/resources/pools/targetChoices';
import { HOST_POOL_PREFIX, hostChoiceTarget, hostPoolId, singleHostId } from '@/features/resources/pools/hostTarget';
import { poolQuery } from '@/api/pools';

export function isResourceSetting(key: string | undefined): boolean {
  return key === 'env.template_id' || key === 'runtime.template_id' || key === 'env.host_id' || key === 'runtime.host_id';
}

type ChosenResource = { kind: 'host'; value: Host } | { kind: 'template'; value: Template };

function useChoices(key: string, open: boolean, selected: string, t: TFunction) {
  const template = key.endsWith('.template_id');
  const hosts = useQuery({ ...hostsQuery(null), enabled: open && !template });
  const templates = useQuery({ ...templatesQuery(key.startsWith('env.') ? 'environment' : 'provider', null), enabled: open && template });
  const rows = template ? templates.data?.map((row) => ({ id: row.id, ...templateChoice(row, t) }))
    : hosts.data?.map((row) => ({ id: row.id, ...hostChoice(row, t, key.startsWith('env.') ? 'environment' : 'provider') }));
  const host = hosts.data?.find((row) => row.id === selected), entry = templates.data?.find((row) => row.id === selected);
  const initial: ChosenResource | undefined = template
    ? entry === undefined ? undefined : { kind: 'template', value: entry }
    : host === undefined ? undefined : { kind: 'host', value: host };
  return { rows, query: template ? templates : hosts, template, initial };
}

/** Reading one selected name never loads or subscribes to the whole picker inventory. */
function useChosen(key: string, selected: string, initial?: ChosenResource) {
  const template = key.endsWith('.template_id');
  const readable = initial === undefined || holds(initial.value.access, initial.kind === 'host' ? 'host.view' : 'template.view');
  const host = useQuery({ ...hostQuery(selected), ...(initial?.kind === 'host' && readable ? { initialData: initial.value } : {}), enabled: readable && selected !== '' && !template, staleTime: Infinity });
  const entry = useQuery({ ...templateQuery(selected), ...(initial?.kind === 'template' && readable ? { initialData: initial.value } : {}), enabled: readable && selected !== '' && template, staleTime: Infinity });
  return template ? entry : host;
}

export function ResourceSettingChoice({ settingKey, value, label, onChange, onEdited, disabled, error, placeholder, inheritance }: ValueControlProps & { settingKey: string }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [typed, setTyped] = useState('');
  const isTemplate = settingKey.endsWith('.template_id');
  const poolId = isTemplate ? undefined : hostPoolId(value);
  const selected = isTemplate && typeof value === 'string' ? value : singleHostId(value) ?? '';
  const pools = usePoolTargetChoices('host', open && !isTemplate, typed);
  const pool = useQuery({ ...poolQuery(poolId ?? ''), enabled: poolId !== undefined, retry: false });
  const { rows, query, template, initial } = useChoices(settingKey, open, selected, t);
  const current = rows?.find((row) => row.id === selected);
  const chosen = useChosen(settingKey, selected, initial);
  // Radix does not accept an empty option value; this sentinel never leaves the control.
  const automatic = '\u0000automatic-host';
  return <div className="flex min-w-0 flex-col gap-2">
    <ResourcePickerLookup inheritance={inheritance} pickerKey="settings.ResourceChoice" label={label} hideLabel size="sm"
      query={typed} onQueryChange={setTyped} initialActiveValue={poolId !== undefined ? `${HOST_POOL_PREFIX}${poolId}` : selected === '' && !template ? automatic : selected}
      open={open && (inheritance !== undefined || !query.isError && !(template && query.isSuccess && rows?.length === 0))} onOpenChange={(open) => { setOpen(open); if (!open) setTyped(''); }}
      displayValue={poolId !== undefined ? pool.data?.name ?? t(pool.isError ? 'settings.resourceChoice.unavailable' : 'common.loading') : selected === '' ? template ? undefined : value === null || value === undefined ? t('providerRuntime.autoHost') : t('settings.resourceChoice.unavailable') : current?.name ?? chosen.data?.name ?? t(chosen.isError ? 'settings.resourceChoice.unavailable' : 'common.loading')}
      placeholder={open && query.isPending ? t('common.loading') : placeholder ?? t('settings.resourceChoice.choose')}
      disabled={disabled} error={error} loading={query.isFetching}
      groups={[{ id:'resources',label:'',options:[
        ...(!template ? [{ value: automatic, label: t('providerRuntime.autoHost'), text:t('providerRuntime.autoHost') }] : []),
        ...(query.isError ? [] : (rows ?? []).map((row)=>({ ...row,value:row.id,label:row.name,text:row.name }))),
      ] }, ...(!template ? [{ id:'pools',label:t('screens.pools.title'),options:[...pools.options,
        ...(poolId !== undefined && !pools.rows.some((row)=>row.id===poolId) ? [{ value:`${HOST_POOL_PREFIX}${poolId}`,label:pool.data?.name ?? t('settings.resourceChoice.unavailable'),text:pool.data?.name ?? t('settings.resourceChoice.unavailable'),disabled:true }] : [])] }] : [])]}
      footer={!template ? pools.footer : undefined}
      onSelect={(next) => { onEdited?.(); onChange(next.value === automatic ? null : template ? next.value : hostChoiceTarget(next.value)); setOpen(false); setTyped(''); }} />
    {query.isError && <ErrorState compact error={query.error} retrying={query.isRefetching} onRetry={() => { setOpen(false); void query.refetch(); }} />}
    {query.isSuccess && !rows?.some((row) => !row.disabled) && <span className="text-sm text-text-muted">{t(template ? 'settings.resourceChoice.noTemplates' : 'settings.resourceChoice.noHosts')}</span>}
  </div>;
}

/** A readable name or an honest unavailable state, never a raw identifier or a guessed private name. */
export function ResourceSettingName({ settingKey, value, example = false, plain = false }: { settingKey: string; value: unknown; example?: boolean; plain?: boolean }) {
  const { t } = useTranslation();
  return example ? <span>{t(settingKey.endsWith('.template_id') ? 'settings.resourceChoice.templateExample' : 'settings.resourceChoice.hostExample')}</span>
    : value === null || value === undefined || value === '' ? <span>{t(settingKey.endsWith('.host_id') ? 'providerRuntime.autoHost' : 'settings.compact.none')}</span>
      : <ChosenName settingKey={settingKey} value={value} plain={plain} />;
}

function ChosenName({ settingKey, value, plain }: { settingKey: string; value: unknown; plain: boolean }) {
  const pool = hostPoolId(value);
  return pool === undefined ? <ChosenResourceName settingKey={settingKey} value={value} plain={plain} /> : <ChosenPoolName id={pool} plain={plain} />;
}
function ChosenPoolName({ id, plain }: { id:string; plain:boolean }) {
  const { t } = useTranslation();
  const reader = useStore(client.session,(state)=>`${state.userId ?? ''}:${state.readerVersion ?? 0}`);
  const query = useQuery({ ...poolQuery(id,reader), retry:false });
  if (query.isError) return <span>{t('settings.resourceChoice.unavailable')}</span>;
  return query.data === undefined ? <span>{t('common.loading')}</span> : plain ? <span>{query.data.name}</span>
    : <ResourceChip subject={{ kind:'pool',pool:query.data }} hint={false} />;
}
function ChosenResourceName({ settingKey, value, plain }: { settingKey: string; value: unknown; plain: boolean }) {
  const { t } = useTranslation();
  const selected = settingKey.endsWith('.template_id') && typeof value === 'string' ? value : singleHostId(value);
  const query = useChosen(settingKey, selected ?? '');
  if (selected === undefined) return <span>{t('settings.resourceChoice.unavailable')}</span>;
  if (query.isError) return plain ? <span>{t('settings.resourceChoice.unavailable')}</span>
    : <ErrorState compact title={t('settings.resourceChoice.unavailable')} error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} />;
  const resource = query.data;
  return resource === undefined ? <span>{t('common.loading')}</span>
    : plain ? <span>{resource.name}</span> : <ResourceChip subject={'capacity' in resource ? { kind: 'host', host: resource } : { kind: 'template', template: resource }} hint={false} />;
}
