/** One inspection card for names and highlighted choices, transient on hover or focus and kept on request. */
import { HoverCard, Popover } from 'radix-ui';
import { cloneElement, createContext, isValidElement, type ReactNode, type Ref, useCallback, useContext, useEffect, useImperativeHandle, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { createPortal, flushSync } from 'react-dom';
import { Info } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { cx } from '@/ui/cx';
import { revealFloatingFocus } from '@/ui/floating';
import { cardClasses, floatingAttachment, floatingClasses } from '@/ui/Popover';
import { usePromptCovered } from '@/ui/PromptScope';
import { isInspectionTarget } from '@/ui/inspection';
import { IconButton } from '@/ui/IconButton';
import { useCardChildren, useChildCard, useKeepParentCard } from '@/ui/CardNesting';

export interface CardPreview {
  /** The full permitted name of the thing being inspected. */
  label: string;
  /** Called only while a card is visible; detail readers belong here. */
  content: () => ReactNode;
  /** The same short fragment beside a selected value while its picker is closed. */
  hint?: ReactNode;
  /** The same non-interactive chip inside an option whose row already owns the card. */
  renderName?: (name: ReactNode) => ReactNode;
}

export interface DetailCardProps extends CardPreview {
  children: ReactNode;
  /** A transient card may offer a shorter view than the kept detail card. */
  passingContent?: (() => ReactNode) | undefined;
  /** Existing controls keep their native roles and selection handlers. */
  asChild?: boolean;
  /** A combobox's keyboard highlight does not move DOM focus. */
  active?: boolean | undefined;
  /** Choice clicks choose; Right Arrow moves into their inspection card. */
  choice?: boolean;
  /** Keep a choice's card outside its list when a side placement cannot fit. */
  avoidChoiceList?: boolean;
  side?: 'top' | 'right' | 'bottom' | 'left';
  className?: string;
  /** A virtual highlight can enter the same card without moving the input's caret first. */
  controller?: Ref<CardController> | undefined;
  /** Keeps a choice card inside its popup's logical focus boundary despite its portal. */
  owner?: string | undefined;
  returnFocus?: (() => void) | undefined;
  /** A menu registers inspection as its own native menu item. */
  renderInspect?: (control: ReactNode) => ReactNode;
}

export interface CardController { keep: () => void }

/** Match the shared card width and the existing gap from its anchor and the screen edge. */
const CARD_WIDTH_REM = 20;
const CARD_GAP_PX = 6;
const CARD_EDGE_PX = 8;

interface InspectionPair { anchor: HTMLElement; control: HTMLButtonElement }
interface InspectionScope {
  target: HTMLDivElement | null;
  register: (anchor: HTMLElement, control: HTMLButtonElement) => () => void;
}
const InspectionContext = createContext<InspectionScope | null>(null);

/** Keep a row's button beside it while its semantic parent remains the picker dialog. */
function placeInspection(pair: InspectionPair, target: HTMLElement) {
  pair.control.hidden = false;
  pair.control.style.position = 'absolute';
  const row = pair.anchor.getBoundingClientRect(), frame = target.getBoundingClientRect();
  const control = pair.control.getBoundingClientRect();
  if (row.height <= 0 || frame.height <= 0 || control.height <= 0 || target.offsetWidth <= 0) return;
  // Popup animations scale every child together; positioned offsets stay in CSS pixels.
  const scale = frame.width / target.offsetWidth;
  if (!Number.isFinite(scale) || scale <= 0) return;
  const viewport = pair.anchor.closest('[role="listbox"], [role="menu"]')?.getBoundingClientRect() ?? frame;
  const top = row.top + (row.height - control.height) / 2;
  const left = `${(row.right - frame.left - control.width) / scale}px`, offset = `${(top - frame.top) / scale}px`;
  if (pair.control.style.left !== left) pair.control.style.left = left;
  if (pair.control.style.top !== offset) pair.control.style.top = offset;
  pair.control.hidden = top < Math.max(frame.top, viewport.top) || top + control.height > Math.min(frame.bottom, viewport.bottom);
}

/** Inspection controls share the dialog, outside its listbox/menu, without adding another scroller. */
export function ChoiceInspection({ children }: { children: ReactNode }) {
  const [target, setTarget] = useState<HTMLDivElement | null>(null);
  const targetRef = useRef<HTMLDivElement | null>(null);
  const pairs = useRef(new Set<InspectionPair>());
  const observer = useRef<ResizeObserver | null>(null);
  const attach = useCallback((node: HTMLDivElement | null) => { targetRef.current = node; if (node !== null) setTarget(node); }, []);
  const register = useCallback((anchor: HTMLElement, control: HTMLButtonElement) => {
    const pair = { anchor, control };
    pairs.current.add(pair); observer.current?.observe(anchor);
    if (targetRef.current !== null) placeInspection(pair, targetRef.current);
    return () => { pairs.current.delete(pair); observer.current?.unobserve(anchor); };
  }, []);
  useLayoutEffect(() => {
    if (target === null) return;
    const update = () => { for (const pair of pairs.current) placeInspection(pair, target); };
    const sizes = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(update);
    observer.current = sizes;
    sizes?.observe(target);
    for (const pair of pairs.current) sizes?.observe(pair.anchor);
    const document = target.ownerDocument, view = document.defaultView;
    document.addEventListener('scroll', update, true); view?.addEventListener('resize', update);
    update();
    return () => { sizes?.disconnect(); observer.current = null; document.removeEventListener('scroll', update, true); view?.removeEventListener('resize', update); };
  }, [target]);
  const scope = useMemo(() => ({ target, register }), [target, register]);
  return <InspectionContext value={scope}>{children}<div ref={attach} className="pointer-events-none absolute inset-0 overflow-hidden" /></InspectionContext>;
}

export function DetailCard({ label, content, passingContent, children, asChild = false, active = false, choice = false, avoidChoiceList = false, side = 'bottom', className, controller, owner, returnFocus, renderInspect }: DetailCardProps) {
  const { t } = useTranslation();
  const inheritedInspection = useContext(InspectionContext);
  const inspection = renderInspect === undefined ? inheritedInspection : null;
  const covered = usePromptCovered();
  const [shown, setShown] = useState(false);
  const [kept, setKept] = useState(false);
  const [childKept, setChildKept] = useState(false);
  const keepParent = useKeepParentCard();
  const nested = useCardChildren(() => setChildKept(true));
  const [dismissed, setDismissed] = useState(false);
  const [lastActive, setLastActive] = useState(active);
  const [choiceSide, setChoiceSide] = useState<'right' | 'top' | 'bottom'>('right');
  const [choiceGap, setChoiceGap] = useState(CARD_GAP_PX);
  const [previewFits, setPreviewFits] = useState(true);
  // A new keyboard highlight asks again without replacing the option's DOM node.
  if (active !== lastActive) { setLastActive(active); if (active) setDismissed(false); }
  const restoringFocus = useRef(false);
  const mounted = useRef(false);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const anchor = useRef<HTMLElement | null>(null);
  const card = useRef<HTMLDivElement>(null);
  const passingCard = useRef<HTMLDivElement>(null);
  const releaseInspector = useRef<(() => void) | undefined>(undefined);
  const attachInspector = useCallback((node: HTMLButtonElement | null) => {
    releaseInspector.current?.();
    releaseInspector.current = node !== null && anchor.current !== null && inspection !== null ? inspection.register(anchor.current, node) : undefined;
  }, [inspection]);
  // A narrow picker keeps its options available. Its explicit inspection control and Right Arrow still open the card.
  const visible = kept || !dismissed && (!choice || previewFits) && (shown || active || nested.open || childKept);
  const announceParent = useChildCard(visible);
  const passing = visible && !kept;
  const link = asChild && isValidElement<{ href?: unknown; to?: unknown }>(children) && (children.props.href !== undefined || children.props.to !== undefined);
  const trigger = asChild ? choice && inspection !== null && isValidElement<{ className?: string }>(children)
    ? cloneElement(children, { className: cx(children.props.className, 'pr-[calc(var(--control-sm)+0.5rem)]') }) : children
    : <button type="button" className={cx('inline-flex min-w-0 max-w-full items-center gap-2 rounded-sm text-left', className)}>{children}</button>;
  const classes = cx('z-(--layer-dropdown) [--floating-width:var(--menu-max-width)] overflow-y-auto text-text', floatingClasses, cardClasses);
  const keep = useCallback(() => { announceParent(true); keepParent?.(); setDismissed(false); setShown(false); setKept(true); }, [announceParent, keepParent]);
  useLayoutEffect(() => {
    const node = anchor.current;
    if (!choice || node?.isConnected !== true) return;
    const view = node.ownerDocument.defaultView;
    if (view === null) return;
    const place = () => {
      const current = anchor.current;
      if (current?.isConnected !== true) return;
      const rem = Number.parseFloat(view.getComputedStyle(node.ownerDocument.documentElement).fontSize);
      if (!Number.isFinite(rem) || rem <= 0) return;
      const box = current.getBoundingClientRect();
      const width = Math.min(CARD_WIDTH_REM * rem, view.innerWidth - CARD_EDGE_PX * 2);
      const room = Math.max(box.left, view.innerWidth - box.right);
      setPreviewFits(room >= width + CARD_GAP_PX + CARD_EDGE_PX);
      if (room >= width + CARD_GAP_PX + CARD_EDGE_PX) {
        setChoiceSide('right'); setChoiceGap(CARD_GAP_PX); return;
      }
      const list = avoidChoiceList ? current.closest('[role="listbox"]')?.getBoundingClientRect() : undefined;
      if (list === undefined) { setChoiceSide('bottom'); setChoiceGap(CARD_GAP_PX); return; }
      const above = list.top - CARD_EDGE_PX;
      const below = view.innerHeight - list.bottom - CARD_EDGE_PX;
      const top = above >= below;
      setChoiceSide(top ? 'top' : 'bottom');
      setChoiceGap(Math.max(CARD_GAP_PX, (top ? box.top - list.top : list.bottom - box.bottom) + CARD_GAP_PX));
    };
    place();
    const sizes = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(place);
    sizes?.observe(node);
    const listFrame = avoidChoiceList ? node.closest('[role="listbox"]')?.parentElement : null;
    const moves = typeof MutationObserver === 'undefined' || listFrame === null || listFrame === undefined ? null : new MutationObserver(place);
    if (listFrame !== null && listFrame !== undefined) moves?.observe(listFrame, { attributes: true, attributeFilter: ['class', 'style'] });
    let frame: number | undefined;
    const resized = () => {
      place();
      if (frame !== undefined) view.cancelAnimationFrame(frame);
      frame = view.requestAnimationFrame(() => { frame = undefined; place(); });
    };
    view.addEventListener('resize', resized);
    view.addEventListener('scroll', place, true);
    return () => { sizes?.disconnect(); moves?.disconnect(); if (frame !== undefined) view.cancelAnimationFrame(frame); view.removeEventListener('resize', resized); view.removeEventListener('scroll', place, true); };
  }, [choice, visible, avoidChoiceList]);
  const close = () => { setChildKept(false); setKept(false); setShown(false); setDismissed(true); };
  const closePassing = () => {
    if (!choice) { close(); return; }
    flushSync(close);
    // A passing HoverCard is a dismissable layer above the picker. Hand Escape back after
    // removing that layer, so an inspection never adds an extra dismissal step to a choice.
    queueMicrotask(() => {
      const triggerNode = anchor.current;
      if (triggerNode?.isConnected === true && triggerNode.closest('[role="listbox"],[role="menu"]') !== null) {
        triggerNode.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
      }
    });
  };
  useImperativeHandle(controller, () => ({ keep }), [keep]);
  const control = choice || link ? <Popover.Anchor data-floating-anchor asChild ref={(node) => { anchor.current = node; }}>{trigger}</Popover.Anchor> : <Popover.Trigger asChild aria-label={asChild ? undefined : label} onClick={(event) => {
    event.stopPropagation();
  }}>{trigger}</Popover.Trigger>;
  const inspector = <IconButton ref={attachInspector} size="sm" label={t('ui.detailCard.inspect', { name: label })} icon={<Info aria-hidden />} data-choice-inspect
    data-choice-for={isValidElement<{ id?: string }>(children) ? children.props.id : undefined}
    tabIndex={-1} className="pointer-events-auto"
    onPointerDown={(event) => { event.preventDefault(); event.stopPropagation(); }}
    onPointerUp={(event) => event.stopPropagation()}
    onClick={(event) => { event.preventDefault(); event.stopPropagation(); keep(); }} />;
  return <Popover.Root open={kept} onOpenChange={(next) => { if (next) keepParent?.(); else setChildKept(false); setKept(next); setShown(false); setDismissed(!next); }}>
      <HoverCard.Root open={passing} onOpenChange={(next) => {
        const node = passingCard.current;
        if (!next && passingContent !== undefined && node?.contains(node.ownerDocument.activeElement)) return;
        if (mounted.current && !kept) setShown(next);
      }}>
      <HoverCard.Trigger asChild onPointerEnter={() => setDismissed(false)}
        // Options keep their primitive's focus handling; plain names need no delayed opening.
        onFocus={(event) => { const target = event.currentTarget; revealFloatingFocus(target); requestAnimationFrame(() => { if (target.isConnected && document.activeElement === target) revealFloatingFocus(target); }); if (!choice) event.preventDefault(); if (!restoringFocus.current) { announceParent(true); setDismissed(false); if (!choice) setShown(true); } restoringFocus.current = false; }} onBlur={(event) => { if (!(event.relatedTarget instanceof Node && passingCard.current?.contains(event.relatedTarget))) setShown(false); }} onKeyDown={(event) => {
          if (choice && event.key === 'ArrowRight') { event.preventDefault(); event.stopPropagation(); keep(); }
          if (!choice && passingContent !== undefined && passing && (event.key === 'ArrowDown' || event.key === 'Tab' && !event.shiftKey)) {
            const first = passingCard.current?.querySelector<HTMLElement>('button:not([disabled]), a[href], input:not([disabled]), select:not([disabled]), [tabindex="0"]');
            if (first !== null && first !== undefined) { event.preventDefault(); first.focus(); }
          }
          if (event.key === 'Escape' && visible) { if (!choice || kept) { event.preventDefault(); event.stopPropagation(); } close(); }
        }}>
        {control}
      </HoverCard.Trigger>
      <HoverCard.Portal><HoverCard.Content ref={passingCard} side={choice ? choiceSide : side} align="start" sideOffset={choice ? choiceGap : CARD_GAP_PX} collisionPadding={CARD_EDGE_PX} {...floatingAttachment}
        style={covered ? { display: 'none' } : undefined}
        data-inspection-card className={cx(classes, 'pointer-events-auto')}
        data-choice-preview={owner}
        onFocusCapture={() => setShown(true)}
        onBlurCapture={(event) => { if (!(event.relatedTarget instanceof Node && (passingCard.current?.contains(event.relatedTarget) || anchor.current?.contains(event.relatedTarget)))) setShown(false); }}
        aria-label={label} onEscapeKeyDown={(event) => { event.preventDefault(); event.stopPropagation(); closePassing(); }}>
        {visible && nested.wrap((passingContent ?? content)())}
      </HoverCard.Content></HoverCard.Portal>
    </HoverCard.Root>
    {choice && (inspection === null ? renderInspect?.(inspector) ?? inspector : inspection.target === null ? null : createPortal(inspector, inspection.target))}
    <Popover.Portal><Popover.Content side={choice ? choiceSide : side} align="start" sideOffset={choice ? choiceGap : CARD_GAP_PX} collisionPadding={CARD_EDGE_PX} {...floatingAttachment}
      style={covered ? { display: 'none' } : undefined}
      data-inspection-card
      ref={card} tabIndex={-1} onOpenAutoFocus={(event) => { event.preventDefault(); card.current?.focus(); }}
      data-choice-preview={owner}
      onInteractOutside={(event) => { if (covered || isInspectionTarget(event.target)) event.preventDefault(); }}
      aria-label={label} className={classes} onClick={(event) => event.stopPropagation()} onEscapeKeyDown={(event) => { event.stopPropagation(); close(); }}
      onCloseAutoFocus={(event) => {
        restoringFocus.current = true;
        if (choice) { event.preventDefault(); if (returnFocus === undefined) anchor.current?.focus(); else returnFocus(); }
        queueMicrotask(() => { restoringFocus.current = false; });
      }}>
      {visible && nested.wrap(content())}
    </Popover.Content></Popover.Portal>
  </Popover.Root>;
}

/** A plain option stays untouched; an inspected one shares the name-card interaction. */
export function ChoiceCard({ preview, children, ...props }: { preview?: CardPreview | undefined; children: ReactNode } & Pick<DetailCardProps, 'active' | 'controller' | 'owner' | 'returnFocus' | 'renderInspect'>) {
  return preview === undefined ? children : <DetailCard {...preview} {...props} asChild choice>{children}</DetailCard>;
}
