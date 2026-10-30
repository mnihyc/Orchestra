/** All three status places share visibility, movement, order and the same lazy inspection cards. */
import { useRef, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useWorkbench } from '@/features/workbench/store';
import { DetailCard } from '@/ui/DetailCard';
import { ContextMenu } from '@/ui/ContextMenu';
import { Popover } from '@/ui/Popover';
import type { MenuEntry } from '@/ui/Menu';
import { cx } from '@/ui/cx';
import { Facts } from '@/features/resources/Facts';
import { ResourceGlyph, resourcePreview } from '@/features/resource-facts/ResourceFacts';
import { StatusDot } from '@/ui/StatusDot';
import { StatusCustomizer } from './Customize';
import { moveStatus, showStatus } from './arrangement';
import { STATUS_ITEMS, STATUS_PLACES, type StatusItemId, type StatusPlace as Place, type StatusValue } from './registry';
import { StatusStrip, type StatusSlot } from './StatusStrip';
import { useStatusValues } from './useValues';

function Value({ title, value }: { title: string; value: StatusValue }) {
  const preview = value.subject === undefined ? undefined : resourcePreview(value.subject);
  return <DetailCard label={title} content={preview?.content ?? (() => <div className="min-w-48 p-3"><h3 className="mb-2 text-sm font-medium">{title}</h3><Facts compact rows={value.details ?? [{ name: title, value: value.text }]} /></div>)}
    className={cx('focus-inset px-1 py-0.5 text-sm whitespace-nowrap', value.warning && 'text-warning-text')}>
    {value.subject !== undefined && <ResourceGlyph kind={value.subject.kind} />}
    {value.warning === true ? <StatusDot tone="warning" label={value.text} showLabel /> : <span className="min-w-0 truncate">{value.text}</span>}
  </DetailCard>;
}

interface StatusPlaceProps { place: Place; threadId?: string; children?: ReactNode; fixed?: readonly StatusSlot[]; activeLabel?: string; className?: string; role?: 'status' }

export function StatusPlace(props: StatusPlaceProps) {
  return props.threadId === undefined ? <StatusSurface {...props} values={[]} /> : <ThreadStatusPlace {...props} threadId={props.threadId} />;
}

function ThreadStatusPlace(props: StatusPlaceProps & { threadId: string }) {
  const values = useStatusValues(props.threadId, props.place, props.activeLabel);
  return <StatusSurface {...props} values={values} />;
}

function StatusSurface({ place, values, children, fixed = [], className, role }: StatusPlaceProps & { values: ReturnType<typeof useStatusValues> }) {
  const { t } = useTranslation();
  const entries = useWorkbench((state) => state.statusItems), set = useWorkbench((state) => state.setStatusItems);
  const [selected, select] = useState<StatusItemId | null>(null), [editing, edit] = useState(false);
  const surface = useRef<HTMLDivElement>(null);
  const current = selected ?? entries.find((entry) => entry.place === place && entry.shown)?.id ?? null;
  const definition = STATUS_ITEMS.find((item) => item.id === current);
  const menu: MenuEntry[] = STATUS_ITEMS.filter((item) => item.places.includes(place)).map((item) => ({ type: 'check', id: item.id, name: t(item.title), checked: entries.some((entry) => entry.id === item.id && entry.place === place && entry.shown), onCheckedChange: (shown) => set(showStatus(entries, item.id, place, shown)) }));
  if (definition !== undefined) menu.push({ type: 'sub', id: 'move', label: t('statusItems.moveTo'), entries: STATUS_PLACES.filter((target) => target !== place && definition.places.includes(target)).map((target) => ({ id: target, name: t(`statusItems.places.${target}`), onSelect: () => set(moveStatus(entries, definition.id, target)) })) });
  menu.push({ type: 'separator', id: 'customize-separator' }, { id: 'customize', name: t('statusItems.customize'), onSelect: () => edit(true) });
  const capture = (target: EventTarget | null) => {
    const id = target instanceof Element ? target.closest('[data-status-item]')?.getAttribute('data-status-item') : null;
    select(STATUS_ITEMS.find((item) => item.id === id)?.id ?? null);
  };
  const slots: StatusSlot[] = [...fixed, ...values.map(({ id, title, value }) => ({ id, text: value.text, content: <Value title={title} value={value} /> }))];
  const region = <div ref={surface} tabIndex={0} role={role ?? 'group'} aria-label={t('statusItems.region', { place: t(`statusItems.places.${place}`) })} data-status-place={place}
    onContextMenuCapture={(event) => capture(event.target)} onPointerDownCapture={(event) => capture(event.target)}
    onKeyDownCapture={(event) => { if (event.key === 'ContextMenu' || event.shiftKey && event.key === 'F10') capture(event.target); }}
    className={cx('focus-inset min-w-0', place === 'header' ? 'flex flex-col' : 'flex flex-1 items-center', className)}>
    {children}{slots.length > 0 && <StatusStrip items={slots} rows={place === 'live' ? 2 : 1} />}
  </div>;
  return <Popover anchor label={t('statusItems.title')} open={editing} onOpenChange={edit} side={place === 'header' ? 'bottom' : 'top'}
    onCloseAutoFocus={(event) => { event.preventDefault(); surface.current?.focus(); }} trigger={<div className={cx('flex min-w-0', place === 'header' ? 'shrink-0 flex-col' : 'flex-1')}><ContextMenu label={t('statusItems.title')} entries={menu}>{region}</ContextMenu></div>}>
    <StatusCustomizer initialPlace={place} onDismiss={() => edit(false)} />
  </Popover>;
}
