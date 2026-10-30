/** A list of facts, each a name and what is true of it: the shape in which a resource says what it is, and a code box for a command to copy.
 */
import type { ReactNode } from 'react';

import { cx } from '@/ui/cx';
import { CopyButton } from '@/ui/CopyButton';

export interface Fact {
  name: ReactNode;
  value: ReactNode;
}

/** Facts as names and values side by side, and one above the other where the room is narrow. */
export function Facts({ rows, label, className, compact = false }: { rows: readonly Fact[]; label?: string; className?: string; compact?: boolean }) {
  return (
    <dl aria-label={label} data-resource-facts={compact || undefined} className={cx(compact ? 'flex min-w-0 flex-col gap-1 text-sm' : 'grid grid-cols-1 gap-x-4 gap-y-1 text-base sm:grid-cols-[minmax(8rem,12rem)_1fr] sm:gap-y-2', className)}>
      {rows.map((row, index) => (
        // The rows of a list of facts are given whole and never reordered.
        <div key={index} data-item className={compact ? 'flex min-w-0 flex-wrap items-baseline gap-x-3 gap-y-1' : 'contents'}>
          <dt className={compact ? 'w-32 shrink-0 wrap-anywhere text-sm text-text-muted' : 'mt-2 text-sm text-text-muted first:mt-0 sm:mt-0 sm:py-1'}>{row.name}{compact && <span aria-hidden>:</span>}</dt>
          <dd className={cx('min-w-0 max-w-full wrap-anywhere text-text', !compact && 'sm:py-1')}>{row.value}</dd>
        </div>
      ))}
    </dl>
  );
}

/** A command to run somewhere else, in a box of its own with the means to copy it. */
export function CommandBox({ command, label, className }: { command: string; label: string; className?: string }) {
  return (
    <div className={cx('flex items-start gap-2 rounded-lg border border-border bg-bg-subtle py-1 pr-1 pl-3', className)}>
      <pre className="min-w-0 flex-1 py-1 text-sm wrap-anywhere whitespace-pre-wrap text-text">
        <code>{command}</code>
      </pre>
      <CopyButton value={command} label={label} />
    </div>
  );
}
