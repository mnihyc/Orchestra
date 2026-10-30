/** One persistent workspace per signed-in person. Navigation changes focus, never the arrangement. */
import { THREAD_GLYPHS, type ThreadIconFacts } from '@/ui/ThreadState';
import type { LucideIcon } from 'lucide-react';
import { useEffect } from 'react';
import { create } from 'zustand';

import { arrangementOf, shippedLayout } from '@/features/workbench/Arrangements';
import {
  activate, addMainTab, allTabs, barHasViews, closeTabs, findGroup, findTab, findView,
  type BarId, type CloseScope, type Layout, LAYOUT_VERSION, makeGroup, makeTab, mapGroups,
  move, normalize, spatialMainGroups, tabsForClose, toggleBar, type Tab,
} from '@/features/workbench/layout';
import { type ClosedTab, loadWorkspace, saveWorkspace, type Workspace } from '@/features/workbench/persistence';
import { viewOf } from '@/features/workbench/views';
import type { ScreenKind } from '@/routes/-access';
import { readViewChoice, writeViewChoice } from '@/lib/viewChoices';
import { capturePresentations, restorePresentations } from './presentation';
import { defaultStatus, type StatusArrangement } from '@/features/status-items/arrangement';
import type { ChipPlace, ChipSize, ChipSizes } from '@/features/people/chipSizes';

/** Resizing emits many intermediate positions; structural changes also flush on pagehide/leave. */
const SAVE_AFTER_MS = 300;
const EMPTY: Layout = {
  version: LAYOUT_VERSION,
  left: { open: false, size: 1, containers: [], active: null },
  right: { open: false, size: 1, containers: [], active: null },
  bottom: { open: false, size: 1, group: makeGroup() },
  main: makeGroup(), maximized: null,
};

export interface TabName { thread?: ThreadIconFacts; title: string; icon?: LucideIcon; path?: string | undefined; source?: 'view' | 'cache' }
export const isContentTab = (tab: Tab): boolean => tab.view === 'screen' || tab.view === 'page' || tab.view === 'file' || tab.view === 'difference' || viewOf(tab.view)?.role === 'content';

function retainedMetadata(layout: Layout, before: Pick<WorkbenchState, 'named' | 'unsaved'>) {
  const live = new Set(allTabs(layout).map((tab) => tab.id));
  return {
    named: [...before.named.keys()].every((id) => live.has(id)) ? before.named : new Map([...before.named].filter(([id]) => live.has(id))),
    unsaved: [...before.unsaved].every((id) => live.has(id)) ? before.unsaved : new Set([...before.unsaved].filter((id) => live.has(id))),
  };
}

interface WorkbenchState extends Workspace {
  chipSizes: ChipSizes;
  setChipSize: (place: ChipPlace, size: ChipSize) => void;
  statusItems: StatusArrangement;
  setStatusItems: (items: StatusArrangement) => void;
  userId: string | null;
  screen: ScreenKind;
  dropped: boolean;
  overlay: BarId | null;
  unsaved: ReadonlySet<string>;
  named: ReadonlyMap<string, TabName>;
  /** A single close decision covers the whole captured set; cancelling changes no membership. */
  pendingClose: readonly string[] | null;
  enter: (userId: string, screen: ScreenKind) => void;
  /** An explicit move carries its addressed tab's focus; other layout edits retain focus. */
  change: (change: (layout: Layout) => Layout, focusedTab?: string) => void;
  choose: (arrangement: string) => void;
  reset: () => void;
  toggle: (bar: BarId, beside: boolean) => void;
  revealBar: (bar: 'left' | 'right', beside: boolean) => void;
  setFocusedGroup: (groupId: string | null) => void;
  focus: (tabId: string) => void;
  openTab: (view: string, of: Tab['of'], mode?: 'normal' | 'new' | 'beside') => string;
  close: (scope: CloseScope) => void;
  finishClose: (ids: readonly string[]) => void;
  cancelClose: () => void;
  reopen: () => void;
  setFocusMode: (focusMode: boolean) => void;
  setOverlay: (overlay: BarId | null) => void;
  acknowledgeDropped: () => void;
  setUnsaved: (tabId: string, unsaved: boolean) => void;
  setNamed: (tabId: string, name: TabName | null) => void;
  flush: () => void;
  leave: () => void;
}

let saveTimer: ReturnType<typeof setTimeout> | undefined;
function saved(state: WorkbenchState): Workspace {
  const { arrangement, layout, focusedGroup, focusedTab, focusedMainGroup, focusedMainTab, activeContent, focusMode, closedTabs } = state;
  const owners = new Set([...allTabs(layout), ...closedTabs.map((entry) => entry.tab)].map((tab) => tab.id));
  const titles = Object.fromEntries([...state.named].filter(([id]) => owners.has(id)).map(([id, name]) => [id, { title: name.title, ...(name.path === undefined ? {} : { path: name.path }) }]));
  return { arrangement, layout, focusedGroup, focusedTab, focusedMainGroup, focusedMainTab, activeContent, focusMode, closedTabs, titles, statusItems: state.statusItems, chipSizes: state.chipSizes, presentations: capturePresentations(owners) };
}
function keep(): void {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => useWorkbench.getState().flush(), SAVE_AFTER_MS);
}

/** Keep the addressed neighbor, then the next occupied group in spatial reading order. */
function focusAfter(layout: Layout, before: Pick<WorkbenchState, 'layout' | 'focusedGroup' | 'focusedTab' | 'focusedMainGroup' | 'focusedMainTab' | 'activeContent'>) {
  const groups = spatialMainGroups(layout.main);
  const oldGroups = spatialMainGroups(before.layout.main);
  const oldIndex = oldGroups.findIndex((group) => group.id === before.focusedMainGroup);
  const orderedIds = [...oldGroups.slice(oldIndex + 1), ...oldGroups.slice(0, Math.max(0, oldIndex))].map((group) => group.id);
  const own = groups.find((group) => group.id === before.focusedMainGroup);
  const next = orderedIds.map((id) => groups.find((group) => group.id === id)).find((group) => group !== undefined && group.tabs.length > 0)
    ?? groups.find((group) => group.tabs.length > 0);
  const still = before.focusedTab === null ? null : findTab(layout, before.focusedTab);
  const priorMain = before.focusedMainTab === null ? null : findTab(layout, before.focusedMainTab);
  const keptEmptyMain = before.focusedMainTab === null && own?.tabs.length === 0;
  const focusedMainTab = still?.area === 'main' ? still.tab.id : priorMain?.area === 'main' ? priorMain.tab.id : keptEmptyMain ? null : own?.active ?? next?.active ?? null;
  const focusedMainGroup = focusedMainTab === null ? own?.id ?? groups[0]?.id ?? null : findTab(layout, focusedMainTab)?.group.id ?? null;
  const focusedTab = still?.tab.id ?? own?.active ?? next?.active ?? null;
  const focusedGroup = focusedTab === null ? own?.id ?? groups[0]?.id ?? null : findTab(layout, focusedTab)?.group.id ?? null;
  const focused = focusedTab === null ? null : findTab(layout, focusedTab)?.tab;
  const oldContent = before.activeContent === null ? null : findTab(layout, before.activeContent)?.tab;
  const activeContent = focused !== null && focused !== undefined && isContentTab(focused) ? focused.id
    : oldContent !== null && oldContent !== undefined && isContentTab(oldContent) ? oldContent.id
      : groups.flatMap((group) => group.tabs).find(isContentTab)?.id ?? null;
  return { focusedTab, focusedGroup, focusedMainTab, focusedMainGroup, activeContent };
}

/** Apply a frame without destroying any existing main view or explicitly targeted inspector. */
function reframe(before: WorkbenchState, arrangement: string, reset: boolean): Layout {
  const template = shippedLayout(before.screen, arrangement);
  const existing = allTabs(before.layout);
  const main = spatialMainGroups(before.layout.main).flatMap((group) => group.tabs);
  const held = new Set(main.map((tab) => tab.id));
  const frame = mapGroups(template, (group, area) => ({ ...group, tabs: group.tabs.flatMap((tab) => {
    if (tab.view === 'screen' || tab.view === 'page' || (reset && area === 'main')) return [];
    const previous = existing.find((candidate) => candidate.view === tab.view && candidate.of === undefined && candidate.scope?.mode !== 'fixed');
    if (previous !== undefined && held.has(previous.id)) return [];
    const kept = previous ?? tab;
    held.add(kept.id);
    return [kept];
  }) }));
  const extra = spatialMainGroups(frame.main).flatMap((group) => group.tabs);
  const targeted = existing.filter((tab) => !held.has(tab.id) && (tab.pinned === true || tab.of !== undefined || tab.scope?.mode === 'fixed'));
  return normalize({ ...frame, main: makeGroup([...extra, ...main, ...targeted], before.focusedMainTab ?? before.activeContent ?? undefined), maximized: null });
}

function rememberBars(userId: string | null, layout: Layout): void {
  for (const side of ['left', 'right'] as const) {
    writeViewChoice(userId, 'global', `${side}Open`, layout[side].open);
    writeViewChoice(userId, 'global', `${side}Size`, layout[side].size);
  }
  writeViewChoice(userId, 'global', 'leftContainer', layout.left.active);
}

export const useWorkbench = create<WorkbenchState>((set, get) => ({
  userId: null, screen: 'page', arrangement: 'default', layout: EMPTY,
  dropped: false, focusedGroup: null, focusedTab: null, focusedMainGroup: null, focusedMainTab: null, activeContent: null, focusMode: false,
  closedTabs: [], overlay: null, unsaved: new Set(), named: new Map(), pendingClose: null,
  statusItems: defaultStatus(),
  chipSizes: {},
  setChipSize: (place, size) => { set((state) => ({ chipSizes: { ...state.chipSizes, [place]: size } })); keep(); },
  setStatusItems: (statusItems) => { set({ statusItems }); keep(); },

  enter: (userId, screen) => {
    const before = get();
    if (before.userId === userId && before.layout !== EMPTY) {
      if (before.screen !== screen) set({ screen });
      return;
    }
    before.flush();
    const { kept, dropped } = loadWorkspace(userId, screen);
    restorePresentations(kept?.presentations);
    const arrangement = kept?.arrangement ?? arrangementOf(screen).name;
    const own = kept?.layout ?? shippedLayout(screen, arrangement);
    const selectedLeft = readViewChoice(userId, 'global', 'leftContainer', own.left.active);
    const layout = kept === null ? { ...own, left: { ...own.left,
      open: readViewChoice(userId, 'global', 'leftOpen', own.left.open),
      size: readViewChoice(userId, 'global', 'leftSize', own.left.size),
      active: own.left.containers.some((container) => container.name === selectedLeft) ? selectedLeft : own.left.active,
    }, right: { ...own.right,
      open: readViewChoice(userId, 'global', 'rightOpen', own.right.open),
      size: readViewChoice(userId, 'global', 'rightSize', own.right.size),
    } } : own;
    const normalized = normalize(layout);
    const focus = focusAfter(normalized, { layout: normalized, focusedGroup: kept?.focusedGroup ?? null, focusedTab: kept?.focusedTab ?? null, focusedMainGroup: kept?.focusedMainGroup ?? null, focusedMainTab: kept?.focusedMainTab ?? null, activeContent: kept?.activeContent ?? null });
    set({ userId, screen, arrangement, layout: normalized, ...focus, focusMode: kept?.focusMode ?? false,
      closedTabs: kept?.closedTabs ?? [], statusItems: kept?.statusItems ?? defaultStatus(), chipSizes: kept?.chipSizes ?? {}, dropped, overlay: null, named: new Map(Object.entries(kept?.titles ?? {}).map(([id, name]) => [id, { ...name, source: 'cache' as const }])), unsaved: new Set(), pendingClose: null });
    get().flush();
  },
  change: (change, focusedTab) => {
    const before = get();
    const changed = change(before.layout);
    if (changed === before.layout) return;
    const layout = normalize(changed);
    rememberBars(before.userId, layout);
    set({ layout, ...focusAfter(layout, focusedTab === undefined ? before : { ...before, focusedTab }), ...retainedMetadata(layout, before) });
    keep();
  },
  choose: (name) => {
    const before = get();
    const arrangement = arrangementOf(before.screen, name).name;
    const layout = reframe(before, arrangement, false);
    rememberBars(before.userId, layout);
    set({ arrangement, layout, ...focusAfter(layout, before), ...retainedMetadata(layout, before), overlay: null });
    get().flush();
  },
  reset: () => {
    const before = get();
    const arrangement = arrangementOf(before.screen).name;
    const layout = reframe(before, arrangement, true);
    rememberBars(before.userId, layout);
    for (const key of ['mobileView', 'mobileContainer', 'mobileTab']) writeViewChoice(before.userId, 'global', key, '');
    set({ arrangement, layout, ...focusAfter(layout, before), ...retainedMetadata(layout, before), statusItems: defaultStatus(), chipSizes: {}, focusMode: false, dropped: false, overlay: null });
    get().flush();
  },
  toggle: (bar, beside) => {
    if (bar === 'right' && !beside) {
      if (barHasViews(get().layout, 'right')) set({ overlay: get().overlay === 'right' ? null : 'right' });
      return;
    }
    get().change((layout) => toggleBar(layout, bar));
    if (get().overlay === bar) set({ overlay: null });
  },
  revealBar: (bar, beside) => {
    get().toggle(bar, beside);
  },
  setFocusedGroup: (groupId) => {
    const group = groupId === null ? null : findGroup(get().layout, groupId);
    if (group?.group.active !== null && group?.group.active !== undefined) get().focus(group.group.active);
    else if (get().focusedGroup !== groupId) { set({ focusedGroup: groupId, focusedTab: null, ...(group?.area === 'main' ? { focusedMainGroup: groupId, focusedMainTab: null } : {}) }); keep(); }
  },
  focus: (tabId) => {
    const before = get(), place = findTab(before.layout, tabId);
    if (place === null || before.focusedTab === tabId && place.group.active === tabId) return;
    const activated = activate(before.layout, tabId);
    const layout = (place.area === 'left' || place.area === 'right') && before.overlay === place.area && !before.layout[place.area].open
      ? { ...activated, [place.area]: { ...activated[place.area], open: false } } : activated;
    set({ layout, focusedTab: tabId, focusedGroup: place.group.id,
      ...(place.area === 'main' ? { focusedMainTab: tabId, focusedMainGroup: place.group.id } : {}),
      activeContent: isContentTab(place.tab) ? tabId : before.activeContent });
    keep();
  },
  openTab: (view, of, mode = 'normal') => {
    const before = get();
    const found = mode === 'normal' ? findView(before.layout, view, of) : null;
    if (found !== null) { get().focus(found.tab.id); return found.tab.id; }
    const tab = makeTab(view, of);
    const groups = spatialMainGroups(before.layout.main);
    const group = groups.find((candidate) => candidate.id === before.focusedMainGroup) ?? groups[0];
    let layout = addMainTab(before.layout, tab, group?.id ?? null);
    if (mode === 'beside' && group !== undefined) {
      layout = move(layout, tab.id, { to: 'split', group: group.id, side: 'right' });
      // Inserting the new view is not a selection change in the group it leaves behind.
      layout = mapGroups(layout, (held) => held.id === group.id ? { ...held, active: group.active } : held);
    }
    get().change(() => layout);
    get().focus(tab.id);
    return tab.id;
  },
  close: (scope) => {
    const state = get(), ids = tabsForClose(state.layout, scope).map((tab) => tab.id);
    if (ids.length === 0) return;
    if (ids.some((id) => state.unsaved.has(id))) { set({ pendingClose: ids }); return; }
    state.finishClose(ids);
  },
  finishClose: (ids) => {
    const before = get();
    const closed: ClosedTab[] = ids.flatMap((id) => {
      const place = findTab(before.layout, id);
      if (place === null) return [];
      const title = before.named.get(id)?.title;
      return [{ tab: place.tab, group: place.group.id, index: place.index, ...(title === undefined ? {} : { title }) }];
    });
    const closedContent = new Set(closed.filter((entry) => isContentTab(entry.tab)).map((entry) => entry.tab.id));
    const layout = normalize(mapGroups(closeTabs(before.layout, new Set(ids)), (group, area) => area === 'left' || area === 'right'
      ? { ...group, tabs: group.tabs.filter((tab) => !closedContent.has(tab.id)) } : group));
    set({ layout, ...focusAfter(layout, before), ...retainedMetadata(layout, before),
      closedTabs: [...before.closedTabs, ...closed], pendingClose: null });
    get().flush();
  },
  cancelClose: () => set({ pendingClose: null }),
  reopen: () => {
    const before = get(), closed = before.closedTabs.at(-1);
    if (closed === undefined) return;
    let layout = addMainTab(before.layout, closed.tab, closed.group);
    if (findGroup(layout, closed.group) !== null) layout = move(layout, closed.tab.id, { to: 'group', group: closed.group, index: closed.index });
    const named = new Map(before.named);
    if (closed.title !== undefined) named.set(closed.tab.id, { title: closed.title });
    set({ layout, named, closedTabs: before.closedTabs.slice(0, -1) });
    get().focus(closed.tab.id);
    get().flush();
  },
  setFocusMode: (focusMode) => { set({ focusMode, overlay: null }); get().flush(); },
  setOverlay: (overlay) => set({ overlay }),
  acknowledgeDropped: () => set({ dropped: false }),
  setUnsaved: (tabId, unsaved) => {
    const held = get().unsaved;
    if (held.has(tabId) === unsaved) return;
    const next = new Set(held);
    if (unsaved) next.add(tabId); else next.delete(tabId);
    set({ unsaved: next });
  },
  setNamed: (tabId, name) => {
    const held = get().named.get(tabId);
    if (name === null ? held === undefined : held?.title === name.title && held.icon === name.icon && held.path === name.path && held.source === name.source && JSON.stringify(held.thread) === JSON.stringify(name.thread)) return;
    const next = new Map(get().named);
    if (name === null) next.delete(tabId); else next.set(tabId, name);
    set({ named: next });
  },
  flush: () => {
    clearTimeout(saveTimer);
    const state = get();
    if (state.userId !== null && state.layout !== EMPTY) saveWorkspace(state.userId, saved(state));
  },
  leave: () => {
    get().flush();
    restorePresentations(undefined);
    set({ userId: null, layout: EMPTY, dropped: false, focusedGroup: null, focusedTab: null, focusedMainGroup: null, focusedMainTab: null, activeContent: null,
      statusItems: defaultStatus(), chipSizes: {}, focusMode: false, closedTabs: [], overlay: null, unsaved: new Set(), named: new Map(), pendingClose: null });
  },
}));

if (typeof window !== 'undefined') window.addEventListener('pagehide', () => useWorkbench.getState().flush());

/**
 * Names the tab of the calling view after what it shows at the moment: the thread in view, the file
 * that is open. Without it a tab bears the name of its view.
 */
export function useTabName(tabId: string, title: string | null | undefined, icon?: LucideIcon, path?: string, pending = false, thread?: ThreadIconFacts): void {
  const setNamed = useWorkbench((state) => state.setNamed);
  const presented = JSON.stringify(thread ?? null);
  useEffect(() => {
    if (pending) return;
    const facts = presented === 'null' ? undefined : JSON.parse(presented) as ThreadIconFacts;
    const glyph = facts === undefined ? icon : THREAD_GLYPHS[facts.display.state];
    setNamed(tabId, title === null || title === undefined || title === '' ? null : { title, source: 'view', ...(facts === undefined ? {} : { thread: facts }), ...(glyph === undefined ? {} : { icon: glyph }), ...(path === undefined ? {} : { path }) });
  }, [setNamed, tabId, title, icon, path, pending, presented]);
}

/** The frame owns this warning so an unsaved file stays protected behind another tab. */
export function useUnsavedGuard(): void {
  const dirty = useWorkbench((state) => state.unsaved.size > 0);
  useEffect(() => {
    if (!dirty) return;
    const warn = (event: BeforeUnloadEvent) => event.preventDefault();
    window.addEventListener('beforeunload', warn);
    return () => window.removeEventListener('beforeunload', warn);
  }, [dirty]);
}
