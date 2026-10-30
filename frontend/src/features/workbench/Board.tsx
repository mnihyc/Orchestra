import { LiveThreadStateIcon } from '@/ui/ThreadState';
/** The workbench drawn from its layout tree: every group of tabs in its area, each view kept alive while its tab moves, and a tab carried from one place to another by pointer or by keyboard.
 *
 * Three things meet here. The frame of areas and borders is the shared `Workbench`. The layout
 * tree says which groups stand in which area and which tab is in front. And every view is drawn
 * once, in a place of its own outside the frame, and shown in whichever group holds its tab: so a
 * view that is moved, or whose group is split, keeps what it was showing and what was typed in it.
 */
import { useNodeLabel } from '@/features/navigation/placeLabels';
import { CachedTabNames, useTabCaptions } from './TabNames';
import { DocumentMarksProvider, forgetDocumentMarks } from '@/features/documents/marks';
import { moveScoped, panelTarget, panelRead, panelContinuation } from '@/features/workbench/panelScope';
import { openFinder } from '@/features/finder/store';
import { ArrangementMenu } from '@/features/workbench/Arrangements';
import { Button } from '@/ui/Button';
import { ViewInstanceContext } from '@/features/workbench/instance';
import { FocusedContentMetadata } from '@/features/workbench/PageView';
import { contentLocation, contentTarget } from './contentAddress';
import { WorkspaceContext } from './WorkspaceContext';
import { ThreadReadProvider } from '@/realtime/threadReadContext';
import { InspectorContext } from '@/shell/RouteContext';
import { forgetScreenHeading, useCurrentHeading } from '@/shell/screen';
import { useTabName, isContentTab } from '@/features/workbench/store';
import { ViewCompactContext, ViewShapeContext, type Shape } from '@/features/workbench/Narrow';
import {
  DndContext,
  DragOverlay,
  KeyboardSensor,
  PointerSensor,
  useDndContext,
  useDraggable,
  useDroppable,
  useSensor,
  useSensors,
} from '@dnd-kit/core';
import { PanelBottomClose, PanelLeftClose, PanelRightClose, Pin, SquareDashedMousePointer } from 'lucide-react';
import { type ReactNode, Fragment, Suspense, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';

import { frameLimits } from '@/features/workbench/Arrangements';
import { useThreadWindowLifetime } from '@/features/workbench/threadWindows';
import {
  allGroups,
  type AreaId,
  type BarId,
  barHasViews,
  findTab,
  type Group,
  type Layout,
  type InspectorTarget,
  type MainNode,
  mainGroups,
  removeEmptyMainGroup,
  resizeBar,
  resizeSplit,
  setBarOpen,
  selectContainer,
  type Side,
  type Tab,
  type Target,
  toggleMaximized,
} from '@/features/workbench/layout';
import {
  areaName,
  carryBetweenSpots,
  DRAG_STARTS_AFTER_PX,
  type DropSpot,
  groupName,
  overSpot,
  spotId,
  spotLabel,
} from '@/features/workbench/MoveView';
import { RightBarOverMain, useShape } from '@/features/workbench/Narrow';
import { sectionElementId, SideBarView } from '@/features/workbench/Sections';
import { viewChoice } from '@/features/workbench/choiceWords';
import { useWorkbenchChoiceHelp } from '@/features/workbench/choiceHelp';
import { forgetViewOpening, openingForView, showViewOpening, ViewOpening } from '@/features/workbench/viewOpening';
import { forgetViewActions, ViewActionSlot } from '@/features/workbench/actions';
import { viewKey } from '@/features/workbench/viewIdentity';
import { useWorkbench } from '@/features/workbench/store';
import { useFrameReadiness } from '@/shell/FrameReadiness';
import { isClosable, tabEntries } from '@/features/workbench/Tabs';
import { SlideOutBar } from './SlideOutBar';
import { useViews, type ViewDefinition, viewOf } from '@/features/workbench/views';
import { useVisibleLayout } from '@/features/workbench/availability';
import { AddViewButton } from '@/features/workbench/OpenViews';
import { cx } from '@/ui/cx';
import { EmptyState } from '@/ui/EmptyState';
import { ContentBoundary } from '@/ui/ContentBoundary';
import { IconButton } from '@/ui/IconButton';
import { SkeletonRows } from '@/ui/Skeleton';
import { ViewGroup } from '@/ui/ViewGroup';
import { ViewTab } from '@/ui/ViewTab';
import { type BarSpec, SplitView, Workbench } from '@/ui/Workbench';

const tabElementId = (tabId: string) => `tab-${tabId}`;
const panelIdOf = (groupId: string) => `panel-${groupId}`;
const SLIDING_BARS = ['left', 'right'] as const;

// ---------------------------------------------------------------- views that outlive their place

/** The element each view is drawn into. A group shows a view by taking its element in. */
const homes = new Map<string, HTMLElement>();

function homeOf(key: string): HTMLElement {
  let home = homes.get(key);
  if (home === undefined) {
    home = document.createElement('div');
    home.className = 'h-full min-h-0';
    homes.set(key, home);
  }
  return home;
}

/** Keeps each saved view's home; its body first mounts when a visible slot leases it. */
export function ViewHost({ layout }: { layout: Layout }) {
  useThreadWindowLifetime();
  const unsaved = useWorkbench((state) => state.unsaved);
  const placed = allGroups(layout).flatMap(({ area, group }) => group.tabs.map((tab) => ({ tab, area })));
  const keys = placed.map(({ tab }) => viewKey(tab)).join('\n');

  // A view whose tab is gone has no home any more.
  useEffect(() => {
    const live = new Set(keys.split('\n'));
    for (const [key, home] of homes) {
      if (live.has(key)) continue;
      home.remove();
      homes.delete(key);
      forgetViewOpening(key);
      forgetViewActions(key);
      forgetScreenHeading(key);
      forgetDocumentMarks(key);
    }
  }, [keys]);

  return <><CachedTabNames layout={layout} /><FocusedContentMetadata />{placed.map(({ tab, area }) => createPortal(<ContentBoundary resetKey={viewKey(tab)} className="h-full"><ViewOpening view={viewKey(tab)} route={openingForView(tab.id)} hold={unsaved.has(tab.id)}><ViewBody tab={tab} area={area} /></ViewOpening></ContentBoundary>, homeOf(viewKey(tab)), viewKey(tab)))}</>;
}

/** Shows a view in the group that holds its tab, by taking in the element the view is drawn into. */
export function ViewSlot({ tab, primary = false }: { tab: Tab; primary?: boolean }) {
  const { ready, pending } = useFrameReadiness();
  const slot = useRef<HTMLDivElement>(null);
  const key = viewKey(tab);
  useLayoutEffect(() => {
    const parent = slot.current;
    if (parent === null) return;
    const home = homeOf(key);
    parent.appendChild(home);
    // Portal events follow React ownership, while this movable home follows DOM placement.
    // Listen at its actual slot so pointer/keyboard focus updates the addressed pane.
    const focus = () => useWorkbench.getState().focus(tab.id);
    parent.addEventListener('focusin', focus, true);
    parent.addEventListener('pointerdown', focus, true);
    const closeOpening = ready ? showViewOpening(key, openingForView(tab.id)) : () => undefined;
    return () => {
      closeOpening();
      parent.removeEventListener('focusin', focus, true);
      parent.removeEventListener('pointerdown', focus, true);
      // Another group may have taken the view in already; then it is no longer this one's to let go.
      if (home.parentElement === parent) parent.removeChild(home);
    };
  }, [key, ready, tab.id]);
  return <div className="h-full min-h-0">
    {!ready && (primary && pending !== null ? pending : <SkeletonRows rows={4} className="p-3" />)}
    <div ref={slot} hidden={!ready} className="h-full min-h-0" />
  </div>;
}

function ViewBody({ tab, area }: { tab: Tab; area: AreaId }) {
  const { t } = useTranslation();
  const definition = useViews().find((candidate) => candidate.name === tab.view);
  const activeContent = useWorkbench((state) => state.activeContent);
  const layout = useWorkbench((state) => state.layout);
  const content = isContentTab(tab);
  const root = useRef<HTMLDivElement>(null);
  const [shape, setShape] = useState<Shape>('wide');
  const [compact, setCompact] = useState(false);
  const activeHeading = useCurrentHeading();
  const location = useMemo(() => contentLocation(tab), [tab]);
  const target = useMemo(() => content ? contentTarget(tab) : panelTarget(tab, layout, activeContent, activeHeading), [content, tab, layout, activeContent, activeHeading]);
  const fixed = !content && (tab.scope?.mode === 'fixed' || tab.of?.thread !== undefined || tab.of?.environment !== undefined);
  const headingOwner = content || fixed ? tab.id : activeContent;
  const instance = useMemo(() => ({ id: tab.id, root, target, headingOwner,
    content, focusedContent: content && activeContent === tab.id }), [tab.id, target, content, headingOwner, activeContent]);
  const read = useMemo(() => content ? null : panelRead(tab, layout, activeContent, target), [content, tab, layout, activeContent, target]);
  useLayoutEffect(() => {
    const element = root.current;
    if (element === null || typeof ResizeObserver === 'undefined') return;
    const tokens = getComputedStyle(document.documentElement);
    const width = (name: string, fallback: number) => {
      const value = tokens.getPropertyValue(name).trim(), size = Number.parseFloat(value);
      return !Number.isFinite(size) ? fallback : value.endsWith('rem') ? size * (Number.parseFloat(tokens.fontSize) || 16) : size;
    };
    const medium = width('--screen-md', 900), large = width('--screen-lg', 1200), compactAt = width('--breakpoint-sm', 640);
    const measure = () => {
      const size = element.clientWidth;
      if (size > 0) { setShape(size >= large ? 'wide' : size >= medium ? 'medium' : 'small'); setCompact(size < compactAt); }
    };
    const observer = new ResizeObserver(measure); observer.observe(element); measure();
    return () => observer.disconnect();
  }, []);
  const Body = definition?.body;
  const contentBody = <ViewInstanceContext.Provider value={instance}><ViewShapeContext.Provider value={shape}><ViewCompactContext.Provider value={compact}>
    {target?.workspace !== undefined ? <WorkspaceContext source={target.workspace} location={content ? location : null} />
      : fixed && target !== null ? <InspectorContext target={target} continuation={panelContinuation(tab)} /> : null}
    <InstanceName id={tab.id} content={content} fixed={fixed} target={target} location={content ? location : null} title={definition === undefined ? tab.view : t(definition.title)} />
    <Suspense fallback={<SkeletonRows rows={5} />}>
      {Body === undefined ? <EmptyState compact icon={<SquareDashedMousePointer aria-hidden />} title={t('workbench.view.unknown')} /> : <Body tab={tab} area={area} />}
    </Suspense>
  </ViewCompactContext.Provider></ViewShapeContext.Provider></ViewInstanceContext.Provider>;
  const body = target?.kind === 'document' ? <DocumentMarksProvider owner={headingOwner ?? tab.id} documentId={target.id}>{contentBody}</DocumentMarksProvider> : contentBody;
  return <div ref={root} className="h-full min-h-0 min-w-0" data-view-instance={tab.id}>
    <ThreadReadProvider read={read}>{body}</ThreadReadProvider>
  </div>;
}

function InstanceName({ id, content, fixed, title, target, location }: { id: string; content: boolean; fixed: boolean; title: string; target: InspectorTarget | null; location: ReturnType<typeof contentLocation> }) {
  const heading = useCurrentHeading();
  const qualified = useNodeLabel(heading?.target?.kind === 'thread' ? heading.target.id : undefined, heading?.tabTitle ?? heading?.title);
  const hierarchy = heading?.path?.map((part) => part.redacted ? '…' : part.name).join(' / ');
  const path = location !== null && location.view !== 'page' ? [hierarchy, location.path].filter(Boolean).join(' / ') : hierarchy;
  const pending = (content || fixed) && (heading === null || heading.tabPending === true || target !== null && (heading.views === undefined
    || heading.target !== undefined && (heading.target.kind !== target.kind || heading.target.id !== target.id)));
  useTabName(id, content ? qualified : fixed ? `${title} · ${qualified ?? title}` : null, content ? heading?.icon : undefined, path, pending, content ? heading?.thread : undefined);
  return null;
}

// ---------------------------------------------------------------- a tab, and where one can be dropped

interface TabHandleProps {
  tab: Tab;
  group: Group;
  index: number;
  area: AreaId;
  active: boolean;
  tabbable: boolean;
  definition: ViewDefinition | undefined;
  presentedTitle?: string;
}

function TabHandle({ tab, group, index, area, active, tabbable, definition, presentedTitle }: TabHandleProps) {
  const { t } = useTranslation();
  const helpFor = useWorkbenchChoiceHelp();
  const layout = useWorkbench((state) => state.layout);
  const change = useWorkbench((state) => state.change);
  const unsaved = useWorkbench((state) => state.unsaved.has(tab.id));
  const named = useWorkbench((state) => state.named.get(tab.id));
  const baseTitle = named?.title ?? (definition === undefined ? tab.view : t(definition.title));
  const title = presentedTitle ?? baseTitle;
  const target: Target = { to: 'group', group: group.id, index };

  const drag = useDraggable({ id: tab.id, data: { title } });
  const drop = useDroppable({
    id: spotId(target),
    data: { target, label: t('workbench.spots.before', { tab: title, group: groupName(layout, group.id, t) }) } satisfies DropSpot,
  });
  const { active: carriedTab } = useDndContext();

  // A tab is a tab to a screen reader, also while it can be carried: what the carrying adds is said
  // in the instructions the tab is described by.
  const { 'aria-pressed': _pressed, 'aria-roledescription': _role, role: _button, tabIndex: _stop, ...described } = drag.attributes;
  const Icon = named?.icon ?? definition?.icon;

  return (
    <ViewTab
      id={tabElementId(tab.id)}
      panelId={panelIdOf(group.id)}
      title={title}
      icon={named?.thread === undefined ? Icon === undefined ? null : <Icon aria-hidden /> : <LiveThreadStateIcon display={named.thread.display} threadId={named.thread.id} />}
      about={named?.path ?? (definition === undefined ? undefined : t(definition.about))}
      active={active}
      tabbable={tabbable}
      unsaved={unsaved}
      kept={false}
      pinned={tab.pinned === true}
      closable={isClosable(layout, tab.id) && tab.pinned !== true}
      dropBefore={drop.isOver && carriedTab !== null && carriedTab.id !== tab.id}
      carried={drag.isDragging}
      menu={tabEntries(layout, tab.id, t, change, helpFor)}
      onSelect={() => useWorkbench.getState().focus(tab.id)}
      onClose={() => useWorkbench.getState().close({ kind: 'tab', tab: tab.id })}
      onDoubleClick={area === 'main' ? () => change((current) => toggleMaximized(current, group.id)) : undefined}
      handleRef={(node) => {
        drag.setNodeRef(node);
        drop.setNodeRef(node);
      }}
      handleProps={{ ...described, ...drag.listeners }}
    />
  );
}

/** The rest of a strip after its tabs: dropping here puts a tab last in the group. */
function StripEnd({ group }: { group: Group }) {
  const { t } = useTranslation();
  const layout = useWorkbench((state) => state.layout);
  const target: Target = { to: 'group', group: group.id };
  const { setNodeRef, isOver } = useDroppable({
    id: spotId(target),
    data: { target, label: spotLabel(layout, target, t) } satisfies DropSpot,
  });
  const { active } = useDndContext();
  return (
    <div ref={setNodeRef} role="presentation" className="relative min-w-4 flex-1">
      {isOver && active !== null && (
        <span aria-hidden className="absolute inset-y-1 left-0 w-0.5 rounded-full bg-accent" />
      )}
    </div>
  );
}

const EDGES: Record<Side, { zone: string; mark: string }> = {
  left: { zone: 'inset-y-0 left-0 w-1/4', mark: 'inset-y-0 left-0 w-1/2' },
  right: { zone: 'inset-y-0 right-0 w-1/4', mark: 'inset-y-0 right-0 w-1/2' },
  up: { zone: 'inset-x-0 top-0 h-1/4', mark: 'inset-x-0 top-0 h-1/2' },
  down: { zone: 'inset-x-0 bottom-0 h-1/4', mark: 'inset-x-0 bottom-0 h-1/2' },
};

function DropZone({ target, zone, mark }: { target: Target; zone: string; mark: string }) {
  const { t } = useTranslation();
  const layout = useWorkbench((state) => state.layout);
  const label = spotLabel(layout, target, t);
  const { setNodeRef, isOver } = useDroppable({ id: spotId(target), data: { target, label } satisfies DropSpot });
  return (
    <>
      <div ref={setNodeRef} className={cx('absolute', zone)} />
      {isOver && (
        <div aria-hidden className={cx('pointer-events-none absolute flex items-center justify-center border-2 border-accent bg-accent/15', mark)}>
          <span className="rounded-sm bg-accent-solid px-2 py-1 text-xs font-medium text-text-on-accent">{label}</span>
        </div>
      )}
    </>
  );
}

/** The places over a group's view where a carried tab can be dropped: on it to join it, on an edge to stand beside it. */
function DropZones({ group, area }: { group: Group; area: AreaId }) {
  const { active } = useDndContext();
  const layout = useWorkbench((state) => state.layout);
  if (active === null) return null;
  const sides = area === 'main' ? (Object.keys(EDGES) as Side[]) : [];
  // The only tab of a group has nowhere to go beside its own group.
  const lone = group.tabs.length === 1 && group.tabs[0]?.id === active.id;
  return (
    <div className="absolute inset-0 z-(--layer-sticky)">
      <DropZone target={{ to: 'group', group: group.id, index: group.tabs.length }} zone="inset-0" mark="inset-0" />
      {!lone &&
        mainGroups(layout.main).length > 0 &&
        sides.map((side) => (
          <DropZone key={side} target={{ to: 'split', group: group.id, side }} zone={EDGES[side].zone} mark={EDGES[side].mark} />
        ))}
    </div>
  );
}

/** A bar that is closed, shown at its edge of the frame while a tab is carried, so that the tab can be put there. */
function Dock({ bar }: { bar: BarId }) {
  const { t } = useTranslation();
  const target: Target = { to: 'area', area: bar };
  const label = areaName(bar, t);
  const { setNodeRef, isOver } = useDroppable({ id: spotId(target), data: { target, label } satisfies DropSpot });
  const place = { left: 'inset-y-2 left-2 w-12', right: 'inset-y-2 right-2 w-12', bottom: 'inset-x-2 bottom-2 h-12' }[bar];
  return (
    <div
      ref={setNodeRef}
      className={cx(
        'absolute z-(--layer-dropdown) flex items-center justify-center rounded-lg border-2 border-dashed text-xs font-medium',
        isOver ? 'border-accent bg-accent-subtle text-accent-text' : 'border-border-strong bg-surface-raised text-text-muted',
        place,
      )}
    >
      <span className={cx(bar !== 'bottom' && '[writing-mode:vertical-rl]')}>{label}</span>
    </div>
  );
}

// ---------------------------------------------------------------- a group in its area

function GroupView({ group: originalGroup, area, actions }: { group: Group; area: AreaId; actions?: ReactNode }) {
  const { ready, pending } = useFrameReadiness();
  const { t } = useTranslation();
  const helpFor = useWorkbenchChoiceHelp();
  const registered = useViews();
  const hasPorts = registered.some((view) => view.name === 'ports');
  const group = { ...originalGroup, tabs: originalGroup.tabs.filter((tab) => tab.view !== 'ports' || hasPorts) };
  const layout = useWorkbench((state) => state.layout);
  const focused = useWorkbench((state) => state.focusedGroup === group.id);
  const setFocusedGroup = useWorkbench((state) => state.setFocusedGroup);
  const named = useWorkbench((state) => state.named);
  const unsaved = useWorkbench((state) => state.unsaved);
  const change = useWorkbench((state) => state.change);
  const frame = useWorkbench();
  const front = group.tabs.find((tab) => tab.id === group.active) ?? group.tabs[0] ?? null;
  const byId = new Map(group.tabs.map((tab, index) => [tab.id, { tab, index }]));
  const captions = useTabCaptions(area === 'main' ? mainGroups(layout.main).flatMap((item) => item.tabs).filter((tab) => tab.view !== 'ports' || hasPorts) : group.tabs);

  return (
    <ViewGroup
      label={groupName(layout, group.id, t)}
      tabIds={group.tabs.map((tab) => tab.id)}
      activeId={front?.id ?? null}
      tabInfo={group.tabs.map((tab) => {
        const definition = viewOf(tab.view);
        const Icon = named.get(tab.id)?.icon ?? definition?.icon;
        const thread = named.get(tab.id)?.thread;
        const choice = definition === undefined ? { name: tab.view, description: '' } : viewChoice(definition, t);
        const title = captions.get(tab.id) ?? named.get(tab.id)?.title ?? choice.name;
        return { id: tab.id, title: t('workbench.choices.tab.name', { name: title }), path: named.get(tab.id)?.path, description: t('workbench.choices.tab.description', { description: choice.description }), help: helpFor(title),
          icon: thread === undefined ? Icon === undefined ? undefined : <Icon aria-hidden /> : <LiveThreadStateIcon display={thread.display} threadId={thread.id} />, unsaved: unsaved.has(tab.id), closable: isClosable(layout, tab.id) && tab.pinned !== true,
          onSelect: () => useWorkbench.getState().focus(tab.id), onClose: () => useWorkbench.getState().close({ kind: 'tab', tab: tab.id }) };
      })}
      tabElementId={tabElementId}
      panelId={panelIdOf(group.id)}
      compactTabs={area === 'main' && mainGroups(layout.main).length > 1}
      focused={focused && area === 'main' && mainGroups(layout.main).length > 1}
      onFocusWithin={() => setFocusedGroup(group.id)}
      actions={<>{ready && front !== null && <ViewActionSlot tab={front} />}{frame.focusMode && area === 'main' && focused && <AddViewButton />}{actions}</>}
      stripEnd={<StripEnd group={group} />}
      overlay={<DropZones group={group} area={area} />}
      empty={
        !ready && pending !== null ? pending : <EmptyState
          className="h-full"
          icon={<SquareDashedMousePointer aria-hidden />}
          title={t('workbench.group.empty')}
          action={area === 'main' ? <>
            <Button size="sm" onClick={() => { setFocusedGroup(group.id); openFinder(''); }}>{t('workbench.workspace.open')}</Button>
            {group.retainEmpty === true && <Button size="sm" onClick={() => change((layout) => removeEmptyMainGroup(layout, group.id))}>{t('workbench.workspace.removeGroup')}</Button>}
            <ArrangementMenu screen={frame.screen} current={frame.arrangement} onChoose={frame.choose} onReset={frame.reset} />
            {frame.focusMode && <Button size="sm" onClick={() => frame.setFocusMode(false)}>{t('workbench.workspace.exitFocus')}</Button>}
          </> : undefined}
        />
      }
      renderTab={(tabId, state) => {
        const entry = byId.get(tabId);
        if (entry === undefined) return null;
        return (
          <TabHandle
            key={tabId}
            tab={entry.tab}
            index={entry.index}
            group={group}
            area={area}
            active={state.active}
            tabbable={state.tabbable}
            definition={viewOf(entry.tab.view)}
            presentedTitle={captions.get(tabId)}
          />
        );
      }}
    >
      {front !== null && <ViewSlot tab={front} primary={area === 'main' && mainGroups(layout.main)[0]?.id === group.id} />}
    </ViewGroup>
  );
}


function MainTree({ node }: { node: MainNode }) {
  const { t } = useTranslation();
  const change = useWorkbench((state) => state.change);
  if (node.kind === 'group') return <GroupView group={node} area="main" />;
  // A split whose parts change is drawn anew with the shares the layout holds for them.
  const shape = node.children.map((child) => child.id).join('|');
  return (
    <SplitView
      key={shape}
      id={node.id}
      direction={node.direction}
      sashLabel={t('workbench.sash.groups')}
      onShares={(shares) => change((current) => resizeSplit(current, node.id, shares))}
      parts={node.children.map((child, index) => ({
        id: child.id,
        share: node.sizes[index] ?? 100 / node.children.length,
        content: <MainTree node={child} />,
      }))}
    />
  );
}

// ---------------------------------------------------------------- the board

export interface BoardProps {
  rail: ReactNode;
  /** What spans the main area and the right bar, above their tabs. */
  top: ReactNode;
  status: ReactNode;
  /** A native authority transition pauses content while the frame stays reachable. */
  pending?: ReactNode;
}

const CLOSE_ICONS = { left: PanelLeftClose, right: PanelRightClose, bottom: PanelBottomClose };

export function Board({ rail, top, status, pending }: BoardProps) {
  const { t } = useTranslation();
  const layout = useVisibleLayout();
  const change = useWorkbench((state) => state.change);
  const overlay = useWorkbench((state) => state.overlay);
  const setOverlay = useWorkbench((state) => state.setOverlay);
  const shape = useShape();
  const [limits] = useState(frameLimits);
  const [carried, setCarried] = useState<{ id: string; title: string } | null>(null);

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: DRAG_STARTS_AFTER_PX } }),
    useSensor(KeyboardSensor, {
      coordinateGetter: carryBetweenSpots,
      // Space picks a tab up; Enter brings it to the front and is left alone.
      keyboardCodes: { start: ['Space'], cancel: ['Escape'], end: ['Enter', 'Space'] },
    }),
  );

  const bar = (id: BarId): BarSpec => {
    const Close = CLOSE_ICONS[id];
    const floating = id !== 'bottom' && overlay === id && !layout[id].open;
    const close = () => { if (floating) setOverlay(null); else change((current) => setBarOpen(current, id, false)); };
    return {
      open: pending === undefined && layout[id].open,
      size: layout[id].size,
      min: limits[id].min,
      max: limits[id].max,
      usable: barHasViews(layout, id),
      sashLabel: t(`workbench.sash.${id}`),
      onOpenChange: (open) => change((current) => setBarOpen(current, id, open)),
      onResize: (size) => change((current) => resizeBar(current, id, size)),
      content: id !== 'bottom' ? <SideBarView bar={id} layout={layout[id]} renderView={(tab) => <ViewSlot tab={tab} />}
        onSelectContainer={(name) => change((current) => { const next = selectContainer(current, id, name); return floating ? { ...next, [id]: { ...next[id], open: false } } : next; })}
        addView={<AddViewButton />}
        actions={<>{floating && <IconButton size="sm" label={t('workbench.bars.pin')} icon={<Pin aria-hidden />} onClick={() => {
          change((current) => setBarOpen(current, id, true)); setOverlay(null);
          requestAnimationFrame(() => document.querySelector<HTMLElement>(`[data-workbench-bar="${id}"] button`)?.focus());
        }} />}
          <IconButton size="sm" label={t(`workbench.bars.close.${id}`)} icon={<Close aria-hidden />} onClick={close} /></>} /> : (
        <GroupView
          group={layout.bottom.group}
          area="bottom"
          actions={
            <>
              <IconButton size="sm" label={t(`workbench.bars.close.${id}`)} icon={<Close aria-hidden />} onClick={close} />
            </>
          }
        />
      ),
    };
  };

  const maximized = layout.maximized === null ? null : (mainGroups(layout.main).find((group) => group.id === layout.maximized) ?? null);
  const spotOf = (data: unknown): DropSpot | null => (data as DropSpot | undefined) ?? null;
  const focusMode = useWorkbench((state) => state.focusMode);
  const beside = shape === 'wide' || focusMode;

  return (
    <DndContext
      sensors={sensors}
      collisionDetection={overSpot}
      accessibility={{
        screenReaderInstructions: { draggable: t('workbench.drag.instructions') },
        announcements: {
          onDragStart: ({ active }) => t('workbench.drag.pickedUp', { title: (active.data.current as { title: string }).title }),
          onDragOver: ({ active, over }) => {
            const spot = spotOf(over?.data.current);
            if (spot === null) return t('workbench.drag.nowhere');
            // A tab that lies where it was picked up has been carried nowhere yet: nothing to say.
            const from = findTab(layout, String(active.id));
            if (from !== null && over?.id === spotId({ to: 'group', group: from.group.id, index: from.index })) return undefined;
            return t('workbench.drag.over', { spot: spot.label });
          },
          onDragEnd: ({ over }) => {
            const spot = spotOf(over?.data.current);
            return spot === null ? t('workbench.drag.putBack') : t('workbench.drag.dropped', { spot: spot.label });
          },
          onDragCancel: () => t('workbench.drag.putBack'),
        },
      }}
      onDragStart={({ active }) => setCarried({ id: String(active.id), title: (active.data.current as { title: string }).title })}
      onDragCancel={() => setCarried(null)}
      onDragEnd={({ active, over }) => {
        setCarried(null);
        const spot = spotOf(over?.data.current);
        if (spot === null) return;
        const tabId = String(active.id);
        change((current) => moveScoped(current, tabId, spot.target), tabId);
        // After popup drop targets close and the new slot mounts, the keyboard goes with the view.
        requestAnimationFrame(() => requestAnimationFrame(() => document.getElementById(findTab(useWorkbench.getState().layout, tabId)?.section === undefined ? tabElementId(tabId) : sectionElementId(tabId))?.focus()));
      }}
    >
      <Workbench focusMode={focusMode}
        // The frame is laid out anew when the right bar gains or loses its place beside the main area.
        key={beside ? 'beside' : 'over'}
        rail={rail}
        top={top}
        status={status}
        left={bar('left')}
        right={!focusMode && beside ? bar('right') : null}
        bottom={bar('bottom')}
        main={
          <main id="main-content" data-main-area data-area-stop tabIndex={-1} aria-label={t('workbench.areas.main')} className="relative h-full outline-none">
            {pending !== undefined ? pending : maximized === null ? (
              <MainTree node={layout.main} />
            ) : (
              <GroupView group={maximized} area="main" />
            )}
            {carried !== null && !layout.left.open && <Dock bar="left" />}
            {carried !== null && (!layout.right.open || !beside) && <Dock bar="right" />}
            {carried !== null && !layout.bottom.open && <Dock bar="bottom" />}
          </main>
        }
      />
      {!focusMode && pending === undefined && SLIDING_BARS.map((id) => {
        if (id === 'right' && !beside) return null;
        const usable = barHasViews(layout, id);
        return <Fragment key={id}>
          {!layout[id].open && usable && <div data-bar-edge={id} aria-hidden className="fixed top-0 bottom-(--status-bar-height) z-(--layer-sticky) w-(--bar-reveal-width)" style={id === 'left' ? { left: 'var(--rail-width)' } : { right: 0 }} onPointerEnter={(event) => { if (event.pointerType !== 'touch') setOverlay(id); }} />}
          <SlideOutBar bar={id} size={layout[id].size} open={usable && overlay === id && !layout[id].open} docked={layout[id].open} onClose={() => setOverlay(null)}>{bar(id).content}</SlideOutBar>
        </Fragment>;
      })}
      {!beside && (
        <RightBarOverMain open={pending === undefined && overlay === 'right'} onOpenChange={(open) => setOverlay(open ? 'right' : null)}>
          <SideBarView bar="right" layout={layout.right} renderView={(tab) => <ViewSlot tab={tab} />}
            externalSwitcher
            addView={null}
            actions={<IconButton size="sm" label={t('workbench.bars.close.right')} icon={<PanelRightClose aria-hidden />} onClick={() => setOverlay(null)} />} />
        </RightBarOverMain>
      )}
      <DragOverlay dropAnimation={null}>
        {carried !== null && (
          <div className="inline-flex h-(--control-sm) cursor-grabbing items-center gap-2 rounded-md border border-border-strong bg-surface-raised px-2 text-sm text-text shadow-md">
            {carried.title}
          </div>
        )}
      </DragOverlay>
    </DndContext>
  );
}
