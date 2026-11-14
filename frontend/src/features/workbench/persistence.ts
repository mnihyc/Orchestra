/** The current per-person workspace: layout, open and recently closed tabs, and recoverable presentation. */
import { allTabs, type Layout, parse, readTab, serialize, type Tab } from '@/features/workbench/layout';
import type { Presentations } from './presentation';
import { readStatus, type StatusArrangement } from '@/features/status-items/arrangement';
import { readChipSizes, type ChipSizes } from '@/features/people/chipSizes';

/** Only presentation is stored here. Resource data, permissions and drafts keep their existing owners. */
export interface ClosedTab {
  tab: Tab;
  group: string;
  index: number;
  title?: string;
}

export interface Workspace {
  arrangement: string;
  layout: Layout;
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
const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

/** Read only this release's workspace. Unknown view registrations retain their saved targets. */
export function loadWorkspace(userId: string): { kept: Workspace | null; dropped: boolean } {
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
  return { kept: null, dropped: false };
}

export function saveWorkspace(userId: string, workspace: Workspace): void {
  try { localStorage.setItem(workspaceKey(userId), JSON.stringify({ ...workspace, version: 1, layout: serialize(workspace.layout) })); }
  catch { /* Storage refusal affects reload only; the live workspace stays usable. */ }
}
