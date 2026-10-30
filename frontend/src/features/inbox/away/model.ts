/** Join supplied readable groups across pages without adding their repeated counts or duplicating row ids. */
import type { AwayNotification, AwayPage, NotificationGroup } from '@/features/settings/personal/away/types';
import { placePath } from '@/features/navigation/places';

export interface PlaceGroup extends NotificationGroup { rows: AwayNotification[] }

/** Current group metadata is the server's; notification kind never grants the right to answer a request. */
export function mergePlacePages(pages: readonly AwayPage[]): PlaceGroup[] {
  const rows = new Map<string, AwayNotification>();
  for (const page of pages) for (const row of page.items) {
    const old = rows.get(row.id);
    if (old === undefined || old.inbox_seq < row.inbox_seq) rows.set(row.id, row);
  }
  const groups = new Map<string, PlaceGroup>();
  for (const row of rows.values()) {
    const metadata = row.group;
    if (metadata === null) continue;
    const group = groups.get(metadata.key);
    if (group === undefined) groups.set(metadata.key, { ...metadata, rows: [row] });
    else { group.rows.push(row); if (metadata.through_seq > group.through_seq) Object.assign(group, metadata); }
  }
  for (const group of groups.values()) group.rows.sort((a, b) => Number(b.waiting_for_me) - Number(a.waiting_for_me) || Date.parse(b.created_at) - Date.parse(a.created_at) || b.id.localeCompare(a.id));
  return [...groups.values()].sort((a, b) => Number(b.waiting_for_me > 0) - Number(a.waiting_for_me > 0) || newest(b) - newest(a));
}
function newest(group: PlaceGroup): number { return Math.max(...group.rows.map((row) => Date.parse(row.created_at))); }

/** An explicit thread request opens its existing catch-up card; there is no generated answer excerpt. */
export function catchupUrl(row: Pick<AwayNotification, 'thread_id'>): string | null {
  return row.thread_id === null ? null : `/thread/${encodeURIComponent(row.thread_id)}?catchup=true`;
}

export function placeUrl(place: NotificationGroup['place']): string | null {
  if (place === null || place.redacted || place.name === null) return null;
  // Nodes opened by id are resolved through the same reader-aware node route as navigation links.
  return placePath(place);
}
