/** Stars this person's current project view or the place alone, and opens a saved view through its ordinary address. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Folder, Star, StarOff } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { starProjectView } from '@/api/day100';
import { favoritesKey, favoritesQuery, unstarNode } from '@/api/personal';
import { normalizeProjectView, projectViewAddress } from '@/features/project/viewState';
import type { Favorite, NodeRef, ProjectView as PersonalProjectView } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { IconButton } from '@/ui/IconButton';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Menu } from '@/ui/Menu';

export function ProjectStar({ projectId, name, view, readableTags, path = [] }: { projectId: string; name: string; view: PersonalProjectView; readableTags: readonly string[]; path?: readonly NodeRef[] }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const favorites = useQuery(favoritesQuery);
  const current: (Favorite & { view?: PersonalProjectView | null }) | undefined = favorites.data?.find((favorite) => favorite.node.id === projectId);
  const saved = current?.view === undefined || current.view === null ? null : normalizeProjectView(current.view);
  const safeSaved = saved === null ? null : { ...saved, tag_ids: saved.tag_ids.filter((id) => readableTags.includes(id)) };
  const save = useMutation({
    mutationFn: (choice: 'view' | 'place' | 'remove') => choice === 'remove' ? unstarNode(projectId) : starProjectView(projectId, choice === 'place' ? null : view),
    onMutate: async (choice) => {
      await client.cancelQueries({ queryKey: favoritesKey });
      const before = client.getQueryData<Favorite[]>(favoritesKey) ?? [];
      const previous = before.find((favorite) => favorite.node.id === projectId);
      const next: Favorite = { node: { id: projectId, kind: 'project', name, redacted: false }, path: previous?.path ?? path.filter((part) => part.id !== projectId), position: previous?.position ?? '', view: choice === 'view' ? view : null };
      client.setQueryData<Favorite[]>(favoritesKey, choice === 'remove' ? before.filter((favorite) => favorite.node.id !== projectId)
        : previous === undefined ? [...before, next] : before.map((favorite) => favorite.node.id === projectId ? next : favorite));
      return { before };
    },
    onError: (_error, _choice, context) => { client.setQueryData(favoritesKey, context?.before ?? []); },
  });
  return (
    <div className="flex flex-wrap items-center gap-2">
      <Menu
        label={t('day100.views.starMenu', { name })}
        trigger={<IconButton size="sm" label={t('day100.views.starMenu', { name })} icon={<Star aria-hidden className={current === undefined ? '' : 'fill-accent text-accent'} />} disabled={favorites.data === undefined} pending={save.isPending || favorites.isPending} pressed={current !== undefined} />}
        entries={[
          { id: 'view', label: t('day100.views.starView'), icon: <Star aria-hidden />, disabled: save.isPending ? t('day100.views.saving') : false, onSelect: () => save.mutate('view') },
          { id: 'place', label: t('day100.views.starPlace'), icon: <Folder aria-hidden />, disabled: save.isPending ? t('day100.views.saving') : false, onSelect: () => save.mutate('place') },
          ...(current === undefined ? [] : [{ id: 'remove', label: t('day100.views.removeStar'), icon: <StarOff aria-hidden />, disabled: save.isPending ? t('day100.views.saving') : false as const, onSelect: () => save.mutate('remove') }]),
        ]}
      />
      {safeSaved !== null && <Button size="sm" variant="ghost" asChild><a href={projectViewAddress(projectId, safeSaved)}>{t('day100.views.openStarred')}</a></Button>}
      {save.error !== null && <ErrorAlert error={save.error} />}
      {favorites.isError && favorites.data === undefined && <ErrorState compact error={favorites.error} onRetry={() => void favorites.refetch()} />}
    </div>
  );
}
