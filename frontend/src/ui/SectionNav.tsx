/** The one list of sections every page of settings uses: a list beside the content, and a strip of the same names above it where the room is narrow, never a dropdown.
 *
 * The list stays in view while the content beside it scrolls, and scrolls by itself where it is longer than the room the window leaves it, so that its last
 * section is as near as its first. The strip scrolls sideways. A list that stands inside the content of another list is a strip as well, so two lists never
 * share a narrow column. Where the names lead to places on the same page, the one the page is at is marked as the page scrolls, and the strip stays at the top.
 */
import { Slot } from 'radix-ui';
import { cloneElement, createContext, type CSSProperties, type ReactElement, type ReactNode, type RefObject, useContext, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useViewInstance } from '@/features/workbench/instance';
import { useWorkbench } from '@/features/workbench/store';

import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Input } from '@/ui/Input';

import { cx } from '@/ui/cx';

export interface SectionNavItem {
  id: string;
  /** Section names are plain text for the same visible, searchable and accessible identity; icons remain separate. */
  label: string;
  icon?: ReactNode;
  /** The caller owns its router destination; navigation owns the common look and current-page semantics. */
  link: ReactElement<{ children?: ReactNode }>;
  group?: string;
}

export interface SectionNavProps {
  label: string;
  current: string;
  items: readonly SectionNavItem[];
  className?: string;
  /** A workbench sidebar already supplies its width and always shows the vertical list. */
  presentation?: 'responsive' | 'sidebar';
  /** The names lead to places on this page rather than to other pages: the current one is the place the page is at, and the strip stays at the top of the area. */
  onThisPage?: boolean;
  /** Where the list begins to stand beside the content: from 600 px of the room it has (a page), or from 48 rem (the groups of one page's settings). */
  listFrom?: 'page' | 'group';
  /** The page has unsaved work: pressing a name that leads away asks once whether to leave it. */
  unsaved?: boolean;
}

/** Whether the list being drawn stands inside the content of another list. */
const InsideSectionContent = createContext(false);

/** The content a list of sections stands beside: any list drawn in it is a strip. */
export function SectionContent({ children }: { children: ReactNode }) {
  return <InsideSectionContent value>{children}</InsideSectionContent>;
}

/** How much room is left to a sticky list below the top of the area it stays in: as tall as the area shows, less what is above it while the page is at its top. */
function useListRoom(nav: RefObject<HTMLElement | null>, active: boolean): number | undefined {
  const [room, setRoom] = useState<number | undefined>(undefined);
  useEffect(() => {
    const element = nav.current;
    const area = element?.closest<HTMLElement>('[data-scroll-area]') ?? null;
    if (!active || element === null || area === null) return;
    const measure = () => {
      const above = Math.max(12, element.getBoundingClientRect().top - area.getBoundingClientRect().top);
      const next = Math.max(120, Math.floor(area.clientHeight - above - 8));
      setRoom((held) => held !== undefined && Math.abs(held - next) < 2 ? held : next);
    };
    measure();
    area.addEventListener('scroll', measure, { passive: true });
    const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(measure);
    observer?.observe(area);
    return () => { area.removeEventListener('scroll', measure); observer?.disconnect(); };
  }, [nav, active]);
  return room;
}

/** The section the page is at is kept in view in the list or the strip: that one is moved, and nothing around it. */
function useCurrentInView(nav: RefObject<HTMLElement | null>, active: boolean, current: string, room: number | undefined): void {
  useEffect(() => {
    const list = nav.current;
    if (!active || list === null) return;
    const item = list.querySelector<HTMLElement>('[aria-current]');
    const row = list.querySelector<HTMLElement>('ul');
    if (item === null || row === null) return;
    if (getComputedStyle(row).flexDirection.startsWith('row')) {
      if (typeof row.scrollTo !== 'function') return;
      if (item.offsetLeft < row.scrollLeft) row.scrollTo({ left: item.offsetLeft - 8 });
      else if (item.offsetLeft + item.offsetWidth > row.scrollLeft + row.clientWidth) row.scrollTo({ left: item.offsetLeft + item.offsetWidth - row.clientWidth + 8 });
      return;
    }
    if (typeof list.scrollTo !== 'function') return;
    if (item.offsetTop < list.scrollTop) list.scrollTo({ top: item.offsetTop - 4 });
    else if (item.offsetTop + item.offsetHeight > list.scrollTop + list.clientHeight) list.scrollTo({ top: item.offsetTop + item.offsetHeight - list.clientHeight + 4 });
  }, [nav, active, current, room]);
}

/** The classes that make the same markup a strip below one width and a list from it, for each place the list may begin and each mark of the current one
 * (whole class names, so that they are found). A strip marks the current name with a bar under it, a list with a bar at its side. */
const FROM = {
  page: {
    nav: 'w-full @min-[600px]:sticky @min-[600px]:top-3 @min-[600px]:w-48 @min-[600px]:max-h-(--section-room) @min-[600px]:overflow-y-auto @min-[600px]:overscroll-contain',
    top: '@min-[600px]:z-auto @min-[600px]:bg-transparent @min-[600px]:py-0',
    list: '@min-[600px]:flex-col @min-[600px]:overflow-x-visible',
    item: '@min-[600px]:shrink',
    link: '@min-[600px]:whitespace-normal',
    group: '@min-[600px]:ml-0 @min-[600px]:mt-3',
    side: {
      page: '@min-[600px]:aria-[current=page]:shadow-[inset_2px_0_0_var(--accent)]',
      location: '@min-[600px]:aria-[current=location]:shadow-[inset_2px_0_0_var(--accent)]',
    },
  },
  group: {
    nav: 'w-full @min-[48rem]:sticky @min-[48rem]:top-3 @min-[48rem]:w-44 @min-[48rem]:max-h-(--section-room) @min-[48rem]:overflow-y-auto @min-[48rem]:overscroll-contain',
    top: '@min-[48rem]:z-auto @min-[48rem]:bg-transparent @min-[48rem]:py-0',
    list: '@min-[48rem]:flex-col @min-[48rem]:overflow-x-visible',
    item: '@min-[48rem]:shrink',
    link: '@min-[48rem]:whitespace-normal',
    group: '@min-[48rem]:ml-0 @min-[48rem]:mt-3',
    side: {
      page: '@min-[48rem]:aria-[current=page]:shadow-[inset_2px_0_0_var(--accent)]',
      location: '@min-[48rem]:aria-[current=location]:shadow-[inset_2px_0_0_var(--accent)]',
    },
  },
} as const;

/** The look of the current name, whichever way the page says it is current. */
const MARKED = {
  page: 'aria-[current=page]:bg-accent-subtle aria-[current=page]:font-semibold aria-[current=page]:text-text',
  location: 'aria-[current=location]:bg-accent-subtle aria-[current=location]:font-semibold aria-[current=location]:text-text',
} as const;
const UNDER = {
  page: 'aria-[current=page]:shadow-[inset_0_-2px_0_var(--accent)]',
  location: 'aria-[current=location]:shadow-[inset_0_-2px_0_var(--accent)]',
} as const;
const BESIDE = {
  page: 'aria-[current=page]:shadow-[inset_2px_0_0_var(--accent)]',
  location: 'aria-[current=location]:shadow-[inset_2px_0_0_var(--accent)]',
} as const;

export function SectionNav({ label, current, items, className, presentation = 'responsive', onThisPage = false, listFrom = 'page', unsaved = false }: SectionNavProps) {
  const { t } = useTranslation();
  const view = useViewInstance()?.id;
  const nav = useRef<HTMLElement>(null);
  const search = useRef<HTMLInputElement>(null);
  const [filter, setFilter] = useState({ current, query: '' });
  if (filter.current !== current) setFilter({ current, query: '' });
  const needle = filter.query.trim().toLocaleLowerCase();
  const shown = items.filter((item) => needle === '' || item.label.toLocaleLowerCase().includes(needle));
  const [asking, setAsking] = useState<HTMLElement | null>(null);
  const passing = useRef(false);
  const inside = useContext(InsideSectionContent);
  const responsive = presentation === 'responsive';
  const strip = responsive && inside;
  const from = FROM[listFrom];
  const mark = onThisPage ? 'location' : 'page';
  const room = useListRoom(nav, responsive && !strip);
  useCurrentInView(nav, responsive, current, room);
  return <><nav ref={nav} aria-label={label} data-section-nav data-scroll-allow={responsive ? 'navigation' : undefined} data-current-section={current} data-section-strip={strip || undefined}
    style={room === undefined ? undefined : { '--section-room': `${room}px` } as CSSProperties}
    onClickCapture={(event) => {
      if (!unsaved || passing.current || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || event.button !== 0) return;
      const link = event.target instanceof Element ? event.target.closest<HTMLElement>('a[data-section-id]') : null;
      if (link === null || link.hasAttribute('aria-current')) return;
      event.preventDefault(); event.stopPropagation();
      if (view === undefined) setAsking(link);
      else {
        const owner = useWorkbench.getState().userId;
        void import('@/features/workbench/CloseViews').then(({ requestViewAction }) => requestViewAction([view],
          () => { passing.current = true; try { link.click(); } finally { passing.current = false; } },
          () => useWorkbench.getState().userId === owner && link.isConnected && nav.current?.contains(link) === true));
      }
    }}
    className={cx('min-w-0 shrink-0', presentation === 'sidebar' && 'max-h-full overflow-y-auto overscroll-contain', responsive && (strip ? 'w-full' : from.nav), responsive && onThisPage && 'sticky top-0 z-(--layer-sticky) bg-surface py-1', responsive && onThisPage && !strip && from.top, className)}>
    {items.length > 10 && <Input ref={search} type="search" hideLabel label={t('ui.sectionNav.search')} placeholder={t('ui.sectionNav.search')}
      value={filter.query} fieldClassName="sticky top-0 z-1 mb-1 min-w-0 bg-surface" onChange={(event) => setFilter({ current, query: event.target.value })}
      onKeyDown={(event) => {
        if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); setFilter({ current, query: '' }); }
        if (event.key === 'ArrowDown') { event.preventDefault(); nav.current?.querySelector<HTMLElement>('a[data-section-id]')?.focus(); }
      }} />}
    <ul className={cx('flex min-w-0 gap-1', presentation === 'sidebar' ? 'flex-col' : 'flex-row overflow-x-auto', responsive && !strip && from.list)}>
      {shown.map((item, index) => <li key={item.id} className={cx('min-w-0 max-w-full', responsive && 'shrink-0', responsive && !strip && from.item,
        item.group !== undefined && index > 0 && shown[index - 1]?.group !== item.group && (presentation === 'sidebar' ? 'mt-3' : cx('ml-3', !strip && from.group)))}>
        <Slot.Root data-section-id={item.id} aria-current={item.id === current ? mark : undefined}
          className={cx('focus-inset flex min-h-(--control-sm) max-w-full items-start gap-2 rounded-sm px-2 py-1 text-sm leading-5 text-text-muted hover:bg-bg-muted hover:text-text', MARKED[mark],
            presentation === 'sidebar' ? BESIDE[mark] : cx('whitespace-nowrap', UNDER[mark], responsive && !strip && from.link, responsive && !strip && from.side[mark]))}>
          {cloneElement(item.link, { children: <>{item.icon !== undefined && <span aria-hidden className="flex h-5 shrink-0 items-center">{item.icon}</span>}<span className="min-w-0 wrap-anywhere">{item.label}</span></> })}
        </Slot.Root>
      </li>)}
    </ul>
    {shown.length === 0 && <p role="status" className="px-2 py-1 text-sm text-text-muted">{t('ui.combobox.empty')}</p>}
  </nav>
  <ConfirmDialog open={asking !== null} onOpenChange={(open) => { if (!open) setAsking(null); }} title={t('settingsEditors.leave.title')} confirmLabel={t('settingsEditors.leave.confirm')} danger
    onConfirm={() => { const link = asking; setAsking(null); passing.current = true; link?.click(); passing.current = false; }}>
    <p>{t('settingsEditors.leave.about')}</p>
  </ConfirmDialog></>;
}
