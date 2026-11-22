/** A native name page for component cases whose focus is what choosing a result does. */
import type { NameSearch } from '@/api/search';
import type { NameHit, NodeRef } from '@/generated/api-types';

export function pickerNamePage(search: NameSearch, read: () => readonly NameHit[] | Promise<readonly NameHit[]>) {
  return {
    queryKey: ['fixture-picker-names', search],
    initialPageParam: null as string | null,
    queryFn: async () => ({items: await read(), partial: false, next_cursor: null}),
    getNextPageParam: () => undefined,
  };
}

export function namedPlace(id: string, kind: NodeRef['kind'], name: string, parent?: NameHit): NameHit {
  return {id, kind, name, path: [...(parent?.path ?? []), {id, kind, name, redacted:false}], home_path: [],
    address:null, avatar:null, archived:false, color:null, custom_color:null, owner:null, recent:false, tag:null, thread_attention:null, username:null};
}

export function placeNamePage(search: NameSearch, places: readonly NameHit[]) {
  return pickerNamePage(search, () => places.filter((place) => search.kinds.includes(place.kind)
    && (search.orgId === undefined || place.path[0]?.id === search.orgId)
    && [place.name, ...place.path.map((step) => step.name ?? '')].some((name) => name.toLowerCase().includes(search.q.toLowerCase()))));
}
