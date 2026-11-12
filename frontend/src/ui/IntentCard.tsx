/** An intent card shares pointer, focus and pinned-click behavior, stays within the screen and keeps nested cards present. */
import { Popover } from 'radix-ui';
import { type ReactElement, type ReactNode, useCallback, useEffect, useRef, useState } from 'react';
import { useCardChildren, useChildCard, useKeepParentCard } from '@/ui/CardNesting';
import { placeStyle, useLayerRoom } from '@/ui/layerRoom';
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
  // The card takes the room it has for the body it has now, not the body it opened with.
  const opener = useRef<HTMLElement | null>(null);
  const attachOpener = useCallback((node: HTMLElement | null) => { opener.current = node; }, []);
  const { place, attach } = useLayerRoom({ shown: open && !covered, anchor: opener, side, watch: children, gap: 6 });
  const attachContent = useCallback((node: HTMLDivElement | null) => { content.current = node; attach(node); }, [attach]);
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
  // A directly owned people list shrinks within the card's existing height after its padding is taken.
  const classes = `z-(--layer-dropdown) w-(--menu-max-width) max-w-[min(20rem,calc(100vw-2rem))] overflow-y-auto [&:has(>[data-people-list])]:flex [&:has(>[data-people-list])]:flex-col ${padded ? 'p-3' : ''} text-base text-text ${cardClasses}`;
  return <Popover.Root open={open} onOpenChange={(next) => { if (!next) pinned(false); }}>
    <Popover.Trigger asChild ref={attachOpener}
      onPointerEnter={(event) => { if (event.pointerType === 'touch') return; clearTimer(); timer.current = setTimeout(() => shown(true), 700); }}
      onPointerLeave={leave}
      onFocus={() => { if (suppressFocus.current) { suppressFocus.current = false; return; } shown(true); }}
      onBlur={leave}
      onClick={(event) => { if (event.currentTarget instanceof HTMLAnchorElement && (event.ctrlKey || event.metaKey || event.shiftKey || event.altKey)) return; event.preventDefault(); event.stopPropagation(); pinned(!kept); }}>
      {trigger}
    </Popover.Trigger>
    <Popover.Portal><Popover.Content data-inspection-card ref={attachContent} aria-label={label} style={covered ? { display: 'none' } : placeStyle(place)} side={place.side} align="start" sideOffset={6} collisionPadding={8}
      className={`max-h-[min(22rem,var(--floating-height,var(--radix-popover-content-available-height)))] ${classes}`}
      onPointerEnter={clearTimer} onPointerLeave={leave} onFocusCapture={clearTimer} onBlurCapture={leave}
      onInteractOutside={(event) => { if (covered || isInspectionTarget(event.target)) event.preventDefault(); }}
      onOpenAutoFocus={(event) => event.preventDefault()}>
      {open && nested.wrap(typeof children === 'function' ? children() : children)}
    </Popover.Content></Popover.Portal>
  </Popover.Root>;
}
