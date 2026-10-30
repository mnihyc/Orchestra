/** List content kept apart from the minimal names that open permission pages. */
import type { HiddenItem } from '@/generated/api-types';

/** Ordinary list consumers and pickers iterate only readable rows; the list frame also draws private names. */
export type ResourceRows<T> = T[] & { privateRows: HiddenItem[] };
type ListedRow<T> = HiddenItem | (T & { content_hidden?: false });

/** Separates the wire union before a status column, action or usable picker sees a row. */
export function resourceRows<T>(listed: readonly ListedRow<T>[]): ResourceRows<T> {
  const content: T[] = [];
  const privateRows: HiddenItem[] = [];
  for (const row of listed) {
    if (row.content_hidden === true) privateRows.push(row);
    else content.push(row);
  }
  privateRows.sort((a, b) => a.name.localeCompare(b.name) || a.id.localeCompare(b.id));
  return Object.assign(content, { privateRows });
}

/** The names kept on a list's original data array, which existing content-only filters leave aside. */
export function privateRowsOf(rows: readonly unknown[]): readonly HiddenItem[] {
  return 'privateRows' in rows && Array.isArray(rows.privateRows) ? rows.privateRows as HiddenItem[] : [];
}
