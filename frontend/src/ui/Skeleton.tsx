/** Shared Skeleton component (states); primitives are wrapped here.
 *
 * The shape of content that is on its way: a first load shows where things will be and never a
 * spinner over the whole view. A screen reader is told once, by whatever holds the skeletons, that
 * the view is loading; the shapes themselves say nothing.
 */
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { cx } from '@/ui/cx';
import { ErrorState } from '@/ui/ErrorState';
import { StateLine } from '@/ui/StateLine';
import { NetworkError } from '@/lib/requests';

/** The read that owns these shapes, including an aggregate's independent initial reads.
 * Non-query waits can keep using the same shapes without reporting a request state.
 */
export interface SkeletonRead {
  data?: unknown;
  failureCount?: number;
  isFetching?: boolean;
  isError?: boolean;
  error?: unknown;
  refetch?: () => unknown;
}
export interface SkeletonReads {
  read?: SkeletonRead;
  reads?: readonly SkeletonRead[];
}

export interface SkeletonProps {
  /** Its size and shape, as classes: `h-4 w-1/2`, `size-8 rounded-full`. */
  className?: string;
}

export function Skeleton({ className }: SkeletonProps) {
  return <span aria-hidden className={cx('block animate-skeleton rounded-sm bg-bg-muted', className)} />;
}

/** Holds the skeletons of one view and says, once, that it is loading. */
export function SkeletonGroup({ children, className, read, reads = [] }: { children: ReactNode; className?: string } & SkeletonReads) {
  const { t } = useTranslation();
  const waiting = (read === undefined ? reads : [read, ...reads]).filter((query) => query.data === undefined);
  const failed = waiting.find((query) => query.isError === true && query.isFetching !== true);
  if (failed !== undefined) return <ErrorState compact error={failed.error}
    message={failed.error instanceof NetworkError ? t('ui.read.failed') : undefined}
    onRetry={failed.refetch === undefined ? undefined : () => { void failed.refetch?.(); }} className={className} />;
  const retrying = waiting.some((query) => (query.failureCount ?? 0) > 0);
  return (
    <div role="status" aria-busy className={className}>
      {retrying ? <StateLine compact message={t('ui.read.retrying')} className="text-text-muted" /> : <span className="sr-only">{t('common.loading')}</span>}
      {children}
    </div>
  );
}

/** Rows of a list that is loading: a mark and a line each, of differing lengths as real names are. */
export function SkeletonRows({ rows = 6, className, read, reads }: { rows?: number; className?: string } & SkeletonReads) {
  const widths = ['w-2/3', 'w-1/2', 'w-3/4', 'w-2/5', 'w-3/5', 'w-1/3'];
  return (
    <SkeletonGroup read={read} reads={reads} className={cx('flex flex-col', className)}>
      {Array.from({ length: rows }, (_, row) => (
        <div key={row} className="flex h-(--row-height) items-center gap-2 px-3">
          <Skeleton className="size-4" />
          <Skeleton className={cx('h-3', widths[row % widths.length])} />
        </div>
      ))}
    </SkeletonGroup>
  );
}
