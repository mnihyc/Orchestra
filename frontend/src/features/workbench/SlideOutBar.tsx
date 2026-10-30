/** A hidden desktop bar floats over content, retaining the shared drawer's focus and Escape behavior. */
import { type ReactNode, useCallback, useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import type { SideBarId } from './layout';
import { Drawer } from '@/ui/Drawer';

/** A short grace period lets a pointer cross to a menu without dismissing its source. */
const LEAVE_MS = 300;

export function SlideOutBar({ bar, size, open, docked, onClose, children }: { bar: SideBarId; size: number; open: boolean; docked: boolean; onClose: () => void; children: ReactNode }) {
  const { t } = useTranslation();
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const body = useRef<HTMLDivElement>(null);
  const cancel = useCallback(() => { clearTimeout(timer.current); timer.current = undefined; }, []);
  const leave = useCallback(function leave() {
    if (timer.current !== undefined) return;
    timer.current = setTimeout(() => {
      timer.current = undefined;
      // A menu or card keeps the bar it came from until its own dismissal.
      if (body.current?.parentElement?.matches(':hover') === true
        || document.querySelector(`[data-bar-edge="${bar}"]:hover, [role="menu"], [data-radix-popper-content-wrapper]:hover`) !== null) leave();
      else onClose();
    }, LEAVE_MS);
  }, [onClose, bar]);
  useEffect(() => { if (!open) cancel(); return cancel; }, [open, cancel]);
  useEffect(() => {
    if (!open) return;
    const move = (event: PointerEvent) => {
      if (event.pointerType === 'touch') return;
      const target = event.target;
      if (target instanceof Element && (body.current?.parentElement?.contains(target) === true
        || target.closest(`[data-bar-edge="${bar}"], [role="menu"], [data-radix-popper-content-wrapper]`) !== null)) cancel();
      else leave();
    };
    document.addEventListener('pointermove', move);
    return () => document.removeEventListener('pointermove', move);
  }, [open, bar, cancel, leave]);
  useEffect(() => {
    if (!open) return;
    const frame = requestAnimationFrame(() => body.current?.querySelector<HTMLElement>('[role="treeitem"][tabindex="0"], a[href], button:not([disabled]), input')?.focus());
    return () => cancelAnimationFrame(frame);
  }, [open]);
  return <Drawer prompt={false} modal={false} persistent hideTitle scroll={false} side={bar} title={t(`workbench.areas.${bar}`)} open={open}
    restoreFocus={!docked} onOpenChange={(open) => { if (!open) { cancel(); onClose(); } }}
    onPointerEnter={cancel} onPointerLeave={(event) => {
      if (event.pointerType === 'touch') return;
      leave();
    }}
    onClickCapture={(event) => {
      if (event.ctrlKey || event.metaKey || event.altKey || event.shiftKey) return;
      if (event.target instanceof Element && event.target.closest('a[href], [data-workbench-target]') !== null) requestAnimationFrame(onClose);
    }}
    className="slide-out-bar overflow-hidden rounded-xl border bg-bar-surface shadow-lg motion-reduce:animate-none"
    style={{ top: 'var(--spacing-2)', bottom: 'calc(var(--status-bar-height) + var(--spacing-2))', height: 'auto', width: size,
      maxWidth: 'calc(100vw - var(--rail-width) - var(--spacing-4))',
      ...(bar === 'left' ? { left: 'calc(var(--rail-width) + var(--spacing-2))' } : { right: 'var(--spacing-2)' }) }}>
    <div ref={body} className="h-full min-h-0">{children}</div>
  </Drawer>;
}
