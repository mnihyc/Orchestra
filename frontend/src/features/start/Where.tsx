/** Where a thread runs: an existing environment its creator may use (the topic's first, every other under More), or a new one with template, place, storage, mounts, starting files and who else manages it.
 *
 * Where a thread works: an environment the person may use (the topic's own first, with its default
 * marked, as the server lists them; those they only see follow, set apart, and picking one asks for its Use), a new one that is made for the thread from the defaults of the place, or one
 * made here by the form that the resources screens use, or a directory of one of their connected machines, which
 * becomes an environment when it is registered. Starting a thread in an environment puts an agent there that
 * reaches all of it, so each row says whose it is, where, how it stands and who already works in it; one the person
 * may not use is shown with the server's reason. Where nothing may be made, the step says so and how to ask.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { FolderPlus, Laptop, Plus } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { choiceOption } from '@/i18n/choices';

import { registerEnvironment } from '@/api/environments';
import { ApiError } from '@/api/client';
import { startOptionsQuery } from '@/api/start';
import { AccessRequest } from '@/features/access/AccessRequest';
import type { PlaceRef } from '@/features/navigation/places';
import { ResourceHint, ResourceName, resourcePreview } from '@/features/resource-facts/ResourceFacts';
import { hostNotes } from '@/features/resources/hosts/words';
import { MakeEnvironmentDialog } from '@/features/resources/environments/MakeEnvironment';
import type { BindingEnvironment, Environment, Machine, PlaceHost } from '@/generated/api-types';
import { useActKey } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { ErrorAlert, MissingAccessList } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { UserChip } from '@/ui/UserChip';

import { canUseEnvironment, NEW_ENVIRONMENT } from './choices';
import type { StartOptions } from '@/api/start';
import { reasonWords } from './words';

export interface WhereProps {
  compact?: boolean;
  /** The topic, or the project where the first thread of a topic made there is asked about. */
  place: PlaceRef;
  options: StartOptions;
  /** The environment that is chosen, or `NEW_ENVIRONMENT`; none while nothing is. */
  value: string | null;
  onChange: (id: string) => void;
  machines: readonly Machine[];
  machinesLoading?: boolean;
  /** The org that owns what is registered here. */
  homeId: string;
  /** A visible environment awaiting Use; it is not a valid placement yet. */
  requesting: BindingEnvironment | null;
  onRequestingChange: (environment: BindingEnvironment | null) => void;
  /** Read current choices after a request changes. */
  onRefresh?: () => void;
}

function RegisterDirectory({ machines, loading = false, homeId, onRegistered }: { machines: readonly Machine[]; loading?: boolean; homeId: string; onRegistered: (environment: Environment) => void }) {
  const { t } = useTranslation();
  const actKey = useActKey();
  const online = machines.filter((machine) => machine.online);
  const [machineId, setMachineId] = useState('');
  const [root, setRoot] = useState('');
  const [name, setName] = useState('');
  const chosen = machineId === '' ? (online[0]?.id ?? '') : machineId;
  const register = useMutation({
    mutationFn: () => registerEnvironment(chosen, { name: name.trim() === '' ? root.trim() : name.trim(), root: root.trim(), home_id: homeId }, actKey({ chosen, root, name, homeId })),
    onSuccess: (environment) => {
      onRegistered(environment);
      setRoot('');
      setName('');
    },
  });
  if (loading) return <p role="status" className="text-sm text-text-muted">{t('startFlow.machinesLoading')}</p>;
  if (machines.length === 0) {
    return <EmptyState compact title={t('start.where.noMachines')} />;
  }
  return (
    <details className="rounded-lg border border-border px-3 py-2">
      <summary className="flex cursor-default items-center gap-2 rounded-md text-sm font-medium text-text select-none">
        <Laptop aria-hidden /> {t('start.where.machineTitle')}
      </summary>
      <form
        className="mt-2 flex flex-col gap-3 pb-1"
        onSubmit={(event) => {
          event.preventDefault();
          if (root.trim() !== '' && chosen !== '') register.mutate();
        }}
      >
        <p className="text-sm text-text-muted">{t('start.where.machineAbout')}</p>
        {online.length === 0 ? (
          <InlineAlert tone="warning">{t('start.where.allOffline')}</InlineAlert>
        ) : (
          <>
            <Select
              label={t('start.where.machine')}
              value={chosen}
              onValueChange={setMachineId}
              options={online.map((machine) => ({ value: machine.id, label: machine.name, description: machine.owner === null ? undefined : <UserChip user={machine.owner} /> }))}
            />
            <Input label={t('start.where.root')} hint={t('start.where.rootHint')} value={root} onChange={(event) => setRoot(event.target.value)} autoComplete="off" />
            <Input label={t('start.where.envName')} optional value={name} onChange={(event) => setName(event.target.value)} autoComplete="off" />
            {register.error !== null && <ErrorAlert error={register.error} />}
            <Button type="submit" size="sm" icon={<FolderPlus aria-hidden />} pending={register.isPending} disabled={root.trim() === ''} className="self-start">
              {t('start.where.use')}
            </Button>
          </>
        )}
      </form>
    </details>
  );
}

/**
 * The hosts a new environment could be made on, with what each has free: the one the scheduler would take first, and
 * one that cannot be chosen with why. The figures are the server's, read when the choices were.
 */
function Hosts({ hosts }: { hosts: readonly PlaceHost[] }) {
  const { t } = useTranslation();
  if (hosts.length === 0) return null;
  // The same words as where a thread is moved: why a host cannot take the environment, or what to know before it does.
  const notes = (place: PlaceHost) => hostNotes(place, t);
  return (
    <ul className="flex flex-col gap-1 text-xs text-text-muted" aria-label={t('start.where.hosts')} data-start-hosts>
      {hosts.map((place) => (
        <li key={place.host.id} className="flex flex-wrap items-center gap-x-2">
          <ResourceName subject={{ kind: 'host', host: place.host, place }} className="flex-wrap font-medium text-text" />
          {place.automatic && <span className="rounded-full border border-accent/40 px-2 text-accent-text">{t('start.where.hostChosen')}</span>}
          {!place.usable && <span>{notes(place).off ?? t('start.where.hostClosed')}</span>}
          {place.usable && notes(place).warn !== null && <span>{notes(place).warn}</span>}
        </li>
      ))}
    </ul>
  );
}

/** Who else works in an environment, in a few words: how many threads, and the other topics they are in. */
function usedBy(row: BindingEnvironment, t: TFunction): string | null {
  const others = row.used === 'topic' ? [] : row.topics.map((topic) => topic.name).filter((name): name is string => name !== null);
  const names = others.slice(0, 2).join(', ');
  const rest = others.length - 2 + row.topics_hidden;
  const where = names === '' ? null : rest > 0 ? t('start.where.alsoUsedByMore', { names, count: rest }) : t('start.where.alsoUsedBy', { names });
  const threads = row.threads > 0 ? t('start.where.threads', { count: row.threads }) : null;
  return [threads, where].filter((part): part is string => part !== null).join(t('startFlow.separator')) || null;
}

/** An environment the person sees and may not start a thread in: picking it asks for its Use. */
const USE_REQUIRED = 'environment_use_required';

export function Where({ compact = false, place, options, value, onChange, machines, machinesLoading = false, homeId, requesting, onRequestingChange, onRefresh }: WhereProps) {
  const { t } = useTranslation();
  const mayMake = options.can_create.allowed;
  const currentRequest = requesting === null ? null : options.environments.find((row) => row.environment.id === requesting.environment.id) ?? requesting;
  // A refusal identifies who can actually give Use; a visible resource does not imply it is requestable.
  const access = useQuery({ ...startOptionsQuery(place.id, requesting?.environment.id ?? null), enabled: requesting !== null, retry: false, staleTime: 0 });
  const missing = access.error instanceof ApiError ? access.error.extra('authz.denied')?.missing.filter((entry) => entry.object.id === requesting?.environment.id && entry.permissions.includes('environment.use')) ?? [] : [];
  const canAsk = missing.some((entry) => entry.closed === null && (entry.ask.length > 0 || entry.ask_more > 0));
  useEffect(() => {
    const granted = access.data?.environments.find((row) => row.environment.id === requesting?.environment.id);
    if (requesting !== null && !access.isFetching && !access.isError && granted !== undefined && canUseEnvironment(granted)) {
      onRequestingChange(null);
      onChange(granted.environment.id);
    }
  }, [requesting, access.data, access.isFetching, access.isError, onRequestingChange, onChange]);
  // What may be used comes first; what may be asked for follows, set apart; what cannot be had now, last.
  const order = (row: BindingEnvironment) => (canUseEnvironment(row) ? 0 : row.reason === USE_REQUIRED ? 1 : 2);
  const listed = [...options.environments].sort((a, b) => order(a) - order(b));
  const grouped = listed.some((row) => order(row) === 0) && listed.some((row) => order(row) === 1);
  const rows = [
    ...listed.map((row) => {
      const used = usedBy(row, t);
      const usable = canUseEnvironment(row);
      const askable = row.reason === USE_REQUIRED;
      const refusal = usable || askable ? null : row.environment.status === 'failed' || row.environment.status === 'removed' ? t('startRecovery.failedChoice') : row.reason === null ? t('start.where.closed') : reasonWords(row.reason);
      return {
        value: row.environment.id,
        preview: resourcePreview({ kind: 'environment', environment: row.environment, host: row.host }),
        accessory: <UserChip user={row.environment.owner} />,
        name: row.environment.name,
        group: grouped ? usable ? t('start.where.canUse') : askable ? t('start.where.askAccess') : undefined : undefined,
        description: (
          <span className="flex flex-wrap items-center gap-x-3 gap-y-1">
            {row.is_default && usable && <span>{t('start.where.topicDefault')}</span>}
            <ResourceHint subject={{ kind: 'environment', environment: row.environment, host: row.host }} />
            {used !== null && !usable && <span>{used}</span>}
            {askable && !grouped && <span className="text-accent-text">{t('start.where.askAccess')}</span>}
            {refusal !== null && <span className="text-warning-text">{refusal}</span>}
          </span>
        ),
        disabled: !usable && !askable,
      };
    }),
    ...(mayMake
      ? [
          {
            value: NEW_ENVIRONMENT,
            ...choiceOption(t, 'environmentDestination.new'),
            icon: <Plus aria-hidden />,
            description: (
              <span className="flex flex-col gap-1">
                <span>{t('start.where.newAbout', { place: place.name })}</span>
                <Hosts hosts={options.hosts} />
              </span>
            ),
          },
        ]
      : []),
  ];
  return (
    <div className="flex flex-col gap-3">
      {rows.length === 0 ? (
        <EmptyState compact title={t('start.where.none')} />
      ) : (
        <Select
          label={t('start.where.title')}
          hideLabel
          value={currentRequest?.environment.id ?? value ?? ''}
          onValueChange={(id) => {
            const row = options.environments.find((each) => each.environment.id === id);
            if (row !== undefined && row.reason === USE_REQUIRED) {
              onRequestingChange(row);
              return;
            }
            onRequestingChange(null);
            onChange(id);
          }}
          options={rows}
        />
      )}
      {currentRequest !== null && access.isPending && <SkeletonRows rows={1} />}
      {currentRequest !== null && missing.length > 0 && !canAsk && <MissingAccessList missing={missing} />}
      {currentRequest !== null && canAsk && (
        <AccessRequest
          key={currentRequest.environment.id}
          fixed
          peopleAsChips
          objectId={currentRequest.environment.id}
          rememberAs={`${currentRequest.environment.id}.environment.use`}
          onChanged={() => { void access.refetch(); onRefresh?.(); }}
          missing={missing}
        />
      )}
      {currentRequest !== null && access.isError && missing.length === 0 && <ErrorAlert error={access.error} action={<Button size="sm" onClick={() => void access.refetch()}>{t('common.tryAgain')}</Button>} />}
      {!compact && <WhereOptions place={place} options={options} value={value} onChange={(id) => { onRequestingChange(null); onChange(id); }} machines={machines} machinesLoading={machinesLoading} homeId={homeId} />}
    </div>
  );
}

/** Extra creation and registration acts live with the dialog's optional setup. */
export function WhereOptions({ place, options, onChange, machines, machinesLoading = false, homeId }: Omit<WhereProps, 'requesting' | 'onRequestingChange' | 'onRefresh'>) {
  const { t } = useTranslation();
  const [making, setMaking] = useState(false);
  return <>
      {options.can_create.allowed ? (
        <Button size="sm" icon={<Plus aria-hidden />} onClick={() => setMaking(true)} className="self-start">
          {t('start.where.makeOne')}
        </Button>
      ) : (
        <details className="text-sm text-text-muted">
          <summary className="cursor-default rounded-md select-none hover:text-text">{t('start.where.mayNotMake')}</summary>
          <div className="mt-2">
            <AccessRequest missing={options.can_create.missing} objectId={options.can_create.missing[0]?.object.id ?? place.id} fixed />
          </div>
        </details>
      )}
      <RegisterDirectory machines={machines} loading={machinesLoading} homeId={homeId} onRegistered={(environment) => onChange(environment.id)} />
      {making && <MakeEnvironmentDialog forThread home={{ id: place.id, name: place.name }} onClose={() => setMaking(false)} onMade={(environment) => onChange(environment.id)} />}
  </>;
}
