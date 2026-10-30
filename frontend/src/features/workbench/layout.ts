/** The layout tree: areas, splits, groups and tabs that each name a view and what it shows; reading it, changing it and writing it.
 *
 * Left and right bars hold containers of sections. The bottom holds a group of tabs; the main area
 * holds groups and splits. Each section or tab names a view and optionally its target. Hidden sections
 * keep their place, expansion and relative size. A version-one side-bar group is read into containers
 * in that same bar, preserving its selected section and every target and kept flag.
 *
 * Everything here is a value: a change takes a layout and returns another, and never touches the
 * one it was given. What is written to the device is this value with the version of its format; a
 * stored layout that this build cannot read is refused whole, and its reader gets the shipped one.
 */

import { placementFor } from '@/features/workbench/placement';
import { migrateMainResult, type WorkspaceSource } from './contentAddress';
import type { ScreenKind } from '@/routes/-access';

/** Version four retains tab pins; older layouts migrate with their existing order and targets. */
export const LAYOUT_VERSION = 4;

export type BarId = 'left' | 'right' | 'bottom';
export type AreaId = BarId | 'main';

export const BARS: readonly BarId[] = ['left', 'right', 'bottom'];
export const AREAS: readonly AreaId[] = ['left', 'main', 'bottom', 'right'];

/** A contextual panel follows content focus, or owns an explicitly named resource. */
export interface InspectorTarget {
  kind: 'thread' | 'document' | 'environment' | 'org' | 'project' | 'topic';
  id: string;
  /** A fixed/follow workspace retains the exact environment and its optional native thread door. */
  workspace?: WorkspaceSource;
}
export type InspectorScope = { mode: 'follow' } | { mode: 'fixed'; target: InspectorTarget; continuationId?: string | null };

export interface Tab {
  /** Its name within the layout. */
  id: string;
  /** The view it shows, by the name the view was registered under. */
  view: string;
  /** What the view shows, where it is a view of one thing among many: a file's path, a thread's id. */
  of?: Record<string, string>;
  scope?: InspectorScope;
  /** Stays when its reader goes to another screen. */
  keep?: boolean;
  /** Stands first in its group and survives bulk close, Reset and layout choices. */
  pinned?: boolean;
}

export interface Group {
  kind: 'group';
  id: string;
  tabs: Tab[];
  /** The tab in front; `null` only while the group is empty. */
  active: string | null;
  /** A requested grid cell remains a drop/open target when its last tab closes. */
  retainEmpty?: boolean;
}

export interface Split {
  kind: 'split';
  id: string;
  /** `row`: side by side. `column`: one above another. */
  direction: 'row' | 'column';
  children: MainNode[];
  /** The share of each child, in percent; they add up to a hundred. */
  sizes: number[];
}

export type MainNode = Group | Split;

export interface Bar {
  open: boolean;
  /** Its width, or for the bottom its height, in pixels. */
  size: number;
  group: Group;
}

export type SideBarId = 'left' | 'right';

/** A view in a side bar. Hiding keeps its position, expansion and share for the next showing. */
export interface Section {
  tab: Tab;
  open: boolean;
  hidden: boolean;
  /** Relative share of the room left after the section headers. */
  size: number;
}

export interface ViewContainer {
  id: string;
  /** The registered name, independent of this layout's id. */
  name: string;
  sections: Section[];
  /** The section last revealed, including the selected tab of an older layout. */
  active: string | null;
}

export interface SideBar {
  open: boolean;
  size: number;
  containers: ViewContainer[];
  active: string | null;
}

export interface Layout {
  version: number;
  left: SideBar;
  right: SideBar;
  bottom: Bar;
  main: MainNode;
  /** The group of the main area that was given the whole of it; `null` when every group has its share. */
  maximized: string | null;
}

/** Where a tab is put. */
export type Target =
  /** Into a group, at a place among its tabs (at the end when no place is given). */
  | { to: 'group'; group: string; index?: number }
  /** A section in a named container of either side bar. */
  | { to: 'container'; bar: SideBarId; container: string; index?: number }
  /** Into a new group beside a group of the main area. */
  | { to: 'split'; group: string; side: Side }
  /** Into an area: a bar's group, or the first group of the main area. */
  | { to: 'area'; area: AreaId };

export type Side = 'left' | 'right' | 'up' | 'down';

// ---------------------------------------------------------------- making

let counter = 0;

/** A name that no other part of a layout made on this page has. */
export function newId(prefix: string): string {
  counter += 1;
  return `${prefix}${Date.now().toString(36)}${counter.toString(36)}`;
}

export function makeTab(view: string, of?: Record<string, string>): Tab {
  return of === undefined ? { id: newId('t'), view } : { id: newId('t'), view, of };
}

export function makeGroup(tabs: Tab[] = [], active?: string): Group {
  return { kind: 'group', id: newId('g'), tabs, active: active ?? tabs[0]?.id ?? null };
}

export function makeContainer(name: string, tabs: Tab[] = [], active?: string): ViewContainer {
  return { id: newId('c'), name, active: active ?? tabs[0]?.id ?? null,
    sections: tabs.map((tab, index) => ({ tab, open: active === undefined ? index === 0 : tab.id === active, hidden: false, size: 1 })) };
}

/** A side bar's sections grouped by their registered starting place, retaining the given order. */
export function containersFrom(tabs: Tab[], active?: string, screen?: ScreenKind, home: (view: string) => string = (view) => placementFor(view, screen).container): ViewContainer[] {
  const result: ViewContainer[] = [];
  for (const tab of tabs) {
    const name = home(tab.view);
    let container = result.find((candidate) => candidate.name === name);
    if (container === undefined) { container = makeContainer(name); result.push(container); }
    container.sections.push({ tab, open: tab.id === active, hidden: false, size: 1 });
    if (container.active === null || tab.id === active) container.active = tab.id;
  }
  for (const container of result) {
    if (!container.sections.some((section) => section.open) && container.sections[0] !== undefined) container.sections[0].open = true;
  }
  return result;
}

export function containerGroup(container: ViewContainer): Group {
  return { kind: 'group', id: container.id, tabs: container.sections.map((section) => section.tab), active: container.active };
}

export function barHasViews(layout: Layout, bar: BarId): boolean {
  return bar === 'bottom' ? layout.bottom.group.tabs.length > 0 : layout[bar].containers.some((container) => container.sections.length > 0);
}

export function activeContainer(bar: SideBar): ViewContainer | null {
  return bar.containers.find((container) => container.name === bar.active) ?? bar.containers[0] ?? null;
}

// ---------------------------------------------------------------- reading

export interface Place {
  area: AreaId;
  group: Group;
  index: number;
  tab: Tab;
  container?: ViewContainer;
  section?: Section;
}

/** Every group of the main area, in split-tree order. */
export function mainGroups(node: MainNode): Group[] {
  return node.kind === 'group' ? [node] : node.children.flatMap(mainGroups);
}

/** Top-to-bottom, then left-to-right, including uneven and nested manual splits. */
export function spatialMainGroups(node: MainNode): Group[] {
  const positioned: { group: Group; left: number; top: number }[] = [];
  const visit = (part: MainNode, left: number, top: number, width: number, height: number) => {
    if (part.kind === 'group') { positioned.push({ group: part, left, top }); return; }
    const sizes = shares(part.sizes, part.children.length);
    let offset = 0;
    part.children.forEach((child, index) => {
      const fraction = (sizes[index] ?? 0) / 100;
      if (part.direction === 'row') visit(child, left + width * offset, top, width * fraction, height);
      else visit(child, left, top + height * offset, width, height * fraction);
      offset += fraction;
    });
  };
  visit(node, 0, 0, 1, 1);
  return positioned.sort((one, other) => one.top - other.top || one.left - other.left).map(({ group }) => group);
}

export type GridPreset = '2x4' | '3x3';

/** Reuses the split tree. Tab objects/IDs survive; only their group arrangement changes. */
export function applyGrid(layout: Layout, preset: GridPreset, focusedTab: string | null): Layout {
  const [rows, columns] = preset === '2x4' ? [2, 4] as const : [3, 3] as const;
  const tabs = spatialMainGroups(layout.main).flatMap((group) => group.tabs);
  const cells = Array.from({ length: rows * columns }, (): Group => ({ ...makeGroup(), retainEmpty: true }));
  tabs.forEach((tab, index) => { cells[Math.min(index, cells.length - 1)]?.tabs.push(tab); });
  for (const cell of cells) cell.active = cell.tabs.find((tab) => tab.id === focusedTab)?.id ?? cell.tabs[0]?.id ?? null;
  const main: Split = { kind: 'split', id: newId('s'), direction: 'column', sizes: Array.from({ length: rows }, () => 100 / rows),
    children: Array.from({ length: rows }, (_, row): Split => ({ kind: 'split', id: newId('s'), direction: 'row',
      sizes: Array.from({ length: columns }, () => 100 / columns), children: cells.slice(row * columns, (row + 1) * columns) })) };
  return normalize({ ...layout, main, maximized: null });
}

/** Every group of a layout with the area it is in, in the order of the areas. */
export function allGroups(layout: Layout): { area: AreaId; group: Group }[] {
  return [
    ...layout.left.containers.map((container) => ({ area: 'left' as const, group: containerGroup(container) })),
    ...mainGroups(layout.main).map((group) => ({ area: 'main' as const, group })),
    { area: 'bottom' as const, group: layout.bottom.group },
    ...layout.right.containers.map((container) => ({ area: 'right' as const, group: containerGroup(container) })),
  ];
}

export function allTabs(layout: Layout): Tab[] {
  return allGroups(layout).flatMap(({ group }) => group.tabs);
}

export function findTab(layout: Layout, tabId: string): Place | null {
  for (const { area, group } of allGroups(layout)) {
    const index = group.tabs.findIndex((tab) => tab.id === tabId);
    const tab = group.tabs[index];
    if (tab !== undefined) {
      const container = area === 'left' || area === 'right' ? layout[area].containers.find((candidate) => candidate.id === group.id) : undefined;
      return container === undefined ? { area, group, index, tab } : { area, group, index, tab, container, section: container.sections[index] };
    }
  }
  return null;
}

export function findGroup(layout: Layout, groupId: string): { area: AreaId; group: Group } | null {
  return allGroups(layout).find(({ group }) => group.id === groupId) ?? null;
}

const sameOf = (one: Tab['of'], other: Tab['of']): boolean => {
  const a = Object.entries(one ?? {});
  const b = other ?? {};
  return a.length === Object.keys(b).length && a.every(([key, value]) => b[key] === value);
};

/** The tab that shows `view` (of `of`, where the view is of one thing among many), if the layout has one. */
export function findView(layout: Layout, view: string, of?: Record<string, string>, scope?: InspectorScope): Place | null {
  const tab = allTabs(layout).find((candidate) => candidate.view === view && sameOf(candidate.of, of)
    && (candidate.scope?.mode ?? 'follow') === (scope?.mode ?? 'follow')
    && (scope?.mode !== 'fixed' || candidate.scope?.mode === 'fixed' && candidate.scope.target.kind === scope.target.kind && candidate.scope.target.id === scope.target.id));
  return tab === undefined ? null : findTab(layout, tab.id);
}

// ---------------------------------------------------------------- changing

export function mapGroups(layout: Layout, change: (group: Group, area: AreaId) => Group): Layout {
  const inMain = (node: MainNode): MainNode =>
    node.kind === 'group' ? change(node, 'main') : { ...node, children: node.children.map(inMain) };
  const side = (area: SideBarId): SideBar => ({ ...layout[area], containers: layout[area].containers.map((container) => {
    const group = change(containerGroup(container), area);
    return { ...container, active: group.active, sections: group.tabs.map((tab) => {
      const before = container.sections.find((section) => section.tab.id === tab.id);
      return before === undefined ? { tab, open: true, hidden: false, size: 1 } : { ...before, tab };
    }) };
  }) });
  return {
    ...layout,
    left: side('left'),
    right: side('right'),
    bottom: { ...layout.bottom, group: change(layout.bottom.group, 'bottom') },
    main: inMain(layout.main),
  };
}

/** Retargets an existing view without replacing its tab, section sizes or chosen position. */
export function setTabTarget(layout: Layout, tabId: string, of: Tab['of']): Layout {
  return mapGroups(layout, (group) => ({ ...group, tabs: group.tabs.map((tab) => tab.id === tabId ? { ...tab, of } : tab) }));
}

/** Shares that add up to a hundred, in the proportions given; equal shares where nothing sensible was given. */
function shares(sizes: number[], count: number): number[] {
  const usable = sizes.length === count && sizes.every((size) => Number.isFinite(size) && size > 0);
  const given = usable ? sizes : Array.from({ length: count }, () => 1);
  const total = given.reduce((sum, size) => sum + size, 0);
  return given.map((size) => (size / total) * 100);
}

/**
 * Puts a layout in order: collapse empty manual groups, retain explicit grid cells, no split of one, no split
 * directly inside a split of the same direction, a tab in front in every group that has one, bars
 * that hold nothing closed, and nothing maximized that is not there.
 */
export function normalize(layout: Layout): Layout {
  const tidy = (group: Group): Group => {
    const ordered = [...group.tabs.filter((tab) => tab.pinned === true), ...group.tabs.filter((tab) => tab.pinned !== true)];
    if (ordered.some((tab, index) => tab !== group.tabs[index])) group = { ...group, tabs: ordered };
    const active = group.tabs.some((tab) => tab.id === group.active) ? group.active : (group.tabs[0]?.id ?? null);
    return active === group.active ? group : { ...group, active };
  };

  const prune = (node: MainNode): MainNode | null => {
    if (node.kind === 'group') {
      const tabs = node.tabs.map(migrateMainResult);
      const group = tabs.every((tab, at) => tab === node.tabs[at]) ? node : { ...node, tabs };
      return group.tabs.length === 0 && group.retainEmpty !== true ? null : tidy(group);
    }
    const kept: { child: MainNode; size: number }[] = [];
    const sizes = shares(node.sizes, node.children.length);
    node.children.forEach((child, index) => {
      const pruned = prune(child);
      if (pruned === null) return;
      const size = sizes[index] ?? 0;
      // A split inside a split of the same direction is the same row or column: its children join it.
      if (pruned.kind === 'split' && pruned.direction === node.direction) {
        const inner = shares(pruned.sizes, pruned.children.length);
        pruned.children.forEach((grandchild, at) => kept.push({ child: grandchild, size: (size * (inner[at] ?? 0)) / 100 }));
      } else {
        kept.push({ child: pruned, size });
      }
    });
    if (kept.length === 0) return null;
    if (kept.length === 1) return kept[0]?.child ?? null;
    return {
      ...node,
      children: kept.map((entry) => entry.child),
      sizes: shares(
        kept.map((entry) => entry.size),
        kept.length,
      ),
    };
  };

  // The main area is never without a group: with nothing in it, it holds one empty group to drop into.
  const main = prune(layout.main) ?? (layout.main.kind === 'group' ? tidy(layout.main) : makeGroup());
  const bar = (given: Bar): Bar => {
    const group = tidy(given.group);
    return { ...given, group, open: given.open && group.tabs.length > 0 };
  };
  const maximized =
    layout.maximized !== null && mainGroups(main).length > 1 && mainGroups(main).some((group) => group.id === layout.maximized)
      ? layout.maximized
      : null;
  const side = (given: SideBar): SideBar => {
    const containers = given.containers.filter((container) => container.sections.length > 0).map((container) => ({
      ...container,
      sections: [...container.sections.filter((section) => section.tab.pinned === true), ...container.sections.filter((section) => section.tab.pinned !== true)],
      active: container.sections.some((section) => section.tab.id === container.active) ? container.active : container.sections[0]?.tab.id ?? null,
    }));
    const active = containers.some((container) => container.name === given.active) ? given.active : containers[0]?.name ?? null;
    return { ...given, containers, active, open: given.open && containers.length > 0 };
  };
  return { ...layout, left: side(layout.left), right: side(layout.right), bottom: bar(layout.bottom), main, maximized };
}

/** Brings a tab to the front of its group, and opens the bar it is in. */
export function activate(layout: Layout, tabId: string): Layout {
  const place = findTab(layout, tabId);
  if (place === null) return layout;
  const next = mapGroups(layout, (group) => (group.id === place.group.id ? { ...group, active: tabId } : group));
  if ((place.area === 'left' || place.area === 'right') && place.container !== undefined) {
    return updateContainer(next, place.area, place.container.name, (container) => ({ ...container,
      sections: container.sections.map((section) => section.tab.id === tabId ? { ...section, open: true, hidden: false } : section),
    }), true);
  }
  return place.area === 'main' ? next : setBarOpen(next, place.area, true);
}

/** Closes a tab. The tab beside it comes to the front: the one after it, or the one before at the end. */
export function close(layout: Layout, tabId: string): Layout {
  const place = findTab(layout, tabId);
  if (place === null) return layout;
  if (place.section !== undefined) return showSection(layout, tabId, false);
  return normalize(
    mapGroups(layout, (group) => {
      if (group.id !== place.group.id) return group;
      const tabs = group.tabs.filter((tab) => tab.id !== tabId);
      const neighbour = tabs[Math.min(place.index, tabs.length - 1)]?.id ?? null;
      return { ...group, tabs, active: group.active === tabId ? neighbour : group.active };
    }),
  );
}

export type CloseScope =
  | { kind: 'tab'; tab: string; explicit?: boolean }
  | { kind: 'group'; group: string }
  | { kind: 'group-others'; tab: string }
  | { kind: 'main' }
  | { kind: 'main-others'; tab: string };

/** A reviewable close plan; resolving it never changes presentation or underlying resources. */
export function tabsForClose(layout: Layout, scope: CloseScope): Tab[] {
  if (scope.kind === 'main') return spatialMainGroups(layout.main).flatMap((group) => group.tabs).filter((tab) => tab.pinned !== true);
  if (scope.kind === 'group') return (findGroup(layout, scope.group)?.group.tabs ?? []).filter((tab) => tab.pinned !== true);
  const place = findTab(layout, scope.tab);
  if (place === null) return [];
  if (scope.kind === 'tab') return place.tab.pinned !== true || scope.explicit === true ? [place.tab] : [];
  if (scope.kind === 'group-others') return place.group.tabs.filter((tab) => tab.id !== scope.tab && tab.pinned !== true);
  return place.area === 'main' ? spatialMainGroups(layout.main).flatMap((group) => group.tabs).filter((tab) => tab.id !== scope.tab && tab.pinned !== true) : [];
}

/** Apply an approved close plan in one immutable update, with next-then-previous neighbor selection. */
export function closeTabs(layout: Layout, ids: ReadonlySet<string>): Layout {
  if (ids.size === 0) return layout;
  const next = mapGroups(layout, (group, area) => {
    if (area === 'left' || area === 'right' || !group.tabs.some((tab) => ids.has(tab.id))) return group;
    const tabs = group.tabs.filter((tab) => !ids.has(tab.id));
    if (group.active === null || !ids.has(group.active)) return { ...group, tabs };
    const activeIndex = group.tabs.findIndex((tab) => tab.id === group.active);
    const neighbor = group.tabs.slice(activeIndex + 1).find((tab) => !ids.has(tab.id))
      ?? group.tabs.slice(0, activeIndex).reverse().find((tab) => !ids.has(tab.id));
    return { ...group, tabs, active: neighbor?.id ?? null };
  });
  // Existing side sections close by hiding, retaining their target and chosen size.
  const side = (bar: SideBar): SideBar => ({ ...bar, containers: bar.containers.map((container) => ({ ...container,
    sections: container.sections.map((section) => ids.has(section.tab.id) ? { ...section, hidden: true } : section),
  })) });
  return normalize({ ...next, left: side(next.left), right: side(next.right) });
}

/** Removing a placeholder is explicit; a nonempty group can never lose its tabs here. */
export function removeEmptyMainGroup(layout: Layout, groupId: string): Layout {
  const group = mainGroups(layout.main).find((candidate) => candidate.id === groupId);
  if (group === undefined || group.tabs.length > 0) return layout;
  const remove = (node: MainNode): MainNode | null => {
    if (node.kind === 'group') return node.id === groupId ? null : node;
    const children: MainNode[] = [], sizes: number[] = [];
    node.children.forEach((child, index) => {
      const kept = remove(child);
      if (kept !== null) { children.push(kept); sizes.push(node.sizes[index] ?? 0); }
    });
    return children.length === 0 ? null : { ...node, children, sizes: shares(sizes, children.length) };
  };
  return normalize({ ...layout, main: remove(layout.main) ?? makeGroup() });
}

/** Add a new content instance to the focused main group. Matching-resource reuse is the opener's decision. */
export function addMainTab(layout: Layout, tab: Tab, focusedGroup: string | null): Layout {
  if (findTab(layout, tab.id) !== null) return activate(layout, tab.id);
  const groups = spatialMainGroups(layout.main);
  const destination = groups.find((group) => group.id === focusedGroup) ?? groups[0];
  if (destination === undefined) return { ...layout, main: makeGroup([tab]) };
  return mapGroups(layout, (group, area) => area === 'main' && group.id === destination.id ? insertInto(group, tab) : group);
}

function insertInto(group: Group, tab: Tab, index?: number, crossing = false): Group {
  const pins = group.tabs.filter((candidate) => candidate.pinned === true).length;
  const at = index === undefined ? (tab.pinned === true ? pins : group.tabs.length) : Math.max(0, Math.min(index, group.tabs.length));
  if (crossing && index !== undefined) {
    const pinned = at < pins || tab.pinned === true && at === pins;
    const { pinned: _before, ...rest } = tab;
    tab = pinned ? { ...rest, pinned: true } : rest;
  }
  const tabs = [...group.tabs.slice(0, at), tab, ...group.tabs.slice(at)];
  return { ...group, tabs, active: tab.id };
}

function splitBeside(node: MainNode, groupId: string, side: Side, added: Group): MainNode {
  const direction = side === 'left' || side === 'right' ? 'row' : 'column';
  const before = side === 'left' || side === 'up';
  if (node.kind === 'group') {
    if (node.id !== groupId) return node;
    return {
      kind: 'split',
      id: newId('s'),
      direction,
      children: before ? [added, node] : [node, added],
      sizes: [50, 50],
    };
  }
  const index = node.children.findIndex((child) => child.kind === 'group' && child.id === groupId);
  if (index === -1 || node.direction !== direction) {
    return { ...node, children: node.children.map((child) => splitBeside(child, groupId, side, added)) };
  }
  // In a split of the same direction the new group takes half of the share of the one it stands beside.
  const sizes = shares(node.sizes, node.children.length);
  const half = (sizes[index] ?? 0) / 2;
  const at = before ? index : index + 1;
  return {
    ...node,
    children: [...node.children.slice(0, at), added, ...node.children.slice(at)],
    sizes: [...sizes.slice(0, index), half, half, ...sizes.slice(index + 1)],
  };
}

/** Whether a tab can be put there: a target that would change nothing, or that does not exist, is refused. */
export function canMove(layout: Layout, tabId: string, target: Target): boolean {
  const place = findTab(layout, tabId);
  if (place === null) return false;
  switch (target.to) {
    case 'container':
      return target.container.length > 0;
    case 'area':
      return true;
    case 'group':
      return findGroup(layout, target.group) !== null;
    case 'split': {
      const beside = findGroup(layout, target.group);
      if (beside?.area !== 'main') return false;
      // The only tab of a group, split off beside its own group, would be where it was.
      return !(place.group.id === target.group && place.group.tabs.length === 1);
    }
  }
}

/**
 * Puts a tab somewhere else: into another group, beside a group of the main area as a group of its
 * own, or into an area. It comes to the front there, the bar it arrives in opens, and what it left
 * empty is tidied away.
 */
export function move(layout: Layout, tabId: string, target: Target): Layout {
  const place = findTab(layout, tabId);
  if (place === null || !canMove(layout, tabId, target)) return layout;
  const { tab } = place;

  // A place among the tabs of its own group counts the tab itself, which is about to leave.
  if (target.to === 'group' && target.group === place.group.id) {
    const at = target.index ?? place.group.tabs.length;
    const index = at > place.index ? at - 1 : at;
    const without = place.group.tabs.filter((candidate) => candidate.id !== tabId);
    const reordered = insertInto({ ...place.group, tabs: without }, tab, index, true);
    return mapGroups(layout, (group) => (group.id === place.group.id ? reordered : group));
  }

  const taken = mapGroups(layout, (group) => {
    if (group.id !== place.group.id) return group;
    const tabs = group.tabs.filter((candidate) => candidate.id !== tabId);
    const neighbour = tabs[Math.min(place.index, tabs.length - 1)]?.id ?? null;
    return { ...group, tabs, active: group.active === tabId ? neighbour : group.active };
  });

  let placed: Layout;
  let arrival: AreaId;
  switch (target.to) {
    case 'container': {
      arrival = target.bar;
      const same = place.area === target.bar && place.container?.name === target.container;
      const index = same && target.index !== undefined && target.index > place.index ? target.index - 1 : target.index;
      placed = insertSection(taken, target.bar, target.container, tab, index, place.section);
      break;
    }
    case 'group': {
      arrival = findGroup(taken, target.group)?.area ?? 'main';
      placed = mapGroups(taken, (group) => (group.id === target.group ? insertInto(group, tab, target.index, true) : group));
      break;
    }
    case 'split': {
      arrival = 'main';
      placed = { ...taken, main: splitBeside(taken.main, target.group, target.side, makeGroup([tab])) };
      break;
    }
    case 'area': {
      arrival = target.area;
      if (target.area === 'main') {
        const first = mainGroups(taken.main)[0];
        placed = first === undefined ? taken : mapGroups(taken, (group) => (group.id === first.id ? insertInto(group, tab) : group));
      } else if (target.area === 'bottom') {
        const bar = taken[target.area];
        placed = { ...taken, [target.area]: { ...bar, group: insertInto(bar.group, tab) } };
      } else {
        const name = taken[target.area].active ?? placementFor(tab.view).container;
        placed = insertSection(taken, target.area, name, tab, undefined, place.section);
      }
      break;
    }
  }

  const destination = findTab(placed, tabId);
  if (place.section !== undefined && destination?.container !== undefined && (destination.area === 'left' || destination.area === 'right')) {
    placed = resizeSections(placed, destination.area, destination.container.name, { [tabId]: place.section.size });
  }
  const opened = arrival === 'main' ? placed : { ...placed, [arrival]: { ...placed[arrival], open: true } };
  return normalize(activate(opened, tabId));
}

/**
 * Opens a view: brings its tab to the front when the layout has one, and otherwise adds one to the
 * area the view starts in.
 */
export function open(layout: Layout, view: string, area: AreaId, of?: Record<string, string>, container = placementFor(view).container): Layout {
  const there = findView(layout, view, of);
  if (there !== null) return activate(layout, there.tab.id);
  const tab = makeTab(view, of);
  if (area === 'main') {
    const first = mainGroups(layout.main)[0];
    if (first === undefined) return layout;
    return mapGroups(layout, (group) => (group.id === first.id ? insertInto(group, tab) : group));
  }
  if (area === 'left' || area === 'right') return insertSection(layout, area, container, tab);
  const bar = layout.bottom;
  return { ...layout, [area]: { ...bar, open: true, group: insertInto(bar.group, tab) } };
}

/** Opens or closes a bar. A bar that holds nothing stays closed: there would be nothing to show. */
export function setBarOpen(layout: Layout, bar: BarId, open: boolean): Layout {
  const current = layout[bar];
  const next = open && barHasViews(layout, bar);
  return current.open === next ? layout : { ...layout, [bar]: { ...current, open: next } };
}

export function toggleBar(layout: Layout, bar: BarId): Layout {
  return setBarOpen(layout, bar, !layout[bar].open);
}

function updateContainer(layout: Layout, bar: SideBarId, name: string, update: (container: ViewContainer) => ViewContainer, reveal = false): Layout {
  const current = layout[bar];
  return { ...layout, [bar]: { ...current, open: reveal || current.open, active: reveal ? name : current.active,
    containers: current.containers.map((container) => container.name === name ? update(container) : container),
  } };
}

function insertSection(layout: Layout, bar: SideBarId, name: string, tab: Tab, index?: number, previous?: Section): Layout {
  const current = layout[bar];
  const existing = current.containers.find((container) => container.name === name);
  const container = existing ?? makeContainer(name);
  const at = Math.max(0, Math.min(index ?? container.sections.length, container.sections.length));
  const section: Section = previous === undefined ? { tab, open: true, hidden: false, size: 1 } : { ...previous, tab, hidden: false, open: true };
  const updated = { ...container, active: tab.id, sections: [...container.sections.slice(0, at), section, ...container.sections.slice(at)] };
  return { ...layout, [bar]: { ...current, open: true, active: name, containers: existing === undefined
    ? [...current.containers, updated] : current.containers.map((candidate) => candidate.id === container.id ? updated : candidate) } };
}

/** Switches a bar to a container; only an activity-bar action toggles the already selected one. */
export function selectContainer(layout: Layout, bar: SideBarId, name: string, toggle = false): Layout {
  const current = layout[bar];
  if (!current.containers.some((container) => container.name === name)) return layout;
  return { ...layout, [bar]: { ...current, active: name, open: toggle && current.active === name ? !current.open : true } };
}

/** Shows or hides a section without losing its place, size or expansion. */
export function showSection(layout: Layout, tabId: string, shown: boolean): Layout {
  const place = findTab(layout, tabId);
  if (place?.container === undefined || (place.area !== 'left' && place.area !== 'right')) return layout;
  return updateContainer(layout, place.area, place.container.name, (container) => ({ ...container,
    sections: container.sections.map((section) => section.tab.id === tabId ? { ...section, hidden: !shown } : section),
  }));
}

export function expandSection(layout: Layout, tabId: string, open: boolean): Layout {
  const place = findTab(layout, tabId);
  if (place?.container === undefined || (place.area !== 'left' && place.area !== 'right')) return layout;
  return updateContainer(layout, place.area, place.container.name, (container) => ({ ...container,
    sections: container.sections.map((section) => section.tab.id === tabId ? { ...section, open } : section),
  }));
}

/** Resizes only the named sections; collapsed and hidden sections keep their previous shares. */
export function resizeSections(layout: Layout, bar: SideBarId, name: string, sizes: Readonly<Record<string, number>>): Layout {
  return updateContainer(layout, bar, name, (container) => ({ ...container, sections: container.sections.map((section) => {
    const size = sizes[section.tab.id];
    return size !== undefined && Number.isFinite(size) && size > 0 ? { ...section, size } : section;
  }) }));
}

/** Sets the width of a bar, or the height of the bottom, in pixels. */
export function resizeBar(layout: Layout, bar: BarId, size: number): Layout {
  if (!Number.isFinite(size) || size <= 0 || layout[bar].size === size) return layout;
  return { ...layout, [bar]: { ...layout[bar], size: Math.round(size) } };
}

/** Sets the shares of a split's children. */
export function resizeSplit(layout: Layout, splitId: string, sizes: number[]): Layout {
  const inMain = (node: MainNode): MainNode => {
    if (node.kind === 'group') return node;
    if (node.id === splitId) {
      return sizes.length === node.children.length ? { ...node, sizes: shares(sizes, sizes.length) } : node;
    }
    return { ...node, children: node.children.map(inMain) };
  };
  return { ...layout, main: inMain(layout.main) };
}

/** Gives one group the whole main area, or gives it back when that group has it already. */
export function toggleMaximized(layout: Layout, groupId: string): Layout {
  if (layout.maximized === groupId) return { ...layout, maximized: null };
  const groups = mainGroups(layout.main);
  if (groups.length < 2 || !groups.some((group) => group.id === groupId)) return layout;
  return { ...layout, maximized: groupId };
}

/** Marks a tab as kept through navigation, or no longer. */
export function setKept(layout: Layout, tabId: string, keep: boolean): Layout {
  return mapGroups(layout, (group) =>
    group.tabs.some((tab) => tab.id === tabId)
      ? {
          ...group,
          tabs: group.tabs.map((tab) => {
            if (tab.id !== tabId) return tab;
            const { keep: _was, ...rest } = tab;
            return keep ? { ...rest, keep: true } : rest;
          }),
        }
      : group,
  );
}

/** Pinning joins the end of the group's pins; unpinning joins the start of its ordinary tabs. */
export function setPinned(layout: Layout, tabId: string, pinned: boolean): Layout {
  return normalize(mapGroups(layout, (group) => {
    const before = group.tabs.find((tab) => tab.id === tabId);
    if (before === undefined || (before.pinned === true) === pinned) return group;
    const { pinned: _previous, ...rest } = before;
    const tab: Tab = pinned ? { ...rest, pinned: true } : rest;
    const others = group.tabs.filter((candidate) => candidate.id !== tabId);
    const at = others.filter((candidate) => candidate.pinned === true).length;
    return { ...group, tabs: [...others.slice(0, at), tab, ...others.slice(at)] };
  }));
}

/**
 * Carries the tabs marked as kept from the layout a reader leaves into the one they arrive in. A
 * kept tab whose view the new layout shows already is not doubled; the others join the first group
 * of the main area, behind what is in front there.
 */
export function carryKept(from: Layout, into: Layout, alsoKeep: ReadonlySet<string> = new Set()): Layout {
  for (const tab of allTabs(from).filter((tab) => tab.pinned === true)) {
    const match = findView(into, tab.view, tab.of);
    if (match !== null) into = setPinned(into, match.tab.id, true);
  }
  const kept = allTabs(from).filter((tab) => (tab.keep === true || tab.pinned === true || alsoKeep.has(tab.id)) && findView(into, tab.view, tab.of) === null);
  if (kept.length === 0) return into;
  const first = mainGroups(into.main)[0];
  if (first === undefined) return into;
  return normalize(mapGroups(into, (group) => (group.id === first.id ? { ...group, tabs: [...group.tabs, ...kept] } : group)));
}

/** Drops the tabs whose view `offered` does not name: a view that this screen, or this build, does not have. */
export function keepOffered(layout: Layout, offered: (view: string) => boolean): Layout {
  return normalize(mapGroups(layout, (group) => ({ ...group, tabs: group.tabs.filter((tab) => offered(tab.view)) })));
}

// ---------------------------------------------------------------- writing and reading back

export function serialize(layout: Layout): string {
  return JSON.stringify(layout);
}

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === 'object' && value !== null && !Array.isArray(value);

export function readTab(value: unknown): Tab | null {
  if (!isRecord(value) || typeof value.id !== 'string' || typeof value.view !== 'string') return null;
  const tab: Tab = { id: value.id, view: value.view };
  if (isRecord(value.of)) {
    const entries = Object.entries(value.of).filter((entry): entry is [string, string] => typeof entry[1] === 'string');
    if (entries.length > 0) tab.of = Object.fromEntries(entries);
  }
  if (value.keep === true) tab.keep = true;
  if (value.pinned === true) tab.pinned = true;
  if (isRecord(value.scope)) {
    if (value.scope.mode === 'follow') tab.scope = { mode: 'follow' };
    else if (value.scope.mode === 'fixed' && isRecord(value.scope.target)) {
      const { kind, id } = value.scope.target;
      if (typeof kind !== 'string' || !['thread', 'document', 'environment', 'org', 'project', 'topic'].includes(kind)
        || typeof id !== 'string' || id === '') return null;
      let workspace: WorkspaceSource | undefined;
      if (value.scope.target.workspace !== undefined) {
        const source = value.scope.target.workspace;
        if (kind !== 'environment' || !isRecord(source) || source.environmentId !== id
          || source.threadId !== null && (typeof source.threadId !== 'string' || source.threadId === '')) return null;
        workspace = { environmentId: id, threadId: source.threadId };
      }
      const continuationId = value.scope.continuationId;
      if (continuationId !== undefined && (kind !== 'thread' || continuationId !== null && (typeof continuationId !== 'string' || continuationId === ''))) return null;
      tab.scope = { mode: 'fixed', target: { kind: kind as InspectorTarget['kind'], id, ...(workspace === undefined ? {} : { workspace }) },
        ...(continuationId === undefined ? {} : { continuationId }) };
    } else return null;
  }
  return tab;
}

function readGroup(value: unknown): Group | null {
  if (!isRecord(value) || value.kind !== 'group' || typeof value.id !== 'string' || !Array.isArray(value.tabs)) return null;
  const tabs: Tab[] = [];
  for (const entry of value.tabs) {
    const tab = readTab(entry);
    if (tab === null) return null;
    tabs.push(tab);
  }
  return { kind: 'group', id: value.id, tabs, active: typeof value.active === 'string' ? value.active : null,
    ...(value.retainEmpty === true ? { retainEmpty: true } : {}) };
}

function readNode(value: unknown, depth: number): MainNode | null {
  // Deeper than anybody splits by hand: a tree that deep was not written by this application.
  if (depth > MAX_DEPTH || !isRecord(value)) return null;
  if (value.kind === 'group') return readGroup(value);
  if (value.kind !== 'split' || typeof value.id !== 'string' || !Array.isArray(value.children)) return null;
  if (value.direction !== 'row' && value.direction !== 'column') return null;
  const children: MainNode[] = [];
  for (const entry of value.children) {
    const child = readNode(entry, depth + 1);
    if (child === null) return null;
    children.push(child);
  }
  const sizes = Array.isArray(value.sizes) ? value.sizes.filter((size): size is number => typeof size === 'number') : [];
  return { kind: 'split', id: value.id, direction: value.direction, children, sizes: shares(sizes, children.length) };
}

function readBar(value: unknown): Bar | null {
  if (!isRecord(value) || typeof value.open !== 'boolean' || typeof value.size !== 'number') return null;
  if (!Number.isFinite(value.size) || value.size <= 0) return null;
  const group = readGroup(value.group);
  return group === null ? null : { open: value.open, size: value.size, group };
}

function readSideBar(value: unknown): SideBar | null {
  if (!isRecord(value) || typeof value.open !== 'boolean' || typeof value.size !== 'number' || !Number.isFinite(value.size) || value.size <= 0 || !Array.isArray(value.containers)) return null;
  const containers: ViewContainer[] = [];
  for (const entry of value.containers) {
    if (!isRecord(entry) || typeof entry.id !== 'string' || typeof entry.name !== 'string' || !Array.isArray(entry.sections)) return null;
    const sections: Section[] = [];
    for (const item of entry.sections) {
      if (!isRecord(item) || typeof item.open !== 'boolean' || typeof item.hidden !== 'boolean' || typeof item.size !== 'number' || !Number.isFinite(item.size) || item.size <= 0) return null;
      const tab = readTab(item.tab);
      if (tab === null) return null;
      sections.push({ tab, open: item.open, hidden: item.hidden, size: item.size });
    }
    containers.push({ id: entry.id, name: entry.name, sections, active: typeof entry.active === 'string' ? entry.active : null });
  }
  if (new Set(containers.map((container) => container.name)).size !== containers.length) return null;
  return { open: value.open, size: value.size, containers, active: typeof value.active === 'string' ? value.active : null };
}

/** An old side bar keeps every tab in its chosen bar, with the chosen tab's container in front. */
function migrateSideBar(value: unknown, screen?: ScreenKind): SideBar | null {
  const old = readBar(value);
  if (old === null) return null;
  const containers = containersFrom(old.group.tabs, old.group.active ?? undefined, screen);
  // The original group identity is retained for existing references to the selected group.
  const front = containers.find((container) => container.sections.some((section) => section.tab.id === old.group.active)) ?? containers[0];
  if (front !== undefined) front.id = old.group.id;
  return { open: old.open, size: old.size, containers, active: front?.name ?? null };
}

/** How deep the main area's tree may be read. */
const MAX_DEPTH = 8;

/**
 * Reads a layout that was written before. Answers `null` for anything that is not a layout of this
 * format, whole and sound: another version, a part missing, a name used twice. A layout that reads
 * is put in order before it is returned.
 */
export function parse(text: string, screen?: ScreenKind): Layout | null {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return null;
  }
  if (!isRecord(value) || ![LAYOUT_VERSION, 3, 2, 1].includes(value.version as number)) return null;
  const left = value.version === 1 ? migrateSideBar(value.left, screen) : readSideBar(value.left);
  const right = value.version === 1 ? migrateSideBar(value.right, screen) : readSideBar(value.right);
  const bottom = readBar(value.bottom);
  const main = readNode(value.main, 0);
  if (left === null || right === null || bottom === null || main === null) return null;
  const layout: Layout = {
    version: LAYOUT_VERSION,
    left,
    right,
    bottom,
    main,
    maximized: typeof value.maximized === 'string' ? value.maximized : null,
  };
  const ids = [...allGroups(layout).map(({ group }) => group.id), ...allTabs(layout).map((tab) => tab.id)];
  if (new Set(ids).size !== ids.length) return null;
  return normalize(layout);
}
