/** Resource rows use readable served origins, without fetching trees or displaying protocol addresses. */
import type { NodeRef, UserRef } from '@/generated/api-types';
import { LABEL_SEPARATOR, placeLabels, type KnownPlaces, type Placed } from '@/lib/placeLabel';

export interface NamedResource {
  id: string;
  name: string;
  home?: NodeRef | null;
  org_id?: string | null;
  owner?: Pick<UserRef, 'id' | 'display_name'> | null;
}

/** Cached readable ancestors help with repeated names; a redacted home never uses a cached name. */
export function resourceLabels(rows: readonly NamedResource[], known: KnownPlaces): ReadonlyMap<string, string> {
  const places = new Map([...known.values()].flat().map((place) => [place.id, place]));
  const placed: Placed[] = rows.map((row) => {
    const above: { id: string; name: string | null }[] = [];
    const home = row.home;
    if (home !== null && home !== undefined && !home.redacted && home.name !== null) {
      const held = places.get(home.id);
      if (held !== undefined) above.push(...held.above);
      else if (row.org_id !== null && row.org_id !== undefined && row.org_id !== home.id) above.push({ id: row.org_id, name: places.get(row.org_id)?.name ?? null });
      above.push({ id: home.id, name: home.name });
    }
    if (row.owner !== null && row.owner !== undefined) above.push({ id: row.owner.id, name: row.owner.display_name });
    return { id: row.id, name: row.name, above };
  });
  const labels = placeLabels(placed);
  // Only actual namesakes need context. A parenthesis is part of the name, not another line of meta text.
  for (const row of rows) {
    const label = labels.get(row.id) ?? row.name;
    const prefix = `${row.name}${LABEL_SEPARATOR}`;
    if (label.startsWith(prefix)) labels.set(row.id, `${row.name} (${label.slice(prefix.length).split(LABEL_SEPARATOR).join(', ')})`);
  }
  return labels;
}
