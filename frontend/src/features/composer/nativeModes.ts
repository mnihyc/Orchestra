/** An identity index of admitted native mode observations; values always come from the current view. */
import type { Item } from '@/generated/api-types';
import type { ThreadView } from '@/realtime/stores/thread';

export type ModeKind = 'approval' | 'sandbox';
type ModeReport = Extract<Extract<Item, { kind: 'marker' }>['payload'], { marker: 'harness_changed' }>;
type Addresses = Partial<Record<ModeKind, string>>;
const addresses = new WeakMap<readonly string[], ReadonlyMap<string, Addresses>>();

/** Native observation IDs retain their segment/change identity. Streaming keeps structural order stable. */
export function reportedMode(view: Pick<ThreadView, 'order' | 'items'>, segment: string | null, kind: ModeKind): ModeReport | null {
  if (segment === null) return null;
  let bySegment = addresses.get(view.order);
  if (bySegment === undefined) {
    const index = new Map<string, Addresses>();
    for (const id of view.order) {
      const item = view.items.get(id);
      if (item?.kind !== 'marker' || item.stream !== 'main' || item.payload.marker !== 'harness_changed') continue;
      const change = item.payload.data.change;
      if (change !== 'approval_mode' && change !== 'sandbox_mode') continue;
      const found = index.get(item.segment_id) ?? {};
      found[change === 'approval_mode' ? 'approval' : 'sandbox'] = id;
      index.set(item.segment_id, found);
    }
    bySegment = index; addresses.set(view.order, bySegment);
  }
  const id = bySegment.get(segment)?.[kind];
  const item = id === undefined ? undefined : view.items.get(id);
  return item?.kind === 'marker' && item.stream === 'main' && item.segment_id === segment
    && item.payload.marker === 'harness_changed' && item.payload.data.change === `${kind}_mode` ? item.payload : null;
}
