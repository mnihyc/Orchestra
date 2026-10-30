/** The same visible section links serve personal, organization and Management settings at every width. */
import { Slot } from 'radix-ui';
import { cloneElement, type ReactElement, type ReactNode, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { cx } from '@/ui/cx';
import { Combobox } from '@/ui/Combobox';

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
  /** Its visible owning tab already names this picker; its accessible label is retained. */
  hidePickerLabel?: boolean;
}

export function SectionNav({ label, current, items, className, presentation = 'responsive', hidePickerLabel = false }: SectionNavProps) {
  const nav = useRef<HTMLElement>(null);
  const { t } = useTranslation();
  const [query, setQuery] = useState('');
  const words = query.trim().toLocaleLowerCase();
  const choices = items.filter((item) => item.label.toLocaleLowerCase().includes(words))
    .map((item) => ({ value: item.id, label: item.label, text: item.label, icon: item.icon }));
  return <nav ref={nav} aria-label={label} data-section-nav data-current-section={current} className={cx('min-w-0 shrink-0', presentation === 'responsive' && 'w-full @min-[600px]:sticky @min-[600px]:top-0 @min-[600px]:w-48', className)}>
    {presentation === 'responsive' && <div className="@min-[600px]:hidden"><Combobox size="sm" label={label} hideLabel={hidePickerLabel}
      query={query} onQueryChange={setQuery} displayValue={items.find((item) => item.id === current)?.label ?? ''}
      onOpenChange={(open) => { if (!open) setQuery(''); }} placeholder={t('ui.combobox.search')}
      groups={[{ id: 'sections', label: '', options: choices }]} empty={t('ui.combobox.empty')}
      onSelect={(choice) => {
        // Use the same real scoped link; searching never reads or changes a destination.
        const link = Array.from(nav.current?.querySelectorAll<HTMLAnchorElement>('a[data-section-id]') ?? []).find((one) => one.dataset.sectionId === choice.value);
        setQuery(''); link?.click();
      }} /></div>}
    <ul className={cx('min-w-0 flex-col gap-1', presentation === 'sidebar' ? 'flex' : 'hidden @min-[600px]:flex')}>
      {items.map((item, index) => <li key={item.id} className={cx('min-w-0 max-w-full', item.group !== undefined && index > 0 && items[index - 1]?.group !== item.group && 'mt-3')}>
        <Slot.Root data-section-id={item.id} aria-current={item.id === current ? 'page' : undefined}
          className="focus-inset flex min-h-(--control-sm) max-w-full items-start gap-2 rounded-sm px-2 py-1 text-sm leading-5 text-text-muted hover:bg-bg-muted hover:text-text aria-[current=page]:bg-accent-subtle aria-[current=page]:font-semibold aria-[current=page]:text-text aria-[current=page]:shadow-[inset_2px_0_0_var(--accent)]">
          {cloneElement(item.link, { children: <>{item.icon !== undefined && <span aria-hidden className="flex h-5 shrink-0 items-center">{item.icon}</span>}<span className="min-w-0 wrap-anywhere">{item.label}</span></> })}
        </Slot.Root>
      </li>)}
    </ul>
  </nav>;
}
