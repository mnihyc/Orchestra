/** The same message frame and control row serve thread and built-in conversations. */
import type { ReactNode } from 'react';
import { useCompactControls } from '@/features/workbench/Narrow';
import { cx } from '@/ui/cx';

export function ComposerFrame({ children }: { children: ReactNode }) {
  return <div data-control-boundary className="flex flex-col gap-1 rounded-lg border border-control-border bg-surface p-2 focus-within:border-control-selected">{children}</div>;
}
export function ComposerControlRow({ label, children }: { label: string; children: ReactNode }) {
  const compact = useCompactControls();
  return <div role="group" aria-label={label} className={cx('@container/composer flex min-w-0 flex-wrap items-center', compact ? 'gap-1' : 'gap-2')} data-composer-controls>{children}</div>;
}
