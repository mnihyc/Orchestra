/** The trash of a node: what was deleted under it, by whom and until when it can be restored.
 *
 * Opened from the menu of an org, a project or a topic for whoever manages it, and from Show on the line a deletion
 * leaves: each thing that was deleted directly under it and has not been purged yet, who deleted it and when, until when
 * it can be brought back, and Restore, which brings it back at once. What comes back comes back archived, with
 * everything that was under it, and the notice that says so offers to bring it back to use at once.
 */
import { PathLink } from '@/shell/TrailLink';
import { useInfiniteQuery, useMutation, useQuery } from '@tanstack/react-query';
import { Eye, History, RotateCcw } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { deletedQuery, nodeQuery, restoreNode, unarchiveNode } from '@/api/nodes';
import { keptDocumentsQuery, keptEnvironmentsQuery } from '@/api/trace';
import { holds } from '@/lib/permissions';
import { useNamedPlace } from '@/features/navigation/namedPlace';
import { PLACE_ICONS, type PlaceRef } from '@/features/navigation/places';
import type { KeptNode } from '@/features/trace/pending';
import { inspectionHref } from '@/features/trace/objects';
import { KeptDocuments } from '@/features/trace/KeptDocuments';
import { KeptEnvironments } from '@/features/trace/KeptEnvironments';
import { useFormatContext } from '@/lib/appearance';
import { formatDateTime } from '@/lib/time';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { toast } from '@/ui/Toast';

export interface RecentlyDeletedProps {
  /** The place whose deleted things are listed. */
  place: PlaceRef;
  open: boolean;
  onClose: () => void;
}

function DeletedRow({ entry, place }: { entry: KeptNode; place: PlaceRef }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  // A project deleted from inside a project was a folder.
  const Icon = PLACE_ICONS[entry.kind === 'project' && place.kind === 'project' ? 'folder' : entry.kind];
  const name = entry.node.name ?? '';
  const restore = useMutation({
    mutationFn: () => restoreNode(entry.node.id),
    onSuccess: (node) => {
      toast({
        tone: 'success',
        title: t('places.deleted.restored', { name }),
        action: {
          label: t('places.deleted.unarchive'),
          run: () => void unarchiveNode(node.id),
        },
      });
    },
  });

  return (
    <li className="flex flex-col gap-2 py-3">
      <div className="grid grid-cols-[auto_minmax(0,1fr)] items-start gap-x-3 gap-y-2 sm:grid-cols-[auto_minmax(0,1fr)_auto]">
        <span className="mt-0.5 text-text-subtle">
          <Icon aria-hidden />
        </span>
        <div className="flex min-w-0 flex-1 flex-col">
          <span className="font-medium wrap-anywhere text-text">{name}</span>
          <span className="text-sm text-text-muted">
            {entry.deleted_by === null
              ? t('places.deleted.by.unknown')
              : t('places.deleted.by.person', { name: entry.deleted_by.display_name })}
            {' · '}
            <RelativeTime at={entry.deleted_at} />
          </span>
          <span className="text-sm text-text-muted">
            {t('places.deleted.until', { time: formatDateTime(entry.purge_at, format) })}
            {entry.children > 0 && ` · ${t('places.deleted.holds', { count: entry.children })}`}
          </span>
        </div>
        {/* The place's list of deleted children has no guard per row: whoever reads it manages the place, which restoring
            and inspecting them take; a row that says no keeps its no, and the server checks each act. */}
        <div className="col-start-2 flex flex-wrap gap-2 sm:col-start-3">
          {entry.can_inspect !== false && <Button size="sm" asChild><PathLink href={inspectionHref(entry.kind, entry.node.id)}><Eye aria-hidden />{t('trace.inspect')}</PathLink></Button>}
          {entry.can_restore !== false && <Button size="sm" icon={<RotateCcw aria-hidden />} pending={restore.isPending} onClick={() => restore.mutate()}>{t('trace.restore')}</Button>}
        </div>
      </div>
      {restore.error !== null && <ErrorAlert error={restore.error} />}
      <span className="sr-only">{place.name}</span>
    </li>
  );
}

export function RecentlyDeleted({ place, open, onClose }: RecentlyDeletedProps) {
  const { t } = useTranslation();
  const named = useNamedPlace(place);
  const node = useQuery({ ...nodeQuery(place.id), enabled: open });
  const mayManageNodes = holds(node.data?.access, `${place.kind}.manage`);
  const list = useInfiniteQuery({ ...deletedQuery(place.id), enabled: open && mayManageNodes });
  const entries = list.isError ? [] : list.data?.pages.flatMap((page) => page.items) ?? [];
  // The child sections use the same query keys, so their readers share these requests and current facts.
  const documents = useInfiniteQuery({ ...keptDocumentsQuery(place.id), enabled: open && node.isSuccess });
  const environments = useInfiniteQuery({ ...keptEnvironmentsQuery(place.id), enabled: open && node.isSuccess });
  const empty = node.isSuccess && (!mayManageNodes || (list.isSuccess && entries.length === 0))
    && documents.isSuccess && documents.data.pages.every((page) => page.items.length === 0)
    && environments.isSuccess && environments.data.pages.every((page) => page.items.length === 0);

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && onClose()}
      title={t('places.deleted.title', { name: named })}
      description={t('trace.recentDeletedLead')}
      footer={<Button onClick={onClose}>{t('common.close')}</Button>}
    >
      {node.isPending ? <SkeletonRows read={node} rows={2} /> : node.isError ? <ErrorState error={node.error} onRetry={() => void node.refetch()} /> : !mayManageNodes ? null : list.isPending ? (
        <SkeletonRows read={list} rows={3} />
      ) : list.isError && entries.length === 0 ? (
        <ErrorState compact title={t('trace.nodeReadFailure')} error={list.error} retrying={list.isRefetching} onRetry={() => void list.refetch()} />
      ) : entries.length === 0 ? null : (
        <>
          <ul className="flex flex-col divide-y divide-border">
            {entries.map((entry) => (
              <DeletedRow key={entry.node.id} entry={entry} place={place} />
            ))}
          </ul>
          <Pagination
            hasMore={list.hasNextPage}
            loading={list.isFetchingNextPage}
            error={list.isFetchNextPageError ? list.error : undefined}
            onMore={() => void list.fetchNextPage()}
          />
        </>
      )}
      {node.isSuccess && <KeptDocuments node={place.id} open={open} hideEmpty />}
      {node.isSuccess && <KeptEnvironments node={place.id} open={open} hideEmpty />}
      {empty && <EmptyState compact icon={<History aria-hidden />} title={t('trace.recentEmpty')} />}
    </Dialog>
  );
}
