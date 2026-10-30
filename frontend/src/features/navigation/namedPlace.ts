/** A place named with where it is, for a title or a heading that has no title bar above it to say so.
 *
 * "Move the topic Fix login, in Acme › Web": the kind of the place, its name and the way down to it, so
 * that no dialog names a place that another of the same name could be. A department is an org that lies
 * in another, a folder a project that lies in a project. An org at the root is named alone, as there is
 * nothing above it; a place above that the reader may not see is said to be private, without its name.
 */
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';

import type { NodeKind } from '@/generated/api-types';
import { useNodePath } from '@/realtime/stores/tree';

/** One place on the way down, outermost first. */
export interface PlaceAbove {
  kind: NodeKind;
  name: string | null;
  redacted?: boolean;
}

/** What a kind of place is called where it lies. */
export function kindOf(kind: NodeKind, parent: PlaceAbove | undefined): 'org' | 'department' | 'project' | 'folder' | 'topic' | 'thread' {
  if (kind === 'org') return parent === undefined ? 'org' : 'department';
  if (kind === 'project') return parent?.kind === 'project' ? 'folder' : 'project';
  return kind;
}

/** The place's name with its kind and the way down to it, or its name alone at the root. */
export function namedPlace(t: TFunction, place: { kind: NodeKind; name: string }, above: readonly PlaceAbove[]): string {
  if (above.length === 0) return t('places.named.root', { name: place.name });
  const path = above.map((one) => (one.redacted === true || one.name === null ? t('places.named.hidden') : one.name)).join(' › ');
  return t('places.named.in', { kind: t(`places.named.kinds.${kindOf(place.kind, above.at(-1))}`), name: place.name, path });
}

/** The place named with where it is, from the tree the reader holds; named alone until the tree holds it. */
export function useNamedPlace(place: { id: string; kind: NodeKind; name: string }): string {
  const { t } = useTranslation();
  const { path } = useNodePath(place.id);
  const above = path.length === 0 ? [] : path.slice(0, -1);
  return namedPlace(t, place, above);
}
