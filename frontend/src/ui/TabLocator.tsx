/** Search the open tabs in one group using the titles and paths already in the workbench. */
import { Search, X } from 'lucide-react';
import { type KeyboardEvent, useId, useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { isComposing } from '@/lib/shortcuts';
import { EmptyState } from '@/ui/EmptyState';
import { IconButton } from '@/ui/IconButton';
import { PaletteDialog } from '@/ui/PaletteDialog';
import { inputClasses, itemClasses, listClasses } from '@/ui/paletteStyle';
import type { GroupTabInfo } from '@/ui/ViewGroup';

interface TabChoicesProps {
  activeId: string | null;
  tabs: readonly GroupTabInfo[];
  select: (tab: GroupTabInfo) => void;
}

function TabChoices({ activeId, tabs, select }: TabChoicesProps) {
  const { t } = useTranslation();
  const listId = useId();
  const [query, setQuery] = useState('');
  const [chosen, setChosen] = useState(activeId);
  const pointerChoice = useRef<string | null>(null);
  const terms = query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
  const shown = terms.length === 0 ? tabs : tabs.filter((tab) => {
    const words = `${tab.title} ${tab.path ?? ''}`.toLocaleLowerCase();
    return terms.every((term) => words.includes(term));
  });
  const selected = Math.max(0, shown.findIndex((tab) => tab.id === chosen));
  const current = shown[selected];
  const currentOptionId = current && `${listId}-${current.id}`;
  const field = t('workbench.workspace.findTab');

  useLayoutEffect(() => {
    if (currentOptionId !== undefined && currentOptionId !== pointerChoice.current) {
      document.getElementById(currentOptionId)?.scrollIntoView({ block: 'nearest' });
    }
  }, [currentOptionId]);

  const navigate = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.defaultPrevented || isComposing(event.nativeEvent) || current === undefined) return;
    let next: number;
    switch (event.key) {
      case 'Enter':
        event.preventDefault();
        select(current);
        return;
      case 'Home': next = 0; break;
      case 'End': next = shown.length - 1; break;
      case 'ArrowDown': next = event.metaKey ? shown.length - 1 : (selected + 1) % shown.length; break;
      case 'ArrowUp': next = event.metaKey ? 0 : (selected + shown.length - 1) % shown.length; break;
      default: return;
    }
    event.preventDefault();
    pointerChoice.current = null;
    setChosen(shown[next]?.id ?? null);
  };

  return <>
    <div className="flex items-center gap-2 border-b border-border px-4">
      <Search className="text-text-subtle" aria-hidden />
      <input role="combobox" aria-label={field} placeholder={field} aria-expanded="true" aria-autocomplete="list"
        aria-controls={listId} aria-activedescendant={currentOptionId} className={inputClasses} autoComplete="off"
        value={query} onChange={(event) => { pointerChoice.current = null; setQuery(event.target.value); setChosen(null); }} onKeyDown={navigate} />
    </div>
    <div id={listId} role="listbox" aria-label={field} className={listClasses}>
      {shown.map((tab) => <div key={tab.id} role="option" id={`${listId}-${tab.id}`} data-value={tab.id}
        aria-selected={tab === current} data-selected={tab === current} className={itemClasses}
        aria-label={tab.unsaved ? t('workbench.tab.unsaved', { title: tab.title }) : tab.title}
        onPointerMove={(event) => { pointerChoice.current = event.currentTarget.id; setChosen(tab.id); }} onClick={() => select(tab)}>
        {tab.icon}<span className="min-w-0 flex-1"><span className="block truncate">{tab.title}</span>
          {tab.path !== undefined && <span className="block truncate text-xs text-text-muted">{tab.path}</span>}
        </span>{tab.unsaved && <span aria-hidden data-unsaved className="size-2 shrink-0 rounded-full bg-text-muted" />}
        {tab.closable !== false && <IconButton size="sm" label={t('workbench.tab.closeNamed', { title: tab.title })} icon={<X aria-hidden />}
          onKeyDown={(event) => event.stopPropagation()} onClick={(event) => { event.stopPropagation(); tab.onClose(); }} />}
      </div>)}
    </div>
    {shown.length === 0 && <EmptyState compact title={t('workbench.workspace.noMatchingTabs')} />}
  </>;
}

export function TabLocator({ label, activeId, tabs, tabElementId }: Omit<TabChoicesProps, 'select'> & {
  label: string;
  tabElementId: (id: string) => string;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const destination = useRef<string | null>(null);
  const name = t('workbench.allTabs', { name: label });
  return <>
    <IconButton size="sm" label={name} icon={<Search aria-hidden />} onClick={() => setOpen(true)} />
    <PaletteDialog label={name} showTitle open={open} onOpenChange={setOpen} onCloseAutoFocus={(event) => {
      const id = destination.current;
      if (id === null) return;
      destination.current = null;
      event.preventDefault();
      requestAnimationFrame(() => document.getElementById(tabElementId(id))?.focus());
    }}>
      <TabChoices activeId={activeId} tabs={tabs} select={(tab) => {
        destination.current = tab.id;
        tab.onSelect();
        setOpen(false);
      }} />
    </PaletteDialog>
  </>;
}
