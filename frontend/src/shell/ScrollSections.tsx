/** The sections of one page, all on one scroll under their headings, with their list beside (a strip above where the room is narrow) marking the one the page is at.
 *
 * Nothing is chosen to be on the page: a person scrolls through every section, or presses a name to be taken to it. This is the layout of the organization's
 * settings, applied wherever a page has groups of settings: a template's settings, a unit's configuration, the groups of a place's settings.
 */
import { type ReactNode, type RefObject, useEffect, useId, useRef, useState } from 'react';

import { PageSectionContext, Section } from '@/shell/Page';
import { cx } from '@/ui/cx';
import { SectionContent, SectionNav, type SectionNavProps } from '@/ui/SectionNav';

export interface ScrollSection {
  id: string;
  title: string;
  body: ReactNode;
}

/** How far below the top of the area the line is that decides which section the page is at: under a strip kept at the top. */
const SPY_LINE_PX = 96;

/** The id of the section the page is at: the last whose top has passed the line, or the last of all where the page is scrolled to its end. */
export function useSectionSpy(root: RefObject<HTMLElement | null>, anchors: readonly string[]): string | undefined {
  const [current, setCurrent] = useState<string | undefined>(undefined);
  const key = anchors.join('\u0000');
  useEffect(() => {
    const element = root.current;
    const area = element?.closest<HTMLElement>('[data-scroll-area]') ?? null;
    const list = key === '' ? [] : key.split('\u0000');
    if (element === null || area === null || list.length === 0) return;
    let frame = 0;
    const measure = () => {
      frame = 0;
      const line = area.getBoundingClientRect().top + SPY_LINE_PX;
      let found: string | undefined = list[0];
      for (const anchor of list) {
        const target = document.getElementById(anchor);
        if (target !== null && target.getBoundingClientRect().top <= line) found = anchor;
      }
      if (area.scrollHeight > area.clientHeight && area.scrollTop + area.clientHeight >= area.scrollHeight - 2) found = list.at(-1);
      setCurrent(found);
    };
    const schedule = () => { if (frame === 0) frame = requestAnimationFrame(measure); };
    schedule();
    area.addEventListener('scroll', schedule, { passive: true });
    const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(schedule);
    observer?.observe(area);
    observer?.observe(element);
    return () => { cancelAnimationFrame(frame); area.removeEventListener('scroll', schedule); observer?.disconnect(); };
  }, [root, key]);
  return current;
}

/** Whole class names, so that they are found: the row begins where the list does. */
const ROW = {
  page: '@min-[600px]:flex-row @min-[600px]:items-start',
  group: '@min-[48rem]:flex-row @min-[48rem]:items-start',
} as const;
const TARGET = {
  page: 'scroll-mt-14 @min-[600px]:scroll-mt-3',
  group: 'scroll-mt-14 @min-[48rem]:scroll-mt-3',
} as const;

export interface ScrollSectionsProps extends Pick<SectionNavProps, 'listFrom'> {
  /** The name of the list, for a screen reader. */
  label: string;
  sections: readonly ScrollSection[];
  /** Heading rank of the sections: below the page's own, below a tab's. */
  level?: 2 | 3;
  className?: string;
}

export function ScrollSections({ label, sections, level = 3, listFrom = 'page', className }: ScrollSectionsProps) {
  const prefix = useId();
  const root = useRef<HTMLDivElement>(null);
  const anchor = (id: string) => `${prefix}-${id}`;
  const spied = useSectionSpy(root, sections.map((section) => anchor(section.id)));
  const [picked, setPicked] = useState<string | null>(null);
  const [seen, setSeen] = useState(spied);
  // A press names the section until the page is next moved by its reader.
  if (seen !== spied) { setSeen(spied); setPicked(null); }
  const first = sections[0]?.id;
  const current = picked ?? sections.find((section) => anchor(section.id) === spied)?.id ?? first ?? '';
  const jump = (id: string) => {
    setPicked(id);
    const target = document.getElementById(anchor(id));
    // Not every environment can scroll an element into view (a test's document cannot): the press still names the section.
    target?.scrollIntoView?.({ block: 'start' });
    target?.focus({ preventScroll: true });
  };
  return <div className="@container min-w-0">
    <div ref={root} data-scroll-sections className={cx('flex min-w-0 flex-col gap-4', ROW[listFrom], className)}>
      {sections.length > 1 && <SectionNav onThisPage listFrom={listFrom} label={label} current={current}
        items={sections.map((section) => ({ id: section.id, label: section.title,
          link: <a href={`#${anchor(section.id)}`} onClick={(event) => { event.preventDefault(); jump(section.id); }} /> }))} />}
      <SectionContent><div className="flex min-w-0 flex-1 flex-col gap-6">
        {sections.map((section) => <div key={section.id} id={anchor(section.id)} tabIndex={-1} data-scroll-section={section.id} className={cx('focus-inset min-w-0 rounded-md', TARGET[listFrom])}>
          <Section id={`${anchor(section.id)}-title`} title={section.title} level={level}>
            <PageSectionContext value={{ title: section.title, id: `${anchor(section.id)}-title` }}>{section.body}</PageSectionContext>
          </Section>
        </div>)}
      </div></SectionContent>
    </div>
  </div>;
}
