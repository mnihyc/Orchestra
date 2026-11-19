/** Tab labels include the settings section and its own readable place, independent of body headings. */
import { useCallback, useMemo, useSyncExternalStore } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { orgsQuery } from '@/api/nodes';
import type { NodeRef } from '@/generated/api-types';
import { knownPlaces, placeLabel, placedOfPath, type KnownPlaces, type Placed } from '@/lib/placeLabel';
import { pathTo, useTreeStore } from '@/realtime/stores/tree';
import { settingsScreenTitle } from '@/features/settings/sectionLabels';

export interface SettingsTitlePlace {
  id: string;
  path: readonly NodeRef[];
  address?: string | null | undefined;
}

/** The current source path wins over cached copies, including every redacted ancestor and self. */
export function settingsTabTitle(section: string, source: SettingsTitlePlace | undefined, known: KnownPlaces): string {
  if (source === undefined || source.path.at(-1)?.id !== source.id) return section;
  const place = placedOfPath(source.path.map((part) => ({ id: part.id, name: part.redacted ? null : part.name })), source.address);
  return place === null ? section : settingsScreenTitle(section, placeLabel(place, known));
}

/** Existing authorized metadata only: no query observer, tree load, feed, or extra native read. */
export function useSettingsTabTitle(section: string, source: SettingsTitlePlace | undefined): string {
  const trees = useTreeStore((state) => state.byOrg);
  const cache = useQueryClient();
  const subscribe = useCallback((notify: () => void) => cache.getQueryCache().subscribe((event) => {
    const key: unknown = event.query.queryKey, expected = orgsQuery.queryKey;
    if (Array.isArray(key) && key.length === expected.length && key.every((part: unknown, index: number) => part === expected[index])) notify();
  }), [cache]);
  const snapshot = useCallback(() => cache.getQueryData(orgsQuery.queryKey), [cache]);
  const orgs = useSyncExternalStore(subscribe, snapshot, () => undefined);
  const known = useMemo(() => {
    const places: Placed[] = (orgs ?? []).map((org) => ({ id: org.id, name: org.name, above: [], address: org.address }));
    for (const tree of trees.values()) for (const node of tree.nodes.values()) {
      if (node.deleted || !['org', 'project', 'topic'].includes(node.kind)) continue;
      places.push({ id: node.id, name: node.name, address: node.address,
        above: pathTo(tree, node.id).slice(0, -1).map((part) => ({ id: part.id, name: part.name })) });
    }
    return knownPlaces(places);
  }, [trees, orgs]);
  return useMemo(() => settingsTabTitle(section, source, known), [section, source, known]);
}
