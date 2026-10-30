/** The same ordered status-item editor is opened from a place's context menu or Layout. */
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { GripVertical } from 'lucide-react';
import { Hint } from '@/features/help/Hint';
import { useWorkbench } from '@/features/workbench/store';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { IconButton } from '@/ui/IconButton';
import { Select } from '@/ui/Select';
import { EmptyState } from '@/ui/EmptyState';
import { defaultStatus, orderStatus, showStatus, stepStatus } from './arrangement';
import { STATUS_ITEMS, STATUS_PLACES, type StatusItemId, type StatusPlace } from './registry';

export function StatusCustomizer({ initialPlace = 'bar', onDismiss }: { initialPlace?: StatusPlace; onDismiss?: () => void }) {
  const { t } = useTranslation();
  const [place, setPlace] = useState(initialPlace);
  const entries = useWorkbench((state) => state.statusItems), set = useWorkbench((state) => state.setStatusItems);
  const dragging = useRef<StatusItemId | null>(null);
  const rows = entries.filter((entry) => entry.place === place);
  return <div className="flex w-64 max-w-full flex-col gap-2" data-status-customizer onKeyDownCapture={(event) => {
    if (event.key === 'Escape' && event.target instanceof Element && event.target.closest('[role="listbox"]') === null) {
      event.stopPropagation(); onDismiss?.();
    }
  }}>
    <Select label={t('statusItems.place')} help={<Hint id="status-items" popover />} size="sm" value={place} onValueChange={setPlace}
      options={STATUS_PLACES.map((value) => ({ value, name: t(`statusItems.places.${value}`) }))} />
    {rows.length === 0 && <EmptyState compact title={t('statusItems.noItems')} />}
    <ul aria-label={t('statusItems.order')} className="flex max-h-[min(24rem,50vh)] flex-col gap-1 overflow-y-auto">
      {rows.map((entry) => {
        const definition = STATUS_ITEMS.find((item) => item.id === entry.id);
        if (definition === undefined) return null;
        const name = t(definition.title);
        return <li key={entry.id} data-status-order={entry.id} draggable
          onDragStart={(event) => { dragging.current = entry.id; event.dataTransfer.effectAllowed = 'move'; event.dataTransfer.setData('text/plain', entry.id); }}
          onDragEnd={() => { dragging.current = null; }}
          onDragOver={(event) => { if (dragging.current !== null) event.preventDefault(); }}
          onDrop={(event) => { event.preventDefault(); if (dragging.current !== null) set(orderStatus(entries, dragging.current, entry.id)); dragging.current = null; }}
          onKeyDown={(event) => { if (event.altKey && (event.key === 'ArrowUp' || event.key === 'ArrowDown')) { event.preventDefault(); set(stepStatus(entries, entry.id, event.key === 'ArrowUp' ? -1 : 1)); } }}
          className="flex min-w-0 items-center gap-1 rounded-md px-1 hover:bg-bg-muted">
          <IconButton size="sm" icon={<GripVertical aria-hidden />} label={t('statusItems.reorder', { name })} className="cursor-grab" />
          <Checkbox label={name} checked={entry.shown} onCheckedChange={(shown) => set(showStatus(entries, entry.id, place, shown))} />
        </li>;
      })}
    </ul>
    <Button size="sm" className="self-end" onClick={() => set(defaultStatus())}>{t('statusItems.reset')}</Button>
  </div>;
}
