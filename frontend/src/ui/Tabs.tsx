/** Shared Tabs component (navigation); primitives are wrapped here.
 *
 * Several panels of one thing, one of which is in view: the tabs of a topic, the sections of an org.
 * The tabs are a row of names with a line under the one that is chosen, the arrow keys go from one to
 * the next, and the panel under them is named by the tab that opened it. What is chosen is the
 * caller's to keep, so that it can live in the address and a link can name it. Where the row is
 * wider than its room, its names wrap onto the next row so every destination stays visible; a page
 * whose tabs stand right under its name keeps them one row instead, the tabs that do not fit (the
 * last first) under More at its end, the chosen one always in the row.
 * Arrow keys include every enabled tab, and long names wrap inside the room the page leaves.
 */
import { Tabs as Primitive } from 'radix-ui';
import { ChevronDown } from 'lucide-react';
import { type KeyboardEvent, type ReactNode, type RefObject, useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { Badge } from '@/ui/Badge';
import { cx } from '@/ui/cx';
import { ContentBoundary } from '@/ui/ContentBoundary';
import { deviceChoiceOf, offeredDeviceChoice } from '@/lib/viewChoiceFacts';
import { DeviceChoiceFacts } from '@/ui/DeviceChoiceFacts';
import { Menu } from '@/ui/Menu';

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
  /** Where the tabs are wider than their room: onto more rows (`wrap`), or one row with the rest under More (`menu`). */
  overflow?: 'wrap' | 'menu';
}

const TAB_TRIGGER = 'focus-inset relative inline-flex min-h-(--control-md) min-w-0 max-w-full items-center gap-2 px-3 py-1 text-base font-medium transition-colors';

export function Tabs<Value extends string>({
  label, value, onValueChange, tabs, navigation = true, className, listClassName, panelClassName,frame, overflow = 'wrap',
}: TabsProps<Value>) {
  const { t } = useTranslation();
  const list = useRef<HTMLDivElement>(null);
  const folded = useFoldedTabs(overflow === 'menu' && navigation, list, tabs, value);
  const receipt = deviceChoiceOf(onValueChange);
  const selected = tabs.find((tab) => tab.value === value), saved = tabs.find((tab) => tab.value === receipt?.saved);
  const observation = receipt === undefined ? undefined : offeredDeviceChoice(receipt, value, tabs.map((option) => option.value));
  const choose = (next: Value) => {
    onValueChange(next);
    requestAnimationFrame(() => {
      const strip = list.current; if (strip === null) return;
      const tab = Array.from(strip.querySelectorAll<HTMLElement>('[data-tab-value]')).find((item) => item.dataset.tabValue === next);
      if (tab !== undefined) tab.focus({ preventScroll: true });
    });
  };
  // The arrows, Home and End go through every enabled tab, those under More too (the chosen one joins the row). Each tab
  // handles them before the primitive's own roving focus, which would pass over a tab that is not in the row.
  const onTabKey = (event: KeyboardEvent<HTMLButtonElement>) => {
    if (event.defaultPrevented || event.ctrlKey || event.altKey || event.metaKey) return;
    const available = tabs.filter((tab) => !tab.disabled);
    const index = available.findIndex((tab) => tab.value === event.currentTarget.dataset.tabValue);
    const next = event.key === 'Home' ? 0 : event.key === 'End' ? available.length - 1 : event.key === 'ArrowRight' ? (index + 1) % available.length : event.key === 'ArrowLeft' ? (index - 1 + available.length) % available.length : null;
    const tab = next === null ? undefined : available[next];
    if (tab !== undefined) { event.preventDefault(); choose(tab.value); }
  };
  const contents = (tab: TabDefinition<Value>) => <>{tab.icon !== undefined && <span className="flex shrink-0 text-text-subtle">{tab.icon}</span>}<span className="min-w-0 text-left wrap-anywhere">{tab.label}</span>{tab.count !== undefined && <Badge count={tab.count} className="h-5 min-w-5 shrink-0 text-xs" />}</>;
  const panelAttributes = (tab: TabDefinition<Value>) => navigation ? {} : {
    role: 'region' as const, 'aria-labelledby': undefined, 'aria-label': tab.menuLabel ?? (typeof tab.label === 'string' ? tab.label : label),
  };
  const strip=<div data-navigation-strip="tabs" hidden={!navigation} style={navigation ? undefined : { display: 'none' }} className={cx('relative flex min-w-0 max-w-full shrink-0 items-stretch border-b border-border', listClassName)}>
      <Primitive.List ref={list} aria-label={label} className={cx('flex min-w-0 flex-1 items-stretch', overflow === 'menu' ? 'flex-nowrap overflow-hidden' : 'flex-wrap')}>
        {tabs.map((tab) => <Primitive.Trigger key={tab.value} value={tab.value} disabled={tab.disabled} data-tab-value={tab.value} aria-label={tab.menuLabel}
          hidden={folded.has(tab.value)} onKeyDown={onTabKey}
          className={cx(TAB_TRIGGER, overflow === 'menu' && 'shrink-0 whitespace-nowrap', 'text-text-muted hover:text-text data-[state=active]:text-text after:absolute after:inset-x-2 after:bottom-0 after:h-0.5 after:rounded-full after:bg-transparent data-[state=active]:after:bg-accent disabled:cursor-not-allowed disabled:text-text-disabled')}>
          {contents(tab)}
        </Primitive.Trigger>)}
      </Primitive.List>
      {overflow === 'menu' && folded.size > 0 && <Menu label={t('ui.tabs.more')} align="end"
        trigger={<button type="button" data-tabs-more className={cx(TAB_TRIGGER, 'shrink-0 whitespace-nowrap text-text-muted hover:text-text')}>{t('ui.tabs.more')}<ChevronDown aria-hidden className="size-4 shrink-0 text-text-subtle" /></button>}
        entries={[{ type: 'radio', id: 'tabs', label, value, onValueChange: (next) => choose(next as Value),
          options: tabs.filter((tab) => folded.has(tab.value) && tab.disabled !== true).map((tab) => ({ value: tab.value, name: tabName(tab) })) }]} />}
      {observation !== undefined && <span className="inline-flex min-w-0 items-center px-2"><DeviceChoiceFacts observation={observation} current={selected?.menuLabel ?? selected?.label} saved={saved?.menuLabel ?? saved?.label} /></span>}
    </div>;
  const panels=tabs.map((tab) => <Primitive.Content key={tab.value} value={tab.value} {...panelAttributes(tab)} className={cx('min-w-0', panelClassName)}>{tab.panelBoundary === false ? tab.panel : <ContentBoundary resetKey={tab.value}>{tab.panel}</ContentBoundary>}</Primitive.Content>);
  return <Primitive.Root value={value} onValueChange={(next) => onValueChange(next as Value)} className={cx('flex min-w-0 flex-col', className)}>
    {frame===undefined ? <>{strip}{panels}</> : frame(strip,panels)}
  </Primitive.Root>;
}

/** A tab's plain name, with how many things its panel holds, for the More menu. */
function tabName<Value extends string>(tab: TabDefinition<Value>): string {
  const name = tab.menuLabel ?? (typeof tab.label === 'string' ? tab.label : tab.value);
  return tab.count === undefined ? name : `${name} (${tab.count})`;
}

/** Which tabs leave a one-row strip for More: as many as fit stay in order, the chosen one always among them. Each tab's
 * width is taken while it is in the row and kept, so a wider room brings them back without a second drawing. */
function useFoldedTabs<Value extends string>(active: boolean, list: RefObject<HTMLDivElement | null>, tabs: readonly TabDefinition<Value>[], value: Value): ReadonlySet<string> {
  const [folded, setFolded] = useState<ReadonlySet<string>>(NONE_FOLDED);
  const widths = useRef(new Map<string, number>());
  const names = tabs.map((tab) => `${tab.value}:${tab.count ?? ''}`).join('|');
  useLayoutEffect(() => {
    const strip = list.current, row = strip?.parentElement ?? null;
    if (!active || strip === null || row === null) return;
    const measure = () => {
      for (const node of strip.querySelectorAll<HTMLElement>('[data-tab-value]')) {
        const key = node.dataset.tabValue;
        if (key !== undefined && !node.hidden && node.offsetWidth > 0) widths.current.set(key, node.offsetWidth);
      }
      // The row's room, less what else stands in it beside the tabs and More.
      const room = row.clientWidth - Array.from(row.children).reduce((sum, child) => child === strip || child.matches('[data-tabs-more]') ? sum : sum + (child as HTMLElement).offsetWidth, 0);
      const width = (tab: TabDefinition<Value>) => widths.current.get(tab.value) ?? 0;
      const next = new Set<string>();
      if (tabs.reduce((sum, tab) => sum + width(tab), 0) > room) {
        // More takes the room of a short tab; the chosen tab keeps its own.
        const more = row.querySelector<HTMLElement>('[data-tabs-more]')?.offsetWidth ?? MORE_ROOM;
        const chosen = tabs.find((tab) => tab.value === value);
        let used = more + (chosen === undefined ? 0 : width(chosen));
        let full = false;
        for (const tab of tabs) {
          if (tab.value === value) continue;
          if (!full && used + width(tab) <= room) used += width(tab);
          else { full = true; next.add(tab.value); }
        }
      }
      setFolded((held) => held.size === next.size && [...next].every((key) => held.has(key)) ? held : next);
    };
    measure();
    const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(measure);
    observer?.observe(row);
    return () => observer?.disconnect();
    // `names` stands for the tabs and their counts; the strip is measured again whenever they or the chosen one change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active, list, names, value]);
  // Tabs that wrap, or that another area navigates, leave none for More; a strip in one row is measured again when it is.
  return active ? folded : NONE_FOLDED;
}

/** The room the More button takes before it is first drawn. */
const MORE_ROOM = 88;
const NONE_FOLDED: ReadonlySet<string> = new Set();
