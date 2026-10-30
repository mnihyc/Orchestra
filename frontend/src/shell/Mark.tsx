/** The application's own mark, drawn in place so that it needs no request and follows the size of the text. */
import { cx } from '@/ui/cx';

export interface MarkProps {
  /** Its size, as classes: `size-8`. */
  className?: string;
  /** What it stands for, where it stands alone; left out where the name is written beside it. */
  label?: string;
}

export function Mark({ className, label }: MarkProps) {
  return (
    <svg
      viewBox="0 0 32 32"
      role={label === undefined ? undefined : 'img'}
      aria-label={label}
      aria-hidden={label === undefined ? true : undefined}
      className={cx('shrink-0', className)}
    >
      <rect width="32" height="32" rx="8" className="fill-accent-solid" />
      <circle cx="15" cy="17" r="7" fill="none" strokeWidth="3" className="stroke-text-on-accent" />
      <circle cx="24.5" cy="7.5" r="2.5" className="fill-text-on-accent" />
    </svg>
  );
}
