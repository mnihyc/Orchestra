/** Defines a tag at an authorized place and assigns it separately, preserving a failed name or a created-but-unassigned tag. */
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { LoaderCircle, Plus, Tags } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';

import { ApiError } from '@/api/client';
import { createPickerTag } from '@/api/day100';
import { nodeQuery, tagsPageQuery, tagsQuery, setNodeTags } from '@/api/nodes';
import { settingsQuery } from '@/api/settings';
import { useHelpOpener } from '@/features/help/open';
import type { Tag, Node } from '@/generated/api-types';
import { denial, holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { Popover } from '@/ui/Popover';
import { ResourcePicker } from '@/ui/ResourcePicker';
import { Pagination } from '@/ui/Pagination';
import { useDebounced } from '@/lib/debounce';
import { usePickerCursorRestart } from '@/lib/pickerPages';
import { recentChoiceRows, usePickerRecents } from '@/lib/pickerRecents';
import { useCollectionSearch } from '@/lib/collectionSearch';
import { SkeletonRows } from '@/ui/Skeleton';
import { TagChip } from '@/ui/TagChip';

export function TagPicker(props: { place: Node; allowed: boolean }) {
  const { scope } = useCollectionSearch(`tag-picker:${props.place.id}`);
  return <TagPickerForTarget key={`${props.place.id}:${scope}`} {...props} />;
}

function TagPickerForTarget({ place, allowed }: { place: Node; allowed: boolean }) {
  const { t } = useTranslation();
  const openGuide = useHelpOpener();
  const client = useQueryClient();
  const [name, setName] = useState('');
  const [created, setCreated] = useState<Tag | null>(null);
  const [scopeId, setScopeId] = useState(place.id);
  const [open, setOpen] = useState(false);
  const typed = name.trim(), settled = useDebounced(typed), waiting = typed !== settled;
  const tagSource = tagsPageQuery(place.id, settled, true);
  const available = useInfiniteQuery({ ...tagSource, enabled: allowed && open && !waiting });
  usePickerCursorRestart(available, tagSource.queryKey);
  const recent = usePickerRecents(allowed && open);
  const listed = !waiting && (!available.isError || available.isFetchNextPageError) ? available.data?.pages.flatMap((page) => page.items) ?? [] : [];
  const scopes = place.path.filter((node) => !node.redacted);
  const placeName = (node: Tag['node']): string => {
    if (node.redacted || node.name === null) return t('access.place.hidden');
    const index = place.path.findIndex((step) => step.id === node.id);
    return index < 0 ? node.name : place.path.slice(0, index + 1).map((step) => step.redacted ? t('access.place.hidden') : step.name ?? t('access.place.hidden')).join(' / ');
  };
  const chosenScope = useQuery({ ...nodeQuery(scopeId), enabled: allowed && open });
  const scope = chosenScope.data?.id === scopeId && !chosenScope.isError && !chosenScope.isFetching ? chosenScope.data : undefined;
  const canDefine = scope !== undefined && holds(scope.access, `${scope.kind}.manage`);
  const chosen = new Set(place.tags.map((tag) => tag.id));
  const matching = listed.find((tag) => tag.name.toLocaleLowerCase() === typed.toLocaleLowerCase());
  const assign = useMutation({ mutationFn: (ids: string[]) => setNodeTags(place.id, ids, place.revision), onSuccess: () => { setCreated(null); setName(''); } });
  const apply = (tag: Tag) => { recent.record({ kind: 'tag', key: tag.id }); assign.mutate([...new Set([...chosen, tag.id])]); };
  const define = useMutation({
    mutationFn: async ({ text, key }: { text: string; key: string }): Promise<Tag> => {
      try { return await createPickerTag(scopeId, text, key); }
      catch (error) {
        if (!(error instanceof ApiError) || !error.is('conflict.exists')) throw error;
        const tags = await client.query({ ...tagsQuery(place.id), staleTime: 0 });
        const existing = tags.find((tag) => tag.name.toLocaleLowerCase() === text.toLocaleLowerCase());
        if (existing === undefined) throw error;
        return existing;
      }
    },
    onSuccess: (tag) => { setCreated(tag); apply(tag); },
  });
  const limits = useQuery({ ...settingsQuery(place.id), enabled: allowed && open });
  const limit = limits.data?.find((row) => row.key === 'tags.max_per_place')?.value;
  const most = typeof limit === 'number' ? limit : null;
  const busy = define.isPending || assign.isPending;
  const atLimit = most === null || chosen.size >= most;
  const recentTags = typed === '' ? recentChoiceRows(listed, recent.choices?.tag ?? [], (tag) => tag.id, (tag) => chosen.has(tag.id) || !atLimit) : [];
  const recentIds = new Set(recentTags.map((tag) => tag.id));
  const shown = [...recentTags, ...listed.filter((tag) => !recentIds.has(tag.id))].filter((tag) => tag.id !== created?.id);
  return (
    <div className="flex flex-wrap items-center gap-1" aria-busy={busy || undefined}>
      {place.tags.map((tag) => <TagChip key={tag.id} name={tag.name} color={tag.color} {...(allowed && !busy ? { onRemove: () => assign.mutate([...chosen].filter((id) => id !== tag.id)) } : {})} />)}
      {busy && <LoaderCircle className="animate-spin text-text-subtle" role="img" aria-label={t('places.menu.working')} />}
      {allowed && <Popover open={open} onOpenChange={setOpen} label={t('placeTags.title')} trigger={<Button size="sm" variant="ghost" icon={<Tags aria-hidden />}>{place.tags.length === 0 ? t('placeTags.add') : t('placeTags.edit')}</Button>} className="flex w-[min(22rem,calc(100vw-2rem))] max-w-full flex-col gap-3">
        <Input label={t('day100.tags.name')} value={name} disabled={busy || created !== null} onChange={(event) => setName(event.target.value)} autoComplete="off" />
        {waiting || available.isPending ? <SkeletonRows read={available} rows={2} /> : available.isError && !available.isFetchNextPageError ? <ErrorState compact error={available.error} onRetry={() => void available.refetch()} /> : <>
          <div className="flex max-h-[30dvh] min-w-0 flex-col gap-1 overflow-y-auto" data-scroll-allow="tags">
            {shown.map((tag, index) => <div key={tag.id} className="min-w-0">
              {index === 0 && recentTags.length > 0 && <div className="px-2 py-1 text-xs font-medium text-text-subtle">{t('ui.picker.recent')}</div>}
              {index === recentTags.length && recentTags.length > 0 && <div className="px-2 py-1 text-xs font-medium text-text-subtle">{t('placeTags.title')}</div>}
              <Checkbox presentation="picker" label={<TagChip name={t('inventoryChoices.tags.tag.name', { name: tag.name })} color={tag.color} />}
                description={t('inventoryChoices.tags.tag.description', { place: placeName(tag.node) })}
                help={{ label: t('helpGroups.tagsGuide'), guide: { article: 'settings-and-usage', heading: 'organization-management' }, onOpen: () => openGuide('settings-and-usage', 'organization-management') }}
                checked={chosen.has(tag.id)} disabled={busy || (!chosen.has(tag.id) && atLimit)} onCheckedChange={(on) => {
                  if (on) recent.record({ kind: 'tag', key: tag.id });
                  assign.mutate(on ? [...new Set([...chosen, tag.id])] : [...chosen].filter((id) => id !== tag.id));
                }} />
            </div>)}
          </div>
          {listed.length === 0 && !available.hasNextPage && typed === '' && <EmptyState compact title={t('placeTags.none')} />}
          <Pagination hasMore={available.hasNextPage} loading={available.isFetchingNextPage} error={available.isFetchNextPageError ? available.error : undefined} onMore={() => void available.fetchNextPage()} />
        </>}
        {recent.error !== null && <ErrorState compact title={t('ui.picker.recent')} error={recent.error} onRetry={recent.retry} />}
        {most !== null && chosen.size >= most && <p className="text-sm text-text-muted">{t('placeTags.limit', { count: most })}</p>}
        {typed !== '' && matching === undefined && created === null && !waiting && !available.isPending && !available.isError && <>
          <ResourcePicker pickerKey="tag-definition-place" groupHomes={false} label={t('day100.tags.defineIn')} value={scopeId} disabled={busy} onValueChange={setScopeId} options={scopes.map((node) => ({ value: node.id, resource: { kind: node.kind, id: node.id, data: node },
            name: t('inventoryChoices.tags.scope.name', { name: node.name ?? t('access.place.hidden') }),
            description: t('inventoryChoices.tags.scope.description', { place: placeName(node) }),
            help: { label: t('helpGroups.tagsGuide'), guide: { article: 'settings-and-usage', heading: 'organization-management' }, onOpen: () => openGuide('settings-and-usage', 'organization-management') },
          }))} />
          {canDefine ? <Button className="max-w-full" icon={<Plus aria-hidden />} pending={define.isPending} disabled={busy || atLimit} onClick={() => define.mutate({ text: typed, key: crypto.randomUUID() })}><span className="min-w-0 truncate">{t('day100.tags.make', { name: typed })}</span></Button> : <p className="text-sm text-text-muted">{scope === undefined ? t('day100.tags.checking') : denial(t, `${scope.kind}.manage`, scope.name, scope.owner)}</p>}
        </>}
        {chosenScope.isError && <ErrorState compact error={chosenScope.error} onRetry={() => void chosenScope.refetch()} />}
        {limits.isError && <ErrorAlert error={limits.error} />}
        {define.error !== null && <ErrorAlert error={define.error} />}
        {assign.error !== null && <ErrorAlert error={assign.error} />}
        {created !== null && assign.isError && <><InlineAlert tone="warning">{t('day100.tags.notAssigned', { name: created.name })}</InlineAlert><TagChip className="self-start" name={created.name} color={created.color} /><Button className="max-w-full" icon={<Plus aria-hidden />} pending={assign.isPending} disabled={atLimit} onClick={() => apply(created)}><span className="min-w-0 truncate">{t('day100.tags.apply', { name: created.name })}</span></Button></>}
      </Popover>}
      {assign.error !== null && created === null && <ErrorAlert error={assign.error} />}
    </div>
  );
}
