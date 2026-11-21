/** Shared Tooltip component (overlays); primitives are wrapped here.
 *
 * The name of a control that shows no words, or the exact value behind a short one. It appears when
 * the pointer rests on its trigger or the keyboard reaches it, and holds nothing a person must read to
 * go on: what matters is on the screen itself.
 */
import { Slot, Tooltip as Primitive } from 'radix-ui';
import { createContext, type ComponentPropsWithRef, type HTMLAttributes, type ReactNode, type Ref, useCallback, useContext, useEffect, useImperativeHandle, useLayoutEffect, useRef, useState, useSyncExternalStore } from 'react';

import { AUTOMATIC_FOCUS } from '@/ui/focus';
import { Kbd } from '@/ui/Kbd';
import { ChoicePicker } from '@/ui/Choice';
import { usePromptCovered, useTransientLayer } from '@/ui/PromptScope';
import { installOverflowTooltips, OVERFLOW_REST_MS } from '@/ui/overflowTooltips';
import { floatingAttachment, floatingClasses } from '@/ui/Popover';

/** How long the pointer rests before a name appears: long enough not to flicker while it crosses a bar. */
const REST_MS = OVERFLOW_REST_MS;
/** Once one name is showing, the next appears at once for this long: reading along a row of buttons. */
const SKIP_REST_MS = 300;

/**
 * Stands once round the application: every tooltip shares its timing. `overflow` draws the fallback for text that the screen
 * shortens, which a harness with no layout (and so no text that is shortened) may leave out where it proves nothing of it.
 */
export function TooltipProvider({ children, overflow = true }: { children: ReactNode; overflow?: boolean }) {
  const [labels] = useState(() => new ControlLabels());
  useEffect(() => (overflow ? installOverflowTooltips(document) : undefined), [overflow]);
  return (
    <Primitive.Provider delayDuration={REST_MS} skipDelayDuration={SKIP_REST_MS}>
      <LabelContext.Provider value={labels}>{children}</LabelContext.Provider>
      <ControlLabelHost labels={labels} />
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

type Placement = Pick<TooltipProps, 'side' | 'align' | 'shortcut'>;
interface TriggerState { state: string; description: string | undefined }
const CLOSED: TriggerState = { state: 'closed', description: undefined };
type Report = (state: TriggerState) => void;
interface Label extends Placement { text: string; element: HTMLElement; report: Report }

/** A toolbar's plain labels use one Radix interaction and positioning tree. Only its current owner receives updates. */
class ControlLabels {
  events: HTMLAttributes<HTMLElement> = {};
  private listeners = new Set<() => void>();
  private value: { label: Label | null; lifetime: number } = { label: null, lifetime: 0 };
  snapshot = () => this.value;
  subscribe = (fn: () => void) => { this.listeners.add(fn); return () => { this.listeners.delete(fn); }; };
  bind(events: HTMLAttributes<HTMLElement>): () => void {
    this.events = events;
    return () => { if (this.events === events) this.events = {}; };
  }

  choose(label: Label): void {
    const before = this.value.label;
    if (before?.report === label.report && before.element === label.element && before.text === label.text
      && before.side === label.side && before.align === label.align && before.shortcut === label.shortcut) return;
    if (before?.report !== label.report) before?.report(CLOSED);
    this.value = { label, lifetime: this.value.lifetime };
    this.listeners.forEach((notify) => notify());
  }

  forget(report: Report): void {
    if (this.value.label?.report !== report) return;
    report(CLOSED);
    // Retiring the native root also cancels any pointer timer for a control that just disappeared.
    this.value = { label: null, lifetime: this.value.lifetime + 1 };
    this.listeners.forEach((notify) => notify());
  }
}

const LabelContext = createContext<ControlLabels | null>(null);

/** The visible control supplies the anchor and receives Radix's trigger attributes and events. */
function LabelAnchor({ labels, label, ref, ...props }: HTMLAttributes<HTMLElement> & {
  labels: ControlLabels; label: Label | null; ref?: Ref<HTMLElement | null>; 'data-state'?: string;
}) {
  const element = label?.element ?? null;
  useImperativeHandle<HTMLElement | null, HTMLElement | null>(ref, () => element, [element]);
  useLayoutEffect(() => labels.bind(props));
  const state = props['data-state'] ?? 'closed', description = props['aria-describedby'];
  useLayoutEffect(() => {
    if (label !== null) label.report(state === 'closed' ? CLOSED : { state, description });
  }, [label, state, description]);
  return null;
}

function ControlLabelHost({ labels }: { labels: ControlLabels }) {
  const { label, lifetime } = useSyncExternalStore(labels.subscribe, labels.snapshot, labels.snapshot);
  return <Primitive.Root key={lifetime}>
    <Primitive.Trigger asChild><LabelAnchor labels={labels} label={label} /></Primitive.Trigger>
    {label !== null && <TipContent content={label.text} side={label.side} align={label.align} shortcut={label.shortcut} />}
  </Primitive.Root>;
}

export function ControlLabel(props: Placement & { content: string; children: ReactNode }) {
  const labels = useContext(LabelContext);
  return labels === null ? <Tooltip {...props} /> : <LabelTrigger {...props} labels={labels} />;
}

function LabelTrigger({ content, side, align, shortcut, children, labels }: Placement & { content: string; children: ReactNode; labels: ControlLabels }) {
  const covered = usePromptCovered();
  const [state, report] = useState(CLOSED);
  const control = useRef<HTMLElement | null>(null);
  const select = (element: HTMLElement) => labels.choose({ element, text: content, side, align, shortcut, report });
  const owns = () => labels.snapshot().label?.report === report;
  const attach = useCallback((element: HTMLElement | null) => {
    control.current = element;
    const current = labels.snapshot().label;
    if (current?.report !== report) return;
    // Composed refs can hand back the same focused DOM node in this commit.
    if (element === null) queueMicrotask(() => {
      if (labels.snapshot().label?.element !== control.current) labels.forget(report);
    });
    else if (current.element !== element) labels.forget(report);
  }, [labels]);
  useLayoutEffect(() => {
    const label = labels.snapshot().label;
    if (label?.report !== report) return;
    if (covered || content === '') labels.forget(report);
    else labels.choose({ ...label, text: content, side, align, shortcut });
  }, [content, side, align, shortcut, labels, covered]);
  return <Slot.Root ref={attach} data-tooltip-trigger={content === '' ? undefined : ''} data-state={state.state} aria-describedby={state.description}
    onFocus={(event) => {
      if (covered || !content || event.defaultPrevented || !event.currentTarget.contains(event.target) || event.currentTarget.closest(`[${AUTOMATIC_FOCUS}]`) !== null) return;
      select(event.currentTarget); labels.events.onFocus?.(event);
    }}
    onPointerMove={(event) => {
      if (covered || !content || event.defaultPrevented || event.pointerType === 'touch' || !(event.target instanceof Node) || !event.currentTarget.contains(event.target)) return;
      select(event.currentTarget); labels.events.onPointerMove?.(event);
    }}
    onPointerDown={(event) => {
      if (covered || !content || event.defaultPrevented || !(event.target instanceof Node) || !event.currentTarget.contains(event.target)) return;
      select(event.currentTarget); labels.events.onPointerDown?.(event);
    }}
    onPointerLeave={(event) => { if (owns()) labels.events.onPointerLeave?.(event); }}
    onBlur={(event) => { if (owns()) labels.events.onBlur?.(event); }}
    onClick={(event) => { if (owns()) labels.events.onClick?.(event); }}>
    {children}
  </Slot.Root>;
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
          if (!event.currentTarget.contains(event.target) || event.currentTarget.closest(`[${AUTOMATIC_FOCUS}]`) !== null) event.preventDefault();
        }}
        onPointerMove={(event) => {
          handedDown.onPointerMove?.(event);
          if (event.target instanceof Node && !event.currentTarget.contains(event.target)) event.preventDefault();
        }}>
        {children}
      </Primitive.Trigger>
      <TipContent content={content} side={side} align={align} shortcut={shortcut} />
    </Primitive.Root>
  );
}

function TipContent({ content, side = 'top', align = 'center', shortcut }: Placement & { content: ReactNode }) {
  return <Primitive.Portal>
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
      </Primitive.Portal>;
}
