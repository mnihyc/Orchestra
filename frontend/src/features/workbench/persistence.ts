/** What the workbench keeps on a device, each under the person signed in: the arrangement of each kind of screen, each thing's open tabs, and text not yet saved, which is removed at sign-out and after a week; and the named arrangements that follow the person.
 *
 * So far: the arrangement of each kind of screen. It is the reader's own and suits the screen it was
 * made on, so it stays in this browser's storage and is never sent anywhere; it is kept under the
 * id of the person signed in, so that two people at one browser never meet each other's. An
 * arrangement that this build cannot read is dropped, and its reader is told once.
 *
 * Not yet: the tabs that belong to one thing (a thread's open files), text typed into a file and
 * not saved, and arrangements saved by name, which follow the person from device to device.
 */
import { allTabs, addMainTab, type Layout, parse, readTab, serialize, type Tab } from '@/features/workbench/layout';
import type { ScreenKind } from '@/routes/-access';
import type { Presentations } from './presentation';
import { readStatus, type StatusArrangement } from '@/features/status-items/arrangement';
import { readChipSizes, type ChipSizes } from '@/features/people/chipSizes';

/** What is kept for one kind of screen: which shipped arrangement it began as, and how it stands now. */
export interface Kept {
  arrangement: string;
  layout: Layout;
}

const key = (userId: string, screen: ScreenKind) => `orch.${userId}.workbench.${screen}`;

/**
 * Reads what was kept for a kind of screen. `dropped` says that something was there and could not
 * be read, so that the reader can be told why their arrangement is the shipped one again.
 */
export function loadKept(userId: string, screen: ScreenKind): { kept: Kept | null; dropped: boolean } {
  let text: string | null;
  try {
    text = localStorage.getItem(key(userId, screen));
  } catch {
    return { kept: null, dropped: false };
  }
  if (text === null) return { kept: null, dropped: false };
  try {
    const value: unknown = JSON.parse(text);
    if (typeof value === 'object' && value !== null) {
      const { arrangement, layout } = value as Record<string, unknown>;
      const read = typeof layout === 'string' ? parse(layout, screen) : null;
      if (typeof arrangement === 'string' && read !== null) return { kept: { arrangement, layout: read }, dropped: false };
    }
  } catch {
    // Not JSON at all: dropped like anything else that cannot be read.
  }
  forgetKept(userId, screen);
  return { kept: null, dropped: true };
}

export function saveKept(userId: string, screen: ScreenKind, kept: Kept): void {
  try {
    localStorage.setItem(key(userId, screen), JSON.stringify({ arrangement: kept.arrangement, layout: serialize(kept.layout) }));
  } catch {
    // A device that keeps nothing still shows the workbench; it only forgets it.
  }
}

export function forgetKept(userId: string, screen: ScreenKind): void {
  try {
    localStorage.removeItem(key(userId, screen));
  } catch {
    // Nothing was kept, then.
  }
}

/** Only presentation is stored here. Resource data, permissions and drafts keep their existing owners. */
export interface ClosedTab {
  tab: Tab;
  group: string;
  index: number;
  title?: string;
}

export interface Workspace extends Kept {
  chipSizes?: ChipSizes;
  statusItems?: StatusArrangement;
  focusedGroup: string | null;
  focusedTab: string | null;
  focusedMainGroup: string | null;
  focusedMainTab: string | null;
  activeContent: string | null;
  focusMode: boolean;
  closedTabs: ClosedTab[];
  presentations?: Presentations;
  titles?: Record<string, { title: string; path?: string }>;
}

const workspaceKey = (userId: string) => `orch.${userId}.workbench.workspace`;
const kinds: readonly ScreenKind[] = ['page', 'org', 'project', 'topic', 'thread', 'document'];
const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

/** Read one workspace. Legacy page-kind layouts contribute their explicitly targeted views once. */
export function loadWorkspace(userId: string, initial: ScreenKind): { kept: Workspace | null; dropped: boolean } {
  let raw: string | null;
  try { raw = localStorage.getItem(workspaceKey(userId)); } catch { return { kept: null, dropped: false }; }
  if (raw !== null) {
    try {
      const value: unknown = JSON.parse(raw);
      if (record(value) && value.version === 1 && typeof value.arrangement === 'string' && typeof value.layout === 'string') {
        const layout = parse(value.layout);
        if (layout !== null) {
          const live = new Set(allTabs(layout).map((tab) => tab.id));
          const member = (id: unknown) => typeof id === 'string' && live.has(id) ? id : null;
          const closedTabs: ClosedTab[] = [];
          if (Array.isArray(value.closedTabs)) for (const entry of value.closedTabs) {
            if (!record(entry)) continue;
            const tab = readTab(entry.tab);
            if (tab === null || typeof entry.group !== 'string' || typeof entry.index !== 'number' || !Number.isInteger(entry.index) || entry.index < 0) continue;
            closedTabs.push({ tab, group: entry.group, index: entry.index, ...(typeof entry.title === 'string' ? { title: entry.title } : {}) });
          }
          const titles: NonNullable<Workspace['titles']> = {};
          if (record(value.titles)) for (const [id, name] of Object.entries(value.titles)) {
            if (live.has(id) && record(name) && typeof name.title === 'string') titles[id] = { title: name.title, ...(typeof name.path === 'string' ? { path: name.path } : {}) };
          }
          return { kept: { arrangement: value.arrangement, layout, focusedGroup: typeof value.focusedGroup === 'string' ? value.focusedGroup : null,
            focusedMainGroup: typeof value.focusedMainGroup === 'string' ? value.focusedMainGroup : null, focusedMainTab: Object.hasOwn(value, 'focusedMainTab') ? member(value.focusedMainTab) : member(value.activeContent),
            focusedTab: member(value.focusedTab), activeContent: member(value.activeContent), focusMode: value.focusMode === true, closedTabs,
            titles, chipSizes: readChipSizes(value.chipSizes), ...(value.statusItems === undefined ? {} : { statusItems: readStatus(value.statusItems) }), ...(record(value.presentations) ? { presentations: Object.fromEntries(Object.entries(value.presentations).filter((entry): entry is [string, Record<string, unknown>] => record(entry[1]))) } : {}) }, dropped: false };
        }
      }
    } catch { /* A malformed workspace does not expose another person's state. */ }
    return { kept: null, dropped: true };
  }
  const first = loadKept(userId, initial);
  let base = first.kept;
  let dropped = first.dropped;
  const legacy = kinds.filter((kind) => kind !== initial).map((kind) => loadKept(userId, kind));
  base ??= legacy.find((entry) => entry.kept !== null)?.kept ?? null;
  if (base === null) return { kept: null, dropped: dropped || legacy.some((entry) => entry.dropped) };
  let layout = base.layout;
  const live = new Set(allTabs(layout).map((tab) => tab.id));
  for (const entry of legacy) {
    dropped ||= entry.dropped;
    if (entry.kept === null) continue;
    for (const tab of allTabs(entry.kept.layout)) {
      if (live.has(tab.id) || (tab.of === undefined && tab.scope?.mode !== 'fixed')) continue;
      layout = addMainTab(layout, tab, null);
      live.add(tab.id);
    }
  }
  return { kept: { arrangement: base.arrangement, layout, focusedGroup: null, focusedTab: null, focusedMainGroup: null, focusedMainTab: null, activeContent: null, focusMode: false, closedTabs: [] }, dropped };
}

export function saveWorkspace(userId: string, workspace: Workspace): void {
  try { localStorage.setItem(workspaceKey(userId), JSON.stringify({ ...workspace, version: 1, layout: serialize(workspace.layout) })); }
  catch { /* Storage refusal affects reload only; the live workspace stays usable. */ }
}
