/** Template and host settings use authorized names; the containing native row owns every write. */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { hostQuery, hostsQuery, templateQuery, templatesQuery } from '@/api/resources';
import type { Host, Template } from '@/generated/api-types';
import type { ValueControlProps } from '@/features/settings/ValueControl';
import { holds } from '@/lib/permissions';
import { ErrorState } from '@/ui/ErrorState';
import { Select } from '@/ui/Select';
import { ResourceChip, resourceChoice } from '@/features/resource-facts/ResourceFacts';

export function isResourceSetting(key: string | undefined): boolean {
  return key === 'env.template_id' || key === 'runtime.template_id' || key === 'env.host_id' || key === 'runtime.host_id';
}

type ChosenResource = { kind: 'host'; value: Host } | { kind: 'template'; value: Template };

function useChoices(key: string, open: boolean, selected: string) {
  const template = key.endsWith('.template_id');
  const hosts = useQuery({ ...hostsQuery(null), enabled: open && !template });
  const templates = useQuery({ ...templatesQuery(key.startsWith('env.') ? 'environment' : 'provider', null), enabled: open && template });
  const rows = template ? templates.data?.map((row) => ({ id: row.id, name: row.name, subject: { kind: 'template' as const, template: row }, available: row.enabled && holds(row.access, 'template.use') }))
    : hosts.data?.map((row) => ({ id: row.id, name: row.name, subject: { kind: 'host' as const, host: row }, available: row.schedulable && holds(row.access, 'host.use') && row.drivers.some((driver) => driver.creates.includes(key.startsWith('env.') ? 'environment' : 'provider')) }));
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

export function ResourceSettingChoice({ settingKey, value, label, onChange, onEdited, disabled, error, placeholder }: ValueControlProps & { settingKey: string }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const selected = typeof value === 'string' ? value : '';
  const { rows, query, template, initial } = useChoices(settingKey, open, selected);
  const current = rows?.find((row) => row.id === selected);
  const chosen = useChosen(settingKey, selected, initial);
  // Radix does not accept an empty option value; this sentinel never leaves the control.
  const automatic = '\u0000automatic-host';
  return <div className="flex min-w-0 flex-col gap-2">
    <Select label={label} hideLabel size="sm" value={selected === '' && !template ? automatic : selected} open={open && !query.isError && !(template && query.isSuccess && rows?.length === 0)} onOpenChange={setOpen}
      valueLabel={selected === '' ? undefined : current?.name ?? chosen.data?.name ?? t(chosen.isError ? 'settings.resourceChoice.unavailable' : 'common.loading')}
      placeholder={open && query.isPending ? t('common.loading') : placeholder ?? t('settings.resourceChoice.choose')}
      disabled={disabled} error={error}
      options={[
        ...(!template ? [{ value: automatic, label: t('providerRuntime.autoHost') }] : []),
        ...(rows ?? []).filter((row) => row.available || row.id === selected).map((row) => ({ value: row.id, label: row.name, ...resourceChoice(row.subject), disabled: !row.available })),
      ]}
      onValueChange={(next) => { onEdited?.(); onChange(next === automatic ? null : next); }} />
    {query.isError && <ErrorState compact error={query.error} retrying={query.isRefetching} onRetry={() => { setOpen(false); void query.refetch(); }} />}
    {query.isSuccess && !rows?.some((row) => row.available) && <span className="text-sm text-text-muted">{t(template ? 'settings.resourceChoice.noTemplates' : 'settings.resourceChoice.noHosts')}</span>}
  </div>;
}

/** A readable name or an honest unavailable state, never a raw identifier or a guessed private name. */
export function ResourceSettingName({ settingKey, value, example = false }: { settingKey: string; value: unknown; example?: boolean }) {
  const { t } = useTranslation();
  return example ? <span>{t(settingKey.endsWith('.template_id') ? 'settings.resourceChoice.templateExample' : 'settings.resourceChoice.hostExample')}</span>
    : value === null || value === undefined || value === '' ? <span>{t(settingKey.endsWith('.host_id') ? 'providerRuntime.autoHost' : 'settings.compact.none')}</span>
      : <ChosenName settingKey={settingKey} value={value} />;
}

function ChosenName({ settingKey, value }: { settingKey: string; value: unknown }) {
  const { t } = useTranslation();
  const query = useChosen(settingKey, typeof value === 'string' ? value : '');
  if (typeof value !== 'string') return <span>{t('settings.resourceChoice.unavailable')}</span>;
  if (query.isError) return <ErrorState compact title={t('settings.resourceChoice.unavailable')} error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} />;
  const resource = query.data;
  return resource === undefined ? <span>{t('common.loading')}</span>
    : <ResourceChip subject={'capacity' in resource ? { kind: 'host', host: resource } : { kind: 'template', template: resource }} hint={false} />;
}
