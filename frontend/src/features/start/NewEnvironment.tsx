/** What a new environment is made from, chosen where it is asked for: the host, the template, the files it starts with and who manages it.
 *
 * The host is a menu of the hosts that could take it (each with what it has free, measured, and why one cannot be chosen);
 * left on the first entry the scheduler takes the one with the most room. The template is a menu of the templates the
 * person may use, shown only where there is a choice; picking one reads the hosts for it again.
 */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';

import { placesQuery } from '@/api/forks';
import { templatesQuery } from '@/api/resources';
import { ResourceHint } from '@/features/resource-facts/ResourceFacts';
import { templateUnavailable } from '@/features/resources/choiceAvailability';
import { hostNotes } from '@/features/resources/hosts/words';
import { NewAccess } from '@/features/resources/NewAccess';
import type { PlaceRef } from '@/features/navigation/places';
import type { Place, PlaceHost } from '@/generated/api-types';
import { Input } from '@/ui/Input';
import { Select, type SelectOption } from '@/ui/Select';

import type { NewPlacement } from './choices';
import { StartingFiles } from './StartingFiles';

/** The value of the host entry that stands for "the one with the most room": a menu cannot hold an empty value. */
const MOST_ROOM = '\u0000most-room';
/** The same for the template the place itself resolves. */
const PLACE_TEMPLATE = '\u0000place-template';

const isHost = (place: Place): place is PlaceHost => 'automatic' in place;

export interface NewEnvironmentFormProps {
  place: PlaceRef;
  /** The hosts the answer lists for the place's own template. */
  hosts: readonly PlaceHost[];
  value: NewPlacement | undefined;
  onChange: (value: NewPlacement) => void;
  disabled?: boolean;
}

export function NewEnvironmentForm({ place, hosts, value, onChange, disabled = false }: NewEnvironmentFormProps) {
  const { t } = useTranslation();
  const templates = useQuery(templatesQuery('environment', null));
  const usable = (templates.data ?? []).filter((entry) => templateUnavailable(entry, t) === null);
  const template = value?.template_id ?? null;
  const reread = useQuery({ ...placesQuery(place.id, template), enabled: template !== null });
  const offered: readonly PlaceHost[] = template === null ? hosts : (reread.data ?? []).filter(isHost);
  const automatic = offered.find((entry) => entry.automatic);
  const host = value?.host_id ?? null;
  const chosenHost = host === null ? undefined : offered.find((entry) => entry.host.id === host);
  const hostOptions: SelectOption<string>[] = [
    {
      value: MOST_ROOM,
      name: automatic === undefined ? t('start.where.hostAuto') : t('start.where.hostAutoNamed', { host: automatic.host.name }),
      description: automatic === undefined ? t('start.where.noHost') : <ResourceHint subject={{ kind: 'host', host: automatic.host, place: automatic }} wrap />,
      disabled: !offered.some((entry) => entry.usable),
    },
    ...offered.map((entry) => {
      const notes = hostNotes(entry, t);
      const refusal = entry.usable ? null : notes.off ?? t('start.where.hostClosed');
      return {
        value: entry.host.id,
        name: entry.host.name,
        description: <span className="flex flex-col gap-0.5">
          <ResourceHint subject={{ kind: 'host', host: entry.host, place: entry }} wrap />
          {refusal !== null && <span>{refusal}</span>}
          {entry.usable && notes.warn !== null && <span className="text-warning-text">{notes.warn}</span>}
        </span>,
        disabled: !entry.usable,
      };
    }),
  ];
  const withoutHost = (next: NewPlacement): NewPlacement => {
    const { host_id: _gone, ...kept } = next;
    return kept;
  };
  return <div className="flex min-w-0 flex-col gap-3" data-new-environment>
    <Input label={t('start.where.environmentName')} optional placeholder={t('start.where.environmentNameDefault')} value={value?.name ?? ''} disabled={disabled}
      onChange={(event) => onChange({ ...value, name: event.target.value })} />
    <div className="flex min-w-0 flex-wrap items-start gap-3">
      <Select<string> label={t('start.where.host')} fieldClassName="min-w-(--field-narrow) flex-1" disabled={disabled}
        value={chosenHost === undefined ? MOST_ROOM : chosenHost.host.id} options={hostOptions}
        onValueChange={(next) => {
          if (next === MOST_ROOM) onChange(withoutHost(value ?? {}));
          else onChange({ ...value, host_id: next });
        }} />
      {usable.length > 1 && <Select<string> label={t('start.where.template')} fieldClassName="min-w-(--field-narrow) flex-1" disabled={disabled}
        value={template ?? PLACE_TEMPLATE}
        options={[
          { value: PLACE_TEMPLATE, name: t('start.where.templateDefault') },
          ...usable.map((entry) => ({ value: entry.id, name: entry.name, ...(entry.description === '' ? {} : { description: entry.description }) })),
        ]}
        onValueChange={(next) => {
          // The hosts that fit one template may not fit another: the host goes back to the one with the most room.
          const { host_id: _gone, template_id: _was, ...kept } = value ?? {};
          onChange(next === PLACE_TEMPLATE ? kept : { ...kept, template_id: next });
        }} />}
    </div>
    <StartingFiles value={value?.files} disabled={disabled} onChange={(files) => onChange({ ...value, files })} />
    <NewAccess label={t('start.where.managers')} value={value?.access ?? 'defaults'} home={place.name} allowHome={false} disabled={disabled}
      onChange={(access) => { if (access !== 'home') onChange({ ...value, access }); }} />
  </div>;
}
