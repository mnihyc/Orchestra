/** Where the result works: the same environment, another one the person manages or a new one at a place, with the files that go along into it.
 *
 * Where the result works, as the dialog that starts a thread asks it, in the three ways a thread can be placed: the
 * environment it is in now (its files are shared and nothing is copied), another environment the person manages
 * (the files are copied into it), or a new one at a place of their choosing, with all, some or none of the files.
 * The places list is the hosts the person may use, with how full each is and what it has, and then the existing
 * environments they manage. A host that is full can still be chosen, unless it takes nothing more, and says so.
 */
import { useQuery } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';

import { placesQuery } from '@/api/forks';
import type { Choices, WhereChoice } from '@/features/change/choices';
import { FilesChoice } from '@/features/change/Files';
import { useFocusFirst } from '@/features/change/Sentence';
import type { MoveMachine, Place, PlaceEnvironment, PlaceHost, Thread } from '@/generated/api-types';
import { hostNotes } from '@/features/resources/hosts/words';
import { HostCapacity, ResourceHint, resourceChoice, resourcePreview } from '@/features/resource-facts/ResourceFacts';
import { Checkbox } from '@/ui/Checkbox';
import { EnvironmentDot } from '@/ui/EnvironmentDot';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { UserChip } from '@/ui/UserChip';

const isEnvironment = (place: Place): place is PlaceEnvironment => 'environment' in place;
const isHost = (place: Place): place is PlaceHost => 'automatic' in place;

/** The destination's short name stays in the page; capacity and refusals are read inside its dropdown. */
export function HostChoices({ hosts, value, onChange, loading, machines = [], machine = null, onMachine }: { hosts: readonly PlaceHost[]; value: string | null; onChange: (hostId: string | null) => void; loading: boolean; machines?: readonly MoveMachine[]; machine?: string | null; onMachine?: (id: string | null) => void }) {
  const { t } = useTranslation();
  if (loading) return <p className="text-sm text-text-muted">{t('changeCompact.loadingPlaces')}</p>;
  const automatic = hosts.find((place) => place.automatic);
  return (
    <Select
      label={t('change.where.place')}
      value={machine === null ? value ?? 'automatic' : `machine:${machine}`}
      fieldClassName="w-fit max-w-full"
      onValueChange={(next) => {
        if (next.startsWith('machine:')) { onMachine?.(next.slice('machine:'.length)); return; }
        onMachine?.(null);
        onChange(next === 'automatic' ? null : next);
      }}
      options={[
        {
          value: 'automatic',
          name: automatic === undefined ? t('changeCompact.automatic') : t('changeCompact.automaticHost', { host: automatic.host.name }),
          ...(automatic === undefined ? { description: t('change.where.noHost') } : resourceChoice({ kind: 'host', host: automatic.host, place: automatic })),
          disabled: !hosts.some((place) => place.usable),
        },
        ...hosts.map((place) => {
          const notes = hostNotes(place, t);
          return {
            value: place.host.id,
            name: place.host.name,
            preview: resourcePreview({ kind: 'host', host: place.host, place }),
            description: <span className="flex flex-col gap-1">
              {place.usable ? <ResourceHint subject={{ kind: 'host', host: place.host, place }} /> : <HostCapacity place={place} />}
              {notes.warn !== null && <span className="text-warning">{notes.warn}</span>}
              {notes.off !== null && <span>{notes.off}</span>}
              {!place.usable && notes.off === null && <span>{t('uniformMoves.unavailable')}</span>}
            </span>,
            disabled: !place.usable || notes.off !== null,
          };
        }),
        ...machines.map((place) => ({ value: `machine:${place.machine.id}`, name: place.machine.name,
          description: <span>{t('uniformMoves.directory')}{!place.usable && <>{' '}{place.reasons.length === 0 ? t('uniformMoves.unavailable') : place.reasons.join(' ')}</>}</span>,
          disabled: !place.usable })),
      ]}
    />
  );
}

export interface WhereProps {
  thread: Thread;
  choices: Choices;
  onChange: (patch: Partial<Choices>) => void;
  /** Whether the result is a new thread: a fork of a thread that stays where it is shares nothing with it. */
  fork: boolean;
  focus?: boolean;
}

export function Where({ thread, choices, onChange, fork, focus = false }: WhereProps) {
  const { t } = useTranslation();
  const places = useQuery(placesQuery(thread.topic_id));
  const where = choices.where;
  const set = (next: WhereChoice) => onChange({ where: next });
  const current = thread.environment;
  const ref = useFocusFirst<HTMLDivElement>(focus);

  const list = places.data ?? [];
  const hosts = list.filter(isHost);
  const environments = list.filter(isEnvironment).filter((place) => place.environment.id !== current?.id);

  const kind = where.kind;
  return (
    <div ref={ref} className="flex flex-col gap-3" data-change-where>
      <Select
        label={t('change.where.label')}
        fieldClassName="w-fit max-w-full"
        value={kind}
        onValueChange={(next) => {
          if (next === 'same') set({ kind: 'same' });
          else if (next === 'use') set({ kind: 'use', id: environments.find((place) => place.usable)?.environment.id ?? '', files: { mode: 'all' } });
          else set({ kind: 'new', hostId: null, name: '', files: { mode: 'all' }, home: false, privateViews: true, onlyMe: false });
        }}
        options={[
          { value: 'same', name: current === null ? t('change.where.sameNone') : t(fork ? 'changeCompact.sharedEnvironment' : 'changeCompact.currentEnvironment', { name: current.name }), ...(current === null ? {} : resourceChoice({ kind: 'environment', environment: current })) },
          { value: 'use', name: t('changeCompact.destination.use') },
          { value: 'new', name: t('changeCompact.destination.new') },
        ]}
      />

      {places.isError && <ErrorAlert error={places.error} />}

      {where.kind === 'use' && (
        <div className="flex flex-col gap-3">
          {places.isPending ? (
            <p className="text-sm text-text-muted">{t('changeCompact.loadingPlaces')}</p>
          ) : environments.length === 0 ? (
            <InlineAlert tone="info">{t('change.where.noOther')}</InlineAlert>
          ) : (
            <Select
              label={t('change.where.another')}
              placeholder={t('changeCompact.chooseEnvironment')}
              fieldClassName="w-fit max-w-full"
              value={where.id}
              onValueChange={(id) => set({ ...where, id })}
              options={environments.map((place) => ({
                value: place.environment.id,
                preview: resourcePreview({ kind: 'environment', environment: place.environment, host: place.host }),
                accessory: <UserChip user={place.environment.owner} />,
                name: (
                  <span className="inline-flex items-center gap-2">
                    {place.environment.name}
                    <EnvironmentDot status={place.environment.status} />
                  </span>
                ),
                description: (
                  <span className="flex flex-wrap gap-x-3 gap-y-1">
                    <ResourceHint subject={{ kind: 'environment', environment: place.environment, host: place.host }} />
                    {!place.usable && <span>{place.reasons.length === 0 ? t('uniformMoves.unavailable') : place.reasons.map((reason) => t(`change.where.envReason.${reason}`)).join(t('startFlow.separator'))}</span>}
                  </span>
                ),
                disabled: !place.usable,
              }))}
            />
          )}
          <FilesChoice value={where.files} onChange={(files) => set({ ...where, files })} />
        </div>
      )}

      {where.kind === 'new' && (
        <div className="flex flex-col gap-3">
          <HostChoices hosts={hosts} value={where.hostId} onChange={(hostId) => set({ ...where, hostId })} loading={places.isPending} />
          <Input label={t('change.where.name')} optional hint={t('change.where.nameHint')} value={where.name} onChange={(event) => set({ ...where, name: event.target.value })} autoComplete="off" />
          <FilesChoice value={where.files} onChange={(files) => set({ ...where, files })} />
          <Checkbox label={t('fork.files.home')} checked={where.home} onCheckedChange={(home) => set({ ...where, home })} />
          <Checkbox label={t('fork.files.privateViews')} checked={where.privateViews} onCheckedChange={(privateViews) => set({ ...where, privateViews })} />
          <Checkbox label={t('change.where.onlyMe')} checked={where.onlyMe} onCheckedChange={(onlyMe) => set({ ...where, onlyMe })} />
        </div>
      )}
    </div>
  );
}

/** The environment a choice comes to, as the sentence names it: "Main", "a new environment on host-3". */
export function whereWords(choices: Choices, thread: Thread, hosts: readonly PlaceHost[], environments: readonly PlaceEnvironment[], t: TFunction): string {
  const where = choices.where;
  if (where.kind === 'same') return thread.environment === null ? t('change.where.sameNone') : thread.environment.name;
  if (where.kind === 'use') return environments.find((place) => place.environment.id === where.id)?.environment.name ?? t('change.where.another');
  const host = where.hostId === null ? hosts.find((place) => place.automatic) : hosts.find((place) => place.host.id === where.hostId);
  return host === undefined ? t('change.sentence.inNew') : t('change.sentence.inNewOn', { host: host.host.name });
}
