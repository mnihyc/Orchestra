/** The visible hierarchy is data: paging, focus and selection do not depend on mounted rows. */
import type { EverythingRow } from '@/generated/api-types';

export interface Branch {
  rows: readonly EverythingRow[];
  pending: boolean;
  error: unknown;
  retired: boolean;
  hasMore: boolean;
  loadingMore: boolean;
  more: () => void;
  retry: () => void;
}
export interface Item {
  type: 'item'; position: string; key: string; parentKey: string | null; parentPosition: string | null;
  row: EverythingRow; depth: number; stale: boolean;
}
export type Entry = Item
  | { type: 'pending'; position: string }
  | { type: 'error'; position: string; source: Branch }
  | { type: 'empty'; position: string; message: 'empty' | 'noMatches' | 'noResults' }
  | { type: 'heading'; position: string; heading: string }
  | { type: 'more'; position: string; source: Branch };

export const keyOf = (row: EverythingRow): string => row.kind + ':' + row.id;
export const isItem = (entry: Entry): entry is Item => entry.type === 'item';

export function visibleRows(branches: ReadonlyMap<string | null, Branch>, parent: string | null, open: (row: EverythingRow) => boolean,
  options: { empty: 'empty' | 'noMatches' | 'noResults'; filtering: boolean;
    order: (rows: readonly EverythingRow[]) => readonly EverythingRow[];
    heading: (row: EverythingRow, previous: EverythingRow | undefined) => string | null }): Entry[] {
  const entries: Entry[] = [];
  type Work = { type: 'branch'; at: string | null; parentKey: string | null; parentPosition: string | null; root: string | null; depth: number }
    | { type: 'entry'; entry: Entry };
  const work: Work[] = [{ type: 'branch', at: parent, parentKey: null, parentPosition: null, root: null, depth: 0 }];
  while (work.length > 0) {
    const next = work.pop();
    if (next === undefined) break;
    if (next.type === 'entry') { entries.push(next.entry); continue; }
    const { at, parentKey, parentPosition, depth } = next;
    const path = parentPosition ?? 'root';
    const source = branches.get(at);
    if (source === undefined || source.pending) { entries.push({ type: 'pending', position: path + '/pending' }); continue; }
    if (source.error !== null) entries.push({ type: 'error', position: path + '/error', source });
    if (source.retired || source.error !== null && source.rows.length === 0) continue;
    const rows = options.order([...new Map(source.rows.map((row) => [keyOf(row), row])).values()]);
    if (rows.length === 0) { entries.push({ type: 'empty', position: path + '/empty', message: options.empty }); continue; }
    if (source.hasMore) work.push({ type: 'entry', entry: { type: 'more', position: path + '/more', source } });
    if (depth === 0 && options.filtering && rows.every((row) => row.kind === 'group' && !row.has_children)) work.push({ type: 'entry', entry: { type: 'empty', position: path + '/filtered', message: 'noResults' } });
    for (let index = rows.length - 1; index >= 0; index -= 1) {
      const row = rows[index];
      if (row === undefined) continue;
      const key = keyOf(row), origin = next.root ?? key;
      // A root and immediate parent distinguish ordinary/shared/resource projections without copying the full ancestry into every identity.
      const position = key + '@' + (parentKey ?? '') + '@' + origin;
      if (open(row)) work.push({ type: 'branch', at: row.id, parentKey: key, parentPosition: position, root: origin, depth: depth + 1 });
      work.push({ type: 'entry', entry: { type: 'item', position, key, parentKey, parentPosition, row, depth, stale: source.error !== null } });
      const heading = options.heading(row, rows[index - 1]);
      if (heading !== null) work.push({ type: 'entry', entry: { type: 'heading', position: position + '/heading', heading } });
    }
  }
  return entries;
}
