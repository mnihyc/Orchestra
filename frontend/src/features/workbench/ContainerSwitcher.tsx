/** Every side-bar container keeps its icon and full name, wrapping controls and labels when needed. */
import type { LucideIcon } from 'lucide-react';
import type { KeyboardEvent } from 'react';
import { useDraggable, useDroppable } from '@dnd-kit/core';
import { useTranslation } from 'react-i18next';

import { containerOf, FALLBACK_CONTAINER_ICON, useContainers } from '@/features/workbench/containers';
import { containerChoice, viewChoice } from '@/features/workbench/choiceWords';
import type { Target, ViewContainer } from '@/features/workbench/layout';
import { spotId, type DropSpot } from '@/features/workbench/MoveView';
import { viewOf } from '@/features/workbench/views';
import { cx } from '@/ui/cx';
import { Tooltip } from '@/ui/Tooltip';

const ITEM = 'focus-inset relative inline-flex min-h-(--tab-strip-height) min-w-0 shrink-0 items-start justify-center gap-2 px-2 text-sm after:absolute after:inset-x-0 after:bottom-0 after:h-0.5';

interface SwitcherItem { id: string; title: string; about: string; Icon: LucideIcon }
interface CarriedSection { id: string; title: string }

function ContainerTab({ item, active, onSelect, section }: { item: SwitcherItem; active: boolean; onSelect: () => void; section?: CarriedSection | undefined }) {
  const target: Target = { to: 'container', bar: 'right', container: item.id };
  const { setNodeRef: setDropNodeRef, isOver } = useDroppable({ id: `switcher:${spotId(target)}`, data: { target, label: item.title } satisfies DropSpot });
  const { setNodeRef: setDragNodeRef, attributes: dragAttributes, listeners, isDragging } = useDraggable({ id: section?.id ?? `container-title:${item.id}`, data: { title: section?.title ?? item.title }, disabled: section === undefined });
  const { 'aria-pressed': _pressed, role: _role, tabIndex: _stop, ...attributes } = dragAttributes;
  const { Icon } = item;
  return <Tooltip content={item.title}>
    <button ref={(node) => { setDropNodeRef(node); setDragNodeRef(node); }} {...(section === undefined ? {} : attributes)} {...listeners}
      type="button" id={`container-tab-${item.id}`} role="tab" aria-label={item.title} aria-description={item.about} aria-selected={active} aria-controls={active ? `container-panel-right-${item.id}` : undefined} tabIndex={active ? 0 : -1}
      style={{ paddingBlock: 'calc((var(--tab-strip-height) - var(--spacing-5)) / 2)' }}
      className={cx(ITEM, 'max-w-full', active ? 'bg-bg text-text after:bg-accent' : 'text-text-muted hover:bg-bg-muted hover:text-text', isOver && 'ring-2 ring-inset ring-accent', isDragging && 'opacity-50')} onClick={onSelect}>
      <span className="flex h-5 shrink-0 items-center"><Icon aria-hidden /></span><span data-container-name data-active-container-title={active ? true : undefined} className="min-w-0 text-left leading-5 wrap-anywhere">{item.title}</span>
    </button>
  </Tooltip>;
}

export function ContainerSwitcher({ containers, active, onSelect, section }: { containers: readonly ViewContainer[]; active: string; onSelect: (name: string) => void; section?: CarriedSection | undefined }) {
  const { t } = useTranslation();
  useContainers();
  const items = containers.map((container) => {
    const definition = containerOf(container.name);
    const view = viewOf(container.sections[0]?.tab.view ?? '');
    const words = definition === undefined && view !== undefined ? viewChoice(view, t) : containerChoice(container.name, t);
    return { id: container.name, title: words.name, about: words.description, Icon: definition?.icon ?? view?.icon ?? FALLBACK_CONTAINER_ICON };
  });
  const choose = (name: string) => {
    onSelect(name);
    requestAnimationFrame(() => document.getElementById(`container-tab-${name}`)?.focus());
  };
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.defaultPrevented || event.altKey || event.ctrlKey || event.metaKey) return;
    const at = items.findIndex((item) => `container-tab-${item.id}` === (event.target as HTMLElement).id);
    if (at < 0) return;
    const index = event.key === 'Home' ? 0 : event.key === 'End' ? items.length - 1 : event.key === 'ArrowRight' ? (at + 1) % items.length : event.key === 'ArrowLeft' ? (at - 1 + items.length) % items.length : null;
    const item = index === null ? undefined : items[index];
    if (item !== undefined) { event.preventDefault(); choose(item.id); }
  };
  return <div className="relative flex min-w-0 flex-1 items-stretch" data-navigation-strip="containers">
    <div role="tablist" aria-label={t('workbench.containers.label')} className="flex min-w-0 flex-1 flex-wrap items-stretch" onKeyDown={onKeyDown}>
      {items.map((item) => <ContainerTab key={item.id} item={item} active={item.id === active} section={item.id === active ? section : undefined} onSelect={() => onSelect(item.id)} />)}
    </div>
  </div>;
}
