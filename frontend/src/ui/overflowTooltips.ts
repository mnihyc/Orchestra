/** One document-level fallback for text shortened by CSS or an explicitly recorded preview.
 * Pointer/focus events are delegated; mutations and resizes only maintain keyboard reach, never a
 * listener per row. Explicit Tooltip triggers keep their own content and behavior.
 */
import { FLOATING_EDGE as EDGE, FLOATING_GAP as GAP, floatingClasses, floatingRoom } from '@/ui/floating';
import { onPromptOpened } from '@/ui/PromptScope';

export const OVERFLOW_REST_MS = 400;
/** Enough time to cross the same six-pixel gap used by the ordinary tooltip. */
const LEAVE_MS = 100;
const CONTROLS = 'a[href],button,input,select,textarea,summary,[contenteditable="true"],[role="button"],[role="link"],[role="tab"],[role="treeitem"],[role="menuitem"],[role="option"],[role="combobox"],[role="checkbox"],[role="radio"],[role="switch"]';
const EXCLUDED = 'script,style,[hidden],[inert],[aria-hidden="true"],[role="tooltip"],[data-tooltip-content]';

/** The value stays text, never HTML. Manual previews retain their original in one explicit member. */
function renderedText(element: { innerText?: string; textContent: string | null }): string {
  // DOM implementations without layout (including the unit harness) have no innerText.
  return element.innerText ?? element.textContent ?? '';
}
export function overflowText(element: HTMLElement): string {
  const shown = renderedText(element).trim();
  const title = element.getAttribute('title');
  return (element.getAttribute('data-overflow-text') ?? (shown.includes('…') && title !== null && title.length > shown.length ? title : shown)).trim();
}

function clipsText(style: CSSStyleDeclaration): boolean {
  return style.textOverflow.split(/\s+/).includes('ellipsis') ||
    Number.parseInt(style.getPropertyValue('-webkit-line-clamp') || style.getPropertyValue('line-clamp'), 10) > 0;
}

function keyboardOwner(element: HTMLElement): HTMLElement | null {
  if (element.matches(CONTROLS) || element.hasAttribute('tabindex')) return element;
  return element.parentElement?.closest<HTMLElement>(CONTROLS) ?? null;
}

/** Both a clipping rule and actual lost space are needed: a truncate class alone loses nothing. */
export function textOverflows(element: HTMLElement): boolean {
  if (element.closest(EXCLUDED) !== null || (element.clientWidth === 0 && element.clientHeight === 0)) return false;
  const style = getComputedStyle(element);
  if (style.display === 'none' || style.visibility === 'hidden') return false;
  const cssLoss = clipsText(style) && (element.scrollWidth > element.clientWidth || element.scrollHeight > element.clientHeight);
  if (!cssLoss && !element.hasAttribute('data-overflow-text') && !element.hasAttribute('title')) return false;
  const shown = renderedText(element).trim();
  const title = element.getAttribute('title');
  const recorded = element.getAttribute('data-overflow-text') ?? (shown.includes('…') && title !== null && title.length > shown.length ? title : null);
  return (cssLoss || (recorded !== null && recorded.trim() !== shown)) && overflowText(element) !== '';
}

/** An unrelated field hint is not an existing tooltip. The fallback's own description is ignored. */
export function hasOwnTooltip(element: HTMLElement, focusTarget?: HTMLElement): boolean {
  const trigger = element.closest('[data-tooltip-trigger]');
  if (trigger !== null && (focusTarget === undefined || trigger.contains(focusTarget))) return true;
  for (let current: HTMLElement | null = element; current !== null; current = current.parentElement) {
    if (focusTarget !== undefined && !current.contains(focusTarget)) continue;
    const ids = current.getAttribute('aria-describedby')?.split(/\s+/) ?? [];
    if (ids.some((id) => {
      const description = element.ownerDocument.getElementById(id);
      return description?.getAttribute('role') === 'tooltip' && !description.hasAttribute('data-overflow-tooltip');
    })) return true;
  }
  return false;
}

interface Installation { users: number; dispose: () => void }
const installed = new WeakMap<Document, Installation>();
let serial = 0;

/** A provider may be nested in a dialog or test harness; its document still has just one handler. */
export function installOverflowTooltips(doc: Document): () => void {
  const existing = installed.get(doc);
  if (existing !== undefined) {
    existing.users += 1;
    return () => release(doc, existing);
  }
  const context = doc.defaultView;
  if (context === null) return () => undefined;
  const view: Window = context;
  const candidates = new Set<HTMLElement>();
  const tabStops = new Set<HTMLElement>();
  const dirty = new Set<HTMLElement>();
  let frame: number | null = null;
  let opening: number | undefined;
  let leaving: number | undefined;
  let pointer: HTMLElement | null = null;
  let focus: HTMLElement | null = null;
  let dismissed: HTMLElement | null = null;
  let active: { source: HTMLElement; owner: HTMLElement; tooltip: HTMLDivElement; keyboard: boolean } | null = null;

  function hide() {
    view.clearTimeout(opening);
    view.clearTimeout(leaving);
    opening = undefined;
    leaving = undefined;
    if (active === null) return;
    const { owner, tooltip } = active;
    const remaining = (owner.getAttribute('aria-describedby') ?? '').split(/\s+/).filter((id) => id !== '' && id !== tooltip.id);
    if (remaining.length === 0) owner.removeAttribute('aria-describedby');
    else owner.setAttribute('aria-describedby', remaining.join(' '));
    tooltip.remove();
    active = null;
  }

  function completeValue(source: HTMLElement, focusTarget?: HTMLElement): string {
    const values = focusTarget?.contains(source)
      ? [source, ...Array.from(focusTarget.querySelectorAll<HTMLElement>('*'))].filter((element) => textOverflows(element) && !hasOwnTooltip(element, focusTarget)).map(overflowText)
      : [overflowText(source)];
    const unique = [...new Set(values)];
    return unique.filter((value) => !unique.some((other) => other !== value && other.includes(value))).join('\n');
  }
  function show(source: HTMLElement, keyboard: boolean) {
    view.clearTimeout(leaving);
    leaving = undefined;
    const focusTarget = keyboard && doc.activeElement instanceof HTMLElement ? doc.activeElement : undefined;
    const ownerNow = focusTarget?.contains(source) ? focusTarget : keyboardOwner(source) ?? source;
    const whole = completeValue(source, focusTarget);
    if (source === dismissed || (active?.source === source && textOverflows(source) && active.tooltip.textContent === whole && active.owner === ownerNow)) return;
    hide();
    if (!textOverflows(source) || hasOwnTooltip(source, focusTarget)) return;
    // Browsers already show a complete native title on hover, but not on keyboard focus.
    const title = source.closest('[title]')?.getAttribute('title');
    if (!keyboard && title?.includes(overflowText(source))) return;
    const open = () => {
      opening = undefined;
      if (!source.isConnected || !textOverflows(source) || hasOwnTooltip(source, focusTarget)) return;
      const tooltip = doc.createElement('div');
      serial += 1;
      tooltip.id = `orch-overflow-tooltip-${serial}`;
      tooltip.setAttribute('role', 'tooltip');
      tooltip.setAttribute('data-overflow-tooltip', '');
      tooltip.textContent = completeValue(source, focusTarget);
      tooltip.className = `${floatingClasses} z-(--layer-tooltip) overflow-auto rounded-sm bg-text px-2 py-1 text-xs whitespace-pre-wrap wrap-anywhere text-bg shadow-md`;
      Object.assign(tooltip.style, { position: 'fixed', left: `${EDGE}px`, top: `${EDGE}px` });
      doc.body.append(tooltip);
      const anchor = source.getBoundingClientRect();
      const size = tooltip.getBoundingClientRect();
      const width = doc.documentElement.clientWidth || view.innerWidth;
      const height = doc.documentElement.clientHeight || view.innerHeight;
      // A keyboard hint can appear between pointer-down and click. Keep it clear of its
      // trigger so a long value cannot swallow the act that opened that control.
      const room = floatingRoom(anchor, height, size.height);
      tooltip.style.setProperty('--floating-height', `${room.height}px`);
      const bounded = tooltip.getBoundingClientRect();
      tooltip.style.left = `${Math.max(EDGE, Math.min(anchor.left, width - bounded.width - EDGE))}px`;
      tooltip.style.top = `${Math.max(EDGE, Math.min(room.above ? anchor.top - bounded.height - GAP : anchor.bottom + GAP, height - bounded.height - EDGE))}px`;
      const owner = focusTarget?.contains(source) ? focusTarget : keyboardOwner(source) ?? source;
      const ids = (owner.getAttribute('aria-describedby') ?? '').split(/\s+/).filter(Boolean);
      owner.setAttribute('aria-describedby', [...ids, tooltip.id].join(' '));
      active = { source, owner, tooltip, keyboard };
    };
    if (keyboard) open();
    else opening = view.setTimeout(open, OVERFLOW_REST_MS);
  }

  function candidate(eventTarget: EventTarget | null, keyboard: boolean): HTMLElement | null {
    let hit = eventTarget instanceof Element ? eventTarget : null;
    while (hit !== null && !(hit instanceof HTMLElement)) hit = hit.parentElement;
    if (!(hit instanceof HTMLElement) || hit.closest('[role="tooltip"],[data-tooltip-content]') !== null) return null;
    const target = hit;
    for (let element: HTMLElement | null = target; element !== null && element !== doc.body; element = element.parentElement) {
      if (textOverflows(element) && !hasOwnTooltip(element, keyboard ? target : undefined)) return element;
    }
    // A link/button receives focus even when it is a child label that actually clips.
    return target.matches(CONTROLS) ? Array.from(target.querySelectorAll<HTMLElement>('*')).find((element) => textOverflows(element) && !hasOwnTooltip(element, keyboard ? target : undefined)) ?? null : null;
  }

  function reconsider() {
    const source = focus ?? pointer;
    if (source === null) hide();
    else show(source, focus !== null);
  }
  function over(event: PointerEvent) {
    if (event.pointerType === 'touch') return;
    if (event.target instanceof Node && active?.tooltip.contains(event.target) === true) {
      view.clearTimeout(leaving);
      return;
    }
    pointer = candidate(event.target, false);
    if (pointer !== null && pointer !== dismissed) dismissed = null;
    if (pointer === null) reconsider();
    else show(pointer, false);
  }
  function out(event: PointerEvent) {
    const next = event.relatedTarget;
    if (next instanceof Node && (pointer?.contains(next) === true || active?.tooltip.contains(next) === true)) return;
    if (dismissed === pointer && dismissed !== focus) dismissed = null;
    pointer = null;
    view.clearTimeout(opening);
    leaving = view.setTimeout(reconsider, LEAVE_MS);
  }
  function focused(event: FocusEvent) {
    focus = candidate(event.target, true);
    if (focus !== dismissed) dismissed = null;
    reconsider();
  }
  function blurred(event: FocusEvent) {
    focus = candidate(event.relatedTarget, true);
    dismissed = null;
    reconsider();
  }
  function key(event: KeyboardEvent) {
    if (event.key !== 'Escape') return;
    const top = active !== null && !event.isComposing;
    dismissed = active?.source ?? focus ?? pointer;
    hide();
    // This value hint sits above an inspection or prompt. Its Escape must finish
    // before that underlying layer handles the same key.
    if (top) { event.preventDefault(); event.stopPropagation(); }
  }
  function scrolled(event: Event) {
    if (event.target instanceof Node && active?.tooltip.contains(event.target) === true) return;
    hide();
  }
  function restoreTab(element: HTMLElement) {
    if (!tabStops.delete(element)) return;
    if (element.getAttribute('tabindex') === '0') element.removeAttribute('tabindex');
  }
  function measure(element: HTMLElement) {
    if (textOverflows(element)) {
      if (tabStops.has(element) && (element.parentElement?.closest(CONTROLS) ?? null) !== null) restoreTab(element);
      if (keyboardOwner(element) === null) {
        element.tabIndex = 0;
        tabStops.add(element);
      }
    } else restoreTab(element);
  }
  const sizes = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver((entries) => {
    for (const entry of entries) if (entry.target instanceof HTMLElement) measure(entry.target);
    if (active !== null && !textOverflows(active.source)) hide();
  });
  function scan(root: HTMLElement) {
    for (const element of [root, ...Array.from(root.querySelectorAll('*'))]) {
      if (!(element instanceof HTMLElement)) continue;
      const wanted = clipsText(getComputedStyle(element)) || element.hasAttribute('data-overflow-text') || (element.hasAttribute('title') && renderedText(element).includes('…'));
      if (!wanted && !candidates.has(element)) continue;
      if (element.closest(EXCLUDED) !== null || element.textContent.trim() === '') continue;
      if (wanted) {
        if (!candidates.has(element)) { candidates.add(element); sizes?.observe(element); }
        measure(element);
      } else if (candidates.delete(element)) { sizes?.unobserve(element); restoreTab(element); }
    }
  }
  function schedule(root: HTMLElement) {
    dirty.add(root);
    if (frame !== null) return;
    frame = view.requestAnimationFrame(() => {
      frame = null;
      const roots = [...dirty];
      dirty.clear();
      for (const element of candidates) if (!element.isConnected) { candidates.delete(element); sizes?.unobserve(element); restoreTab(element); }
      for (const root of roots) if (root.isConnected && !roots.some((other) => other !== root && other.contains(root))) scan(root);
      if (active !== null && (!active.source.isConnected || !textOverflows(active.source))) hide();
      else if (active !== null) show(active.source, active.keyboard);
    });
  }
  const mutations = new MutationObserver((records) => {
    for (const record of records) {
      const root = record.target instanceof HTMLElement ? record.target : record.target.parentElement;
      if (root?.closest('[role="tooltip"],[data-tooltip-content]') !== null) continue;
      // The fallback's own portal must not trigger a scan of the application.
      if (record.type === 'childList' && [...Array.from(record.addedNodes), ...Array.from(record.removedNodes)].every((node) => node instanceof HTMLElement && node.hasAttribute('data-overflow-tooltip'))) continue;
      schedule(root);
    }
  });
  function resized() { hide(); schedule(doc.body); }
  const forgetCovered = onPromptOpened(() => { pointer = null; focus = null; hide(); });
  const fontOwner: { fonts?: FontFaceSet } = doc;
  const fonts = fontOwner.fonts;
  doc.addEventListener('pointerover', over);
  doc.addEventListener('pointerout', out);
  doc.addEventListener('focusin', focused);
  doc.addEventListener('focusout', blurred);
  view.addEventListener('keydown', key, true);
  doc.addEventListener('pointerdown', hide);
  doc.addEventListener('scroll', scrolled, true);
  view.addEventListener('resize', resized);
  fonts?.addEventListener('loadingdone', resized);
  mutations.observe(doc.body, { subtree: true, childList: true, characterData: true, attributes: true, attributeFilter: ['class', 'style', 'hidden', 'title', 'data-overflow-text'] });
  scan(doc.body);

  const installation: Installation = { users: 1, dispose: () => {
    forgetCovered();
    hide();
    if (frame !== null) view.cancelAnimationFrame(frame);
    mutations.disconnect();
    sizes?.disconnect();
    for (const element of tabStops) restoreTab(element);
    doc.removeEventListener('pointerover', over);
    doc.removeEventListener('pointerout', out);
    doc.removeEventListener('focusin', focused);
    doc.removeEventListener('focusout', blurred);
    view.removeEventListener('keydown', key, true);
    doc.removeEventListener('pointerdown', hide);
    doc.removeEventListener('scroll', scrolled, true);
    view.removeEventListener('resize', resized);
    fonts?.removeEventListener('loadingdone', resized);
  } };
  installed.set(doc, installation);
  return () => release(doc, installation);
}

function release(doc: Document, installation: Installation) {
  installation.users -= 1;
  if (installation.users !== 0) return;
  installation.dispose();
  installed.delete(doc);
}
