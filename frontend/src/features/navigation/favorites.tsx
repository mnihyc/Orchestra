/** Starring a place, which is a person's own and easily undone: it shows at once and is put right if the server refuses.
 *
 * A star is a shortcut kept in the person's own list (Starred). The list is read once and kept; starring or
 * unstarring changes it on the spot, and a refusal puts it back and says so. The star of a place is one button
 * wherever it is drawn: in a screen's header, and as an entry of every menu of that place.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Star } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { favoritesKey, favoritesQuery, starNode, unstarNode } from '@/api/personal';
import type { NodeRef, Shortcut } from '@/generated/api-types';
import type { PlaceRef } from '@/features/navigation/places';
import { pathTo, useTreeStore } from '@/realtime/stores/tree';
import { useErrorText } from '@/ui/errorText';
import { IconButton } from '@/ui/IconButton';
import { toast } from '@/ui/Toast';

/** The places above one, from the tree that holds it: what a favorite says of where it lies. */
function ancestorsOf(id: string): NodeRef[] {
  for (const tree of useTreeStore.getState().byOrg.values()) {
    if (!tree.nodes.has(id)) continue;
    return pathTo(tree, id)
      .slice(0, -1)
      .map((node): NodeRef => ({ id: node.id, kind: node.kind, name: node.name, redacted: false }));
  }
  return [];
}

/** The person's own shortcuts, in order, as far as they have arrived. */
export function useFavorites(): readonly Shortcut[] {
  return useQuery(favoritesQuery).data ?? [];
}

/** Whether the thing with this identity is in the person's own list. */
export function useIsStarred(id: string): boolean {
  return useFavorites().some((favorite) => favorite.target.id === id);
}

/** A place in the person's own list, as it reads before the server has answered. */
export function starredPlace(place: PlaceRef, path: NodeRef[] = ancestorsOf(place.id)): Shortcut {
  return {
    id: `pending:${place.id}`,
    node_id: null,
    target: { kind: place.kind, id: place.id },
    position: '',
    state: 'open',
    item: { name: place.name, path, org_id: path[0]?.id ?? null, archived: place.archived, color: null },
    view: null,
  };
}

/** What the list reads like with a place put on it or taken off. */
function toggled(list: readonly Shortcut[], place: PlaceRef, starred: boolean): Shortcut[] {
  if (!starred) return list.filter((favorite) => favorite.target.id !== place.id);
  if (list.some((favorite) => favorite.target.id === place.id)) return [...list];
  return [...list, starredPlace(place)];
}

/** Stars or unstars a place: the list changes first, and is put back if the server says no. */
export function useStar(place: PlaceRef): { starred: boolean; set: (starred: boolean) => void } {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const describe = useErrorText();
  const starred = useIsStarred(place.id);

  const change = useMutation({
    mutationFn: (next: boolean) => (next ? starNode(place.id) : unstarNode(place.id)),
    onMutate: async (next) => {
      await queryClient.cancelQueries({ queryKey: favoritesKey });
      const before = queryClient.getQueryData<Shortcut[]>(favoritesKey);
      queryClient.setQueryData<Shortcut[]>(favoritesKey, (list) => toggled(list ?? [], place, next));
      return { before };
    },
    onError: (error, next, context) => {
      queryClient.setQueryData(favoritesKey, context?.before);
      toast({
        tone: 'danger',
        title: next ? t('places.star.notStarred', { name: place.name }) : t('places.star.notUnstarred', { name: place.name }),
        description: describe(error).message,
      });
    },
    // The server's order, positions and each shortcut's own identity are the ones that last: a success reads the list
    // again through the act's own views (`acts.ts`).
    onSettled: (_data, error) => { if (error !== null) void queryClient.invalidateQueries({ queryKey: favoritesKey }); },
  });

  return { starred, set: (next) => change.mutate(next) };
}

/** The star in a screen's header: pressed while the place is starred. */
export function StarButton({ place }: { place: PlaceRef }) {
  const { t } = useTranslation();
  const { starred, set } = useStar(place);
  return (
    <IconButton
      size="sm"
      label={starred ? t('places.star.unstar') : t('places.star.star')}
      pressed={starred}
      icon={<Star className={starred ? 'fill-accent text-accent' : undefined} aria-hidden />}
      onClick={() => set(!starred)}
    />
  );
}
