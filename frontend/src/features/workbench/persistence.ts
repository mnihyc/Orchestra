/** The current per-person workspace: layout, open and recently closed tabs, and recoverable presentation. */
import { allTabs, type Layout, parse, readTab, serialize, type Tab } from '@/features/workbench/layout';
import type { Presentations } from './presentation';
import { readStatus, type StatusArrangement } from '@/features/status-items/arrangement';
import { readChipSizes, type ChipSizes } from '@/features/people/chipSizes';
import { DEFAULT_INTERFACE_READING } from '@/lib/interfaceReading';

/** Only presentation is stored here. Resource data, permissions and drafts keep their existing owners. */
export interface ClosedTab {
  tab: Tab;
  group: string;
  index: number;
  title?: string;
}

/** Hidden sections remain open; only distinct genuinely closed views use the extra model slots. */
export function retainedClosedTabs(layout: Layout, closed: readonly ClosedTab[], count: number): ClosedTab[] {
  const open = new Set(allTabs(layout).map((tab) => tab.id));
  const recent = new Map<string, ClosedTab>();
  for (const entry of closed) {
    if (open.has(entry.tab.id)) continue;
    recent.delete(entry.tab.id); recent.set(entry.tab.id, entry);
  }
  return count === 0 ? [] : [...recent.values()].slice(-count);
}

/** Keep one locator for Reopen when bodies are disabled; hidden open sections use no closed slot. */
export function boundedClosedHistory(layout: Layout, closed: readonly ClosedTab[], count: number): ClosedTab[] {
  const unique = new Map<string, ClosedTab>();
  for (const entry of closed) { unique.delete(entry.tab.id); unique.set(entry.tab.id, entry); }
  const entries = [...unique.values()];
  const owners = new Set([...allTabs(layout).map((tab) => tab.id), ...retainedClosedTabs(layout, entries, Math.max(1, count)).map((entry) => entry.tab.id)]);
  return entries.filter((entry) => owners.has(entry.tab.id));
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
  /** Last confirmed native retention count; a delayed shell must not replace it with a guessed default. */
  closedLimit?: number;
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
          const closedLimit = typeof value.closedLimit === 'number' && Number.isSafeInteger(value.closedLimit) && value.closedLimit >= 0
            ? value.closedLimit : DEFAULT_INTERFACE_READING.kept_closed_tabs;
          const readClosed: ClosedTab[] = [];
          if (Array.isArray(value.closedTabs)) for (const entry of value.closedTabs) {
            if (!record(entry)) continue;
            const tab = readTab(entry.tab);
            if (tab === null || typeof entry.group !== 'string' || typeof entry.index !== 'number' || !Number.isInteger(entry.index) || entry.index < 0) continue;
            readClosed.push({ tab, group: entry.group, index: entry.index, ...(typeof entry.title === 'string' ? { title: entry.title } : {}) });
          }
          const closedTabs = boundedClosedHistory(layout, readClosed, closedLimit);
          const presentationOwners = new Set([...live, ...retainedClosedTabs(layout, closedTabs, closedLimit).map((entry) => entry.tab.id)]);
          const titles: NonNullable<Workspace['titles']> = {};
          if (record(value.titles)) for (const [id, name] of Object.entries(value.titles)) {
            if (live.has(id) && record(name) && typeof name.title === 'string') titles[id] = { title: name.title, ...(typeof name.path === 'string' ? { path: name.path } : {}) };
          }
          return { kept: { arrangement: value.arrangement, layout, focusedGroup: typeof value.focusedGroup === 'string' ? value.focusedGroup : null,
            focusedMainGroup: typeof value.focusedMainGroup === 'string' ? value.focusedMainGroup : null, focusedMainTab: Object.hasOwn(value, 'focusedMainTab') ? member(value.focusedMainTab) : member(value.activeContent),
            focusedTab: member(value.focusedTab), activeContent: member(value.activeContent), focusMode: value.focusMode === true, closedTabs, closedLimit,
            titles, chipSizes: readChipSizes(value.chipSizes), ...(value.statusItems === undefined ? {} : { statusItems: readStatus(value.statusItems) }), ...(record(value.presentations) ? { presentations: Object.fromEntries(Object.entries(value.presentations).filter((entry): entry is [string, Record<string, unknown>] => presentationOwners.has(entry[0]) && record(entry[1]))) } : {}) }, dropped: false };
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
