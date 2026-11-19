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
import { ErrorAlert } from '@/ui/ErrorState';
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
  const actions: { action: CallRecoveryAction; label: string }[] = attention
    ? [{ action: 'retry', label: t('calls.retry') }, { action: 'continue', label: t('calls.continue') }]
    : [...(recovery.state === 'scheduled' ? [{ action: 'retry_now' as const, label: t('calls.retryNow') }] : []), { action: 'stop', label: t('calls.stop') }];
  return <div className="flex flex-wrap items-center gap-1">
    {actions.map(({ action, label }) => <Tooltip key={action} content={reason}><Button action={action === 'stop' ? 'cancel' : action === 'continue' ? 'start' : 'retry'} size="sm" variant="secondary" blocked={!may} disabled={pending} onClick={() => onAction(action)}>{label}</Button></Tooltip>)}
    {attention &&
      <Tooltip content={!maySwitch ? t('calls.configureRequired') : undefined}><Button size="sm" variant="secondary" blocked={!maySwitch} disabled={pending} onClick={() => openChange({ kind: 'thread', id: thread.id, name: thread.name }, { kind: 'switch' })}>{t('calls.switch')}</Button></Tooltip>
    }
  </div>;
}

export function ThreadCallRecovery({ thread, banner = false }: { thread: Thread; banner?: boolean }) {
  const { t } = useTranslation();
  const read = useThreadReadRef(thread.id);
  const revision = useThreadPart(thread.id, (view) => view.thread?.seq ?? null);
  const seen = useRef(revision);
  const state = useQuery(callRecoveryQuery(thread.id));
  const { refetch } = state;
  useEffect(() => {
    if (seen.current === revision) return;
    seen.current = revision;
    void refetch({ cancelRefetch: false });
  }, [revision, refetch]);
  const control = useMutation({ mutationFn: (action: CallRecoveryAction) => controlCallRecovery(read, action) });
  if (state.isPending) return <SkeletonRows rows={1} />;
  if (state.isError) return <ErrorAlert error={state.error} action={<Button size="sm" onClick={() => void state.refetch()}>{t('common.tryAgain')}</Button>} />;
  const recovery = state.data.recovery;
  if (recovery === null) return null;
  const attention = recovery.state === 'stopped' || recovery.state === 'waiting';
  if (banner && !attention) return null;
  if (!banner && attention) return null;
  const controls = <CallRecoveryControls thread={thread} recovery={recovery} pending={control.isPending} onAction={(action) => control.mutate(action)} />;
  const status = <span className="inline-flex min-w-0 flex-wrap items-center gap-1"><CallRecoveryStatus recovery={recovery} /><CallHelp section="recovery" /></span>;
  return <div className="min-w-0">
    {banner ? <Banner tone="warning" action={controls} actionBelowOnMobile>{status}</Banner> : <div role="status" aria-label={t('calls.status')} className="flex min-w-0 flex-wrap items-center justify-center gap-x-2 gap-y-1 py-1 text-sm text-text-muted">{status}{!attention && controls}</div>}
    {control.error !== null && <ErrorAlert error={control.error} />}
  </div>;
}
