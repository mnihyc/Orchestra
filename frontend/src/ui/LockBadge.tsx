/** Shared LockBadge component (status); primitives are wrapped here.
 *
 * A small lock for something that only some people may open: a row that a place above decides, the
 * lock a place sets on everything inside it, a thread whose page was cut loose from its topic's
 * defaults. With words it says what it is beside the lock: where the reader may go to the lock to change
 * it, the badge is a button that goes there, otherwise it only says. Alone, where there is no room for
 * words, the lock says what it means under the pointer and to a screen reader. The lock is never the
 * only sign where there is room: the words are always written.
 */
import { Lock } from 'lucide-react';
import type { ReactNode } from 'react';

import { cx } from '@/ui/cx';
import { Tooltip } from '@/ui/Tooltip';

export type LockBadgeProps =
  | {
      /** What the lock says, written beside it: "Acme decides this". */
      children: ReactNode;
      /** Present where the reader may change the lock: the badge then goes there. */
      onOpen?: () => void;
      className?: string;
      label?: never;
    }
  | {
      /** What the lock alone means here. */
      label: string;
      className?: string;
      children?: never;
      onOpen?: never;
    };

const LOOK = 'inline-flex max-w-full min-w-0 items-center gap-1 rounded-sm text-sm text-text-muted';

export function LockBadge(props: LockBadgeProps) {
  if (props.label !== undefined) {
    return (
      <Tooltip content={props.label}>
        <span role="img" aria-label={props.label} className={cx('inline-flex shrink-0 text-text-subtle', props.className)}>
          <Lock aria-hidden />
        </span>
      </Tooltip>
    );
  }
  const { children, onOpen, className } = props;
  const inside = (
    <>
      <Lock className="shrink-0" size={14} aria-hidden />
      <span className="min-w-0 wrap-anywhere">{children}</span>
    </>
  );
  if (onOpen === undefined) return <span className={cx(LOOK, className)}>{inside}</span>;
  return (
    <button type="button" className={cx(LOOK, 'underline decoration-border-strong underline-offset-2 hover:text-text hover:decoration-text-subtle', className)} onClick={onOpen}>
      {inside}
    </button>
  );
}
