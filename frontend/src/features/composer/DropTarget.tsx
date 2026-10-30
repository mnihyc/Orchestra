/** An accepting place names the destination only while files or an Orch reference are over it. */
import { type ReactNode, useState } from 'react';

import { carriesFiles, carriesReference, draggedFileCount } from '@/lib/drop';
import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { cx } from '@/ui/cx';

export function DropTarget({ label, countLabel, children, onDrop, disabled = false, acceptReferences = true, className }: {
  label: string; children: ReactNode; onDrop: (data: DataTransfer, point: { left: number; top: number }) => void;
  countLabel?: (values: { count: number; number: string }) => string;
  disabled?: boolean; acceptReferences?: boolean; className?: string;
}) {
  const [over, setOver] = useState(false);
  const [count, setCount] = useState<number | null>(null);
  const locale = useAppearance((state) => state.locale);
  return <div className={cx('relative min-w-0', className, over && !disabled && 'rounded-lg outline-2 -outline-offset-2 outline-accent')}
    onDragOver={(event) => {
      if (disabled || !carriesFiles(event.dataTransfer) && !(acceptReferences && carriesReference(event.dataTransfer))) return;
      event.preventDefault(); event.stopPropagation(); event.dataTransfer.dropEffect = 'copy'; setCount(draggedFileCount(event.dataTransfer)); setOver(true);
    }}
    onDragLeave={(event) => { if (!(event.relatedTarget instanceof Node) || !event.currentTarget.contains(event.relatedTarget)) setOver(false); }}
    onDrop={(event) => {
      setOver(false);
      if (disabled || !carriesFiles(event.dataTransfer) && !(acceptReferences && carriesReference(event.dataTransfer))) return;
      event.preventDefault(); event.stopPropagation();
      onDrop(event.dataTransfer, { left: event.clientX, top: event.clientY });
    }}>
    {over && !disabled && <span role="status" className="pointer-events-none absolute inset-x-1 -top-7 z-10 truncate rounded-md border border-border bg-surface px-2 py-1 text-sm text-text-muted shadow-sm">{count !== null && countLabel !== undefined ? countLabel({ count, number: formatNumber(count, locale) }) : label}</span>}
    {children}
  </div>;
}
