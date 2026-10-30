/** The optional columns of Everything, kept separately for each signed-in person on this device. */
import type { EverythingSort } from '@/generated/api-types';

export const OPTIONAL_COLUMNS = ['admin_access', 'kind', 'owner', 'created', 'created_by', 'last_activity', 'state', 'size', 'my_access', 'tags'] as const;
export type OptionalColumn = (typeof OPTIONAL_COLUMNS)[number];
export type Column = 'name' | 'where' | 'opened' | 'activity_by' | OptionalColumn;
export const DEFAULT_COLUMNS: readonly OptionalColumn[] = ['admin_access', 'kind', 'owner', 'last_activity'];
export const COLUMN_SORT: Record<Column, EverythingSort> = {
  name: 'name', admin_access: 'my_access', kind: 'kind', owner: 'owner', created: 'created', created_by: 'created_by',
  last_activity: 'last_activity', state: 'state', size: 'size', my_access: 'my_access', tags: 'tags',
  where: 'where', opened: 'opened', activity_by: 'activity_by',
};

export function columnKey(person: string, arrangement: 'tree' | 'recent' = 'tree'): string { return arrangement === 'tree' ? `orch.everything.columns.${person}` : `orch.everything.recent.columns.${person}`; }

export function readColumns(person: string, arrangement: 'tree' | 'recent' = 'tree'): OptionalColumn[] {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(columnKey(person, arrangement)) ?? 'null');
    if (Array.isArray(value)) return OPTIONAL_COLUMNS.filter((column) => value.includes(column));
  } catch { /* A blocked storage area or an older preference falls back to the usual columns. */ }
  return arrangement === 'tree' ? [...DEFAULT_COLUMNS] : [];
}

export function saveColumns(person: string, columns: readonly OptionalColumn[], arrangement: 'tree' | 'recent' = 'tree'): void {
  try { localStorage.setItem(columnKey(person, arrangement), JSON.stringify(columns)); } catch { /* This choice still works for this visit. */ }
}
