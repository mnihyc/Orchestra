/** Native node-scoped fleet moves retain one plan, audience and retry identity per selected home. */
import { useMutation, useQueries, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client, ApiError } from '@/api/client';
import { planNodeMoves, startNodeMoves } from '@/api/changes';
import { forkKeys, jobQuery, planGone } from '@/api/forks';
import { hostsQuery, machinesQuery, templatesQuery } from '@/api/resources';
import { blockerWords } from '@/features/change/plan';
import { moveLists } from '@/features/change/EnvironmentChange';
import { PlanLists } from '@/features/change/Lists';
import { Time } from '@/features/change/Time';
import { transferFacts } from '@/features/change/environmentTransfer';
import { openChange } from '@/features/change/target';
import { JobProgress } from '@/features/change/Progress';
import { MeasurementReason, Measurements } from '@/features/change/Measurements';
import { NotifyPeople } from '@/features/notifications/NotifyPeople';
import { usePlaceLabel } from '@/features/navigation/placeLabels';
import { ResourceChip, resourceChoice } from '@/features/resource-facts/ResourceFacts';
import type { Environment, EnvironmentBatch, EnvironmentMovePlan, EnvironmentRef, ForkCreateBody, NodeEnvironmentMoveCreateReceipt } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';
import { useActKey } from '@/lib/forms';
import { useNow } from '@/lib/time';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { ResourcePicker } from '@/ui/ResourcePicker';
import { Popover } from '@/ui/Popover';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { Table } from '@/ui/Table';
import { EnvironmentBatchProgress } from './MoveDialog';
import { RunningTurnChoice } from './UpdateDialog';

export interface MoveGroup { home: Environment['home']; environments: Environment[] }
export function environmentMoveGroups(environments: readonly Environment[]): MoveGroup[] {
  const groups = new Map<string, MoveGroup>();
  const seen = new Set<string>();
  for (const environment of environments) {
    if (seen.has(environment.id)) continue;
    seen.add(environment.id);
    const group = groups.get(environment.home.id) ?? { home: environment.home, environments: [] };
    group.environments.push(environment); groups.set(environment.home.id, group);
  }
  return [...groups.values()];
}

export function NodeMovesDialog({ environments, onClose }: { environments: readonly Environment[]; onClose: () => void }) {
  const reader = useStore(client.session, (state) => state.readerVersion);
  return <NodeMoves key={reader} environments={environments} reader={reader} onClose={onClose} />;
}
function NodeMoves({ environments, reader, onClose }: { environments: readonly Environment[]; reader: number; onClose: () => void }) {
  const { t } = useTranslation();
  const placeLabel = usePlaceLabel();
  const groups = environmentMoveGroups(environments);
  const [selected, setSelected] = useState(groups[0]?.home.id ?? '');
  const [pending, setPending] = useState(false);
  const [receipt, setReceipt] = useState<NodeEnvironmentMoveCreateReceipt | null>(null);
  return <Dialog open onOpenChange={(open) => { if (!open && !pending) onClose(); }} title={t(receipt === null ? 'fleetMoves.title' : 'fleetMoves.accepted')} size="lg">
    {receipt !== null ? <AcceptedMoves ids={receipt.job_ids} onClose={onClose} /> : groups.length === 0 ? <EmptyState compact title={t('loss.noEnvironments')} action={<Button onClick={onClose}>{t('common.close')}</Button>} /> : <>
      {groups.length > 1 && <ResourcePicker pickerKey="fleet-move-place" label={t('change.where.place')} value={selected} onValueChange={setSelected} disabled={pending}
        options={groups.map((group) => ({ value: group.home.id, name: placeLabel(group.home) ?? t('ui.nameUnknown') }))} />}
      {groups.map((group) => <div key={group.home.id} hidden={group.home.id !== selected}>
        <GroupMoves group={group} active={group.home.id === selected} reader={reader} onPending={setPending} onReceipt={setReceipt} onClose={onClose} />
      </div>)}
    </>}
  </Dialog>;
}

interface MoveRow { environment: EnvironmentRef; plan: EnvironmentMovePlan | null; refusal: string | null }
function GroupMoves({ group, active, reader, onPending, onReceipt, onClose }: {
  group: MoveGroup; active: boolean; reader: number; onPending: (pending: boolean) => void;
  onReceipt: (receipt: NodeEnvironmentMoveCreateReceipt) => void; onClose: () => void;
}) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const cache = useQueryClient();
  const actKey = useActKey();
  const now = useNow();
  const [destination, setDestination] = useState('automatic');
  const [template, setTemplate] = useState('inherited');
  const [copy, setCopy] = useState(false);
  const [notify, setNotify] = useState<boolean | undefined>();
  const [turn, setTurn] = useState<'wait' | 'interrupt' | null>(null);
  const [wait, setWait] = useState<number | null>(null);
  const [started, setStarted] = useState<EnvironmentBatch | null>(null);
  const [retryIds, setRetryIds] = useState<string[] | null>(null);
  const [revision, setRevision] = useState(0);
  const ids = retryIds ?? group.environments.map((environment) => environment.id);
  const hosts = useQuery({ ...hostsQuery(null), enabled: active && started === null });
  const machines = useQuery({ ...machinesQuery, enabled: active && started === null });
  const needsTemplate = group.environments.some((environment) => environment.template === null);
  const templates = useQuery({ ...templatesQuery('environment', null), enabled: active && started === null && needsTemplate });
  const host = destination.startsWith('host:') ? destination.slice(5) : null;
  const machine = destination.startsWith('machine:') ? destination.slice(8) : null;
  const plan = useQuery({ queryKey: ['fleet-node-moves', reader, group.home.id, ids, destination, template, copy, revision],
    queryFn: ({ signal }) => planNodeMoves(group.home.id, ids, host, signal, { ...(machine === null ? {} : { machine_id: machine }), ...(template === 'inherited' ? {} : { template_id: template }), copy_directories: copy }),
    enabled: active && started === null, retry: false, staleTime: Number.POSITIVE_INFINITY, gcTime: 0 });
  const current = plan.isError ? undefined : plan.data;
  const expired = current !== undefined && Date.parse(current.expires_at) <= now;
  const ready = active && current !== undefined && !plan.isError && !plan.isFetching && !expired && current.executable > 0;
  const run = useMutation({ mutationFn: async (body: ForkCreateBody) => {
    if (!ready || current === undefined || body.preflight_id !== current.preflight_id || reader !== client.session.getState().readerVersion) throw new Error('The current native move plan is required');
    return startNodeMoves(group.home.id, body, actKey(body));
  }, onMutate: () => onPending(true), onSettled: () => onPending(false), onSuccess: (answer) => {
    if (reader !== client.session.getState().readerVersion) return;
    if ('projection' in answer) { onReceipt(answer); return; }
    for (const row of answer.jobs) cache.setQueryData(forkKeys.job(row.job.id), row.job);
    setStarted(answer);
  }, onError: (error) => { if (planGone(error) || error instanceof ApiError && (error.is('plan.blocked') || error.is('conflict.stale_revision'))) void plan.refetch(); } });
  if (started !== null) return <EnvironmentBatchProgress batch={started} onClose={onClose} transfers={current?.plans ?? []}
    label={t('fleetMoves.title')} runningLabel={t('fleetMoves.running')} doneLabel={t('fleetMoves.done')}
    onRetry={(failed) => { setRetryIds(failed); setStarted(null); setRevision((value) => value + 1); run.reset(); }} />;
  const rows: MoveRow[] = [...(current?.plans ?? []).map((value) => ({ environment: value.environment, plan: value, refusal: null })),
    ...(current?.skipped ?? []).map((value) => ({ environment: value.environment, plan: null, refusal: t(value.reason === 'no_permission' ? 'fleetMoves.noPermission' : 'fleetMoves.notMovable') }))];
  const running = current?.plans.reduce((count, value) => count + value.threads.filter((thread) => thread.turn_running).length, 0) ?? 0;
  const waiting = current?.plans.some((value) => value.blockers.some((blocker) => blocker.code === 'measurement_pending')) === true;
  const kinds = new Set(current?.plans.map((value) => transferFacts(value).kind));
  const confirmKey = kinds.size === 1 && kinds.has('switch_copy') ? 'envTransfer.copyCount' : kinds.size === 1 && kinds.has('move') ? 'fleetMoves.confirm' : 'envTransfer.changeCount';
  return <Measurements plan={current} active={active && started === null} onReady={() => void plan.refetch()}><div className="flex min-w-0 flex-col gap-3" data-node-move-plan>
    <ResourcePicker pickerKey="fleet-move-destination" label={t('loss.destinationHost')} value={destination} onValueChange={setDestination} disabled={run.isPending}
      options={[{ value: 'automatic', name: t('changeCompact.automatic') },
        ...(hosts.isError ? [] : hosts.data ?? []).map((value) => ({ value: `host:${value.id}`, name: value.name, resource: { kind: 'host' as const, id: value.id, data: value } })),
        ...(machines.isError ? [] : machines.data ?? []).map((value) => ({ value: `machine:${value.id}`, name: value.name, resource: { kind: 'machine' as const, id: value.id, data: value } }))]} />
    {hosts.isError && <ErrorState compact error={hosts.error} onRetry={() => void hosts.refetch()} />}
    {machines.isError && <ErrorState compact error={machines.error} onRetry={() => void machines.refetch()} />}
    {needsTemplate && <ResourcePicker pickerKey="fleet-move-template" label={t('environments.make.template')} value={template} onValueChange={setTemplate} disabled={run.isPending}
      options={[{ value: 'inherited', name: t('uniformMoves.inheritedTemplate') }, ...(templates.isError ? [] : templates.data ?? []).filter((value) => value.enabled && value.access.permissions.includes('template.use')).map((value) => ({ value: value.id, name: value.name, ...resourceChoice({ kind: 'template', template: value }) }))]} />}
    {needsTemplate && templates.isError && <ErrorState compact error={templates.error} onRetry={() => void templates.refetch()} />}
    <Checkbox label={t('changeCompact.copyDirectories')} checked={copy} onCheckedChange={setCopy} disabled={run.isPending} />
    {plan.isError ? <ErrorState compact error={plan.error} onRetry={() => void plan.refetch()} /> : plan.isPending ? <SkeletonRows read={plan} rows={3} /> : <>
      {running > 0 && <RunningTurnChoice activeTurn={turn ?? plan.data.options.active_turn} minutes={wait ?? plan.data.options.wait_minutes} running={running} disabled={run.isPending} onChoice={setTurn} onMinutes={setWait} />}
      <Table label={t('fleetMoves.title')} rows={rows} rowKey={(row) => row.environment.id} columns={[
        { id: 'environment', header: t('loss.environment'), cell: (row) => <div className="flex min-w-0 flex-col gap-1"><ResourceChip subject={{ kind: 'environment', environment: row.environment }} />
          {row.plan !== null && <span className="text-sm text-text-muted">{t(transferFacts(row.plan).kind === 'switch_copy' ? 'envTransfer.copy' : 'change.env.confirm.move')}</span>}
          {row.refusal !== null && <span className="text-sm text-text-muted">{row.refusal}</span>}
          {row.plan?.blockers.map((blocker) => <span key={blocker.code} className="text-sm text-warning-text">{blockerWords(blocker, null, t).text}</span>)}
          {row.plan !== null && <span className="text-sm text-text-muted">{row.plan.files.measured ? t('fleetMoves.files', { count: row.plan.files.file_count, number: formatNumber(row.plan.files.file_count, locale), size: formatBytes(row.plan.files.total_bytes, locale) }) : '–'}</span>}
        </div> },
        { id: 'destination', header: t('loss.destinationHost'), from: 'sm', cell: (row) => row.plan === null ? null : 'automatic' in row.plan.to ? <ResourceChip subject={{ kind: 'host', host: row.plan.to.host }} /> : <ResourceChip subject={{ kind: 'machine', machine: row.plan.to.machine }} /> },
        { id: 'details', header: t('common.details'), cell: (row) => <Popover label={t('common.details')} className="w-80" trigger={<Button size="sm" variant="ghost">{t('common.details')}</Button>}>
          <div className="flex flex-col gap-3">{row.plan !== null && <PlanLists {...moveLists(row.plan, locale, t)} />}
            <Button size="sm" onClick={() => { onClose(); openChange({ kind: 'environment', id: row.environment.id, name: row.environment.name }, { kind: 'move' }); }}>{t('resources.act.open')}</Button>
          </div>
        </Popover> },
      ]} />
      <Time estimate={plan.data.estimate} options={{ ...plan.data.options, active_turn: turn ?? plan.data.options.active_turn, wait_minutes: wait ?? plan.data.options.wait_minutes }} turnRunning={running > 0}
        stands={t('change.env.standsStill', { count: plan.data.plans.reduce((count, value) => count + value.threads.length, 0) })} />
    </>}
    {expired && <StateLine compact title={t('change.expired.title')} action={<Button size="sm" onClick={() => void plan.refetch()}>{t('change.expired.again')}</Button>} />}
    {run.error !== null && run.variables?.preflight_id === current?.preflight_id && <ErrorAlert error={run.error} />}
    <div className="flex flex-wrap items-center justify-end gap-2">
      <div className="me-auto min-w-0">{waiting ? <span role="status" data-change-reason><MeasurementReason waiting planning={plan.isFetching} fallback={null} /></span> : <NotifyPeople plan={current?.notification} value={notify} onChange={setNotify} disabled={run.isPending || plan.isFetching} />}</div>
      <Button onClick={onClose} disabled={run.isPending}>{t('common.cancel')}</Button>
      <Button variant="primary" pending={run.isPending} blocked={!ready} onClick={() => { if (ready && current !== undefined) run.mutate({ preflight_id: current.preflight_id, notify: notify ?? current.notification.default_on,
        options: { active_turn: turn ?? current.options.active_turn, wait_minutes: wait ?? current.options.wait_minutes } }); }}>{plan.isError ? t('changeReady.failed') : waiting ? t('liveWork.waitCount') : current === undefined || plan.isFetching ? t('liveWork.planning') : t(confirmKey, { count: current.executable, number: formatNumber(current.executable, locale) })}</Button>
    </div>
  </div></Measurements>;
}

function AcceptedMoves({ ids, onClose }: { ids: readonly string[]; onClose: () => void }) {
  const { t } = useTranslation();
  const jobs = useQueries({ queries: ids.map((id) => jobQuery(id)) });
  return <div className="flex flex-col gap-3" data-node-move-receipt>
    {jobs.map((read, index) => <div key={ids[index]}>{read.isError ? <ErrorState compact error={read.error} onRetry={() => void read.refetch()} /> : read.data === undefined ? <SkeletonRows read={read} rows={1} /> : <JobProgress job={read.data} heading={t('fleetMoves.accepted')} reconnecting={read.failureCount > 0} />}</div>)}
    <Button onClick={onClose} className="self-end">{t('common.close')}</Button>
  </div>;
}
