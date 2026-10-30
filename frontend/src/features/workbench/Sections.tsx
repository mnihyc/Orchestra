import { moveScoped, panelScopeEntries } from '@/features/workbench/panelScope';
import { useFrameReady } from '@/shell/FrameReadiness';
/** A container's stacked, independently expanded and resized views. Bodies stay alive when their section moves. */
import { useDraggable, useDroppable } from '@dnd-kit/core';
import { ChevronRight, Ellipsis, EyeOff, Terminal } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Group, Panel } from 'react-resizable-panels';

import { ContainerSwitcher } from '@/features/workbench/ContainerSwitcher';
import { ViewActionSlot } from '@/features/workbench/actions';
import { containerOf, FALLBACK_CONTAINER_ICON } from '@/features/workbench/containers';
import { viewChoice } from '@/features/workbench/choiceWords';
import { useWorkbenchChoiceHelp } from '@/features/workbench/choiceHelp';
import { activeContainer, expandSection, findTab, resizeSections, selectContainer, showSection, type Section, type SideBar, type SideBarId, type Tab, type Target, type ViewContainer } from '@/features/workbench/layout';
import { moveEntries, spotId, type DropSpot } from '@/features/workbench/MoveView';
import { revealView } from '@/features/workbench/openView';
import { useWorkbench } from '@/features/workbench/store';
import { useViews, viewOf } from '@/features/workbench/views';
import { ContextMenu } from '@/ui/ContextMenu';
import { cx } from '@/ui/cx';
import { EmptyState } from '@/ui/EmptyState';
import { IconButton } from '@/ui/IconButton';
import { Tooltip } from '@/ui/Tooltip';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { Sash } from '@/ui/Sash';

export const sectionElementId = (id: string) => `section-${id}`;

function useSectionDetails(section: Section | undefined) {
  const { t } = useTranslation();
  const helpFor = useWorkbenchChoiceHelp();
  const layout = useWorkbench((state) => state.layout);
  const change = useWorkbench((state) => state.change);
  const named = useWorkbench((state) => section === undefined ? undefined : state.named.get(section.tab.id));
  const definition = useViews().find((view) => view.name === section?.tab.view);
  const place = section === undefined ? null : findTab(layout, section.tab.id);
  const repeatsContainer = place?.container?.name === section?.tab.view && (place?.container?.sections.filter((item) => !item.hidden).length ?? 0) > 1;
  const title = named?.title ?? (repeatsContainer ? t('workbench.section.overview') : definition === undefined ? section?.tab.view ?? '' : viewChoice(definition, t).name);
  const entries: MenuEntry[] = section === undefined ? [] : [
    ...panelScopeEntries(section.tab, t),
    { id: 'hide', label: t('workbench.section.hide'), onSelect: () => change((current) => showSection(current, section.tab.id, false)) },
    { type: 'sub', id: 'move', label: t('workbench.move.label'), entries: moveEntries(layout, section.tab.id, t, (target) => change((current) => moveScoped(current, section.tab.id, target), section.tab.id), helpFor) },
  ];
  return { definition, title, entries };
}

function SectionActions({ section, bar, details, menu = true }: { section: Section; bar: SideBarId; details: ReturnType<typeof useSectionDetails>; menu?: boolean }) {
  const ready = useFrameReady();
  const { t } = useTranslation();
  const { definition, title, entries } = details;
  const Actions = definition?.actions;
  const Add = definition?.add;
  return <>
    {ready && (!menu || section.open) && <ViewActionSlot tab={section.tab} />}
    {ready && Actions !== undefined && definition?.menu === undefined && <Actions tab={section.tab} area={bar} />}
    {ready && menu && Add !== undefined && <Add tab={section.tab} area={bar} />}
    {menu && <SectionMenu section={section} bar={bar} details={details} label={t('workbench.section.menu', { name: title })} entries={entries} />}
  </>;
}

function SectionMenu({ section, bar, details, label, entries }: { section?: Section; bar: SideBarId; details: ReturnType<typeof useSectionDetails>; label: string; entries: MenuEntry[] }) {
  const ready = useFrameReady();
  const NativeMenu = details.definition?.menu;
  if (ready && NativeMenu !== undefined && section !== undefined) return <NativeMenu tab={section.tab} area={bar} label={label} entries={entries} />;
  return <Menu label={label} entries={entries} align="end" trigger={<IconButton action="actions" size="sm" label={label} icon={<Ellipsis aria-hidden />} />} />;
}

function SectionHeader({ section, bar, details }: { section: Section; bar: SideBarId; details: ReturnType<typeof useSectionDetails> }) {
  const ready = useFrameReady();
  const { t } = useTranslation();
  const change = useWorkbench((state) => state.change);
  const { definition, title, entries } = details;
  const { setNodeRef, attributes: dragAttributes, listeners, isDragging } = useDraggable({ id: section.tab.id, data: { title } });
  const { 'aria-pressed': _pressed, role: _role, tabIndex: _stop, ...attributes } = dragAttributes;
  const Summary = definition?.summary;
  return <ContextMenu label={t('workbench.section.menu', { name: title })} entries={entries}>
    <div className={cx('flex h-(--section-heading-height) shrink-0 items-center border-b border-border bg-bg-subtle pr-1', isDragging && 'opacity-50')} data-section-header>
      <button type="button" id={sectionElementId(section.tab.id)} ref={setNodeRef} {...attributes} {...listeners}
        aria-label={title} aria-expanded={section.open} aria-controls={`section-body-${section.tab.id}`}
        className="focus-inset flex h-full min-w-0 flex-1 cursor-default items-center gap-1 px-2 text-left text-sm"
        onClick={() => change((current) => expandSection(current, section.tab.id, !section.open))}>
        <ChevronRight aria-hidden className={cx('shrink-0 transition-transform', section.open && 'rotate-90')} />
        <span className="truncate font-medium">{title}</span>
        {ready && Summary !== undefined && <span className="ml-auto truncate pl-1 text-xs font-normal text-text-muted" data-section-summary><Summary /></span>}
      </button>
      <SectionActions section={section} bar={bar} details={details} />
    </div>
  </ContextMenu>;
}

/** A lone view uses the bar's title as its carry handle; it has no redundant disclosure row. */
function SingleSectionTitle({ section, details, title, icon: Icon, iconOnly = false }: { section: Section; details: ReturnType<typeof useSectionDetails>; title: string; icon: LucideIcon; iconOnly?: boolean }) {
  const ready = useFrameReady();
  const { t } = useTranslation();
  const { setNodeRef, attributes: dragAttributes, listeners, isDragging } = useDraggable({ id: section.tab.id, data: { title: details.title } });
  const { 'aria-pressed': _pressed, ...attributes } = dragAttributes;
  const Summary = details.definition?.summary;
  return <ContextMenu label={t('workbench.section.menu', { name: details.title })} entries={details.entries}>
    <Tooltip content={iconOnly ? title : undefined}>
    <button type="button" ref={setNodeRef} {...attributes} {...listeners} aria-label={title} aria-controls={sectionElementId(section.tab.id)}
      className={cx('focus-inset flex h-(--tab-strip-height) min-w-0 flex-1 cursor-default items-center gap-2 px-3 text-left text-sm font-medium', isDragging && 'opacity-50')}>
      <Icon aria-hidden className="shrink-0" /><span className={iconOnly ? 'sr-only' : 'truncate'}>{title}</span>
      {ready && Summary !== undefined && <span className="ml-auto truncate text-xs font-normal text-text-muted" data-section-summary><Summary /></span>}
    </button>
    </Tooltip>
  </ContextMenu>;
}

function SectionView({ section, bar, renderView, merged = false }: { section: Section; bar: SideBarId; renderView: (tab: Tab) => ReactNode; merged?: boolean }) {
  const details = useSectionDetails(section);
  return <section id={merged ? sectionElementId(section.tab.id) : undefined} aria-label={merged ? details.title : undefined} tabIndex={merged ? -1 : undefined}
    className="focus-inset flex h-full min-h-0 min-w-0 flex-col overflow-hidden" data-section-view={section.tab.view}>
    {!merged && <SectionHeader section={section} bar={bar} details={details} />}
    <div id={`section-body-${section.tab.id}`} aria-labelledby={merged ? undefined : sectionElementId(section.tab.id)} hidden={!merged && !section.open} className="relative min-h-0 min-w-0 flex-1 overflow-hidden">
      {(merged || section.open) && renderView(section.tab)}
    </div>
  </section>;
}

// Match the shared heading token before styles are available, including component tests.
const FALLBACK_HEADING_PX = 40;

/** The collapsed section uses the shared heading height at the current text size. */
function headerPixels(): number {
  const style = getComputedStyle(document.documentElement);
  const value = style.getPropertyValue('--section-heading-height').trim();
  const size = Number.parseFloat(value);
  const rem = Number.parseFloat(style.fontSize) || 16;
  return Number.isFinite(size) ? size * (value.endsWith('rem') ? rem : 1) : FALLBACK_HEADING_PX;
}

function SectionStack({ container, bar, renderView }: { container: ViewContainer; bar: SideBarId; renderView: (tab: Tab) => ReactNode }) {
  const { t } = useTranslation();
  const change = useWorkbench((state) => state.change);
  const [header] = useState(headerPixels);
  const sections = container.sections.filter((section) => !section.hidden);
  if (sections.length === 0) return <EmptyState compact icon={<EyeOff aria-hidden />} title={t('workbench.containers.empty')} description={t('workbench.containers.show')} />;
  if (sections.length === 1 && sections[0] !== undefined) return <SectionView section={sections[0]} bar={bar} renderView={renderView} merged />;
  if (!sections.some((section) => section.open)) return <div className="flex min-h-0 flex-col">{sections.map((section) => <div key={section.tab.id} style={{ height: header }}><SectionView section={section} bar={bar} renderView={renderView} /></div>)}</div>;
  const shape = sections.map((section) => `${section.tab.id}:${section.open}`).join('|');
  const total = sections.filter((section) => section.open).reduce((sum, section) => sum + section.size, 0);
  return <Group key={shape} id={`sections-${container.id}`} orientation="vertical" className="min-h-0 min-w-0 flex-1" onLayoutChanged={(sizes, meta) => {
    if (meta.isUserInteraction) change((current) => resizeSections(current, bar, container.name, Object.fromEntries(sections.filter((section) => section.open).map((section) => [section.tab.id, sizes[section.tab.id] ?? section.size]))));
  }}>
    {sections.flatMap((section, index) => [
      index > 0 && section.open && sections.slice(0, index).some((before) => before.open) && <Sash key={`${section.tab.id}-sash`} id={`${section.tab.id}-sash`} line="horizontal" label={t('workbench.section.resize')} />,
      <Panel key={section.tab.id} id={section.tab.id} minSize={header} maxSize={section.open ? '100%' : header} defaultSize={section.open ? `${100 * section.size / total}%` : header} style={{ overflow: 'hidden' }} className="min-h-0 min-w-0">
        <SectionView section={section} bar={bar} renderView={renderView} />
      </Panel>,
    ])}
  </Group>;
}

function ContainerDrop({ container, bar, children }: { container: ViewContainer; bar: SideBarId; children: ReactNode }) {
  const { t } = useTranslation();
  const definition = containerOf(container.name);
  const target: Target = { to: 'container', bar, container: container.name };
  const { setNodeRef, isOver } = useDroppable({ id: spotId(target), data: { target, label: definition === undefined ? container.name : t(definition.title) } satisfies DropSpot });
  return <div ref={setNodeRef} className={cx('relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden', isOver && 'ring-2 ring-inset ring-accent')}>{children}</div>;
}

export function SideBarView({ bar, layout, renderView, actions, addView, onSelectContainer, externalSwitcher = false }: { bar: SideBarId; layout: SideBar; renderView: (tab: Tab) => ReactNode; actions?: ReactNode; addView: ReactNode; onSelectContainer?: (name: string) => void; externalSwitcher?: boolean }) {
  const ready = useFrameReady();
  const { t } = useTranslation();
  const change = useWorkbench((state) => state.change);
  const helpFor = useWorkbenchChoiceHelp();
  const container = activeContainer(layout);
  const visible = container?.sections.filter((section) => !section.hidden) ?? [];
  const single = visible.length === 1 ? visible[0] : undefined;
  const details = useSectionDetails(single);
  const Add = details.definition?.add;
  if (container === null) return null;
  const definition = containerOf(container.name);
  const view = viewOf(container.sections[0]?.tab.view ?? '');
  const title = definition === undefined ? view === undefined ? container.name : t(view.title) : t(definition.title);
  const Icon = definition?.icon ?? view?.icon ?? FALLBACK_CONTAINER_ICON;
  const sections: MenuEntry[] = container.sections.map((section) => {
    const view = viewOf(section.tab.view);
    const choice = view === undefined ? { name: t('views.screen.title'), description: t('views.screen.about') } : viewChoice(view, t);
    return { type: 'check', id: section.tab.id, ...choice, help: helpFor(choice.name), checked: !section.hidden,
      onCheckedChange: (shown) => change((current) => showSection(current, section.tab.id, shown)),
    };
  });
  const menuEntries: MenuEntry[] = single === undefined ? sections : [...details.entries, { type: 'separator', id: 'sections' }, ...sections];
  const panelRole = externalSwitcher ? 'region' : bar === 'right' ? 'tabpanel' : undefined;
  const labelledBy = externalSwitcher ? `container-choice-${bar}:${container.name}` : bar === 'right' ? `container-tab-${container.name}` : undefined;
  return <section aria-label={t(`workbench.areas.${bar}`)} data-area-stop tabIndex={-1} data-workbench-bar={bar} data-active-container={container.name} className="focus-inset flex h-full min-h-0 min-w-0 flex-col overflow-hidden bg-bar-surface">
    <div className="flex min-h-(--bar-heading-height) min-w-0 shrink-0 items-stretch border-b border-border bg-bar-surface" data-container-title>
      {externalSwitcher ? <span className="min-w-0 flex-1" /> : bar === 'right' ? <ContainerSwitcher containers={layout.containers} active={container.name} section={single === undefined ? undefined : { id: single.tab.id, title: details.title }} onSelect={(name) => { if (onSelectContainer !== undefined) onSelectContainer(name); else change((current) => selectContainer(current, bar, name)); }} />
        : single === undefined ? <span className="flex h-(--tab-strip-height) min-w-0 flex-1 items-center gap-2 px-3 text-sm font-medium"><Icon aria-hidden /><span className="truncate">{title}</span></span>
          : <SingleSectionTitle section={single} details={details} title={title} icon={Icon} />}
      <div className="flex h-(--tab-strip-height) shrink-0 items-center gap-1 self-start px-1">
        {single !== undefined && <SectionActions section={single} bar={bar} details={details} menu={false} />}
        {ready && single !== undefined && Add !== undefined ? <Add tab={single.tab} area={bar} /> : addView}
        <SectionMenu section={single} bar={bar} details={details} label={t('workbench.containers.menu', { name: title })} entries={menuEntries} />
        {actions}
      </div>
    </div>
    <div role={panelRole} id={`container-panel-${bar}-${container.name}`} aria-labelledby={labelledBy} className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden [&_.empty-state-inline]:p-3">
      <ContainerDrop container={container} bar={bar}><SectionStack container={container} bar={bar} renderView={renderView} /></ContainerDrop>
    </div>
    {container.name === 'environment' && <button type="button" className="focus-inset flex h-(--control-md) shrink-0 items-center gap-2 border-t border-border px-3 text-sm text-text-muted hover:bg-bg-muted" onClick={() => revealView('terminal')}><Terminal aria-hidden />{t('workbench.section.openTerminal')}</button>}
  </section>;
}
