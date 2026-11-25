/** An accepted thread job remains readable even after its original thread has been deleted. */
import { useMutation, useQuery } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { cancelJob, isOver, jobQuery } from '@/api/forks';
import { Frame } from '@/features/change/Frame';
import { committed, JobProgress } from '@/features/change/Progress';
import { Result } from '@/features/change/Result';
import { useRefreshWhenOver } from '@/features/change/refresh';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

export function ThreadJobProgress({ id, waited, open, onClose, moving = true }: { id: string; waited: number | null; open: boolean; onClose: () => void; moving?: boolean }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [cancelling, setCancelling] = useState(false);
  const read = useQuery({ ...jobQuery(id), enabled: open });
  const job = read.data;
  const over = job !== undefined && isOver(job.state);
  const cancel = useMutation({ mutationFn: () => cancelJob(id), onSuccess: () => setCancelling(false) });
  useRefreshWhenOver(over, id);
  const result = job?.result;
  const target = job?.state === 'succeeded' && typeof result === 'object' && result !== null && 'thread_id' in result && typeof result.thread_id === 'string' ? result.thread_id : null;
  const heading = t(moving ? 'thread.moveOut.progress' : 'change.progress.thread');
  return <Frame title={heading} at={over ? 'done' : 'progress'} failed={job?.state === 'failed' || job?.state === 'cancelled' || job?.state === 'lost'} open={open} onClose={onClose} dismissible={!cancel.isPending}
    footer={<>
      <Button onClick={onClose}>{t(over ? 'common.close' : 'change.progress.hide')}</Button>
      {target !== null && <Button variant="primary" onClick={() => { onClose(); void navigate({ to: '/thread/$threadId', params: { threadId: target } }); }}>{t('thread.moveOut.open')}</Button>}
      {job !== undefined && !over && <Button variant="danger" disabled={committed(job) || job.progress.activity?.stopping === true || cancel.isPending} onClick={() => setCancelling(true)}>{t(job.progress.activity?.stopping === true ? 'liveWork.stopping' : 'change.progress.cancel')}</Button>}
    </>}>
    {job === undefined ? read.isError ? <ErrorState compact error={read.error} onRetry={() => void read.refetch()} /> : <SkeletonRows read={read} rows={3} />
      : over ? <Result job={job} done={t(moving ? 'thread.moveOut.done' : 'source.job.succeeded')} stoppedTurn={false} waited={waited} subject={t('thread.moveOut.original')} />
        : <JobProgress job={job} heading={heading} reconnecting={read.failureCount > 0} />}
    {cancel.error !== null && <ErrorAlert error={cancel.error} />}
    <ConfirmDialog open={cancelling} onOpenChange={setCancelling} title={t('change.cancel.title')} confirmLabel={t('change.progress.cancel')} danger pending={cancel.isPending} error={cancel.error ?? undefined} onConfirm={() => cancel.mutate()}>{t('change.cancel.body', { subject: t('thread.moveOut.original') })}</ConfirmDialog>
  </Frame>;
}
