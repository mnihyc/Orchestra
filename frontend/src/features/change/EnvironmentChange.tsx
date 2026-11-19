/** An environment brought up to date, made again or moved to another host: what changes, what stays, what ends, and the threads it holds meanwhile.
 *
 * The same four views as a switch, for a thing that is not a thread. Updating gives the environment the unit it would
 * be made from now (a changed template, a newer image, a newer connector) on the same volumes; recreating makes a
 * new unit with nothing new in it; moving takes the environment, its files and its threads to another host. All of
 * them lock the environment and every thread in it from the confirmation, and all of them undo themselves when they
 * fail before the commit. Moves choose a declared unit or directory lifecycle on a permitted machine.
 * Existing directories need explicit copy consent and stay on their source machine.
 */
import { keepPreviousData, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { Clock, SlidersHorizontal } from 'lucide-react';
import { useId, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { NotifyPeople } from '@/features/notifications/NotifyPeople';
import { EmptyState } from '@/ui/EmptyState';

import { environmentMovePlacesQuery, planEnvironmentMove, planEnvironmentUpdate, startEnvironmentMove, startEnvironmentUpdate } from '@/api/changes';
import { ApiError } from '@/api/client';
import { environmentQuery } from '@/api/environments';
import { isTechnicalChange, runtimeChangeText, programChangeText } from '@/features/resources/TechnicalValue';
import { moveRemovesSoftware } from '@/features/resources/isolation/runtimes';
import { TargetBuilds } from '@/features/change/TargetBuilds';
import { UpdateNotify } from '@/features/change/UpdateNotify';
import { RunningTurnChoice } from '@/features/fleet/UpdateDialog';
import { templatesQuery } from '@/api/resources';
import { blockersOf, cancelJob, forkKeys, heldBy, isOver, jobQuery, planGone } from '@/api/forks';
import { Blockers } from '@/features/change/Blockers';
import { Frame, type View } from '@/features/change/Frame';
import { PlanLists } from '@/features/change/Lists';
import { MeasurementReason, Measurements, waitsForCount } from '@/features/change/Measurements';
import { copyLines, filesLines, type Line, mcpLines, mountLines, rangeWords } from '@/features/change/plan';
import { JobProgress, committed } from '@/features/change/Progress';
import { useRefreshWhenOver } from '@/features/change/refresh';
import { Result } from '@/features/change/Result';
import { HostChoices } from '@/features/change/Where';
import { FilesChoice } from '@/features/change/Files';
import { ALL_FILES, type FileChoice, type MountPick, mountChoicesOf, selectionOf, withFate, withPick } from '@/features/change/choices';
import { MountFates } from '@/features/change/MountFates';
import type { ChangeOptions as ApiOptions, EnvironmentMovePlan, EnvironmentMovePreflightBody, EnvironmentUpdatePlan, Job, PendingUpdate } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { useActKey } from '@/lib/forms';
import { newActKey } from '@/lib/requests';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { resourceChoice } from '@/features/resource-facts/ResourceFacts';

type Kind = 'update' | 'recreate' | 'move';

export interface EnvironmentChangeProps {
  environmentId: string;
  kind: Kind;
  job: string | null;
  open: boolean;
  onClose: () => void;
}

/** What the options of a change that the wizard asks about are, until the person changes them. */
interface RunOptions {
  activeTurn: 'wait' | 'interrupt' | null;
  waitMinutes: number | null;
}

const WHAT_KEYS = {
  template: 'change.env.what.template',
  image: 'change.env.what.image',
  settings: 'change.env.what.settings',
  connector: 'change.env.what.connector',
  executor: 'change.env.what.executor',
  harness: 'change.env.what.harness',
} as const satisfies Record<PendingUpdate['what'], string>;

function optionsBody(options: RunOptions): ApiOptions | null {
  const body: ApiOptions = { ...(options.activeTurn === null ? {} : { active_turn: options.activeTurn }), ...(options.waitMinutes === null ? {} : { wait_minutes: options.waitMinutes }) };
  return Object.keys(body).length === 0 ? null : body;
}

export function EnvironmentChange(props: EnvironmentChangeProps) {
  const { t } = useTranslation();
  const environment = useQuery(environmentQuery(props.environmentId));
  if (environment.isPending || environment.isError) {
    return (
      <Frame title={t(`change.env.title.${props.kind}`, { name: '' })} at="plan" open={props.open} onClose={props.onClose} footer={<Button onClick={props.onClose}>{t('common.close')}</Button>}>
        {environment.isError ? <ErrorState compact error={environment.error} retrying={environment.isRefetching} onRetry={() => void environment.refetch()} /> : <SkeletonRows rows={4} />}
      </Frame>
    );
  }
  return <Body {...props} name={environment.data.name} hasUnit={environment.data.template !== null} carryHomeDefault={environment.data.template !== null || (environment.data.data_sources !== undefined && environment.data.data_sources !== null)} />;
}

function Body({ environmentId, kind, job, open, onClose, name, hasUnit, carryHomeDefault }: EnvironmentChangeProps & { name: string; hasUnit: boolean; carryHomeDefault: boolean }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const queryClient = useQueryClient();
  const actKey = useActKey();
  const reasonId = useId();
  const move = kind === 'move';
  const [hostId, setHostId] = useState<string | null>(null);
  const [machineId, setMachineId] = useState<string | null>(null);
  const [templateId, setTemplateId] = useState<string | null>(null);
  const [carryHome, setCarryHome] = useState(carryHomeDefault);
  const [copyDirectories, setCopyDirectories] = useState(false);
  const [carryPrivate, setCarryPrivate] = useState(true);
  const [files, setFiles] = useState<FileChoice>(ALL_FILES);
  const [picks, setPicks] = useState<Readonly<Record<string, MountPick>>>({});
  const [showOptions, setShowOptions] = useState(false);
  const [chosen, setChosen] = useState<RunOptions>({ activeTurn: null, waitMinutes: null });
  const [jobId, setJobId] = useState<string | null>(job);
  const [cancelling, setCancelling] = useState(false);
  const [ran, setRan] = useState<{ done: string; threads: number } | null>(null);

  const templates = useQuery({ ...templatesQuery('environment', null), enabled: open && jobId === null && move && !hasUnit });
  const places = useQuery({ ...environmentMovePlacesQuery(environmentId, templateId), enabled: open && jobId === null && move });
  const hosts = (places.data ?? []).flatMap((place) => 'automatic' in place ? [place] : []);
  const machines = (places.data ?? []).flatMap((place) => 'machine' in place ? [place] : []);

  const planKey = ['change-env-plan', environmentId, kind, hostId, machineId, templateId, carryHome, carryPrivate, copyDirectories, files, picks];
  const moveBody: EnvironmentMovePreflightBody = { ...(machineId === null ? hostId === null ? {} : { host_id: hostId } : { machine_id: machineId }), ...(templateId === null ? {} : { template_id: templateId }), home: carryHome, private_views: carryPrivate, copy_directories: copyDirectories, files: selectionOf(files), ...(Object.keys(picks).length === 0 ? {} : { mounts: mountChoicesOf(picks) }) };
  const plan = useQuery({
    queryKey: planKey,
    queryFn: ({ signal }): Promise<EnvironmentUpdatePlan | EnvironmentMovePlan> => (move ? planEnvironmentMove(environmentId, moveBody, signal) : planEnvironmentUpdate(environmentId, kind === 'recreate' ? 'recreate' : 'update', signal)),
    enabled: open && jobId === null && (hasUnit || move),
    staleTime: Number.POSITIVE_INFINITY,
    gcTime: 0,
    retry: false,
    placeholderData: keepPreviousData,
  });
  const [notify, setNotify] = useState<boolean | undefined>();
  const fingerprints = useMutation({
    mutationFn: ({ body, requestId }: { body: EnvironmentMovePreflightBody; key: readonly unknown[]; requestId: string }) => planEnvironmentMove(environmentId, { ...body, check_fingerprints: true, fingerprint_request_id: requestId }),
    onSuccess: (checked, { key }) => { queryClient.setQueriesData({ queryKey: key, exact: true }, checked); },
  });
  const checkError = JSON.stringify(fingerprints.variables?.body) === JSON.stringify(moveBody) ? fingerprints.error : null;
  const current = plan.data;
  const stale = plan.isPlaceholderData || plan.isFetching || fingerprints.isPending;

  const jobAnswer = useQuery({ ...jobQuery(jobId ?? ''), enabled: jobId !== null });
  const running = jobAnswer.data;
  useRefreshWhenOver(running !== undefined && isOver(running.state), running?.id, running?.environment_id === undefined || running.environment_id === null ? [] : [running.environment_id]);
  const view: View = running === undefined ? 'plan' : running.state === 'queued' || running.state === 'running' ? 'progress' : 'done';

  const body = optionsBody(chosen);
  const confirm = useMutation({
    mutationFn: async (): Promise<Job> => {
      if (current === undefined) throw new Error('there is no plan');
      const effectiveNotify = move && 'notification' in current ? notify ?? current.notification.default_on : notify ?? true;
      const create = { preflight_id: current.preflight_id, ...(body === null ? {} : { options: body }), ...(move ? { notify: effectiveNotify } : {}) };
      const key = actKey({ plan: current.preflight_id, body, notify: effectiveNotify });
      return move ? startEnvironmentMove(environmentId, create, key) : startEnvironmentUpdate(environmentId, create, key, effectiveNotify);
    },
    onSuccess: (started) => {
      queryClient.setQueryData(forkKeys.job(started.id), started);
      if (current !== undefined) setRan({ done: move ? t('change.env.doneMove', { name, host: destinationName(current as EnvironmentMovePlan, t) }) : t('change.env.doneUpdate', { name }), threads: current.threads.length });
      setJobId(started.id);
    },
    onError: (error) => {
      if (planGone(error) || (error instanceof ApiError && (error.is('plan.blocked') || error.is('conflict.exists') || error.is('conflict.stale_revision')))) void plan.refetch();
    },
  });
  const cancel = useMutation({
    mutationFn: () => cancelJob(jobId ?? ''),
    onSuccess: () => {
      setCancelling(false);
    },
  });

  // A job that was opened by its number says what it is itself: a move or an update.
  const shown: Kind = running?.kind === 'environment_move' ? 'move' : kind;
  const title = t(`change.env.title.${shown}`, { name });
  const about = t(`change.env.about.${shown}`);
  const heading = t(`change.env.heading.${shown}`, { name, host: shown === 'move' ? destinationName(current as EnvironmentMovePlan | undefined, t) : '' });
  const blockers = confirm.error instanceof ApiError && confirm.error.is('plan.blocked') ? blockersOf(confirm.error) : (current?.blockers ?? []);
  const held = heldBy(confirm.error);
  const canConfirm = current !== undefined && !stale && blockers.length === 0 && !confirm.isPending;
  const threads = current?.threads.length ?? 0;
  const turnRunning = current?.threads.some((entry) => entry.turn_running) === true;
  const minutes = chosen.waitMinutes ?? current?.options.wait_minutes ?? 10;
  const activeTurn = chosen.activeTurn ?? current?.options.active_turn ?? 'wait';
  const currentMove = move ? current as EnvironmentMovePlan | undefined : undefined;
  const chosenMachine = machineId ?? (hostId === null && currentMove !== undefined && !('automatic' in currentMove.to) ? currentMove.to.machine.id : null);

  if (running !== undefined) {
    const over = running.state === 'succeeded' || running.state === 'failed' || running.state === 'cancelled';
    return (
      <Frame
        title={title}
        about={about}
        at={view}
        failed={running.state === 'failed' || running.state === 'cancelled'}
        open={open}
        onClose={onClose}
        dismissible={!cancel.isPending}
        footer={
          over ? (
            <>
              {(running.state === 'failed' || running.state === 'cancelled') && (
                <Button
                  onClick={() => {
                    setJobId(null);
                    confirm.reset();
                    void plan.refetch();
                  }}
                >
                  {t('change.result.tryAgain')}
                </Button>
              )}
              <Button variant="primary" onClick={onClose}>
                {running.state === 'succeeded' ? t('change.result.doneButton') : t('common.close')}
              </Button>
            </>
          ) : (
            <>
              <Button onClick={onClose}>{t('change.progress.hide')}</Button>
              <Button variant="danger" disabled={committed(running) || running.progress.activity?.stopping === true || cancel.isPending} onClick={() => setCancelling(true)}>
                {t(running.progress.activity?.stopping === true ? 'liveWork.stopping' : 'change.progress.cancel')}
              </Button>
            </>
          )
        }
      >
        {over ? (
          <Result job={running} done={ran?.done ?? t('change.env.doneUpdate', { name })} stoppedTurn={false} waited={minutes} subject={name}>
            {running.state === 'succeeded' && ran !== null && ran.threads > 0 && <p className="text-sm text-text-muted">{t('change.env.threadsOpen', { count: ran.threads })}</p>}
          </Result>
        ) : (
          <JobProgress job={running} heading={heading} reconnecting={jobAnswer.failureCount > 0} />
        )}
        {cancel.error !== null && <ErrorAlert error={cancel.error} />}
        <ConfirmDialog open={cancelling} onOpenChange={setCancelling} title={t('change.cancel.title')} confirmLabel={t('change.progress.cancel')} danger pending={cancel.isPending} error={cancel.error ?? undefined} onConfirm={() => cancel.mutate()}>
          {t('change.cancel.body', { subject: name })}
        </ConfirmDialog>
      </Frame>
    );
  }
  if (jobId !== null) {
    return (
      <Frame title={title} about={about} at="progress" open={open} onClose={onClose} footer={<Button onClick={onClose}>{t('change.progress.hide')}</Button>}>
        {jobAnswer.isError ? <ErrorState compact error={jobAnswer.error} retrying={jobAnswer.isRefetching} onRetry={() => void jobAnswer.refetch()} /> : <SkeletonRows rows={3} />}
      </Frame>
    );
  }

  return (
    <Measurements plan={currentMove} homeFallback={move} active={open} onReady={() => {
      const checked = fingerprints.variables;
      if (checked !== undefined && JSON.stringify(checked.body) === JSON.stringify(moveBody) && currentMove?.mounts.some((mount) => mount.fingerprint_measurement !== undefined && mount.fingerprint_measurement !== null)) fingerprints.mutate(checked);
      else void plan.refetch();
    }}>
    <Frame
      aside={
        <Button size="sm" variant="ghost" icon={<SlidersHorizontal aria-hidden />} aria-expanded={showOptions} onClick={() => setShowOptions((on) => !on)}>
          {optionsBody(chosen) === null ? t('change.options.button') : t('change.options.buttonChanged', { count: Object.keys(optionsBody(chosen) ?? {}).length })}
        </Button>
      }
      title={title}
      about={about}
      at={confirm.isPending ? 'confirm' : 'plan'}
      open={open}
      onClose={onClose}
      dismissible={!confirm.isPending}
      footer={
        <>
          {!waitsForCount(current) && current !== undefined && (!move || !('notification' in current)) && <p className="basis-full text-sm text-text-muted">{threads === 0 ? t('change.env.lockNone') : t('change.env.lock', { count: threads, number: formatNumber(threads, format.locale) })}</p>}
          {!waitsForCount(current) && move && current !== undefined && 'notification' in current && <div className="me-auto min-w-0"><NotifyPeople plan={current.notification} value={notify} onChange={setNotify} disabled={confirm.isPending || stale} /></div>}
          {!move && <UpdateNotify audience={(current as EnvironmentUpdatePlan | undefined)?.notify} checked={notify ?? true} onChange={setNotify} disabled={confirm.isPending} />}
          {waitsForCount(current) && <span id={reasonId} className="min-w-0 basis-full text-sm text-text-muted sm:basis-auto sm:flex-1" role="status" data-change-reason><MeasurementReason waiting planning={stale} fallback={null} /></span>}
          <Button disabled={confirm.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" pending={confirm.isPending} blocked={!canConfirm && !confirm.isPending} aria-describedby={waitsForCount(current) ? reasonId : undefined} onClick={() => canConfirm && confirm.mutate()} data-confirm>
            {waitsForCount(current) ? t('liveWork.waitCount') : move && threads > 0 ? t('change.env.confirm.moveThreads', { count: threads }) : t(`change.env.confirm.${kind}`)}
          </Button>
        </>
      }
    >
      {!hasUnit && !move ? (
        <InlineAlert tone="info" title={t('change.env.noUnit.title')}>
          {t('change.env.noUnit.update')}
        </InlineAlert>
      ) : (
        <>
          {move && (
            <div className="flex min-w-0 flex-col gap-3" data-change-destination>
              {!hasUnit && <Select label={t('environments.make.template')}
                value={templateId ?? 'inherited'} onValueChange={(next) => { setTemplateId(next === 'inherited' ? null : next); setHostId(null); }}
                options={[{ value: 'inherited', name: t('uniformMoves.inheritedTemplate'), description: t('uniformMoves.inheritedTemplateAbout') },
                  ...(templates.data ?? []).filter((entry) => entry.enabled && entry.access.permissions.includes('template.use')).map((entry) => ({ value: entry.id, name: entry.name, ...resourceChoice({ kind: 'template', template: entry }) }))]} />}
              {!hasUnit && templates.isError && <ErrorAlert error={templates.error} />}
              {places.isError && <ErrorAlert error={places.error} />}
              <HostChoices hosts={hosts} value={hostId} onChange={setHostId} loading={places.isPending} machines={machines} machine={chosenMachine} onMachine={(next) => { setMachineId(next); if (next !== null) setHostId(null); }} />
            </div>
          )}
          {showOptions && (
            <div className="flex flex-col gap-3 rounded-lg border border-border bg-bg-subtle p-4" data-change-options>
              <div className="flex flex-col gap-2">
                <h4 className="text-sm font-medium text-text">{t('change.options.turn')}</h4>
                <div className="flex flex-wrap items-end gap-3">
                  <SegmentedControl
                    label={t('change.options.turn')}
                    value={activeTurn}
                    onValueChange={(next) => setChosen((value) => ({ ...value, activeTurn: next }))}
                    segments={[
                      { value: 'wait', name: t('changeCompact.turn.wait') },
                      { value: 'interrupt', name: t('changeCompact.turn.interrupt') },
                    ]}
                  />
                  {activeTurn === 'wait' && (
                    <Input label={t('change.options.minutes')} type="number" min={0} inputMode="numeric" value={String(minutes)} onChange={(event) => { const waitMinutes = Number(event.target.value); if (Number.isSafeInteger(waitMinutes) && waitMinutes >= 0) setChosen((value) => ({ ...value, waitMinutes })); }} fieldClassName="w-(--field-narrow)" />
                  )}
                </div>
              </div>
            </div>
          )}
          {plan.isError && <ErrorAlert error={plan.error} message={!move && plan.error instanceof ApiError && plan.error.is('request.invalid') ? t('programVersions.planRefused', { reason: plan.error.problem.detail }) : undefined} action={<Button size="sm" onClick={() => void plan.refetch()}>{t('common.tryAgain')}</Button>} />}
          {move && <FilesChoice value={files} onChange={setFiles} />}
          {plan.isPending && <><p role="status" className="text-sm text-text-muted">{t('liveWork.planning')}</p><SkeletonRows rows={4} /></>}
          {current !== undefined && (
            <div className={stale ? 'flex flex-col gap-4 opacity-60 transition-opacity' : 'flex flex-col gap-4'} aria-busy={stale} data-plan>
              {planGone(confirm.error) && (
                <InlineAlert tone="warning" title={t('change.expired.title')}>
                  {t('change.expired.made')}
                </InlineAlert>
              )}
              <Blockers blockers={blockers.filter((blocker) => blocker.code !== 'measurement_pending')} plan={null} onShow={held === null ? undefined : () => setJobId(held)} />
              {blockers.some((blocker) => blocker.code !== 'measurement_pending') && <Button size="sm" className="self-start" pending={plan.isFetching} onClick={() => void plan.refetch()}>{t('common.tryAgain')}</Button>}
              {!move && turnRunning && !showOptions && <RunningTurnChoice activeTurn={activeTurn} minutes={minutes} running={current.threads.filter((thread) => thread.turn_running).length} onChoice={(activeTurn) => setChosen((value) => ({ ...value, activeTurn }))} onMinutes={(waitMinutes) => setChosen((value) => ({ ...value, waitMinutes }))} />}
              {move ? <PlanLists {...moveLists(current as EnvironmentMovePlan, format.locale, t)} /> : <><TargetBuilds updates={(current as EnvironmentUpdatePlan).updates} programs={(current as EnvironmentUpdatePlan).target_programs} /><PlanLists {...updateLists(current as EnvironmentUpdatePlan, t)} /></>}
              {move && <div className="flex flex-col gap-2" data-mount-choices>
                <MountFates mounts={(current as EnvironmentMovePlan).mounts} picks={picks} onPickAll={(fate) => setPicks((now) => withFate(now, (current as EnvironmentMovePlan).mounts, fate))} onPick={(mount, pick) => setPicks((now) => withPick(now, mount, pick))} onCheck={() => fingerprints.mutate({ body: moveBody, key: planKey, requestId: newActKey() })} checking={fingerprints.isPending} checkDisabled={plan.isFetching || plan.isPlaceholderData || plan.isError} checkError={checkError} />
                <Checkbox label={t('fork.files.home')} checked={carryHome} onCheckedChange={setCarryHome} />
                <Checkbox label={t('changeCompact.copyDirectories')} checked={copyDirectories} onCheckedChange={setCopyDirectories} />
                {(current as EnvironmentMovePlan).files.private_views.length > 0 && <Checkbox label={t('fork.files.privateViews')} checked={carryPrivate} onCheckedChange={setCarryPrivate} />}
              </div>}
              <ThreadsHeld plan={current} />
              <div className="flex flex-col gap-1 text-base text-text" data-plan-time>
                <p className="flex items-start gap-2">
                  <Clock className="mt-0.5 shrink-0 text-text-subtle" aria-hidden />
                  <span>
                    {t('change.time.aboutStill', { range: rangeWords(current.estimate.seconds_low, current.estimate.seconds_high, format.locale), stands: threads === 0 ? t('change.env.standsStillAlone') : t('change.env.standsStill', { count: threads }) })}
                    {current.estimate.basis === 'assumed' && <span className="ml-1 text-text-muted">{t('change.time.assumed')}</span>}
                  </span>
                </p>
                {turnRunning && <p className="pl-6 text-text-muted">{activeTurn === 'wait' ? t('change.time.turnWait', { minutes }) : t('change.time.turnStop')}</p>}
              </div>
            </div>
          )}
      {confirm.error !== null && !(planGone(confirm.error) || (confirm.error instanceof ApiError && (confirm.error.is('plan.blocked') || confirm.error.is('conflict.exists')))) && <ErrorAlert error={confirm.error} />}
        </>
      )}
    </Frame>
    </Measurements>
  );
}

/** "template, image and connector": what an update brings, in the reader's language and list. */
export function whatWords(updates: readonly PendingUpdate[], locale: string, t: TFunction): string {
  const kinds = [...new Set(updates.map((update) => update.what))];
  if (kinds.length === 0) return t('change.env.what.nothing');
  return new Intl.ListFormat(locale, { style: 'long', type: 'conjunction' }).format(kinds.map((kind) => t(WHAT_KEYS[kind])));
}

function updateLists(plan: EnvironmentUpdatePlan, t: TFunction): { changes: Line[]; stays: Line[]; ends: Line[]; ifFails: Line[] } {
  const changes: Line[] =
    plan.updates.length === 0
      ? [{ id: 'recreate', tone: 'plain', text: t('change.env.plan.recreate') }]
      : plan.updates.map((update) => ({ id: `update-${update.name}`, tone: 'plain', text: update.what === 'harness' ? programChangeText(update, t) : isTechnicalChange(update) ? runtimeChangeText(update, t) : t('change.env.plan.update', { name: update.name, from: update.from ?? '', to: update.to ?? '' }), ...(isTechnicalChange(update) ? { technical: update } : {}) }));
  const stays: Line[] = [
    { id: 'owner', tone: 'plain', text: t('change.env.plan.staysOwners', { name: plan.environment.owner.display_name }) },
    ...plan.kept.map((entry, index): Line => ({ id: `kept-${index}`, tone: 'plain', text: entry })),
  ];
  const ends: Line[] = [
    ...plan.lost.map((entry, index): Line => ({ id: `lost-${index}`, tone: 'warning', text: entry })),
  ];
  const ifFails: Line[] = [
    { id: 'rollback', tone: 'plain', text: t(`change.env.plan.rollback.${plan.rollback}`, { name: plan.environment.name }) },
  ];
  return { changes, stays, ends, ifFails };
}

function moveLists(plan: EnvironmentMovePlan, locale: string, t: TFunction): { changes: Line[]; stays: Line[]; ends: Line[] } {
  const source = plan.from?.name ?? plan.from_machine?.name ?? t('uniformMoves.source');
  const changes: Line[] = [
    { id: 'moves', tone: 'plain', text: t('change.env.plan.moves', { from: source, to: destinationName(plan, t) }) },
    ...filesLines(plan.files, locale, t),
    ...mountLines(plan.mounts.filter((mount) => mount.action !== 'detach'), t),
    ...copyLines(plan.mounts, locale, t),
    ...mcpLines(plan.mcp.filter((server) => server.fate === 'new'), t),
    ...(plan.files.home === null ? [{ id: 'home-left', tone: 'plain' as const, text: t('mountPlan.homeLeft') }] : []),
  ];
  if (plan.held_still) changes.push({ id: 'still', tone: 'plain', text: t('change.env.plan.heldStill') });
  const stays: Line[] = [
    { id: 'owner', tone: 'plain', text: t('change.env.plan.staysOwners', { name: plan.environment.owner.display_name }) },
    { id: 'page', tone: 'plain', text: t('change.env.plan.pageStays') },
    ...plan.threads.map((entry, index): Line => ({ id: `thread-${index}`, tone: entry.action === 'blocked' ? 'danger' : 'plain', text: t(`change.env.plan.thread.${entry.action}`, { name: entry.thread?.name ?? t('change.env.plan.hidden'), note: entry.note }) })),
  ];
  const ends: Line[] = [
    { id: 'processes', tone: 'plain', text: t('change.env.plan.processes') },
    ...(moveRemovesSoftware(plan) ? [{ id: 'software', tone: 'warning' as const, text: t('isolation.replaced') }] : []),
    ...mountLines(plan.mounts.filter((mount) => mount.action === 'detach'), t),
    ...mcpLines(plan.mcp.filter((server) => server.fate === 'lost' || server.fate === 'unavailable'), t),
    { id: 'old', tone: 'plain', text: plan.from === null ? t('uniformMoves.oldDirectory') : t('change.env.plan.oldUnit', { name: source }) },
  ];
  return { changes, stays, ends };
}

function destinationName(plan: EnvironmentMovePlan | undefined, t: TFunction): string {
  return plan === undefined ? t('uniformMoves.automatic') : 'automatic' in plan.to ? plan.to.host.name : plan.to.machine.name;
}

/** The threads in the environment, which stand still with it: named, with a turn that is running marked, and the ones the person may not see counted. */
function ThreadsHeld({ plan }: { plan: EnvironmentUpdatePlan | EnvironmentMovePlan }) {
  const { t } = useTranslation();
  const named = plan.threads.filter((entry) => entry.thread !== null);
  const hidden = plan.threads.length - named.length;
  if (plan.threads.length === 0) return <EmptyState compact title={t('change.env.noThreads')} />;
  return (
    <div className="flex flex-col gap-1" data-held-threads>
      <h4 className="text-sm font-medium text-text">{t('change.env.threads', { count: plan.threads.length })}</h4>
      <ul className="flex flex-col gap-1 text-base text-text">
        {named.map((entry, index) => (
          <li key={entry.thread?.id ?? index} className="flex flex-wrap items-baseline gap-2">
            <span>{entry.thread?.name}</span>
            {entry.turn_running && <span className="text-sm text-text-muted">{t('change.env.turnRunning')}</span>}
          </li>
        ))}
        {hidden > 0 && <li className="text-sm text-text-muted">{t('change.env.hiddenThreads', { count: hidden })}</li>}
      </ul>
    </div>
  );
}
