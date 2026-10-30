/** Safe linked places for settings and inspection in the shared title bar. */
import type { NodeRef } from '@/generated/api-types';
import type { TrailPart } from '@/shell/screen';
export function nodeTrail(path: readonly NodeRef[]): TrailPart[] {
  return path.filter((place) => !place.redacted && place.name !== null).map((place) => ({ label: place.name ?? '', to: `/${place.kind}/${place.id}` }));
}
