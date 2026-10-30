/** Inbox identities come with the row; naming a notification never opens another navigation tree. */
import { useMemo } from 'react';

import { usePlaceLabels } from '@/features/navigation/placeLabels';
import type { NodeRef, Notification } from '@/generated/api-types';
import type { Placed } from '@/lib/placeLabel';

/** A redacted reference stays redacted even if a previously opened tree holds its old name. */
export function notificationThread(row: Notification): NodeRef | null {
  return row.thread ?? (row.node?.kind === 'thread' ? row.node : null);
}

/** The nearest named place, with only the supplied readable names of its ancestors. */
export function notificationPlace(row: Notification): Placed | null {
  const path = row.path ?? [];
  const target = path.findLastIndex((node) => node.kind !== 'thread' && !node.redacted && node.name !== null);
  if (target >= 0) {
    const node = path[target];
    if (node !== undefined && node.name !== null) return {
      id: node.id, name: node.name,
      above: path.slice(0, target).filter((one) => one.kind !== 'thread').map((one) => ({ id: one.id, name: one.redacted ? null : one.name })),
    };
  }
  const node = row.node;
  return node !== null && node.kind !== 'thread' && !node.redacted && node.name !== null
    ? { id: node.id, name: node.name, above: [] } : null;
}

/** Peer rows and already held trees use the application's one place-label matcher, without fetching. */
export function useNotificationLabels(rows: readonly Notification[]): ReadonlyMap<string, string> {
  const places = useMemo(() => rows.flatMap((row) => {
    const place = notificationPlace(row);
    return place === null ? [] : [place];
  }), [rows]);
  return usePlaceLabels(places, { loadTrees: false });
}
