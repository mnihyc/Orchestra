/** Shared Tabs component (navigation); primitives are wrapped here.
 *
 * Several panels of one thing, one of which is in view: the tabs of a topic, the sections of an org.
 * The tabs are a row of names with a line under the one that is chosen, the arrow keys go from one to
 * the next, and the panel under them is named by the tab that opened it. What is chosen is the
 * caller's to keep, so that it can live in the address and a link can name it. Where the row is
 * wider than its room, the single row scrolls horizontally. Selection and focus reveal the current
 * tab inside that row; the arrow keys include every enabled tab.
 */
import { Tabs as Primitive } from 'radix-ui';
import { type ReactNode, useLayoutEffect, useRef } from 'react';

import { Badge } from '@/ui/Badge';
import { cx } from '@/ui/cx';
import { ContentBoundary } from '@/ui/ContentBoundary';

export interface TabDefinition<Value extends string> {
  value: Value;
  label: ReactNode;
  /** A complex label can supply its plain accessible name explicitly. */
  menuLabel?: string;
  icon?: ReactNode;
  /** A number beside the name: how many things the panel holds. */
  count?: number | undefined;
  disabled?: boolean;
  /** What the panel shows. It is drawn only while its tab is the chosen one. */
  panel: ReactNode;
  /** A section layout owns its inner content boundaries so its navigation stays outside them. */
  panelBoundary?: boolean | undefined;
}

export interface TabsProps<Value extends string> {
  /** A page may put this same tab list on its section heading while retaining the tab-panel relationship. */
  frame?: ((navigation:ReactNode,panels:ReactNode)=>ReactNode) | undefined;
  /** What the row of tabs is of, for a screen reader. */
  label: string;
  value: Value;
  onValueChange: (value: Value) => void;
  /** Another visible area can own the choices while this panel keeps its state and identity. */
  navigation?: boolean;
  tabs: readonly TabDefinition<Value>[];
  className?: string;
  /** Classes of the row of tabs, where it needs room at its ends. */
  listClassName?: string;
  /** Classes of the panel. */
  panelClassName?: string;
}

const TAB_TRIGGER = 'focus-inset relative inline-flex min-h-(--control-md) min-w-0 shrink-0 items-center gap-2 px-3 py-1 text-base font-medium transition-colors';

/** Reveal a tab in its own strip without moving the page or stealing keyboard focus. */
function reveal(list: HTMLElement, tab: HTMLElement) {
  if (list.clientWidth <= 0) return;
  const viewport = list.getBoundingClientRect(), value = tab.getBoundingClientRect();
  const left = value.left < viewport.left ? value.left - viewport.left : value.right > viewport.right ? value.right - viewport.right : 0;
  if (left !== 0) list.scrollBy({ left, behavior: 'instant' });
}


export function Tabs<Value extends string>({
  label, value, onValueChange, tabs, navigation = true, className, listClassName, panelClassName,frame,
}: TabsProps<Value>) {
  const list = useRef<HTMLDivElement>(null);
  const identity = JSON.stringify(tabs.map((tab) => tab.value));
  useLayoutEffect(() => {
    const strip = list.current; if (!navigation || strip === null) return;
    let active = true;
    const update = () => {
      if (!active) return;
      const tab = Array.from(strip.querySelectorAll<HTMLElement>('[data-tab-value]')).find((item) => item.dataset.tabValue === value);
      if (tab !== undefined) reveal(strip, tab);
    };
    update();
    const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(update);
    observer?.observe(strip); for (const tab of Array.from(strip.children)) observer?.observe(tab);
    const wheel = (event: WheelEvent) => {
      if (event.defaultPrevented || event.ctrlKey || event.metaKey || event.altKey || event.shiftKey || event.deltaX !== 0 || event.deltaY === 0 || strip.scrollWidth <= strip.clientWidth) return;
      const style = getComputedStyle(strip);
      const line = Number.parseFloat(style.lineHeight) || Number.parseFloat(style.fontSize) || strip.clientHeight;
      const unit = event.deltaMode === WheelEvent.DOM_DELTA_LINE ? line : event.deltaMode === WheelEvent.DOM_DELTA_PAGE ? strip.clientWidth : 1;
      const before = strip.scrollLeft;
      strip.scrollBy({ left: event.deltaY * unit * (style.direction === 'rtl' ? -1 : 1), behavior: 'instant' });
      if (strip.scrollLeft !== before) event.preventDefault();
    };
    strip.addEventListener('wheel', wheel, { passive: false });
    if (document.fonts !== undefined) void document.fonts.ready.then(update);
    return () => { active = false; observer?.disconnect(); strip.removeEventListener('wheel', wheel); };
  }, [value, identity, navigation]);
  const choose = (next: Value) => {
    onValueChange(next);
    requestAnimationFrame(() => {
      const strip = list.current; if (strip === null) return;
      const tab = Array.from(strip.querySelectorAll<HTMLElement>('[data-tab-value]')).find((item) => item.dataset.tabValue === next);
      if (tab !== undefined) { tab.focus({ preventScroll: true }); reveal(strip, tab); }
    });
  };
  const contents = (tab: TabDefinition<Value>) => <>{tab.icon !== undefined && <span className="flex shrink-0 text-text-subtle">{tab.icon}</span>}<span className="min-w-0 whitespace-nowrap">{tab.label}</span>{tab.count !== undefined && <Badge count={tab.count} className="h-5 min-w-5 shrink-0 text-xs" />}</>;
  const panelAttributes = (tab: TabDefinition<Value>) => navigation ? {} : {
    role: 'region' as const, 'aria-labelledby': undefined, 'aria-label': tab.menuLabel ?? (typeof tab.label === 'string' ? tab.label : label),
  };
  const strip=<div data-navigation-strip="tabs" hidden={!navigation} style={navigation ? undefined : { display: 'none' }} className={cx('relative flex min-w-0 max-w-full shrink-0 items-stretch border-b border-border', listClassName)}>
      <Primitive.List ref={list} aria-label={label} className="flex min-w-0 flex-1 flex-nowrap items-stretch overflow-x-auto overflow-y-hidden overscroll-x-contain" onKeyDown={(event) => {
        if (event.defaultPrevented || event.ctrlKey || event.altKey || event.metaKey || (event.target as HTMLElement).getAttribute('role') !== 'tab') return;
        const available = tabs.filter((tab) => !tab.disabled);
        const index = available.findIndex((tab) => tab.value === (event.target as HTMLElement).dataset.tabValue);
        const next = event.key === 'Home' ? 0 : event.key === 'End' ? available.length - 1 : event.key === 'ArrowRight' ? (index + 1) % available.length : event.key === 'ArrowLeft' ? (index - 1 + available.length) % available.length : null;
        const tab = next === null ? undefined : available[next];
        if (tab !== undefined) { event.preventDefault(); choose(tab.value); }
      }}>
        {tabs.map((tab) => <Primitive.Trigger key={tab.value} value={tab.value} disabled={tab.disabled} data-tab-value={tab.value} aria-label={tab.menuLabel} onFocus={(event) => { if (list.current !== null) reveal(list.current, event.currentTarget); }}
          className={cx(TAB_TRIGGER, 'text-text-muted hover:text-text data-[state=active]:text-text after:absolute after:inset-x-2 after:bottom-0 after:h-0.5 after:rounded-full after:bg-transparent data-[state=active]:after:bg-accent disabled:cursor-not-allowed disabled:text-text-disabled')}>
          {contents(tab)}
        </Primitive.Trigger>)}
      </Primitive.List>
    </div>;
  const panels=tabs.map((tab) => <Primitive.Content key={tab.value} value={tab.value} {...panelAttributes(tab)} className={cx('min-w-0', panelClassName)}>{tab.panelBoundary === false ? tab.panel : <ContentBoundary resetKey={tab.value}>{tab.panel}</ContentBoundary>}</Primitive.Content>);
  return <Primitive.Root value={value} onValueChange={(next) => onValueChange(next as Value)} className={cx('flex min-w-0 flex-col', className)}>
    {frame===undefined ? <>{strip}{panels}</> : frame(strip,panels)}
  </Primitive.Root>;
}
