/** Shared Popover component (overlays); primitives are wrapped here.
 *
 * A small card that opens beside the control that asked for it and holds a little more than a
 * tooltip may: a list to pick from, a value with its control, an explanation with a link. It closes
 * on Escape and on a press beside it, and gives the focus back.
 */
import { Popover as Primitive } from 'radix-ui';
import { useLayoutEffect, useRef, useState, type ComponentProps, type ReactNode } from 'react';

import { cx } from '@/ui/cx';
import { ChoicePicker } from '@/ui/Choice';
import { ContentBoundary } from '@/ui/ContentBoundary';
import { useTransientLayer } from '@/ui/PromptScope';
import { isInspectionTarget } from '@/ui/inspection';
import { floatingAttachment, floatingClasses, FLOATING_GAP, FLOATING_EDGE } from '@/ui/floating';

export interface PopoverProps {
  /** The one control that opens it. */
  trigger: ReactNode;
  /** An existing region anchors a card opened through its context menu instead of a click. */
  anchor?: boolean;
  children: ReactNode;
  /** What the card is, for a screen reader. */
  label: string;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  side?: 'top' | 'right' | 'bottom' | 'left';
  align?: 'start' | 'center' | 'end';
  className?: string;
  /** A picker with its own scrolling list keeps that list as its only viewport. */
  scroll?: 'content' | 'children';
  onOpenAutoFocus?: ComponentProps<typeof Primitive.Content>['onOpenAutoFocus'];
  onCloseAutoFocus?: ComponentProps<typeof Primitive.Content>['onCloseAutoFocus'];
}

export const cardClasses = 'rounded-lg border border-border bg-surface-raised shadow-md';
export { floatingAttachment, floatingClasses, FLOATING_GAP, FLOATING_EDGE, floatingPoint, floatingPlacement } from '@/ui/floating';

/** A submenu may overlap its parent when neither side has room for its readable surface. */
export function useFloatingSubmenu() {
  const trigger = useRef<HTMLDivElement>(null);
  const [surface, attach] = useState<HTMLDivElement | null>(null);
  // Preserve the native submenu's existing gap while there is room for it.
  const gap = 4;
  const [offset, setOffset] = useState(gap);
  useLayoutEffect(() => {
    const anchor = trigger.current;
    if (surface === null || anchor === null) return;
    const measure = () => {
      const box = anchor.getBoundingClientRect(), width = surface.getBoundingClientRect().width;
      const room = Math.max(box.left - FLOATING_EDGE, innerWidth - FLOATING_EDGE - box.right);
      setOffset(Math.min(gap, room - width));
    };
    measure();
    const sizes = new ResizeObserver(measure); sizes.observe(anchor); sizes.observe(surface);
    const movement = new MutationObserver(measure);
    const wrapper = anchor.closest('[data-radix-popper-content-wrapper]');
    if (wrapper !== null) movement.observe(wrapper, { attributes: true, attributeFilter: ['style'] });
    window.addEventListener('resize', measure); document.addEventListener('scroll', measure, true);
    return () => { sizes.disconnect(); movement.disconnect(); window.removeEventListener('resize', measure); document.removeEventListener('scroll', measure, true); };
  }, [surface]);
  return { trigger, attach, offset };
}


export function Popover({
  trigger,
  anchor = false,
  children,
  label,
  open,
  onOpenChange,
  side = 'bottom',
  align = 'start',
  className,
  scroll = 'content',
  onOpenAutoFocus,
  onCloseAutoFocus,
}: PopoverProps) {
  const [shown, changeOpen, covered] = useTransientLayer(open, onOpenChange);
  return (
    <Primitive.Root open={shown} onOpenChange={changeOpen}>
      {anchor ? <Primitive.Anchor data-floating-anchor asChild>{trigger}</Primitive.Anchor> : <Primitive.Trigger asChild>{trigger}</Primitive.Trigger>}
      <Primitive.Portal>
        <Primitive.Content
          style={covered ? { display: 'none' } : undefined}
          onInteractOutside={(event) => { if (covered || isInspectionTarget(event.target)) event.preventDefault(); }}
          aria-label={label}
          side={side}
          align={align}
          sideOffset={FLOATING_GAP}
          collisionPadding={FLOATING_EDGE}
          {...floatingAttachment}
          {...(onOpenAutoFocus === undefined ? {} : { onOpenAutoFocus })}
          {...(onCloseAutoFocus === undefined ? {} : { onCloseAutoFocus })}
          className={cx(
            // Never taller than the room the screen has: a long content scrolls inside the card.
            'relative z-(--layer-dropdown) animate-rise-in p-3 text-base text-text',
            scroll === 'children' ? 'flex flex-col overflow-hidden' : 'overflow-y-auto',
            floatingClasses,
            cardClasses,
            className,
          )}
        >
          <ContentBoundary resetKey={shown}><ChoicePicker>{children}</ChoicePicker></ContentBoundary>
        </Primitive.Content>
      </Primitive.Portal>
    </Primitive.Root>
  );
}
