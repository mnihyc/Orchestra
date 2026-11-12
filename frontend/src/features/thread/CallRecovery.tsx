/** One recovery sentence and the same controls in the live line and attention banner. */
import { useMutation, useQuery } from '@tanstack/react-query';
import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { callRecoveryQuery, controlCallRecovery } from '@/api/calls';
import type { CallRecovery, CallRecoveryAction, Thread } from '@/generated/api-types';
import { openChange } from '@/features/change/target';
import { holds } from '@/lib/permissions';
import { useThreadPart } from '@/realtime/stores/thread';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { Banner } from '@/ui/Banner';
import { Button } from '@/ui/Button';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { StateLine } from '@/ui/StateLine';
import { SkeletonRows } from '@/ui/Skeleton';
import { Tooltip } from '@/ui/Tooltip';
import { jobQuery } from '@/api/forks';
import { JobActivityLine } from '@/features/change/Progress';
import { CallRecoveryStatus } from '@/features/calls/Status';
import { CallHelp } from '@/features/calls/Help';
export { CallRecoveryStatus } from '@/features/calls/Status';
import type { ReactNode } from 'react';

/** Workflow waits use the existing live job reader and the same recovery words as agent turns. */
export function HandoffRecoveryLine({ job, fallback }: { job: string; fallback: ReactNode }) {
  const current=useQuery(jobQuery(job));
  const recovery=current.data?.call_recovery;
  return current.data === undefined || recovery === null || recovery === undefined ? fallback : <JobActivityLine job={current.data} />;
}

export function CallRecoveryControls({ thread, recovery, pending, onAction }: { thread: Thread; recovery: CallRecovery; pending: boolean; onAction: (action: CallRecoveryAction) => void }) {
  const { t } = useTranslation();
  const may = holds(thread.access, 'thread.interact');
  const attention = recovery.state === 'stopped' || recovery.state === 'waiting';
  const reason = !may ? t('calls.interactRequired') : undefined;
  const maySwitch = holds(thread.access, 'thread.manage');
  const pausedService = typeof recovery.service === 'string';
  const actions: { action: CallRecoveryAction; label: string }[] = pausedService
    ? [{ action: 'retry_now', label: t('calls.resumeService') }, ...(recovery.state === 'scheduled' ? [{ action: 'stop' as const, label: t('calls.stop') }] : [])]
    : attention
    ? [{ action: 'retry', label: t('calls.retry') }, { action: 'continue', label: t('calls.continue') }]
    : [...(recovery.state === 'scheduled' ? [{ action: 'retry_now' as const, label: t('calls.retryNow') }] : []), { action: 'stop', label: t('calls.stop') }];
  return <div className="flex flex-wrap items-center gap-1">
    {actions.map(({ action, label }) => <Tooltip key={action} content={reason}><Button action={action === 'stop' ? 'cancel' : action === 'continue' ? 'start' : 'retry'} size="sm" variant="secondary" blocked={!may} disabled={pending} onClick={() => onAction(action)}>{label}</Button></Tooltip>)}
    {attention && !pausedService &&
      <Tooltip content={!maySwitch ? t('calls.configureRequired') : undefined}><Button size="sm" variant="secondary" blocked={!maySwitch} disabled={pending} onClick={() => openChange({ kind: 'thread', id: thread.id, name: thread.name }, { kind: 'switch' })}>{t('calls.switch')}</Button></Tooltip>
    }
  </div>;
}

export function ThreadCallRecovery({ thread, banner = false, serviceOnly = false }: { thread: Thread; banner?: boolean; serviceOnly?: boolean }) {
  const { t } = useTranslation();
  const read = useThreadReadRef(thread.id);
  const revision = useThreadPart(thread.id, (view) => view.thread?.seq ?? null);
  const seen = useRef(revision);
  const state = useQuery(callRecoveryQuery(thread.id));
  const { refetch } = state;
  const reset = typeof state.data?.recovery?.service === 'string' ? state.data.recovery.due_at : null;
  useEffect(() => {
    if (reset === null) return;
    const due = Date.parse(reset);
    if (!Number.isFinite(due)) return;
    let timer: number | undefined;
    const refresh = () => {
      const left = due - Date.now();
      // Browser timers use a signed 32-bit delay; a later reset schedules its remaining span.
      if (left > 0) timer = window.setTimeout(refresh, Math.min(left, 2_147_483_647));
      else void refetch({ cancelRefetch: false });
    };
    refresh();
    return () => { if (timer !== undefined) window.clearTimeout(timer); };
  }, [reset, refetch]);
  useEffect(() => {
    if (seen.current === revision) return;
    seen.current = revision;
    void refetch({ cancelRefetch: false });
  }, [revision, refetch]);
  const control = useMutation({ mutationFn: (action: CallRecoveryAction) => controlCallRecovery(read, action) });
  if (state.isPending) return serviceOnly
    ? <StateLine compact role="status" message={t('calls.checkingRecovery')} className="text-text-muted" />
    : <SkeletonRows read={state} rows={1} />;
  if (state.isError) {
    const action = <Button size="sm" pending={state.isFetching} onClick={() => void state.refetch()}>{t('common.tryAgain')}</Button>;
    return serviceOnly ? <ErrorState compact title={t('calls.recoveryUnavailable')} error={state.error} action={action} onRetry={() => void state.refetch()} retrying={state.isFetching} />
      : <ErrorAlert error={state.error} action={action} />;
  }
  const recovery = state.data.recovery;
  if (recovery === null || (serviceOnly && typeof recovery.service !== 'string')) return null;
  const pausedService = typeof recovery.service === 'string';
  const attention = recovery.state === 'stopped' || recovery.state === 'waiting';
  if (banner && (!attention || pausedService)) return null;
  if (!banner && attention && !pausedService) return null;
  const controls = <CallRecoveryControls thread={thread} recovery={recovery} pending={control.isPending} onAction={(action) => control.mutate(action)} />;
  const status = <span className="inline-flex min-w-0 flex-wrap items-center gap-1"><CallRecoveryStatus recovery={recovery} /><CallHelp section="recovery" /></span>;
  return <div className="min-w-0">
    {banner ? <Banner tone="warning" action={controls} actionBelowOnMobile>{status}</Banner> : <div role="status" aria-label={t('calls.status')} className="flex min-w-0 flex-wrap items-center justify-center gap-x-2 gap-y-1 py-1 text-sm text-text-muted">{status}{(!attention || pausedService) && controls}</div>}
    {control.error !== null && <ErrorAlert error={control.error} />}
  </div>;
}
