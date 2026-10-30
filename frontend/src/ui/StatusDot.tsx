/** Shared StatusDot component (status); primitives are wrapped here.
 *
 * A small dot that says how something stands, always with words: beside it where there is room,
 * otherwise for screen readers and under the pointer.
 */
import { cx } from '@/ui/cx';
import { Tooltip } from '@/ui/Tooltip';
import { StatusChip } from '@/ui/StateChip';

export type DotTone = 'info' | 'success' | 'warning' | 'danger' | 'accent' | 'idle' | 'off';

export interface StatusDotProps {
  tone: DotTone;
  /** What the dot says. */
  label: string;
  /** Writes the words beside the dot instead of keeping them for screen readers and the pointer. */
  showLabel?: boolean;
  className?: string;
}

const TONES: Record<DotTone, string> = {
  info: 'bg-info',
  success: 'bg-success',
  warning: 'bg-warning',
  danger: 'bg-danger',
  accent: 'bg-accent',
  idle: 'border border-text-subtle',
  off: 'bg-text-disabled',
};

export function StatusDot({ tone, label, showLabel = false, className }: StatusDotProps) {
  const dot = <span aria-hidden className={cx('inline-block size-2 shrink-0 rounded-full', TONES[tone])} />;
  if (showLabel) {
    return (
      <StatusChip tone={tone} icon={dot} className={className}>
        {label}
      </StatusChip>
    );
  }
  return (
    <Tooltip content={label}>
      <span role="img" aria-label={label} className={cx('inline-flex size-4 shrink-0 items-center justify-center', className)}>
        {dot}
      </span>
    </Tooltip>
  );
}
