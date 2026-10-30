/** A fork or a switch of one thread: ordinary pickers, the plan, confirmation, progress and result.
 *
 * One procedure for two acts. A switch gives the thread another provider, agent profile or environment and it goes on as
 * the same thread (its address, owner and page stay); a fork makes a new thread of its own out of it, from any message
 * and with as much of the history as is wanted. The wizard opens on compact shared selects, with the
 * plan under them (what changes, what stays, what ends, who may go on talking to it, how long) and, behind
 * Options, the few parameters. A plan that has a blocker cannot be confirmed, and says what would lift it; one older than
 * a quarter of an hour is made again. Pressing the button takes the lock and turns the view into the job's progress;
 * Hide closes it and changes nothing, since the job is the server's. Where the server does not answer these requests
 * yet, the plan says so and nothing else is offered.
 */
import { keepPreviousData, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { SlidersHorizontal } from 'lucide-react';
import type { TFunction } from 'i18next';
import { useEffect, useId, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { openDependency } from '@/api/changes';
import { ApiError, client } from '@/api/client';
import { blockersOf, cancelJob, forkKeys, heldBy, isOver, jobQuery, planFork, planGone, startFork } from '@/api/forks';
import { saveDraft } from '@/api/threads';
import { nodeQuery } from '@/api/nodes';
import { agentsQuery, providersQuery } from '@/api/resources';
import { Blockers } from '@/features/change/Blockers';
import { type Choices, changedOptions, changesSomething, initialChoices, requestOf, withFate, withPick } from '@/features/change/choices';
import { MountFates } from '@/features/change/MountFates';
import { Depends } from '@/features/change/Depends';
import { Frame, type View } from '@/features/change/Frame';
import { HistoryChoices } from '@/features/change/History';
import { HistoryReportTable, PlanLists } from '@/features/change/Lists';
import { Options } from '@/features/change/Options';
import { changesOf, endsOf, planProviderName, plannedApprovalWords, targetModelName, targetModelWords, type PlanContext, staysOf } from '@/features/change/plan';
import { providerLabels } from '@/features/resources/providers/labels';
import { JobProgress, committed } from '@/features/change/Progress';
import { MeasurementReason, Measurements, waitsForCount } from '@/features/change/Measurements';
import { useRefreshWhenOver } from '@/features/change/refresh';
import { Result } from '@/features/change/Result';
import { agentFor, hasReported, movesRuns, profilesFor, RunsOn, useModelFor } from '@/features/change/RunsOn';
import { Time } from '@/features/change/Time';
import { Where } from '@/features/change/Where';
import type { ForkPlan, ForkRequest, Job, Provider, Thread } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { useActKey } from '@/lib/forms';
import { newActKey } from '@/lib/requests';
import { useDebounced } from '@/lib/debounce';
import { useNow } from '@/lib/time';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { PromptScope, usePrompt } from '@/ui/PromptScope';
import { SkeletonRows } from '@/ui/Skeleton';

/** How long the choices rest before a plan is asked for: a person who is clicking through a list gets one plan, not six. */
const PLAN_PAUSE_MS = 300;

/** The reasons a thread is blocked that another provider or profile gets it out of. */
const PROVIDER_WAYS_OUT = new Set(['provider_exhausted', 'provider_unavailable', 'agent_disabled', 'agent_offline', 'binding_invalid', 'model_not_allowed']);

export interface ThreadChangeProps {
  threadId: string;
  kind: 'switch' | 'fork';
  toTurn: number | null;
  focus: 'provider' | 'environment' | null;
  /** A job that was started already, whose progress is the first view. */
  job: string | null;
  say: string | null;
  open: boolean;
  onClose: () => void;
}

export function ThreadChange(props: ThreadChangeProps) {
  const layer = usePrompt(props.open, 'scope');
  return <PromptScope layer={layer}><ThreadChangeContent {...props} /></PromptScope>;
}

function ThreadChangeContent(props: ThreadChangeProps) {
  const { t } = useTranslation();
  const node = useQuery({ ...nodeQuery(props.threadId), enabled: props.open });
  if (node.isPending || node.isError || node.data.kind !== 'thread') {
    return (
      <Frame steps={props.kind === 'fork' || props.focus === 'environment'} title={t(props.kind === 'fork' ? 'change.title.fork' : 'change.title.switch', { name: '' })} at="plan" open={props.open} onClose={props.onClose} footer={<Button onClick={props.onClose}>{t('common.close')}</Button>}>
        {node.isError ? <ErrorState compact error={node.error} retrying={node.isRefetching} onRetry={() => void node.refetch()} /> : <SkeletonRows rows={4} />}
      </Frame>
    );
  }
  return <Body {...props} thread={node.data} />;
}

function Body({ thread, kind, toTurn, focus, job, say, open, onClose }: ThreadChangeProps & { thread: Thread }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const actKey = useActKey();
  const meId = useStore(client.session, (state) => state.userId);
  const now = useNow();
  const fork = kind === 'fork';
  const simple = !fork && focus !== 'environment';
  const reasonId = useId();
  // A side thread that is forked is kept as a thread of the topic.
  const keeping = fork && thread.side_of !== null;

  const [choices, setChoices] = useState<Choices>(() => initialChoices(fork ? 'new' : 'replace', toTurn));
  const patch = (next: Partial<Choices>) => setChoices((current) => ({ ...current, ...next }));
  // A provider-blocked thread starts with the next usable provider, with every picker directly available.
  const blockedReason = thread.state === 'blocked' && thread.blocked !== null ? thread.blocked.reason : null;
  const startsOnDefault = !fork && job === null && focus !== 'environment' && blockedReason !== null && PROVIDER_WAYS_OUT.has(blockedReason);
  const [options, setOptions] = useState(false);
  const [jobId, setJobId] = useState<string | null>(job);
  const [ticked, setTicked] = useState<ReadonlySet<string>>(new Set());
  const [cancelling, setCancelling] = useState(false);
  /** What the plan said when it was confirmed, kept for the views after it: the plan itself goes with its job. */
  const [ran, setRan] = useState<{ heading: string; done: string; interrupted: boolean; waited: number; fork: boolean } | null>(null);

  const picking = open && jobId === null;
  const usable = useQuery({ ...providersQuery('usable'), enabled: picking });
  const agents = useQuery({ ...agentsQuery(), enabled: picking });
  const [defaulted, setDefaulted] = useState(false);
  if (!defaulted && startsOnDefault && jobId === null && usable.data !== undefined && agents.data !== undefined) {
    // Once, as soon as the lists are here: the choice is the person's from then on.
    setDefaulted(true);
    const next = usable.data.find((candidate: Provider) => candidate.id !== thread.binding?.provider.id && candidate.status === 'ready' && profilesFor(candidate, agents.data).some(hasReported));
    if (next !== undefined) setChoices((current) => ({ ...current, providerId: next.id, agentId: agentFor(next, agents.data, thread) }));
  }

  const models = useModelFor(thread, choices, patch, picking);

  // The plan, asked for once the choices have rested, and kept: a plan is a statement of what would happen now.
  const request: ForkRequest = useMemo(() => requestOf(choices), [choices]);
  const asked = JSON.stringify(request);
  const rested = useDebounced(asked, PLAN_PAUSE_MS);
  const settled = !movesRuns(choices) || choices.model !== null;
  const wanted = picking && settled && (fork || changesSomething(choices));
  const plan = useQuery({
    queryKey: ['change-plan', thread.id, rested],
    queryFn: ({ signal }) => planFork(thread.id, JSON.parse(rested) as ForkRequest, signal),
    enabled: wanted && rested === asked,
    staleTime: Number.POSITIVE_INFINITY,
    gcTime: 0,
    retry: false,
    placeholderData: keepPreviousData,
  });
  const current = plan.data;
  const fingerprints = useMutation({
    mutationFn: ({ body, requestId }: { body: ForkRequest; requestId: string }) => planFork(thread.id, { ...body, check_fingerprints: true, fingerprint_request_id: requestId }),
    onSuccess: (checked, { body }) => { queryClient.setQueriesData({ queryKey: ['change-plan', thread.id, JSON.stringify(body)], exact: true }, checked); },
  });
  const checkingContent = fingerprints.isPending;
  const checkError = JSON.stringify(fingerprints.variables?.body) === asked ? fingerprints.error : null;
  const planning = wanted && (plan.isFetching || rested !== asked || checkingContent);
  const expired = current !== undefined && Date.parse(current.expires_at) <= now;

  const providerNames = providerLabels(usable.isFetchedAfterMount && !usable.isError ? usable.data ?? [] : [], new Map());
  const context: PlanContext = { kind, thread: { name: thread.name, model: thread.model, modelDisplay: thread.model_display, effort: thread.effort, owner: thread.owner }, me: meId, locale: format.locale, models, providerNames };

  // The job, once there is one: asked for again while it is under way, and shown wherever it is.
  const jobAnswer = useQuery({ ...jobQuery(jobId ?? ''), enabled: open && jobId !== null });
  const running = jobAnswer.data;
  useRefreshWhenOver(running !== undefined && isOver(running.state), running?.id);
  const view: View = running === undefined ? 'plan' : running.state === 'queued' || running.state === 'running' ? 'progress' : 'done';

  const confirm = useMutation({
    mutationFn: async (): Promise<Job> => {
      if (current === undefined) throw new Error('there is no plan');
      setRan({ heading: headingOf(current, fork, t, context), done: doneOf(current, fork, t, context), interrupted: current.turn_running && current.options.active_turn === 'interrupt', waited: current.options.wait_minutes, fork });
      for (const dependency of current.depends.filter((entry) => ticked.has(entry.object.id))) await openDependency(dependency);
      return startFork(thread.id, { preflight_id: current.preflight_id }, actKey({ plan: current.preflight_id, options: choices.options }));
    },
    onSuccess: (started) => {
      queryClient.setQueryData(forkKeys.job(started.id), started);
      setJobId(started.id);
    },
    onError: (error) => {
      // A plan that went out of date, or that the world moved from under, is made again: what the person chose stays.
      if (planGone(error) || (error instanceof ApiError && (error.is('plan.blocked') || error.is('conflict.exists')))) void plan.refetch();
    },
  });

  // A fork made from a message of a person: what was said waits in the box of the new thread, to be asked again or asked differently.
  const sayRef = useRef(say);
  const newThread = fork && running?.state === 'succeeded' ? (running.result as { thread_id?: string } | null)?.thread_id : undefined;
  useEffect(() => {
    const words = sayRef.current;
    if (newThread === undefined || words === null || words === '') return;
    sayRef.current = null;
    void saveDraft(newThread, [{ type: 'text', text: words }], 'queue').catch(() => undefined);
  }, [newThread]);

  const cancel = useMutation({
    mutationFn: () => cancelJob(jobId ?? ''),
    onSuccess: () => {
      setCancelling(false);
    },
  });

  const blockers = confirm.error instanceof ApiError && confirm.error.is('plan.blocked') ? blockersOf(confirm.error) : (current?.blockers ?? []);
  const held = heldBy(confirm.error);
  const planFresh = wanted && current !== undefined && rested === asked && !plan.isError && !plan.isFetching && !plan.isPlaceholderData && !expired && !checkingContent;
  const canConfirm = planFresh && blockers.length === 0 && !confirm.isPending;
  const firstBlocker = blockers[0];
  const refusalKey = firstBlocker?.code === 'access' ? 'access' : firstBlocker?.code === 'already_running' ? 'running' : firstBlocker?.code === 'provider_not_ready' ? 'provider' : firstBlocker?.code === 'model_not_allowed' ? 'model' : firstBlocker?.code === 'no_host' || firstBlocker?.code === 'no_capacity' ? 'capacity' : 'blocked';
  const reason = confirm.isPending ? t('changeReady.applying')
    : !fork && !changesSomething(choices) ? t('changeReady.unchanged')
    : !settled ? t(simple ? 'switchChoice.modelUnavailable' : 'changeReady.chooseModel')
    : planning || plan.isPending ? t('changeReady.checking')
    : plan.isError ? t('changeReady.failed')
    : expired ? t('changeReady.expired')
    : firstBlocker !== undefined ? t(`changeReady.${refusalKey}`)
    : !planFresh ? t('changeReady.checking') : null;
  const topic = thread.path.at(-2)?.name ?? '';

  // ---------------------------------------------------------------- the views

  // A job opened by its number (from a banner, a chip or a menu) is whatever holds the thread: a switch, or the change of its environment or provider.
  const reopened = job !== null;
  const title = reopened ? t('change.title.job', { name: thread.name }) : keeping ? t('change.title.keep', { name: thread.name }) : t(fork ? 'change.title.fork' : 'change.title.switch', { name: thread.name });
  const about = reopened ? t('change.about.job') : keeping ? t('changeCompact.about.keep') : t(fork ? 'changeCompact.about.fork' : 'changeCompact.about.switch');

  if (running !== undefined) {
    const over = running.state === 'succeeded' || running.state === 'failed' || running.state === 'cancelled';
    const result = fork && running.state === 'succeeded' ? (running.result as { thread_id?: string } | null)?.thread_id : undefined;
    const subject = thread.name;
    return (
      <Frame
        title={title}
        about={simple ? undefined : about}
        steps={!simple}
        size={simple ? 'md' : 'lg'}
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
              {running.state === 'failed' && running.error?.code === 'wait_timeout' && !fork && (
                <Button
                  onClick={() => {
                    patch({ options: { ...choices.options, activeTurn: 'interrupt' } });
                    setJobId(null);
                    confirm.reset();
                  }}
                >
                  {t('change.result.stopAndSwitch')}
                </Button>
              )}
              {running.state === 'succeeded' && result !== undefined ? (
                <Button
                  variant="primary"
                  onClick={() => {
                    onClose();
                    void navigate({ to: '/thread/$threadId', params: { threadId: result } });
                  }}
                >
                  {t('change.result.openFork')}
                </Button>
              ) : (
                <Button variant="primary" onClick={onClose}>
                  {running.state === 'succeeded' ? t('change.result.doneButton') : t('common.close')}
                </Button>
              )}
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
          <Result job={running} done={ran?.done ?? t('change.result.doneGeneric', { name: thread.name })} stoppedTurn={ran?.interrupted ?? false} waited={ran?.waited ?? 10} subject={subject}>
            {running.state === 'succeeded' && current?.leaves?.environment?.action === 'remove' && <p className="text-sm text-text-muted">{t('change.result.oldBeingRemoved', { name: current.leaves.environment.environment.name })}</p>}
            {running.state === 'succeeded' && current !== undefined && current.files !== null && current.files.missing.count > 0 && <p className="text-sm text-text-muted">{t('change.plan.filesMissing', { count: current.files.missing.count })}</p>}
          </Result>
        ) : (
          <JobProgress job={running} heading={ran?.heading ?? kindHeading(running, thread.name, t)} reconnecting={jobAnswer.failureCount > 0} />
        )}
        {cancel.error !== null && <ErrorAlert error={cancel.error} />}
        <ConfirmDialog
          open={cancelling}
          onOpenChange={setCancelling}
          title={t('change.cancel.title')}
          confirmLabel={t('change.progress.cancel')}
          danger
          pending={cancel.isPending}
          error={cancel.error ?? undefined}
          onConfirm={() => cancel.mutate()}
        >
          {t('change.cancel.body', { subject: thread.name })}
        </ConfirmDialog>
      </Frame>
    );
  }
  if (jobId !== null && jobAnswer.isError) {
    return (
      <Frame steps={!simple} title={title} about={simple ? undefined : about} at="progress" open={open} onClose={onClose} footer={<Button onClick={onClose}>{t('common.close')}</Button>}>
        <ErrorState compact error={jobAnswer.error} retrying={jobAnswer.isRefetching} onRetry={() => void jobAnswer.refetch()} />
      </Frame>
    );
  }
  if (jobId !== null) {
    return (
      <Frame steps={!simple} title={title} about={simple ? undefined : about} at="progress" open={open} onClose={onClose} footer={<Button onClick={onClose}>{t('change.progress.hide')}</Button>}>
        <SkeletonRows rows={3} />
      </Frame>
    );
  }

  const confirmLabel = keeping ? t('change.confirm.keep') : t(fork ? 'change.confirm.fork' : 'change.confirm.switch');
  return (
    <Measurements plan={current} active={picking} onReady={() => {
      const checked = fingerprints.variables;
      if (checked !== undefined && JSON.stringify(checked.body) === asked && current?.mounts.some((mount) => mount.fingerprint_measurement !== undefined && mount.fingerprint_measurement !== null)) fingerprints.mutate(checked);
      else void plan.refetch();
    }}>
    <Frame
      aside={simple ? undefined :
        <Button size="sm" variant="ghost" icon={<SlidersHorizontal aria-hidden />} aria-expanded={options} onClick={() => setOptions((on) => !on)}>
          {changedOptions(choices.options) > 0 ? t('change.options.buttonChanged', { count: changedOptions(choices.options) }) : t('change.options.button')}
        </Button>
      }
      title={title}
      about={simple ? undefined : about}
      steps={!simple}
      size={simple ? 'md' : 'lg'}
      at={confirm.isPending ? 'confirm' : 'plan'}
      open={open}
      onClose={onClose}
      dismissible={!confirm.isPending}
      footer={
        <>
          {!fork && !waitsForCount(current) && current !== undefined && current.locks.length > 0 && <p className="basis-full text-sm text-text-muted">{t(simple ? 'switchChoice.hold' : 'change.confirm.lock')}</p>}
          <div className={waitsForCount(current) ? "flex w-full min-w-0 flex-wrap items-center justify-end gap-2 sm:flex-nowrap" : "flex w-full min-w-0 items-center justify-end gap-2"}>
            {(reason !== null || waitsForCount(current)) && <span id={reasonId} className={waitsForCount(current) ? "min-w-0 basis-full text-sm text-text-muted sm:basis-auto sm:flex-1" : "min-w-0 flex-1 text-sm text-text-muted"} role="status" data-change-reason><MeasurementReason waiting={waitsForCount(current)} planning={planning} fallback={reason} /></span>}
            <Button disabled={confirm.isPending} onClick={onClose}>{t('common.cancel')}</Button>
            <Button variant="primary" pending={confirm.isPending} blocked={!canConfirm && !confirm.isPending} aria-describedby={reason === null && !waitsForCount(current) ? undefined : reasonId} onClick={() => canConfirm && confirm.mutate()} data-confirm>
              {waitsForCount(current) ? t('liveWork.waitCount') : confirmLabel}
            </Button>
          </div>
        </>
      }
    >
      <div className="flex min-w-0 flex-col gap-3" data-thread-change-pickers>
        {fork && <HistoryChoices threadId={thread.id} choices={choices} onChange={patch} planned={planFresh ? current.history : undefined} focus={open && focus === null} />}
        <RunsOn compact={simple} thread={thread} choices={choices} onChange={patch} focus={open && (focus === 'provider' || (!fork && focus === null))} />
        {/* A switch is first what it runs on, then what of the conversation it carries there. */}
        {!fork && <HistoryChoices threadId={thread.id} choices={choices} onChange={patch} planned={planFresh ? current.history : undefined} />}
        {!simple && <Where thread={thread} choices={choices} onChange={patch} fork={fork} focus={open && focus === 'environment'} />}
        {!simple && current !== undefined && <MountFates mounts={current.mounts} picks={choices.mounts} onPickAll={(fate) => patch({ mounts: withFate(choices.mounts, current.mounts, fate) })} onPick={(mount, pick) => patch({ mounts: withPick(choices.mounts, mount, pick) })} onCheck={() => fingerprints.mutate({ body: request, requestId: newActKey() })} checking={checkingContent} checkDisabled={!planFresh && !checkingContent} checkError={checkError} />}
      </div>
      {!simple && options && <Options choices={choices} onChange={patch} applied={current?.options ?? null} plan={current ?? null} topic={topic} />}

      {wanted && plan.isError && <PlanFailure error={plan.error} onRetry={() => void plan.refetch()} />}
      {wanted && plan.isPending && <><p role="status" className="text-sm text-text-muted">{t('liveWork.planning')}</p><SkeletonRows rows={4} /></>}

      {wanted && current !== undefined && (
        <div className={planning ? 'flex flex-col gap-4 opacity-60 transition-opacity' : 'flex flex-col gap-4'} aria-busy={planning} data-plan data-planning={planning ? 'true' : undefined}>
          {expired && (
            <InlineAlert
              tone="warning"
              title={t('change.expired.title')}
              action={
                <Button size="sm" onClick={() => void plan.refetch()} pending={plan.isFetching}>
                  {t('change.expired.again')}
                </Button>
              }
            >
              {t('change.expired.about')}
            </InlineAlert>
          )}
          {planGone(confirm.error) && <InlineAlert tone="warning" title={t('change.expired.title')}>{t('change.expired.made')}</InlineAlert>}
          <Blockers blockers={blockers.filter((blocker) => blocker.code !== 'measurement_pending')} plan={current} onShow={held === null ? undefined : () => setJobId(held)} />
          {plannedApprovalWords(current.to, t) !== null && <p className="text-sm text-text-muted" data-planned-approval>{plannedApprovalWords(current.to, t)}</p>}
          <PlanLists compact={simple} changes={simple ? [] : changesOf(current, context, t)} stays={simple ? [] : staysOf(current, context, t)} ends={endsOf(current, context, t)} />
          {(!simple || current.history.report.fidelity !== 'lossless' || current.history.report.warnings.length > 0 || current.history.report.dropped.length > 0) && <HistoryReportTable report={current.history.report} />}
          {!fork && <Depends depends={simple ? current.depends.filter((dependency) => dependency.open !== 'all') : current.depends} model={targetModelName(current, context, t)} ticked={ticked} onTick={(id, on) => setTicked((set) => { const next = new Set(set); if (on) next.add(id); else next.delete(id); return next; })} />}
          {(!simple || current.turn_running) && <Time estimate={current.estimate} options={current.options} turnRunning={current.turn_running && !fork} stands={fork ? null : t('change.time.thisThread')} />}
        </div>
      )}
      {confirm.error !== null && !(planGone(confirm.error) || (confirm.error instanceof ApiError && (confirm.error.is('plan.blocked') || confirm.error.is('conflict.exists')))) && <ErrorAlert error={confirm.error} />}
    </Frame>
    </Measurements>
  );
}

/** A missing thread or saved plan can be planned again; other failures keep their ordinary cause. */
export function PlanFailure({ error, onRetry }: { error: unknown; onRetry: () => void }) {
  const { t } = useTranslation();
  if (error instanceof ApiError && error.status === 404) {
    return (
      <InlineAlert tone="warning" action={<Button size="sm" onClick={onRetry}>{t('change.expired.again')}</Button>}>
        {t('change.planGone')}
      </InlineAlert>
    );
  }
  return <ErrorAlert error={error} action={<Button size="sm" onClick={onRetry}>{t('common.tryAgain')}</Button>} />;
}

/** What a job that was opened by its number is, in the words of its bar. */
function kindHeading(job: Job, name: string, t: TFunction): string {
  switch (job.kind) {
    case 'environment_recreate':
      return t('change.progress.environmentUpdate');
    case 'environment_move':
      return t('change.progress.environmentMove');
    case 'provider_recreate':
      return t('change.progress.providerUpdate');
    default:
      return t('change.progress.switching', { name });
  }
}

/** "Switching to Team Pro": what the bar says is under way. */
function headingOf(plan: ForkPlan, fork: boolean, t: TFunction, context: PlanContext): string {
  if (fork) return t('change.progress.forking', { name: plan.source.name ?? '' });
  const target = planProviderName(plan, 'to', context, t);
  return t('change.progress.switchingTo', { target });
}

/** What the end says was done, in the past. */
function doneOf(plan: ForkPlan, fork: boolean, t: TFunction, context: PlanContext): string {
  if (fork) return t('change.result.forked');
  return t('change.result.switched', { provider: planProviderName(plan, 'to', context, t), model: targetModelWords(plan, context, t) });
}
