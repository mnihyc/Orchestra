/** Shared StatusBar component (workbench): the line at the foot, each part of which opens what it names.
 *
 * One line of facts about where its reader is. A part is shown only where it means something, says
 * it in a few words with an icon, and opens what it names when it is pressed. Parts that concern
 * the thing in view stand at the left; the ones that concern the application at the right.
 */
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { cx } from '@/ui/cx';
import { Tooltip } from '@/ui/Tooltip';

export interface StatusBarProps {
  /** The parts about the thing in view. */
  children?: ReactNode;
  /** The parts about the application itself, at the far end. */
  end?: ReactNode;
  className?: string;
}

export function StatusBar({ children, end, className }: StatusBarProps) {
  const { t } = useTranslation();
  return (
    <footer
      data-area-stop tabIndex={-1}
      aria-label={t('workbench.status.label')}
      className={cx(
        'flex h-(--status-bar-height) shrink-0 items-stretch justify-between gap-4 border-t border-border bg-bg-subtle px-2 text-xs text-text-muted',
        className,
      )}
    >
      <div className="flex min-w-0 flex-1 items-stretch gap-1">{children}</div>
      <div className="flex shrink-0 items-stretch gap-1">{end}</div>
    </footer>
  );
}

export interface StatusPartProps {
  /** What the part says. */
  children: ReactNode;
  icon?: ReactNode;
  /** A sentence that says what the fact means, read when the pointer rests on the part. */
  hint?: string;
  /** Marks a part that reports something wrong. */
  tone?: 'plain' | 'warning' | 'danger';
  /** Opens what the part names. Without it the part is read and not pressed. */
  onOpen?: (() => void) | undefined;
  /** The name of the part where what it shows is no words: "Help". */
  label?: string;
}

const TONES = { plain: '', warning: 'text-text', danger: 'text-danger-text' };

export function StatusPart({ children, icon, hint, tone = 'plain', onOpen, label }: StatusPartProps) {
  const content = (
    <>
      {icon !== undefined && (
        <span className={cx('flex shrink-0', tone === 'warning' && 'text-warning')}>{icon}</span>
      )}
      <span className="truncate">{children}</span>
    </>
  );
  const classes = cx('flex min-w-0 items-center gap-1 px-1', TONES[tone]);
  return (
    <Tooltip content={hint} side="top">
      {onOpen === undefined ? (
        <span className={classes}>{content}</span>
      ) : (
        <button
          type="button"
          aria-label={label}
          className={cx(classes, 'focus-inset rounded-sm transition-colors hover:bg-bg-muted hover:text-text')}
          onClick={onOpen}
        >
          {content}
        </button>
      )}
    </Tooltip>
  );
}
