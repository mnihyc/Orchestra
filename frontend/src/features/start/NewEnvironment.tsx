/** The same searchable host and template fields wherever work is placed. */
import { useQuery } from '@tanstack/react-query';
import { useStore } from 'zustand';
import { type KeyboardEvent, useState } from 'react';
import { isComposing } from '@/lib/shortcuts';
import { useTranslation } from 'react-i18next';

import { placesQuery } from '@/api/forks';
import { templatesQuery } from '@/api/resources';
import { client } from '@/api/client';
import { poolQuery } from '@/api/pools';
import { usePoolTargetChoices } from '@/features/resources/pools/targetChoices';
import { HOST_POOL_PREFIX, hostChoiceTarget, hostPoolId, singleHostId } from '@/features/resources/pools/hostTarget';
import { ResourceHint } from '@/features/resource-facts/ResourceFacts';
import { hostUnavailable, templateUnavailable } from '@/features/resources/choiceAvailability';
import { hostNotes } from '@/features/resources/hosts/words';
import { NewAccess } from '@/features/resources/NewAccess';
import type { PlaceRef } from '@/features/navigation/places';
import type { Host, Place, PlaceHost } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { Combobox, type ComboboxOption } from '@/ui/Combobox';
import { ErrorAlert } from '@/ui/ErrorState';

import type { NewPlacement } from './choices';
import { StartingFiles } from './StartingFiles';

const MOST_ROOM = '\u0000most-room';
const DEFAULT_HOST = '\u0000default-host';
const PLACE_TEMPLATE = '\u0000place-template';
const isHost = (place: Place): place is PlaceHost => 'automatic' in place;

export interface PlacementFieldsProps {
  hosts: readonly (PlaceHost | Host)[];
  value: Pick<NewPlacement, 'host_id' | 'template_id'> | undefined;
  onChange: (value: Pick<NewPlacement, 'host_id' | 'template_id'>) => void;
  kind?: 'environment' | 'provider';
  disabled?: boolean;
  pending?: boolean;
  failed?: unknown;
  onRetry?: () => void;
  defaultTemplate?: string;
}

export function PlacementFields({ hosts, value, onChange, kind = 'environment', disabled = false, pending = false, failed, onRetry, defaultTemplate }: PlacementFieldsProps) {
  const { t, i18n } = useTranslation();
  const templates = useQuery(templatesQuery(kind, null));
  const [hostQuery, setHostQuery] = useState('');
  const [hostOpen, setHostOpen] = useState(false);
  const reader = useStore(client.session, (state) => `${state.userId ?? ''}:${state.readerVersion ?? 0}`);
  const pools = usePoolTargetChoices('host', hostOpen, hostQuery);
  const [templateQuery, setTemplateQuery] = useState('');
  const host = value?.host_id;
  const hostId = singleHostId(host), poolId = hostPoolId(host);
  const selectedPool = useQuery({ ...poolQuery(poolId ?? '', reader), enabled: poolId !== undefined, retry: false });
  const poolName = pools.rows.find((row) => row.id === poolId)?.name ?? selectedPool.data?.name;
  const template = value?.template_id ?? null;
  const hostOf = (entry: PlaceHost | Host) => 'automatic' in entry ? entry.host : entry;
  const chosenHost = hostId === undefined ? undefined : hosts.find((entry) => hostOf(entry).id === hostId);
  const chosenName = chosenHost === undefined ? undefined : hostOf(chosenHost).name;
  const [remembered, setRemembered] = useState<{ id: string; name: string; reader: string } | null>(null);
  if (hostId !== undefined && chosenName !== undefined && (remembered?.id !== hostId || remembered.name !== chosenName || remembered.reader !== reader)) {
    setRemembered({ id: hostId, name: chosenName, reader });
  }
  const automatic = hosts.find((entry): entry is PlaceHost => 'automatic' in entry && entry.automatic);
  const autoName = automatic === undefined ? t('start.where.hostAuto') : t('start.where.hostAutoNamed', { host: automatic.host.name });
  const goneName = remembered !== null && remembered.id === hostId && remembered.reader === reader ? remembered.name : t('start.where.hostGone');
  const poolLabel = poolName ?? t(selectedPool.isPending ? 'common.loading' : 'resourceGroups.selectionUnavailable');
  const selectedValue = poolId === undefined ? hostId : `${HOST_POOL_PREFIX}${poolId}`;
  const defaultHost = t('start.where.hostDefault');
  const matches = (name: string, query: string) => name.normalize('NFKC').toLocaleLowerCase(i18n.language).includes(query.trim().normalize('NFKC').toLocaleLowerCase(i18n.language));
  const hostOptions: ComboboxOption<string>[] = [
    { value: DEFAULT_HOST, label: defaultHost, text: defaultHost },
    ...(kind === 'environment' ? [{ value: MOST_ROOM, label: autoName, text: autoName,
      description: automatic === undefined ? t('start.where.noHost') : <ResourceHint subject={{ kind: 'host', host: automatic.host, place: automatic }} />,
      disabled: !hosts.some((entry) => 'automatic' in entry ? entry.usable : hostUnavailable(entry, t, kind) === null) }] : []),
    ...hosts.map((entry) => {
      if (!('automatic' in entry)) {
        const driver = (templates.data ?? []).find((row) => row.id === (template ?? defaultTemplate))?.driver;
        const refusal = hostUnavailable(entry, t, kind, driver);
        return { value: entry.id, label: entry.name, text: entry.name, disabled: refusal ?? false,
          description: refusal ?? <ResourceHint subject={{ kind: 'host', host: entry }} /> };
      }
      const notes = hostNotes(entry, t);
      return { value: entry.host.id, label: entry.host.name, text: entry.host.name,
        description: notes.off ?? notes.warn ?? <ResourceHint subject={{ kind: 'host', host: entry.host, place: entry }} />,
        disabled: entry.usable ? false : notes.off ?? t('start.where.hostClosed') };
    }),
    ...(hostId !== undefined && chosenHost === undefined ? [{ value: hostId, label: goneName, text: goneName, disabled: t('start.where.hostGone') }] : []),
  ];
  const inheritedName = (templates.data ?? []).find((entry) => entry.id === defaultTemplate)?.name;
  const defaultName = [t('start.where.templateDefault'), inheritedName].filter(Boolean).join(t('startFlow.separator'));
  const templateOptions: ComboboxOption<string>[] = [
    { value: PLACE_TEMPLATE, label: defaultName, text: defaultName },
    ...(templates.data ?? []).map((entry) => ({ value: entry.id, label: entry.name, text: entry.name, disabled: templateUnavailable(entry, t) ?? false })),
  ];
  const chosenTemplate = templateOptions.find((entry) => entry.value === template);
  const keepSearchKey = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'Enter' || !(event.target instanceof HTMLInputElement) || event.target.getAttribute('role') !== 'combobox') return;
    if (isComposing(event.nativeEvent)) event.stopPropagation(); else event.preventDefault();
  };
  return <div className="contents" onKeyDownCapture={keepSearchKey}>
    <div className="flex min-w-0 flex-wrap items-start gap-3">
      <Combobox<string> label={t('start.where.host')} fieldClassName="min-w-0 basis-(--field-narrow) flex-1" disabled={disabled}
        query={hostQuery} onQueryChange={setHostQuery} displayValue={host === undefined ? defaultHost : host === null ? autoName : poolId !== undefined ? poolLabel : chosenName ?? goneName}
        initialActiveValue={host === undefined ? DEFAULT_HOST : host === null ? MOST_ROOM : selectedValue} loading={pending || pools.loading}
        groups={[{ id: 'hosts', label: '', options: hostOptions.filter((entry) => matches(entry.text, hostQuery)) },
          { id: 'pools', label: t('screens.pools.title'), options: [...pools.options,
            ...(poolId !== undefined && !pools.rows.some((row) => row.id === poolId) ? [{ value: `${HOST_POOL_PREFIX}${poolId}`, label: poolLabel, text: poolLabel, disabled: t('resourceGroups.selectionUnavailable') }] : [])].filter((entry) => matches(entry.text, hostQuery)) }]}
        footer={pools.footer} empty={t('start.where.noMatches')} onOpenChange={(open) => { setHostOpen(open); if (!open) setHostQuery(''); }}
        onSelect={(entry) => { const { host_id: _host, ...kept } = value ?? {}; onChange(entry.value === DEFAULT_HOST ? kept : { ...kept, host_id: entry.value === MOST_ROOM ? null : hostChoiceTarget(entry.value) }); setHostQuery(''); }} />
      <Combobox<string> label={t('start.where.template')} fieldClassName="min-w-0 basis-(--field-narrow) flex-1" disabled={disabled}
        query={templateQuery} onQueryChange={setTemplateQuery} displayValue={template === null ? defaultName : chosenTemplate?.text ?? template}
        initialActiveValue={template ?? PLACE_TEMPLATE} loading={templates.isPending}
        groups={[{ id: 'templates', label: '', options: templateOptions.filter((entry) => matches(entry.text, templateQuery)) }]}
        empty={t('start.where.noMatches')} onOpenChange={(open) => { if (!open) setTemplateQuery(''); }}
        onSelect={(entry) => { const { template_id: _template, ...kept } = value ?? {}; onChange(entry.value === PLACE_TEMPLATE ? kept : { ...kept, template_id: entry.value }); setTemplateQuery(''); }} />
    </div>
    {hostId !== undefined && chosenHost === undefined && !pending && !failed && <span role="status" className="text-sm text-text-muted">{t('start.where.hostGone')}</span>}
    {failed !== undefined && failed !== null && <ErrorAlert error={failed} action={onRetry === undefined ? undefined : <Button size="sm" onClick={onRetry}>{t('common.tryAgain')}</Button>} />}
    {templates.isError && <ErrorAlert error={templates.error} action={<Button size="sm" onClick={() => void templates.refetch()}>{t('common.tryAgain')}</Button>} />}
  </div>;
}

export interface NewEnvironmentFormProps {
  place: PlaceRef;
  hosts: readonly PlaceHost[];
  value: NewPlacement | undefined;
  onChange: (value: NewPlacement) => void;
  disabled?: boolean;
  /** The binding preview already reads hosts for the selected template. */
  assessed?: boolean;
}

export function NewEnvironmentForm({ place, hosts, value, onChange, disabled = false, assessed = false }: NewEnvironmentFormProps) {
  const { t } = useTranslation();
  const template = value?.template_id ?? null;
  const reread = useQuery({ ...placesQuery(place.id, template), enabled: !assessed && template !== null });
  const offered = assessed || template === null ? hosts : (reread.data ?? []).filter(isHost);
  return <div className="flex min-w-0 flex-col gap-3" data-new-environment>
    <PlacementFields hosts={offered} value={value} disabled={disabled} pending={!assessed && template !== null && reread.isPending}
      failed={!assessed && template !== null ? reread.error : null} onRetry={() => void reread.refetch()}
      onChange={(placement) => { const { host_id: _host, template_id: _template, ...kept } = value ?? {}; onChange({ ...kept, ...placement }); }} />
    <StartingFiles value={value?.files} disabled={disabled} onChange={(files) => onChange({ ...value, files })} />
    <NewAccess label={t('start.where.managers')} value={value?.access ?? 'defaults'} home={place.name} allowHome={false} disabled={disabled}
      onChange={(access) => { if (access !== 'home') onChange({ ...value, access }); }} />
  </div>;
}
