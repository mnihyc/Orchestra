/** One compact status sentence and its one next action, shared by empty views and failures. */
import type { ReactNode } from 'react';
import { cx } from '@/ui/cx';

export interface StateLineProps {
  title?: ReactNode;
  message?: ReactNode;
  icon?: ReactNode;
  /** One useful next action; the surrounding toolbar may already provide it. */
  action?: ReactNode;
  tone?: 'neutral' | 'danger' | 'warning';
  role?: 'alert' | 'status' | 'group';
  titleAs?: 'h1' | 'h2' | 'h3' | 'p';
  compact?: boolean;
  /** Embedded empty views retain a title and a separate, quieter explanation. */
  separateMessage?: boolean;
  className?: string;
}

function words(value: ReactNode): string | null {
  return typeof value === 'string' ? value.trim().replace(/\s+/g, ' ').replace(/[.!?。！？]+$/u, '').toLocaleLowerCase() : null;
}

export function StateLine({ title, message, icon, action, tone = 'neutral', role, titleAs: Title = 'p', compact = false, separateMessage = false, className }: StateLineProps) {
  const heading = words(title);
  const explanation = words(message);
  const repeated = heading !== null && explanation !== null && heading === explanation;
  const hasTitle = title !== undefined && title !== null && title !== false && title !== '';
  const hasMessage = !repeated && message !== undefined && message !== null && message !== false && message !== '';
  return <div role={role} data-state-line data-state-tone={tone} className={cx(
    'flex min-w-0 flex-wrap items-start gap-x-2 gap-y-1 text-sm text-text', compact ? 'py-1' : 'py-3',
    tone === 'danger' && 'rounded-md border border-border bg-danger-subtle px-3',
    tone === 'warning' && 'rounded-md border border-border bg-warning-subtle px-3', className,
  )}>
    {icon !== undefined && <span aria-hidden className={cx('mt-[calc((1.5em-1rem)/2)] inline-flex size-4 shrink-0 items-center justify-center [&>svg]:size-4', tone === 'danger' ? 'text-danger-text' : tone === 'warning' ? 'text-warning-text' : 'text-text-muted')}>{icon}</span>}
    <div data-state-copy className="min-w-0 flex-1 basis-[min(100%,12rem)] leading-[1.5] wrap-anywhere">
      {hasTitle && <Title className={cx(separateMessage ? 'block' : 'inline', 'font-medium')}>{title}</Title>}
      {hasTitle && hasMessage && !separateMessage && ' '}
      {hasMessage && <div className={separateMessage ? 'block text-text-muted [&>p]:block' : 'inline [&>p]:inline'}>{message}</div>}
    </div>
    {action !== undefined && action !== null && action !== false && <div data-state-action className="min-w-0 max-w-full shrink-0 self-start [&_button]:h-auto [&_button]:min-h-(--control-sm) [&_button]:max-w-full [&_button]:whitespace-normal [&_button]:wrap-anywhere [&>a]:min-h-(--control-sm)">{action}</div>}
  </div>;
}
