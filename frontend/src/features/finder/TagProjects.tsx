/** A tag opens its defining project or a readable project below the place that defines it. */
import { useQuery } from '@tanstack/react-query';
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { fetchTree } from '@/api/nodes';
import type { HiddenItem, Tag, TreeNode } from '@/generated/api-types';
import type { Destination } from '@/features/finder/destinations';
import { placedOfRow, usePlaceLabels } from '@/features/navigation/placeLabels';
import type { OrgTree } from '@/realtime/stores/tree';
import { Dialog } from '@/ui/Dialog';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { NodeIcon } from '@/ui/NodeIcon';
import { SkeletonRows } from '@/ui/Skeleton';
import { Tag as TagIcon } from 'lucide-react';

export type FinderTag = Pick<Tag, 'id' | 'node' | 'name'>;

/** The route's tag filter is an array; the finder writes it as JSON into its ordinary address. */
export function tagProjectDestination(tag: Pick<Tag, 'id'>, projectId: string): Destination {
  return { to: '/project/$projectId', params: { projectId }, search: { view: 'list', tag: JSON.stringify([tag.id]) } };
}

/** Only readable projects under the defining place are eligible; hidden rows supply structural ancestry alone. */
export function projectsForTag(tag: FinderTag, nodes: readonly TreeNode[], hidden: readonly Pick<HiddenItem, 'id' | 'parent_id'>[] = []): TreeNode[] {
  const parents = new Map([...nodes, ...hidden].map((node) => [node.id, node.parent_id]));
  const inside = (id: string): boolean => {
    const seen = new Set<string>();
    for (let current: string | null | undefined = id; current !== null && current !== undefined && !seen.has(current); current = parents.get(current)) {
      if (current === tag.node.id) return true;
      seen.add(current);
    }
    return false;
  };
  return nodes.filter((node) => node.kind === 'project' && !node.deleted && inside(node.id))
    .sort((one, other) => one.name.localeCompare(other.name) || one.id.localeCompare(other.id));
}

export function TagProjects({ tag, rootOrgId, onChoose, onClose }: { tag: FinderTag; rootOrgId: string; onChoose: (destination: Destination) => void; onClose: () => void }) {
  const { t } = useTranslation();
  const [text, setText] = useState('');
  const query = useQuery({ queryKey: ['finder-tag-projects', rootOrgId], queryFn: ({ signal }) => fetchTree(rootOrgId, null, signal), retry: false });
  const needle = text.trim().toLocaleLowerCase();
  const eligible = useMemo(() => query.data === undefined ? [] : projectsForTag(tag, query.data.nodes, query.data.hidden), [query.data, tag]);
  const placed = useMemo(() => {
    const data = query.data;
    if (data === undefined) return [];
    const tree: OrgTree = { nodes: new Map(data.nodes.map((node) => [node.id, node])), hidden: new Map(data.hidden.map((node) => [node.id, node])), lastSeq: data.seq, loaded: data.complete, error: null };
    return eligible.map((node) => placedOfRow(tree, node));
  }, [query.data, eligible]);
  const labels = usePlaceLabels(placed);
  const choices = eligible.filter((node) => (labels.get(node.id) ?? node.name).toLocaleLowerCase().includes(needle));
  return <Dialog open onOpenChange={(open) => { if (!open) onClose(); }} title={t('finderTags.choose', { name: tag.name })} description={t('finderTags.about')} footer={<Button onClick={onClose}>{t('common.cancel')}</Button>}>
    <Input type="search" label={t('finderTags.search')} value={text} onChange={(event) => setText(event.target.value)} />
    {query.error !== null ? <ErrorState compact error={query.error} onRetry={() => void query.refetch()} /> : query.data === undefined ? <SkeletonRows rows={4} /> : choices.length === 0 ? <EmptyState compact icon={<TagIcon aria-hidden />} title={t('finderTags.none')} /> : <ul aria-label={t('finderTags.projects')} className="flex flex-col divide-y divide-border">{choices.map((node) => <li key={node.id}><button type="button" className="focus-inset flex w-full min-w-0 items-center gap-2 rounded-md px-2 py-2 text-left hover:bg-bg-muted" onClick={() => onChoose(tagProjectDestination(tag, node.id))}><NodeIcon kind="project" color={node.color} own={node.custom_color} /><span className="truncate" title={labels.get(node.id) ?? node.name}>{labels.get(node.id) ?? node.name}</span></button></li>)}</ul>}
  </Dialog>;
}
