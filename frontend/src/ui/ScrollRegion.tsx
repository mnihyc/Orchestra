/** A code or table scroller is a named keyboard destination with the shared visible focus ring. */
import type { ReactNode, Ref } from 'react';
import { ScrollArea as Primitive } from 'radix-ui';
import { cx } from '@/ui/cx';
import './scroll-region.css';

export function ScrollRegion({ label, kind, children, className, ref, visibleScrollbars = false }: {
  label: string;
  kind: 'code' | 'table';
  children: ReactNode;
  className?: string;
  ref?: Ref<HTMLDivElement>;
  /** Tables with optional wide columns keep a persistent scrollbar whenever they overflow. */
  visibleScrollbars?: boolean;
}) {
  if (visibleScrollbars) return <Primitive.Root type="auto" className={cx('scroll-region-custom relative min-h-0 min-w-0 max-w-full', className)}>
    <Primitive.Viewport ref={ref} role="region" aria-label={label} tabIndex={0} data-scroll-allow={kind} className="scroll-region-viewport focus-inset size-full rounded-[inherit]">{children}</Primitive.Viewport>
    {/* Let the thumb fill the track's height while Radix keeps its measured horizontal width. */}
    <Primitive.Scrollbar orientation="horizontal" data-visible-scrollbar="horizontal" className="flex h-3 touch-none flex-col bg-bg-subtle p-0.5 select-none">
      <Primitive.Thumb className="relative flex-1 rounded-full bg-border-strong" />
    </Primitive.Scrollbar>
    <Primitive.Scrollbar orientation="vertical" data-visible-scrollbar="vertical" className="flex w-3 touch-none bg-bg-subtle p-0.5 select-none">
      <Primitive.Thumb className="relative flex-1 rounded-full bg-border-strong" />
    </Primitive.Scrollbar>
    <Primitive.Corner className="bg-bg-subtle" />
  </Primitive.Root>;
  return <div ref={ref} role="region" aria-label={label} tabIndex={0} data-scroll-allow={kind}
    className={cx('focus-inset min-w-0 max-w-full overflow-x-auto', className)}>{children}</div>;
}
