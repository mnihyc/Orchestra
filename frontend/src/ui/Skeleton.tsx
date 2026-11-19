/** Shared Skeleton component (states); primitives are wrapped here.
 *
 * The shape of content that is on its way: a first load shows where things will be and never a
 * spinner over the whole view. A screen reader is told once, by whatever holds the skeletons, that
 * the view is loading; the shapes themselves say nothing.
 */
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { cx } from '@/ui/cx';

export interface SkeletonProps {
  /** Its size and shape, as classes: `h-4 w-1/2`, `size-8 rounded-full`. */
  className?: string;
}

export function Skeleton({ className }: SkeletonProps) {
  return <span aria-hidden className={cx('block animate-skeleton rounded-sm bg-bg-muted', className)} />;
}

/** Holds the skeletons of one view and says, once, that it is loading. */
export function SkeletonGroup({ children, className }: { children: ReactNode; className?: string }) {
  const { t } = useTranslation();
  return (
    <div role="status" aria-busy className={className}>
      <span className="sr-only">{t('common.loading')}</span>
      {children}
    </div>
  );
}

/** Rows of a list that is loading: a mark and a line each, of differing lengths as real names are. */
export function SkeletonRows({ rows = 6, className }: { rows?: number; className?: string }) {
  const widths = ['w-2/3', 'w-1/2', 'w-3/4', 'w-2/5', 'w-3/5', 'w-1/3'];
  return (
    <SkeletonGroup className={cx('flex flex-col', className)}>
      {Array.from({ length: rows }, (_, row) => (
        <div key={row} className="flex h-(--row-height) items-center gap-2 px-3">
          <Skeleton className="size-4" />
          <Skeleton className={cx('h-3', widths[row % widths.length])} />
        </div>
      ))}
    </SkeletonGroup>
  );
}
