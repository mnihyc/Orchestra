/** Shared Tooltip component (overlays); primitives are wrapped here.
 *
 * The name of a control that shows no words, or the exact value behind a short one. It appears when
 * the pointer rests on its trigger or the keyboard reaches it, and holds nothing a person must read to
 * go on: what matters is on the screen itself.
 */
import { Slot, Tooltip as Primitive } from 'radix-ui';
import { type ComponentPropsWithRef, type ReactNode, useEffect } from 'react';

import { Kbd } from '@/ui/Kbd';
import { ChoicePicker } from '@/ui/Choice';
import { useTransientLayer } from '@/ui/PromptScope';
import { installOverflowTooltips, OVERFLOW_REST_MS } from '@/ui/overflowTooltips';
import { floatingAttachment, floatingClasses } from '@/ui/Popover';

/** How long the pointer rests before a name appears: long enough not to flicker while it crosses a bar. */
const REST_MS = OVERFLOW_REST_MS;
/** Once one name is showing, the next appears at once for this long: reading along a row of buttons. */
const SKIP_REST_MS = 300;

/** Stands once round the application: every tooltip shares its timing. */
export function TooltipProvider({ children }: { children: ReactNode }) {
  useEffect(() => installOverflowTooltips(document), []);
  return (
    <Primitive.Provider delayDuration={REST_MS} skipDelayDuration={SKIP_REST_MS}>
      {children}
    </Primitive.Provider>
  );
}

export interface TooltipProps extends Omit<ComponentPropsWithRef<'button'>, 'content' | 'children'> {
  /** What it says. Nothing is shown when this is empty. */
  content: ReactNode;
  /** A shortcut shown after the words. */
  shortcut?: string | undefined;
  side?: 'top' | 'right' | 'bottom' | 'left';
  align?: 'start' | 'center' | 'end';
  /** A touch hold may reveal the same name as pointer rest and keyboard focus. */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  /** The one element it belongs to; it must take a ref and the pointer. */
  children: ReactNode;
}

/**
 * What a control that opens something (a popover, a menu) hands to the element it stands round goes on
 * to that element: a tooltip between the two does not stop the one from opening the other.
 */
export function Tooltip({ content, shortcut, side = 'top', align = 'center', children, open: suppliedOpen, onOpenChange, ...handedDown }: TooltipProps) {
  const [open, changeOpen, covered] = useTransientLayer(suppliedOpen, onOpenChange);
  if (content === null || content === undefined || content === '' || content === false) return <Slot.Root {...handedDown}>{children}</Slot.Root>;
  return (
    <Primitive.Root open={open && !covered} onOpenChange={changeOpen}>
      <Primitive.Trigger asChild data-tooltip-trigger="" {...handedDown}
        // A portalled child card does not focus or hover its ancestor's tooltip trigger.
        onFocus={(event) => {
          handedDown.onFocus?.(event);
          if (!event.currentTarget.contains(event.target)) event.preventDefault();
        }}
        onPointerMove={(event) => {
          handedDown.onPointerMove?.(event);
          if (event.target instanceof Node && !event.currentTarget.contains(event.target)) event.preventDefault();
        }}>
        {children}
      </Primitive.Trigger>
      <Primitive.Portal>
        <Primitive.Content
          data-tooltip-content=""
          side={side}
          align={align}
          sideOffset={6}
          collisionPadding={8} {...floatingAttachment}
          className={`z-(--layer-tooltip) flex animate-rise-in items-start gap-2 overflow-y-auto rounded-sm bg-text px-2 py-1 text-xs text-bg shadow-md ${floatingClasses}`}
        >
          <span className="wrap-anywhere"><ChoicePicker>{content}</ChoicePicker></span>
          {shortcut !== undefined && <Kbd shortcut={shortcut} tone="inverse" />}
        </Primitive.Content>
      </Primitive.Portal>
    </Primitive.Root>
  );
}
