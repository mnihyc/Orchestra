/** Resource sub-sections use the shared settings navigation and mount only the selected work. */
import { Activity, type ComponentProps, type ReactNode, isValidElement, useContext, useId, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { PageSectionContext } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { ContentBoundary } from '@/ui/ContentBoundary';
import { cx } from '@/ui/cx';
import { SectionNav } from '@/ui/SectionNav';
import { Tabs, type TabsProps } from '@/ui/Tabs';
import { HeadingActs } from '@/features/resources/acts';
import type { MenuEntry } from '@/ui/Menu';
import { DETAIL_PAGE_ROWS } from '@/features/resources/detailPageSize';

export { DETAIL_PAGE_ROWS } from '@/features/resources/detailPageSize';

/** Page menus do not repeat a visible primary/editor or an operation already shown in this panel. */
export function ResourceHeadingActs({ omit = [], ...props }: ComponentProps<typeof HeadingActs> & { omit?: readonly string[] }) {
  const hidden = new Set(omit);
  const primary = props.primary !== null && !hidden.has(props.primary.id) ? props.primary : null;
  const edit = props.edit !== null && !hidden.has(props.edit.id) ? props.edit : null;
  const repeated = new Set([...hidden, ...(primary === null ? [] : [primary.id]), ...(edit === null ? [] : ['edit', 'rename'])]);
  const filter = (entries: readonly MenuEntry[]): MenuEntry[] => {
    const kept: MenuEntry[] = [];
    for (const entry of entries) {
      if (repeated.has(entry.id)) continue;
      if (entry.type === 'separator' && (kept.length === 0 || kept.at(-1)?.type === 'separator')) continue;
      if (entry.type === 'sub') { const children = filter(entry.entries); if (children.length > 0) kept.push({ ...entry, entries: children }); }
      else kept.push(entry);
    }
    if (kept.at(-1)?.type === 'separator') kept.pop();
    return kept;
  };
  return <HeadingActs {...props} primary={primary} edit={edit} menu={filter(props.menu)} />;
}

/** The visible tab already names the section; nested Section components retain that accessible name once. */
export function ResourceTabs<Value extends string>(props: TabsProps<Value>) {
  const id = useId();
  return <Tabs {...props} tabs={props.tabs.map((tab) => ({ ...tab,
    panelBoundary: isValidElement(tab.panel) && tab.panel.type === DetailSections ? false : tab.panelBoundary,
    panel: typeof tab.label === 'string'
    ? <PageSectionContext value={{ title: tab.label, id: `${id}-${tab.value}` }}><section role="region" aria-labelledby={`${id}-${tab.value}`} className="contents"><h2 className="sr-only" id={`${id}-${tab.value}`}>{tab.label}</h2>{tab.panel}</section></PageSectionContext>
    : tab.panel }))} />;
}

export interface DetailSection<Value extends string> { value: Value; label: string; panel: ReactNode }
export function DetailSections<Value extends string>({ label, sections, value, defaultValue, onValueChange, hrefOf, blocked = false, className, panelClassName }: {
  label: string; sections: readonly DetailSection<Value>[]; value?: Value; defaultValue?: Value; onValueChange?: (value: Value) => void;
  hrefOf?: (value: Value) => string; blocked?: boolean; className?: string; panelClassName?: string;
}) {
  const id = useId();
  const owner = useContext(PageSectionContext);
  const [local, setLocal] = useState<Value | undefined>(defaultValue ?? sections[0]?.value);
  const [visited, setVisited] = useState<ReadonlySet<Value>>(new Set());
  const selected = sections.find((section) => section.value === (value ?? local)) ?? sections[0];
  if (selected === undefined) return null;
  if (!visited.has(selected.value)) setVisited((before) => new Set(before).add(selected.value));
  const change = (next: Value) => { if (!blocked) { if (onValueChange === undefined) setLocal(next); else onValueChange(next); } };
  return <div data-resource-sections className={cx('flex min-w-0 flex-col gap-4 @min-[600px]:flex-row @min-[600px]:items-start', className)}>
    <SectionNav label={label} hidePickerLabel={owner?.title === label} current={selected.value} items={sections.map((section) => ({ id: section.value, label: section.label,
      link: <a id={`${id}-choice-${section.value}`} href={hrefOf?.(section.value) ?? `#${id}-${section.value}`} aria-controls={`${id}-${section.value}`} aria-disabled={blocked || undefined} onClick={(event) => {
        if (hrefOf !== undefined && (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || event.button !== 0)) return;
        event.preventDefault(); change(section.value);
      }} />,
    }))} />
    <div className={cx('@container min-w-0 flex-1', panelClassName)}>{sections.map((section) => (section.value === selected.value || visited.has(section.value)) &&
      <Activity key={section.value} mode={section.value === selected.value ? 'visible' : 'hidden'}>
        <div id={`${id}-${section.value}`} data-resource-section={section.value} className="min-h-0 min-w-0 h-full">
          <ContentBoundary resetKey={section.value}><PageSectionContext value={{ title: section.label, id: `${id}-choice-${section.value}` }}>{section.panel}</PageSectionContext></ContentBoundary>
        </div>
      </Activity>)}</div>
  </div>;
}

/** Technical reports are opt-in; editor drafts may stay mounted, while hidden queries can use `open`. */
export function ResourceDetails({ children, keepMounted = false, pauseEffects = true, defaultOpen = false, label }: { children: ReactNode | ((open: boolean) => ReactNode); keepMounted?: boolean; pauseEffects?: boolean; defaultOpen?: boolean; label?: string }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(defaultOpen);
  const [visited, setVisited] = useState(defaultOpen);
  return <details data-resource-details open={open} onToggle={(event) => { const next = event.currentTarget.open; setOpen(next); if (next) setVisited(true); }}>
    <summary className="cursor-pointer rounded-sm py-1 text-sm text-text-muted focus-visible:outline-2 focus-visible:outline-accent">{label ?? t('common.details')}</summary>
    {(open || keepMounted && visited) && <Activity mode={open || !pauseEffects ? 'visible' : 'hidden'}><div hidden={!open} className="min-w-0 pt-3">{typeof children === 'function' ? children(open) : children}</div></Activity>}
  </details>;
}

export function DetailPages<Item>({ items, children, blocked = false, defaultPage = 0 }: { items: readonly Item[]; children: (items: readonly Item[]) => ReactNode; blocked?: boolean; defaultPage?: number }) {
  const { t } = useTranslation();
  const [page, setPage] = useState(defaultPage);
  const [visited, setVisited] = useState<ReadonlySet<number>>(new Set([defaultPage]));
  const pages = Math.max(1, Math.ceil(items.length / DETAIL_PAGE_ROWS));
  const current = Math.min(page, pages - 1);
  const choose = (next: number) => { setPage(next); setVisited((before) => new Set(before).add(next)); };
  return <div className="flex min-w-0 flex-col gap-3" data-resource-pages>
    {Array.from({ length: pages }, (_, index) => (index === current || visited.has(index)) && <Activity key={index} mode={index === current ? 'visible' : 'hidden'}>
      <div data-resource-page={index}>{children(items.slice(index * DETAIL_PAGE_ROWS, (index + 1) * DETAIL_PAGE_ROWS))}</div>
    </Activity>)}
    {pages > 1 && <div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
      <span role="status" className="text-sm text-text-muted">{t('resourceDetails.layout.range', { from: current * DETAIL_PAGE_ROWS + 1, to: Math.min(items.length, (current + 1) * DETAIL_PAGE_ROWS), count: items.length })}</span>
      <div className="flex gap-2"><Button size="sm" disabled={blocked || current === 0} onClick={() => choose(current - 1)}>{t('resourceDetails.layout.previous')}</Button>
        <Button size="sm" disabled={blocked || current === pages - 1} onClick={() => choose(current + 1)}>{t('resourceDetails.layout.next')}</Button></div>
    </div>}
  </div>;
}
