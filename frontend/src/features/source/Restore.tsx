/** Putting a thread's directory back as it was before one of its turns: a plan read before anything is written, one job, and the state it keeps to undo it.
 *
 * A turn whose files were recorded completely offers it from its footer. The plan names the directory and
 * its coverage, what each path would undergo, the paths changed since with who changed them, the threads
 * that write there now, and what stops it. A plan that went stale is refused by the server before anything
 * is written, and a fresh one is asked for; the choices made are kept. The job, once accepted, is followed
 * and never asked for twice; when it is done, "Undo this restore…" plans the way back from the state it
 * kept. Opening the action reads its coverage; an incomplete copy explains why and opens per-file Undo in Changes.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { Undo2 } from 'lucide-react';
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { isOver, jobQuery } from '@/api/forks';
import { planRestore, restoreFiles, snapshotQuery } from '@/api/source';
import { workspaceDenial } from '@/features/files/door';
import { revealView } from '@/features/workbench/openView';
import type { ActiveTurn, ChangeOptions, Job, RestorePlan, Thread, Turn } from '@/generated/api-types';
import { newActKey } from '@/lib/requests';
import { useThreadRow } from '@/realtime/stores/thread';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { IconButton } from '@/ui/IconButton';
import { Meter } from '@/ui/Meter';
import { JobActivityLine } from '@/features/change/Progress';
import { RadioGroup } from '@/ui/RadioGroup';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { Tooltip } from '@/ui/Tooltip';

import { confirmable, planWentStale, restorable, restoreBlocker, restoreResult, unrecordedReason } from './words';

/** The longest a restore waits for a turn that writes there, in minutes: a day, as the change wizard bounds it. */
const WAIT_MOST_MINUTES = 1_440;
const READ_PERMISSION = 'files.view';
const WRITE_PERMISSION = 'files.manage';

/** A finished turn's recorded-copy action reads metadata only after the reader opens it. */
export function RestoreFromTurn({ threadId, turn, compact = false }: { threadId: string; turn: Turn; compact?: boolean }) {
  const { t } = useTranslation();
  const thread = useThreadRow(threadId);
  const [open, setOpen] = useState(false);
  const reads = thread?.environment_permissions.includes('files.view') === true;
  const writes = thread?.environment_permissions.includes('files.manage') === true;
  const snapshotId = turn.before_snapshot_id;
  if (!thread?.environment || !reads || !writes || turn.completed_at === null || snapshotId === null) return null;
  return (
    <>
      {compact ? <IconButton action="restore" size="sm" label={t('source.restore.open')} title={t('source.restore.titleTurn')} data-restore-turn={turn.id} onClick={() => setOpen(true)} /> : <Tooltip content={t('source.restore.titleTurn')}>
        <Button action="restore" size="sm" variant="ghost" data-restore-turn={turn.id} onClick={() => setOpen(true)}>
          {t('source.restore.open')}
        </Button>
      </Tooltip>}
      {open && <RestoreDialog thread={thread} snapshotId={snapshotId} onClose={() => setOpen(false)} />}
    </>
  );
}

/** One restore from the first plan to the end of its job, and on to its undo. */
export function RestoreDialog({ thread, snapshotId: first, onClose }: { thread: Thread; snapshotId: string; onClose: () => void }) {
  const { t } = useTranslation();
  const live = useThreadRow(thread.id) ?? thread;
  const environment = thread.environment;
  // A thread moved to another environment, or a right taken away, ends what this dialog may show and do.
  const reads = live.environment?.id === environment?.id && live.environment_permissions.includes('files.view');
  const writes = reads && live.environment_permissions.includes('files.manage');
  const environmentId = environment?.id ?? '';
  const [target, setTarget] = useState({ snapshot: first, undoing: false });
  const [options, setOptions] = useState<ChangeOptions>({});
  const [attempt, setAttempt] = useState(0);
  const [jobId, setJobId] = useState<string | null>(null);
  const key = useRef<{ plan: string; key: string } | null>(null);
  const recorded = useQuery({ ...snapshotQuery(target.snapshot), enabled: reads });
  const plan = useQuery({
    queryKey: ['restore-plan', environmentId, target.snapshot, options, attempt],
    queryFn: () => planRestore(environmentId, target.snapshot, options),
    enabled: reads && writes && jobId === null && recorded.data !== undefined && restorable(recorded.data),
    retry: false,
    staleTime: Number.POSITIVE_INFINITY,
    gcTime: 0,
  });
  const apply = useMutation({
    mutationFn: (shown: RestorePlan) => {
      // One key for one plan: a press repeated after a lost answer is the same act, a new plan is a new one.
      if (key.current?.plan !== shown.preflight_id) key.current = { plan: shown.preflight_id, key: newActKey() };
      return restoreFiles(environmentId, shown.preflight_id, key.current.key);
    },
    onSuccess: (job) => setJobId(job.id),
  });
  const job = useQuery({ ...jobQuery(jobId ?? ''), enabled: jobId !== null && reads });
  const undoId = job.data?.state === 'succeeded' ? restoreResult(job.data.result).undo : null;
  const freshPlan = () => {
    apply.reset();
    setAttempt((count) => count + 1);
  };
  const choose = (next: ChangeOptions) => {
    apply.reset();
    setOptions((before) => ({ ...before, ...next }));
  };
  const undo = (undoId: string) => {
    apply.reset();
    setJobId(null);
    setOptions({});
    setTarget({ snapshot: undoId, undoing: true });
  };
  const shown = plan.data;
  const active: ActiveTurn = options.active_turn ?? shown?.options.active_turn ?? 'wait';
  const minutes = options.wait_minutes ?? shown?.options.wait_minutes ?? 10;
  const following = jobId !== null;
  const running = following && (job.data === undefined || !isOver(job.data.state));
  const stale = apply.error !== null && planWentStale(apply.error);

  const footer = (
    <>
      <Button onClick={onClose}>{running ? t('source.restore.hide') : t('common.close')}</Button>
      {!following && writes && shown !== undefined && (stale || plan.isError) && (
        <Button action="refresh" onClick={freshPlan}>
          {t('source.restore.freshPlan')}
        </Button>
      )}
      {!following && writes && shown !== undefined && !stale && (
        <Button action="restore" variant="primary" pending={apply.isPending} disabled={!confirmable(shown) || plan.isFetching} onClick={() => apply.mutate(shown)}>
          {target.undoing ? t('source.restore.confirmUndo') : t('source.restore.confirm')}
        </Button>
      )}
      {following && writes && undoId !== null && !target.undoing && (
        <Button icon={<Undo2 aria-hidden />} onClick={() => undo(undoId)}>
          {t('source.restore.undo')}
        </Button>
      )}
      {following && writes && job.data !== undefined && (job.data.state === 'failed' || job.data.state === 'cancelled') && (
        <Button action="refresh" onClick={() => { setJobId(null); freshPlan(); }}>
          {t('source.restore.freshPlan')}
        </Button>
      )}
    </>
  );

  return (
    <Dialog open size="lg" title={target.undoing ? t('source.restore.titleUndo') : t('source.restore.titleTurn')} description={t('source.restore.lead')} onOpenChange={(next) => { if (!next) onClose(); }} footer={footer}>
      <div className="flex flex-col gap-3" data-restore-dialog>
        {environment === null || !reads ? (
          <InlineAlert tone="warning">{environment === null ? t('source.restore.noEnvironment') : workspaceDenial(t, READ_PERMISSION, environment)}</InlineAlert>
        ) : (
          <>
            <p className="text-sm text-text-muted">
              {t('source.restore.where', { environment: environment.name })} <code className="wrap-anywhere">{thread.directory === '' ? '/' : thread.directory}</code>
            </p>
            {recorded.error !== null && <ErrorState compact error={recorded.error} onRetry={() => void recorded.refetch()} />}
            {recorded.data === undefined ? (
              recorded.error === null && <SkeletonRows rows={2} />
            ) : (
              <p className="flex flex-wrap gap-x-3 gap-y-1 text-sm text-text-muted">
                <span>{t('source.restore.coverage', { count: recorded.data.file_count })}</span><RelativeTime at={recorded.data.created_at} />
              </p>
            )}
            {recorded.data !== undefined && !restorable(recorded.data) && <InlineAlert tone="warning">
              {unrecordedReason(t, recorded.data)}
              <Button action="changes" size="sm" variant="ghost" onClick={() => { onClose(); revealView('changes'); }}>{t('source.restore.review')}</Button>
            </InlineAlert>}
            {!writes && <InlineAlert tone="warning">{workspaceDenial(t, WRITE_PERMISSION, environment)}</InlineAlert>}
            {!following && writes && (
              <>
                <RadioGroup<ActiveTurn>
                  variant="list"
                  label={t('source.restore.activeTurn')}
                  value={active}
                  disabled={apply.isPending}
                  onValueChange={(value) => choose({ active_turn: value })}
                  options={[
                    { value: 'wait', ...choiceOption(t, 'activeTurn.wait'), description: t('source.restore.wait') },
                    { value: 'interrupt', ...choiceOption(t, 'activeTurn.interrupt'), description: t('source.restore.interruptHint') },
                  ]}
                />
                {active === 'wait' && (
                  <Input
                    label={t('source.restore.waitMinutes')}
                    type="number"
                    inputMode="numeric"
                    min={0}
                    max={WAIT_MOST_MINUTES}
                    value={String(minutes)}
                    disabled={apply.isPending}
                    onChange={(event) => {
                      const value = Number(event.target.value);
                      if (Number.isInteger(value) && value >= 0 && value <= WAIT_MOST_MINUTES) choose({ wait_minutes: value });
                    }}
                  />
                )}
                <p className="text-sm text-text-muted">{t('source.restore.boundary')}</p>
                {plan.isFetching && (
                  <p role="status" className="text-sm text-text-muted">
                    {t('source.restore.checking')}
                  </p>
                )}
                {plan.error !== null && !plan.isFetching && <ErrorAlert error={plan.error} action={<Button action="retry" size="sm" onClick={freshPlan}>{t('common.tryAgain')}</Button>} />}
                {shown !== undefined && !plan.isFetching && <PlanBody plan={shown} />}
              </>
            )}
            {following && <JobBody job={job.data} error={job.error} />}
            {apply.error !== null && <ErrorAlert error={apply.error} message={stale ? t('source.restore.stale') : undefined} />}
          </>
        )}
      </div>
    </Dialog>
  );
}

/** What a plan would do, as the person reads it before confirming. */
function PlanBody({ plan }: { plan: RestorePlan }) {
  const { t } = useTranslation();
  const later = plan.paths.filter((path) => path.later_change === true).length;
  return (
    <div className="flex flex-col gap-2">
      {plan.writers.map((writer, index) => (
        <InlineAlert key={writer.thread?.id ?? `hidden-${index}`} tone={writer.active ? 'warning' : 'info'}>
          {writer.thread === null ? t('source.restore.hiddenWriter') : t(writer.active ? 'source.restore.activeWriter' : 'source.restore.sharedWriter', { name: writer.thread.name })}
        </InlineAlert>
      ))}
      {plan.blockers.map((blocker) => ({ blocker, words: restoreBlocker(t, blocker) })).filter((entry) => entry.words !== null).map(({ blocker, words }) => (
        <InlineAlert key={blocker} tone="danger">
          {words}
        </InlineAlert>
      ))}
      {(!plan.undo_available || plan.blockers.includes('complete_undo_unavailable')) && <InlineAlert tone="warning">{t('source.restore.noUndo')}</InlineAlert>}
      {plan.paths.length === 0 ? (
        <EmptyState compact icon={<Undo2 aria-hidden />} title={t('source.restore.noDifference')} />
      ) : (
        <>
          <p className="flex flex-wrap gap-x-3 gap-y-1 text-sm">
            <span>{t('source.restore.count', { count: plan.paths.length })}</span>
            {later > 0 && <span>{t('source.restore.laterCount', { count: later })}</span>}
          </p>
          <ul aria-label={t('source.restore.paths')} tabIndex={0} className="max-h-64 overflow-auto rounded-lg border border-border focus-visible:outline-2 focus-visible:outline-focus">
            {plan.paths.map((path) => (
              <li key={path.path} className="flex flex-col gap-1 border-b border-border px-3 py-2 last:border-0" data-restore-path={path.path}>
                <span className="flex min-w-0 flex-wrap items-center gap-2 text-sm">
                  <Badge tone={path.action === 'remove' ? 'warning' : 'neutral'}>{t(`source.restore.action.${path.action}`)}</Badge>
                  <code className="min-w-0 wrap-anywhere">{path.path}</code>
                </span>
                {path.later_change !== false && (
                  <span className="text-xs text-text-muted">
                    {path.later_change === null ? t('source.restore.laterUnknown') : path.later_writers.length === 0 ? t('source.restore.laterChanged') : t('source.restore.laterBy', { names: path.later_writers.map((writer) => writer.name).join(', ') })}
                  </span>
                )}
              </li>
            ))}
          </ul>
        </>
      )}
      {plan.ignored.length > 0 && (
        <details className="text-sm text-text-muted">
          <summary>{t('source.restore.ignored', { count: plan.ignored.length })}</summary>
          {plan.ignored.map((path) => (
            <p key={path} className="font-mono text-xs wrap-anywhere">
              {path}
            </p>
          ))}
        </details>
      )}
      <p className="text-xs text-text-muted">
        {t('source.restore.expires')} <RelativeTime at={plan.expires_at} />
      </p>
      {plan.undo_available && <p className="text-sm text-text-muted">{t('source.restore.undoKept')}</p>}
    </div>
  );
}

/** The job of a confirmed restore, as far as it has come. */
function JobBody({ job, error }: { job: Job | undefined; error: unknown }) {
  const { t } = useTranslation();
  if (job === undefined) return error === null ? <SkeletonRows rows={2} /> : <ErrorAlert error={error} />;
  return (
    <div className="flex flex-col gap-2" data-restore-job={job.state}>
      {job.error === null && isOver(job.state) && job.state !== 'succeeded' && <p role="status" className="text-sm">
        {t(`source.job.${job.state}`)}
      </p>}
      {!isOver(job.state) && <><Meter look="progress" value={job.progress.percent} label={t('source.restore.titleTurn')} /><JobActivityLine job={job} /></>}
      {job.state === 'succeeded' && <InlineAlert tone="success">{t('source.restore.done')}</InlineAlert>}
      {job.error !== null && <ErrorAlert error={job.error} />}
      {job.error?.undone === true && <p className="text-sm text-text-muted">{t('source.restore.rolledBack')}</p>}
    </div>
  );
}
