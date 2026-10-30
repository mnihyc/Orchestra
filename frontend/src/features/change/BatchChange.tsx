/** Several threads switched at once, from a topic, a provider, an environment or a selection: one plan for each, and one bar for the batch.
 *
 * The same four views as a switch of one thread, using the configured batch size. The threads are listed and ticked (all, to
 * begin with); the target is chosen once (a provider, and an agent, a model and an effort that may be left as each
 * thread's own); one plan comes back for each thread, each carrying its own blockers, so one thread that cannot go does
 * not hold the others back: the batch carries out those that can and leaves the rest alone. Threads the person may not
 * see are counted and not named. While it runs there is one bar for the batch and a row for each thread with its own
 * bar and phase; the batch can be stopped, and what failed can be tried again.
 */
import { keepPreviousData, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Check, CircleX, OctagonAlert, Undo2 } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { reasonWords } from '@/features/change/Result';
import { StateLine } from '@/ui/StateLine';
import { BatchDestination } from '@/features/change/BatchDestination';

import { ApiError } from '@/api/client';
import { resourceImpactQuery } from '@/api/resource-impact';
import { NotifyPeople } from '@/features/notifications/NotifyPeople';
import { batchQuery, blockersOf, cancelBatch, forkKeys, planBatch, planGone, startBatch } from '@/api/forks';
import { nodeQuery } from '@/api/nodes';
import { agentsQuery, providerModelsQuery, providersQuery } from '@/api/resources';
import { Blockers } from '@/features/change/Blockers';
import { Frame, type View } from '@/features/change/Frame';
import { effortLabel, modelLabel } from '@/features/change/modelLabel';
import { rangeWords } from '@/features/change/plan';
import { useRefreshWhenOver } from '@/features/change/refresh';
import { hasReported, profilesFor } from '@/features/change/RunsOn';
import { resourceChoice } from '@/features/resource-facts/ResourceFacts';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';
import type { ForkBatch, ForkBatchPlan, ForkBatchPreflightBody, ForkEnvironment, ForkPlan } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { useActKey } from '@/lib/forms';
import { useOrgTree, useTreeStore } from '@/realtime/stores/tree';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { cx } from '@/ui/cx';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Meter } from '@/ui/Meter';
import { JobActivityLine } from '@/features/change/Progress';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { ThreadStateIcon } from '@/ui/ThreadState';
import type { ThreadDisplay } from '@/generated/api-types';


/** Where the threads were gathered, and so which of them there are. */
export type BatchSource =
  | { kind: 'threads'; threads: { id: string; name: string }[]; label: string | null }
  | { kind: 'topic'; id: string }
  | { kind: 'provider'; id: string }
  | { kind: 'agent'; id: string }
  | { kind: 'model'; providerId: string; id: string }
  | { kind: 'environment'; id: string };

export interface BatchChangeProps {
  source: BatchSource;
  /** A batch that was started already, whose progress is the first view. */
  batch: string | null;
  open: boolean;
  onClose: () => void;
}

interface Candidate {
  id: string;
  name: string;
  display: ThreadDisplay | null;
}

type BatchSelector=Pick<ForkBatchPreflightBody,'all_of_provider'|'all_of_agent'|'all_of_environment'|'source_model_id'>;

/** Resource-wide candidates are one projection of current bindings, including the complete private count. */
function useCandidates(source: BatchSource, active:boolean): { threads: Candidate[]; hidden: number; total:number; label: string; all: BatchSelector | null; loading: boolean; error: unknown } {
  const trees = useTreeStore((state) => state.byOrg);
  const displayOf = (id: string) => [...trees.values()].map((held) => held.nodes.get(id)?.thread?.display).find((display) => display !== undefined) ?? null;
  const { t } = useTranslation();
  const topic = useQuery({ ...nodeQuery(source.kind === 'topic' ? source.id : ''), enabled: active && source.kind === 'topic' });
  const tree = useOrgTree(topic.data?.org_id ?? null);
  const resource=source.kind==='provider'||source.kind==='agent'||source.kind==='model'||source.kind==='environment';
  const object=source.kind==='model'?{kind:'provider' as const,id:source.providerId}:source.kind==='provider'||source.kind==='agent'||source.kind==='environment'?{kind:source.kind,id:source.id}:{kind:'provider' as const,id:''};
  const impact=useQuery({...resourceImpactQuery({object,model_id:source.kind==='model'?source.id:null}),enabled:active&&resource,refetchInterval:false});
  if(source.kind==='threads')return {threads:source.threads.map((row)=>({...row,display:displayOf(row.id)})),hidden:0,total:source.threads.length,label:source.label??t('change.batch.selected'),all:null,loading:false,error:null};
  if(source.kind==='topic') {
    const nodes=[...tree.nodes.values()].filter((node)=>node.parent_id===source.id&&node.kind==='thread'&&!node.deleted&&!node.archived);
    return {threads:nodes.map((node)=>({id:node.id,name:node.name,display:node.thread?.display??null})),hidden:0,total:nodes.length,label:topic.data?.name??'',all:null,loading:topic.isPending,error:topic.error};
  }
  const bound=impact.data?.facts.dependents.find((group)=>group.relation==='binding');
  const threads=bound?.visible.flatMap((row)=>row.object?.kind==='thread'?[{id:row.object.id,name:row.name,display:impact.data?.thread_displays?.[row.object.id]??displayOf(row.object.id)}]:[])??[];
  const all:BatchSelector=source.kind==='provider'?{all_of_provider:source.id}:source.kind==='agent'?{all_of_agent:source.id}:source.kind==='model'?{all_of_provider:source.providerId,source_model_id:source.id}:{all_of_environment:source.id};
  return {threads,hidden:bound?.hidden??0,total:bound?.count??0,label:impact.data?.name??'',all,loading:impact.isPending,error:impact.error};
}

export function BatchChange({ source, batch, open, onClose }: BatchChangeProps) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const queryClient = useQueryClient();
  const actKey = useActKey();
  const [unticked, setUnticked] = useState<ReadonlySet<string>>(new Set());
  const [providerId, setProviderId] = useState<string | null>(null);
  const [destination, setDestination] = useState<ForkEnvironment | undefined>();
  const [notify,setNotify]=useState<boolean|undefined>();
  const [agentId, setAgentId] = useState<string | null>(null);
  const [model, setModel] = useState<string | null>(null);
  const [effort, setEffort] = useState<string | null | undefined>(undefined);
  const [batchId, setBatchId] = useState<string | null>(batch);
  const candidates = useCandidates(source,open&&batchId===null);
  const [cancelling, setCancelling] = useState(false);
  const [only, setOnly] = useState<readonly string[] | null>(null);

  const providers = useQuery(providersQuery('usable'));
  const agents = useQuery(agentsQuery());
  const models = useQuery({ ...providerModelsQuery(providerId ?? ''), enabled: providerId !== null });
  const listed = (models.data ?? []).filter((candidate) => candidate.enabled);
  const chosenModel = listed.find((candidate) => candidate.model_name === model);
  const efforts = chosenModel?.parameters.efforts ?? [];
  // A model that is chosen takes an effort of its own with it: the one the person picked, else the model's usual one, and none where it lists none.
  const effortUsed: string | null | undefined = model === null ? undefined : efforts.length === 0 ? null : (efforts.includes(effort ?? '') ? effort : (chosenModel?.parameters.default_effort ?? efforts[0] ?? null));
  const provider = (providers.data ?? []).find((candidate) => candidate.id === providerId);
  const offered = profilesFor(provider, agents.data ?? []).filter(hasReported);

  const threads = only === null ? candidates.threads : candidates.threads.filter((entry) => only.includes(entry.id));
  const selected = threads.filter((entry) => !unticked.has(entry.id)).map((entry) => entry.id);
  const everything = candidates.all !== null && unticked.size === 0 && only === null;

  // What is asked, by value: the plan is asked again only when this text is not the one it was asked for.
  const body: ForkBatchPreflightBody | null =
    (providerId === null && destination === undefined) || (selected.length === 0 && !everything)
      ? null
      : {
          ...(everything && candidates.all !== null ? candidates.all : { thread_ids: selected }),
          ...(providerId === null || (everything && destination !== undefined) ? {} : { to: { provider_id: providerId, ...(agentId === null ? {} : { agent_id: agentId }), ...(model === null ? {} : { model }), ...(effortUsed === undefined ? {} : { effort: effortUsed }) } }),
          ...(destination === undefined ? {} : { environment: destination }),
        };
  const asked = JSON.stringify(body);

  const plan = useQuery({
    queryKey: ['change-batch-plan', asked],
    queryFn: ({ signal }) => planBatch(JSON.parse(asked) as ForkBatchPreflightBody, signal),
    enabled: batchId === null && body !== null,
    staleTime: Number.POSITIVE_INFINITY,
    gcTime: 0,
    retry: false,
    placeholderData: keepPreviousData,
  });
  const current = plan.data;
  const stale = plan.isPlaceholderData || plan.isFetching;

  const running = useQuery({ ...batchQuery(batchId ?? ''), enabled: batchId !== null });
  useRefreshWhenOver(running.data !== undefined && running.data.counts.queued + running.data.counts.running === 0, batchId ?? undefined);
  const view: View = running.data === undefined ? 'plan' : running.data.counts.queued + running.data.counts.running > 0 ? 'progress' : 'done';

  const confirm = useMutation({
    mutationFn: () => {
      if (current === undefined) throw new Error('there is no plan');
      return startBatch({ preflight_id: current.preflight_id, notify:notify??current.notification?.default_on??false }, actKey({ plan: current.preflight_id }));
    },
    onSuccess: (started) => {
      queryClient.setQueryData(forkKeys.batch(started.batch_id), started);
      setBatchId(started.batch_id);
    },
    onError: (error) => {
      if (planGone(error) || (error instanceof ApiError && (error.is('plan.blocked') || error.is('request.invalid')))) void plan.refetch();
    },
  });
  const cancel = useMutation({
    mutationFn: () => cancelBatch(batchId ?? ''),
    onSuccess: () => {
      setCancelling(false);
    },
  });

  const label = candidates.label;
  const title = t(`change.batch.title.${source.kind}`, { name: label, count: candidates.total });
  const about = t('change.batch.about');
  const executable = current?.executable ?? 0;
  const canConfirm = current !== undefined && !stale && executable > 0 && !confirm.isPending;

  if (running.data !== undefined) {
    const data = running.data;
    const over = data.counts.queued + data.counts.running === 0;
    const failed = data.jobs.filter((entry) => entry.job.state === 'failed');
    return (
      <Frame
        title={title}
        about={about}
        at={view}
        failed={over && (data.counts.failed > 0 || data.counts.cancelled > 0)}
        open={open}
        onClose={onClose}
        dismissible={!cancel.isPending}
        footer={
          over ? (
            <>
              {failed.length > 0 && (
                <Button
                  onClick={() => {
                    setOnly(failed.map((entry) => entry.thread.id));
                    setUnticked(new Set());
                    setBatchId(null);
                    confirm.reset();
                  }}
                >
                  {t('change.batch.retry', { count: failed.length })}
                </Button>
              )}
              <Button variant="primary" onClick={onClose}>
                {t('change.result.doneButton')}
              </Button>
            </>
          ) : (
            <>
              <Button onClick={onClose}>{t('change.progress.hide')}</Button>
              <Button variant="danger" disabled={cancel.isPending} onClick={() => setCancelling(true)}>
                {t('change.progress.cancel')}
              </Button>
            </>
          )
        }
      >
        <BatchProgress data={data} over={over} reconnecting={running.failureCount > 0} />
        {cancel.error !== null && <ErrorAlert error={cancel.error} />}
        <ConfirmDialog open={cancelling} onOpenChange={setCancelling} title={t('change.batch.cancelTitle')} confirmLabel={t('change.progress.cancel')} danger pending={cancel.isPending} error={cancel.error ?? undefined} onConfirm={() => cancel.mutate()}>
          {t('change.batch.cancelBody')}
        </ConfirmDialog>
      </Frame>
    );
  }
  if (batchId !== null) {
    return (
      <Frame title={title} about={about} at="progress" open={open} onClose={onClose} footer={<Button onClick={onClose}>{t('change.progress.hide')}</Button>}>
        {running.isError ? <ErrorState compact error={running.error} retrying={running.isRefetching} onRetry={() => void running.refetch()} /> : <SkeletonRows rows={3} />}
      </Frame>
    );
  }

  return (
    <Frame
      title={title}
      aside={<ArticleGuideLink article="switch-threads" heading="switch-a-batch" title={t('change.batch.guide')} />}
      about={about}
      at={confirm.isPending ? 'confirm' : 'plan'}
      open={open}
      onClose={onClose}
      dismissible={!confirm.isPending}
      footer={
        <>
          <div className="me-auto min-w-0"><NotifyPeople plan={current?.notification} value={notify} onChange={setNotify} disabled={confirm.isPending||stale} /></div>
          <Button disabled={confirm.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" pending={confirm.isPending} blocked={!canConfirm && !confirm.isPending} onClick={() => canConfirm && confirm.mutate()} data-confirm>
            {t('change.batch.confirm', { count: executable })}
          </Button>
        </>
      }
    >
      {candidates.error !== null && candidates.error !== undefined && <ErrorAlert error={candidates.error} />}
      <section aria-label={t('change.batch.threads')} className="flex flex-col gap-2" data-batch-threads>
        <div className="flex items-center justify-between gap-2">
          <h3 className="text-base font-semibold text-text">{t('change.batch.threadsTitle', { count: selected.length })}</h3>
          {threads.length > 1 && (
            <Button size="sm" variant="ghost" onClick={() => setUnticked(unticked.size === 0 ? new Set(threads.map((entry) => entry.id)) : new Set())}>
              {unticked.size === 0 ? t('change.batch.none') : t('change.batch.all')}
            </Button>
          )}
        </div>
        {candidates.loading ? (
          <SkeletonRows rows={3} />
        ) : (
          <ul className="flex max-h-(--list-max-height) flex-col gap-1 overflow-y-auto rounded-lg border border-border bg-surface p-2">
            {threads.map((entry) => (
              <li key={entry.id} className="flex items-center gap-2">
                <Checkbox label={entry.name} checked={!unticked.has(entry.id)} onCheckedChange={(on) => setUnticked((set) => { const next = new Set(set); if (on) next.delete(entry.id); else next.add(entry.id); return next; })} />
                <ThreadStateIcon display={entry.display} />
              </li>
            ))}
            {candidates.hidden > 0 && <li className="px-1 text-sm text-text-muted">{t('change.batch.hidden', { count: candidates.hidden })}</li>}
          </ul>
        )}
      </section>

      <section aria-label={t('change.batch.target')} className="flex flex-col gap-2">
        <h3 className="text-base font-semibold text-text">{t('change.batch.target')}</h3>
        <div className="grid gap-3 sm:grid-cols-2">
          <Select
            label={t('change.runs.provider')}
            value={providerId ?? ''}
            placeholder={t('change.batch.chooseProvider')}
            onValueChange={(next) => {
              setProviderId(next);
              setAgentId(null);
              setModel(null);
              setEffort(undefined);
            }}
            options={(providers.data ?? []).filter((candidate) => candidate.status === 'ready').map((candidate) => ({ value: candidate.id, name: candidate.name, ...resourceChoice({ kind: 'provider', provider: candidate }) }))}
          />
          <Select
            label={t('change.runs.agent')}
            value={agentId ?? ''}
            disabled={providerId === null}
            onValueChange={(next) => setAgentId(next === '' ? null : next)}
            options={[{ value: '', name: t('change.batch.keepOwn') }, ...offered.map((agent) => ({ value: agent.id, name: agent.name, ...resourceChoice({ kind: 'agent', agent, provider, models: listed }) }))]}
          />
          <Select
            label={t('change.runs.model')}
            value={model ?? ''}
            disabled={providerId === null}
            onValueChange={(next) => {
              setModel(next === '' ? null : next);
              setEffort(undefined);
            }}
            options={[{ value: '', name: t('change.batch.keepOwn') }, ...listed.map((candidate) => ({ value: candidate.model_name, name: modelLabel(candidate.model_name, listed, null, t), ...resourceChoice({ kind: 'model', model: candidate, providerId: providerId ?? '', usage: provider?.choice_usage, effort: effortUsed }) }))]}
          />
          {model !== null && efforts.length > 0 && <Select label={t('change.runs.effort')} value={effortUsed ?? ''} onValueChange={(next) => setEffort(next)} options={efforts.map((value) => ({ value, name: effortLabel(value, t) ?? '' }))} />}
        </div>
      </section>

      <BatchDestination value={destination} onChange={setDestination} sources={source.kind === 'environment' ? [source.id] : []} />

      {plan.isError && <ErrorAlert error={plan.error} action={<Button size="sm" onClick={() => void plan.refetch()}>{t('common.tryAgain')}</Button>} />}
      {body !== null && plan.isPending && <SkeletonRows rows={3} />}
      {current !== undefined && (
        <div className={cx('flex flex-col gap-3', stale && 'opacity-60 transition-opacity')} aria-busy={stale} data-plan>
          {planGone(confirm.error) && (
            <InlineAlert tone="warning" title={t('change.expired.title')}>
              {t('change.expired.made')}
            </InlineAlert>
          )}
          {confirm.error instanceof ApiError && confirm.error.is('plan.blocked') && <Blockers blockers={blockersOf(confirm.error)} plan={null} />}
          <p className="text-base text-text" data-batch-summary>
            {t('change.batch.summary', { executable, count: current.plans.length, blocked: current.plans.length - executable })}
            {current.skipped.length > 0 && <span className="ml-1 text-text-muted">{t('change.batch.skipped', { count: current.skipped.length })}</span>}
          </p>
          <PlanRows plans={current.plans} />
          <p className="flex items-start gap-2 text-base text-text">
            {t('change.time.about', { range: rangeWords(current.estimate.seconds_low, current.estimate.seconds_high, format.locale) })}
            {current.turns_running > 0 && <span className="text-text-muted">{t('change.batch.turnsRunning', { count: current.turns_running, minutes: current.options.wait_minutes })}</span>}
          </p>
        </div>
      )}
      {confirm.error !== null && !(planGone(confirm.error) || (confirm.error instanceof ApiError && confirm.error.is('plan.blocked'))) && <ErrorAlert error={confirm.error} />}
    </Frame>
  );
}

/** One plan for each thread: where it comes from and goes to, and what stops it, in the same words as a switch of one. */
function PlanRows({ plans }: { plans: readonly ForkPlan[] }) {
  const { t } = useTranslation();
  return (
    <ul className="flex flex-col gap-1 rounded-lg border border-border bg-surface p-2" aria-label={t('change.batch.plans')} data-batch-plans>
      {plans.map((plan) => {
        const blocked = plan.blockers.length > 0;
        return (
          <li key={plan.preflight_id} data-plan-row={plan.source.id} data-blocked={blocked ? 'true' : undefined} className="flex flex-col gap-1 border-b border-border p-2 last:border-b-0">
            <div className="flex flex-wrap items-center gap-2">
              {blocked ? <OctagonAlert className="shrink-0 text-danger" aria-hidden /> : <Check className="shrink-0 text-success" aria-hidden />}
              <span className="font-medium text-text">{plan.source.redacted ? t('change.batch.hiddenName') : (plan.source.name ?? '')}</span>
              <span className="text-sm text-text-muted">{t('change.batch.rowMoves', { from: plan.from.provider.name, to: plan.to.provider.name })}</span>
              <span className="sr-only">{blocked ? t('change.batch.rowBlocked') : t('change.batch.rowReady')}</span>
              {plan.turn_running && <span className="text-sm text-text-muted">{t('change.env.turnRunning')}</span>}
            </div>
            {blocked && <Blockers blockers={plan.blockers} plan={plan} />}
          </li>
        );
      })}
    </ul>
  );
}

/** The batch under way or over: one bar for all of it, a row for each thread with its own bar and phase. */
function BatchProgress({ data, over, reconnecting }: { data: ForkBatch; over: boolean; reconnecting: boolean }) {
  const { t } = useTranslation();
  const { counts } = data;
  return (
    <div className="flex flex-col gap-3" data-batch-state={over ? 'over' : 'running'}>
      <div className="flex items-baseline justify-between gap-3">
        <h3 className="text-base font-semibold text-text">{over ? t('change.batch.over') : t('change.batch.running')}</h3>
        <span className="text-base font-medium text-text tabular-nums" data-job-percent>
          {data.percent} %
        </span>
      </div>
      <Meter look="progress" value={data.percent} label={t('change.batch.running')} />
      <p className="text-base text-text-muted" aria-live="polite" data-batch-counts>
        {t('change.batch.counts', { done: counts.succeeded, running: counts.running, queued: counts.queued, failed: counts.failed, cancelled: counts.cancelled })}
        {data.hidden > 0 && <span className="ml-1">· {t('change.batch.hidden', { count: data.hidden })}</span>}
      </p>
      <ul aria-label={t('change.batch.jobs')} className="flex flex-col gap-1 rounded-lg border border-border bg-surface p-2">
        {data.jobs.map(({ job, thread }) => (
          <li key={job.id} data-batch-job={thread.id} data-job-state={job.state} className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-3 gap-y-1 border-b border-border p-2 last:border-b-0 sm:grid-cols-[minmax(0,1fr)_10rem_auto]">
            <span className="min-w-0 truncate font-medium text-text">{thread.name ?? t('change.batch.hiddenName')}</span>
            <span className="hidden sm:block">
              <Meter look="progress" value={job.progress.percent} label={thread.name ?? ''} />
            </span>
            <span className="flex items-center gap-1 text-sm text-text-muted">
              {job.state === 'succeeded' && <Check className="text-success" aria-hidden />}
              {job.state === 'failed' && <CircleX className="text-danger" aria-hidden />}
              {job.state === 'cancelled' && <Undo2 aria-hidden />}
              {t(`change.batch.state.${job.state}`, { phase: t(`change.phase.${job.progress.phase}.short`, { defaultValue: job.progress.phase }) })}
            </span>
            {(job.state === 'queued' || job.state === 'running') && <div className="col-span-full"><JobActivityLine job={job} /></div>}
            {job.state === 'failed' && job.error !== null && (
              <StateLine role="alert" tone="danger" compact className="col-span-full" message={<>
                <span data-failure-code={job.error.code}>{reasonWords(job, t, 10)}</span>{' '}{job.error.undone ? t('change.batch.undone') : ''}
              </>} />
            )}
          </li>
        ))}
      </ul>
      {reconnecting && (
        <InlineAlert tone="info" title={t('change.progress.reconnecting')}>
          {t('change.progress.reconnectingAbout')}
        </InlineAlert>
      )}
    </div>
  );
}

export type { ForkBatchPlan };
