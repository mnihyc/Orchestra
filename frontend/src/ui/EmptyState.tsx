/** Shared EmptyState component (states); primitives are wrapped here.
 *
 * What a view shows when it has nothing to show: an icon, one sentence that says why, and one thing
 * to do about it when its reader can do something.
 */
import type { ReactNode } from 'react';

import { cx } from '@/ui/cx';
import { StateLine } from '@/ui/StateLine';

export interface EmptyStateProps {
  /** An icon of the thing that is missing, at its usual size; the frame round it is drawn here. */
  icon?: ReactNode;
  /** One sentence. */
  title: ReactNode;
  /** A whole-page state supplies its page heading; embedded states keep ordinary text. */
  titleAs?: 'h1' | 'h2' | 'h3' | 'p';
  /** A quieter explanation under the title. */
  description?: ReactNode;
  /** The one thing to do. */
  action?: ReactNode;
  /** Less room round it, for a narrow bar. */
  compact?: boolean;
  className?: string;
}

export function EmptyState({ icon, title, titleAs: Title = 'p', description, action, compact = false, className }: EmptyStateProps) {
  const detail = typeof title === 'string' && typeof description === 'string' && title.trim() === description.trim() ? undefined : description;
  if (compact) return <StateLine icon={icon} title={title} titleAs={Title} message={detail} action={action} compact separateMessage className={cx('empty-state-inline', className)} />;
  return <div className={cx('flex min-w-0 flex-col items-center justify-center gap-3 px-6 py-12 text-center', className)}>
    {icon !== undefined && <div aria-hidden className="flex size-10 shrink-0 items-center justify-center rounded-full bg-bg-muted text-text-muted">{icon}</div>}
    <div className="flex max-w-(--container-form) flex-col gap-1">
      <Title className="text-lg font-medium text-text">{title}</Title>
      {detail !== undefined && <div className="text-sm leading-[1.5] text-text-muted">{detail}</div>}
    </div>
    {action !== undefined && <div className="mt-1 flex max-w-full flex-wrap items-center justify-center gap-2">{action}</div>}
  </div>;
}
