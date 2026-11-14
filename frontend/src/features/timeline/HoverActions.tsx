/** Conversation actions reserve their row and remain reachable by keyboard and touch. */
import type { ReactNode } from 'react';
import { useCompactControls } from '@/features/workbench/Narrow';
import { cx } from '@/ui/cx';

export function HoverActions({ owner, end = false, children }: { owner: 'person' | 'turn'; end?: boolean; children: ReactNode }) {
  const compact = useCompactControls();
  return <div data-hover-actions={owner} data-always-visible={compact || undefined} className={cx('flex min-h-(--control-sm) shrink-0 items-center gap-1', end && 'justify-end', !compact && (owner === 'person'
    ? 'opacity-0 transition-opacity group-focus-within/item:opacity-100 group-hover/item:opacity-100'
    : 'opacity-0 transition-opacity group-focus-within/turn:opacity-100 group-hover/turn:opacity-100'))}>{children}</div>;
}
