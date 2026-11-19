/** An intent card shares pointer, focus and pinned-click behavior, stays within the screen and keeps nested cards present. */
import { Popover } from 'radix-ui';
import { type ReactElement, type ReactNode, useEffect, useRef, useState } from 'react';
import { useCardChildren, useChildCard, useKeepParentCard } from '@/ui/CardNesting';
import { cardClasses } from '@/ui/Popover';
import { usePromptCovered } from '@/ui/PromptScope';
import { isInspectionTarget } from '@/ui/inspection';

/** Reading a short card never changes the trigger's underlying choice. */
export function IntentCard({ trigger, label, children, onOpenChange, padded = true, side = 'bottom' }: { trigger: ReactElement; label: string; children: ReactNode | (() => ReactNode); onOpenChange?: (open: boolean) => void; padded?: boolean; side?: 'top' | 'right' | 'bottom' | 'left' }) {
  const covered = usePromptCovered();
  const suppressFocus = useRef(false);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const content = useRef<HTMLDivElement>(null);
  const [hover, setHover] = useState(false);
  const [kept, setKept] = useState(false);
  const keepParent = useKeepParentCard();
  const nested = useCardChildren(() => pinned(true));
  const open = hover || kept || nested.open;
  useChildCard(open);
  useEffect(() => () => clearTimeout(timer.current), []);
  function clearTimer() { clearTimeout(timer.current); }
  function shown(next: boolean) { clearTimer(); setHover(next); onOpenChange?.(next || kept); }
  function pinned(next: boolean) {
    clearTimer();
    if (next) keepParent?.();
    setKept(next); setHover(false);
    if (!next) suppressFocus.current = true;
    onOpenChange?.(next);
  }
  function leave() {
    clearTimer();
    // Match the shared hover card's ordinary close delay so the pointer can cross its gap.
    timer.current = setTimeout(() => {
      if (!content.current?.contains(document.activeElement)) shown(false);
    }, 300);
  }
  const classes = `z-(--layer-dropdown) w-(--menu-max-width) max-w-[min(20rem,calc(100vw-2rem))] overflow-y-auto ${padded ? 'p-3' : ''} text-base text-text ${cardClasses}`;
  return <Popover.Root open={open} onOpenChange={(next) => { if (!next) pinned(false); }}>
    <Popover.Trigger asChild
      onPointerEnter={(event) => { if (event.pointerType === 'touch') return; clearTimer(); timer.current = setTimeout(() => shown(true), 700); }}
      onPointerLeave={leave}
      onFocus={() => { if (suppressFocus.current) { suppressFocus.current = false; return; } shown(true); }}
      onBlur={leave}
      onClick={(event) => { if (event.currentTarget instanceof HTMLAnchorElement && (event.ctrlKey || event.metaKey || event.shiftKey || event.altKey)) return; event.preventDefault(); event.stopPropagation(); pinned(!kept); }}>
      {trigger}
    </Popover.Trigger>
    <Popover.Portal><Popover.Content data-inspection-card ref={content} aria-label={label} style={covered ? { display: 'none' } : undefined} side={side} align="start" sideOffset={6} collisionPadding={8}
      className={`max-h-[min(22rem,var(--radix-popover-content-available-height))] ${classes}`}
      onPointerEnter={clearTimer} onPointerLeave={leave} onFocusCapture={clearTimer} onBlurCapture={leave}
      onInteractOutside={(event) => { if (covered || isInspectionTarget(event.target)) event.preventDefault(); }}
      onOpenAutoFocus={(event) => event.preventDefault()}>
      {open && nested.wrap(typeof children === 'function' ? children() : children)}
    </Popover.Content></Popover.Portal>
  </Popover.Root>;
}
