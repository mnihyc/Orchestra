/** A separately read part owns its loading line, failure and exact-key read-only retry. */
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { hasReadFailure, type RecoverableRead } from '@/api/reads';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonGroup, SkeletonRows } from '@/ui/Skeleton';

export function ReadFeedback({ read, reads = [], pending = false, title, pendingMessage }: {
  read?: RecoverableRead;
  reads?: readonly RecoverableRead[];
  pending?: boolean;
  title?: ReactNode;
  pendingMessage?: ReactNode;
}) {
  const { t } = useTranslation();
  const all = read === undefined ? reads : [read, ...reads];
  if (all.some(hasReadFailure)) return <>{all.map((owner, at) => !hasReadFailure(owner) ? null : <ErrorState
    key={at} compact error={owner.error} title={title} retrying={owner.isFetching === true}
    onRetry={owner.refetch === undefined ? undefined : () => { void owner.refetch?.({ cancelRefetch: false }); }}
    action={owner.refetch === undefined ? undefined : <Button size="sm" pending={owner.isFetching === true}
      onClick={() => { void owner.refetch?.({ cancelRefetch: false }); }}>{t('common.tryAgain')}</Button>} />)}</>;
  return !pending ? null : pendingMessage === undefined ? <SkeletonRows reads={all} rows={1} />
    : <SkeletonGroup reads={all}><span className="text-sm text-text-muted">{pendingMessage}</span></SkeletonGroup>;
}
