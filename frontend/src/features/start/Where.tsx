/** Where a thread runs: one compact control (`PlacePicker`) that lists the environments its creator may use, asks for the Use of those they only see, and makes "New environment…" the first row, which opens a small form of the host and the template it is made from, the files it starts with and who manages it.
 *
 * Starting a thread in an environment puts an agent there that reaches all of it. The topic's default, from whichever place
 * sets it, is the one that stands unless the person chose, and the line under the control says where it came from or why it
 * does not stand. Where the agent the person wanted cannot run in the environment that stands, the line says why and what runs
 * instead, and offers the act that lifts it. A directory of one of their connected machines becomes an environment when it is
 * registered. Where nothing may be made, the step says so and how to ask.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { FolderPlus, Laptop, Plus } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';

import { registerEnvironment } from '@/api/environments';
import { ApiError, client } from '@/api/client';
import { startOptionsQuery } from '@/api/start';
import { AccessRequest } from '@/features/access/AccessRequest';
import type { PlaceRef } from '@/features/navigation/places';
import { MakeEnvironmentDialog } from '@/features/resources/environments/MakeEnvironment';
import { BindingPickerScope } from '@/features/resources/PickerContext';
import { ResourcePicker } from '@/ui/ResourcePicker';
import type { BindingEnvironment, Machine } from '@/generated/api-types';
import { useActKey } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { ErrorAlert, MissingAccessList } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { SkeletonRows } from '@/ui/Skeleton';
import { UserChip } from '@/ui/UserChip';
import { useStore } from 'zustand';

import { type AgentHere, canCreateEnvironment, canUseEnvironment, CHOOSE_EXISTING_ENVIRONMENT, type EnvironmentNote, type NewPlacement, NEW_ENVIRONMENT } from './choices';
import type { StartOptions } from '@/api/start';
import { AgentHereLine, EnvironmentNoteLine } from './EnvironmentWords';
import { NewEnvironmentForm } from './NewEnvironment';
import { NEW_PLACE, PlacePicker, type PlaceRow } from './PlacePicker';
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
  newEnvironment?: NewPlacement;
  onNewEnvironmentChange?: (value: NewPlacement) => void;
  /** What the control says of the default environment where the person did not choose one. */
  note?: EnvironmentNote | null;
  /** The profile the person wanted, when the environment that stands shuts it. */
  agentHere?: AgentHere | null;
}

function RegisterDirectory({ machines, loading = false, homeId, onRegistered }: { machines: readonly Machine[]; loading?: boolean; homeId: string; onRegistered: (environmentId: string) => void }) {
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
      onRegistered('environment_id' in environment ? String(environment.environment_id) : environment.id);
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
            <ResourcePicker pickerKey="thread-directory-machine"
              label={t('start.where.machine')}
              value={chosen}
              onValueChange={setMachineId}
              options={online.map((machine) => ({ value: machine.id, label: machine.name, resource:{kind:'machine',id:machine.id,data:machine}, description: machine.owner === null ? undefined : <UserChip user={machine.owner} /> }))}
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

/** An environment the person sees and may not start a thread in: picking it asks for its Use. */
const USE_REQUIRED = 'environment_use_required';

/** A row of the list for one environment, with the reason that sets it apart where it cannot be used at once. */
function rowOf(row: BindingEnvironment, t: TFunction, me: string | null): PlaceRow {
  const usable = canUseEnvironment(row);
  const askable = row.reason === USE_REQUIRED;
  const failed = row.environment.status === 'failed' || row.environment.status === 'removed';
  return {
    id: row.environment.id,
    name: row.environment.name,
    on: row.machine?.name ?? row.host?.name ?? null,
    owner: row.environment.owner.id === me ? null : row.environment.owner.display_name,
    state: usable && row.environment.status !== 'running' ? t(`environments.status.${row.environment.status}`) : null,
    group: usable ? 'ready' : askable ? 'ask' : 'unavailable',
    reason: usable ? null : askable ? t('start.where.askAccess') : failed ? t('startRecovery.failedChoice') : row.reason === null ? t('start.where.closed') : reasonWords(row.reason),
    status: row.environment.status,
    isDefault: row.is_default,
    keywords: [row.environment.address, row.environment.owner.display_name, ...row.topics.map((topic) => topic.name)],
  };
}

export function Where({ compact = false, place, options, value, onChange, machines, machinesLoading = false, homeId, requesting, onRequestingChange, onRefresh, newEnvironment, onNewEnvironmentChange, note = null, agentHere = null }: WhereProps) {
  const { t } = useTranslation();
  const reader = useStore(client.session, (state) => JSON.stringify([state.userId, state.readerVersion ?? 0]));
  const context = JSON.stringify([reader, place.id]);
  const [requestedAt, setRequestedAt] = useState<{ context: string; id: string } | null>(() => requesting === null ? null : { context, id: requesting.environment.id });
  const [granted, setGranted] = useState<{ context: string; id: string } | null>(null);
  const mayMake = options.can_create.allowed;
  const canMakeNew = canCreateEnvironment(options);
  const currentRequest = requesting === null ? null : options.environments.find((row) => row.environment.id === requesting.environment.id) ?? requesting;
  // A refusal identifies who can actually give Use; a visible resource does not imply it is requestable.
  const access = useQuery({ ...startOptionsQuery(place.id, requesting?.environment.id ?? null), enabled: requesting !== null, retry: false });
  const missing = access.error instanceof ApiError ? access.error.extra('authz.denied')?.missing.filter((entry) => entry.object.id === requesting?.environment.id && entry.permissions.includes('environment.use')) ?? [] : [];
  const canAsk = missing.some((entry) => entry.closed === null && (entry.ask.length > 0 || entry.ask_more > 0));
  const available = access.data?.environments.find((row) => row.environment.id === requesting?.environment.id);
  const acquired = requesting !== null && requestedAt?.context === context && requestedAt.id === requesting.environment.id
    && !access.isFetching && !access.isError && available !== undefined && canUseEnvironment(available) ? available : undefined;
  if (acquired !== undefined && (granted?.context !== context || granted.id !== acquired.environment.id)) setGranted({ context, id: acquired.environment.id });
  if (acquired === undefined && granted !== null && (granted.context !== context || granted.id !== value)) setGranted(null);
  useEffect(() => {
    if (acquired !== undefined) {
      onRequestingChange(null);
      onChange(acquired.environment.id);
    }
  }, [acquired, onRequestingChange, onChange]);
  // The default comes first; the rest keep the order the server gave them.
  const listed = [...options.environments].sort((a, b) => Number(b.is_default) - Number(a.is_default));
  const me = useStore(client.session, (state) => state.userId);
  const rows = listed.map((row) => rowOf(row, t, me));
  const selectedId = currentRequest?.environment.id ?? value;
  const fresh = currentRequest === null && value === NEW_ENVIRONMENT;
  const selected = listed.find((row) => row.environment.id === selectedId) ?? null;
  const pickExisting = (id: string) => {
    const row = options.environments.find((each) => each.environment.id === id);
    if (row === undefined) return;
    setGranted(null);
    if (row.reason === USE_REQUIRED) { setRequestedAt({ context, id }); onRequestingChange(row); return; }
    if (!canUseEnvironment(row)) return;
    setRequestedAt(null);
    onRequestingChange(null); onChange(id);
  };
  const chooseNew = () => { setGranted(null); setRequestedAt(null); onRequestingChange(null); onChange(NEW_ENVIRONMENT); };
  const standing = fresh ? null : selected === null ? null : selected.environment.name;
  const shownValue = fresh ? NEW_PLACE : currentRequest !== null ? currentRequest.environment.id : value === CHOOSE_EXISTING_ENVIRONMENT ? null : value;
  return <BindingPickerScope options={options} screen="thread-environment" currentHomes={[place.id,...(place.parentId===null?[]:[place.parentId])]}>{(
    <div className="flex flex-col gap-2" data-start-where>
      <PlacePicker label={t('start.where.title')} value={shownValue} rows={rows} onPick={pickExisting}
        {...(mayMake ? { newRow: { onNew: chooseNew, blocked: canMakeNew ? null : t('start.where.noHost') } } : {})} fieldClassName="min-w-0 max-w-full" />
      {granted?.context === context && granted.id === value && <span role="status" className="text-sm text-text-muted">{t('startImplicit.grantedSelected')}</span>}
      {listed.length === 0 && (!mayMake || !canMakeNew) && <p className="text-sm text-text-muted">{t('start.where.noExisting')}</p>}
      {mayMake && !canMakeNew && <p role="status" className="text-sm text-text-muted">{t('start.where.noHost')}</p>}
      {!mayMake && <p className="text-sm text-text-muted">{t('start.where.mayNotMake')}</p>}
      {note !== null && <EnvironmentNoteLine note={note} standing={standing} fresh={fresh} />}
      {agentHere !== null && <AgentHereLine here={agentHere} environment={fresh ? null : selected} onChanged={() => onRefresh?.()} />}
      {fresh && canMakeNew && onNewEnvironmentChange !== undefined && <NewEnvironmentForm place={place} hosts={options.hosts} value={newEnvironment} onChange={onNewEnvironmentChange} />}
      {currentRequest !== null && access.isPending && <SkeletonRows read={access} rows={1} />}
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
      {!compact && <WhereOptions place={place} options={options} value={value} onChange={(id) => { setGranted(null); setRequestedAt(null); onRequestingChange(null); onChange(id); }} machines={machines} machinesLoading={machinesLoading} homeId={homeId} />}
    </div>
  )}</BindingPickerScope>;
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
      <RegisterDirectory machines={machines} loading={machinesLoading} homeId={homeId} onRegistered={(environmentId) => onChange(environmentId)} />
      {making && <MakeEnvironmentDialog forThread home={{ id: place.id, name: place.name }} onClose={() => setMaking(false)} onMade={(environment) => onChange(environment.id)} />}
  </>;
}
