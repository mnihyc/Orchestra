import { useCollectionSearch } from '@/lib/collectionSearch';
import { Input } from '@/ui/Input';
import { ScrollRegion } from '@/ui/ScrollRegion';
/** The documents beside a place: the list of an org at any level, a project and a topic, with open discussions, what changed since the reader was there, the mark of one an agent made, New document and Import Markdown.
 *
 * A row is the title, a lock when it is private, where it is written when the list spans several nodes,
 * who changed it last and when, a count of open discussions, and a dot when it changed since the reader
 * last had it open. One that an agent made carries the mark of the agent. The newest change comes first.
 * Deleted documents are listed at the foot, with Restore, for whoever could have deleted them. Rows are picked (a box,
 * Shift for a range, Ctrl or ⌘ to add one) to delete them together.
 */
import { DeviceChoiceWriterException, DeviceExpansionFacts } from '@/ui/DeviceChoiceFacts';
import { useListChoice } from '@/lib/listState';
import { useInfiniteQuery, useMutation } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { ChevronDown, FilePlus2, FileText, Folder, Trash2, Upload } from 'lucide-react';
import { useId, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { actionName } from '@/ui/actions';
import { documentsOfNodeQuery, restoreDocument } from '@/api/documents';
import { ParticipationFilter } from '@/features/access/ParticipationFilter';
import { DocumentRow } from '@/features/documents/DocumentRow';
import { PlaceShortcuts } from '@/features/navigation/shortcuts';
import { NameNewDocument } from '@/features/documents/NameNewDocument';
import { CopyDocumentDialog } from '@/features/people/CopyDocument';
import { openImport } from '@/features/people/importHostState';
import { Templates } from '@/features/people/Templates';
import { type NewDocument, useMakeDocument } from '@/features/documents/useMakeDocument';
import type { Access, NodeKind } from '@/generated/api-types';
import { denial, holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { FilterBar } from '@/ui/FilterBar';
import { Checkbox } from '@/ui/Checkbox';
import { cx } from '@/ui/cx';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { quietButtonClasses } from '@/ui/link';
import { Pagination } from '@/ui/Pagination';
import { Menu } from '@/ui/Menu';
import { PermissionGate } from '@/ui/PermissionGate';
import { SkeletonRows } from '@/ui/Skeleton';
import { toast } from '@/ui/Toast';
import { IconButton } from '@/ui/IconButton';
import { RowBox, SelectedBar, useRowSelection } from '@/ui/RowSelection';
import { useDocumentTrash } from '@/features/trash/useDocumentTrash';
import { placeById } from '@/features/trash/where';

/** The node a list is of: what it needs to know of it. */
export interface ListedNode {
  id: string;
  kind: NodeKind;
  name: string;
  access: Pick<Access, 'permissions'>;
  archived: boolean;
  owner?: { display_name: string };
}



export function DocumentList({ node, below = false }: { node: ListedNode; below?: boolean }) {
  const { t } = useTranslation();
  const me = useStore(client.session, (session) => session.userId);
  const [under, setUnder] = useListChoice('documents', 'under', false);
  const [participating, setParticipating] = useListChoice('documents', 'participating', false);
  const [priv, setPrivate] = useListChoice('documents', 'private', false);
  const [deleted, setDeleted] = useListChoice('documents', 'deleted', false);
  const [copying, setCopying] = useState(false);
  const search = useCollectionSearch('documents:' + node.id);
  const list = useInfiniteQuery(documentsOfNodeQuery(node.id, { under, participating, q: search.q }));
  const gone = useInfiniteQuery(documentsOfNodeQuery(node.id, { deleted: true, participating, q: search.q }));
  const deletedId = useId();
  const deletedDocs = gone.data?.pages.flatMap((page) => page.items) ?? [];
  const canMake = holds(node.access, 'doc.create') && !node.archived;
  const docs = list.data?.pages.flatMap((page) => page.items) ?? [];

  const selection = useRowSelection(docs, (doc) => doc.id);
  const trash = useDocumentTrash();
  const [trashing, setTrashing] = useState(false);
  const trashSelected = async () => {
    setTrashing(true);
    try {
      const result = await trash(selection.selected.map((doc) => ({ item: { kind: 'document' as const, id: doc.id }, name: doc.title })), placeById(node.id, node.kind, node.name));
      if (result.groups.length > 0) selection.clear();
    } finally { setTrashing(false); }
  };
  const make = useMakeDocument(node);
  const restore = useMutation({
    mutationFn: (id: string) => restoreDocument(id),
    onSuccess: () => {
      toast({ tone: 'success', title: t('documents.list.restored') });
    },
  });
  const create = (from: NewDocument) => make.mutate({ ...from, private: priv });
  // Creation is immediate; the opened document's title is where its name is written.
  const named = (title: string | undefined) => create(title === undefined ? {} : { title });

  const reason = node.archived ? t('documents.list.archived') : denial(t, 'doc.create', node.name, node.owner);
  return (
    <section aria-label={t('documents.list.title')} className="flex flex-col gap-3">
      <FilterBar search={<Input type="search" hideLabel label={t('ui.collection.find', { name: t('documents.list.title') })} placeholder={t('ui.combobox.search')} value={search.text} onChange={(event) => search.onText(event.target.value)} />} participation={<ParticipationFilter checked={participating} onCheckedChange={setParticipating} />} action={<>
          <Button asChild icon={<Folder aria-hidden />}><Link to="/documents/$nodeId" params={{ nodeId: node.id }}>{t('documentFolders.browse')}</Link></Button>
          <PermissionGate allowed={canMake} mode="block" reason={reason}>
            <Button icon={<Upload aria-hidden />} onClick={() => openImport({ id: node.id, label: node.name })}>
              {t('people.import')}
            </Button>
          </PermissionGate>
          <PermissionGate allowed={canMake} mode="block" reason={reason}>
            <span className="inline-flex">
              <NameNewDocument
                pending={make.isPending}
                onMake={named}
                trigger={
                  <Button variant="primary" icon={<FilePlus2 aria-hidden />} pending={make.isPending} className="rounded-r-none">
                    {priv ? t('documents.list.newPrivate') : t('documents.list.new')}
                  </Button>
                }
              />
              <Menu
                label={t('documents.list.newMenu')}
                align="end"
                trigger={<IconButton label={t('documents.list.newMenu')} variant="secondary" icon={<ChevronDown aria-hidden />} className="rounded-l-none border-l-0" />}
                entries={[{ id: 'from', label: t('people.newFrom'), onSelect: () => setCopying(true) }, { type: 'check', id: 'private', ...choiceOption(t, 'visibility.private'), description: t('documents.list.privateSwitch'), checked: priv, onCheckedChange: setPrivate }]}
              />
            </span>
          </PermissionGate>
        </>}>
        {below && <Checkbox label={t('documents.list.below')} checked={under} onCheckedChange={setUnder} />}
      </FilterBar>
      {make.isError && <ErrorAlert error={make.error} />}
      {canMake && <Templates nodeId={node.id} />}
      {copying && <CopyDocumentDialog destination={node.id} onClose={() => setCopying(false)} />}
      {selection.selected.length > 0 && <SelectedBar count={selection.selected.length} total={docs.length} onSelectAll={() => selection.toggleAll(true)} busy={trashing} onClear={selection.clear}>
        <Button size="sm" variant="danger-ghost" icon={<Trash2 aria-hidden />} pending={trashing} onClick={() => void trashSelected()}>{t('trash.delete')}</Button>
      </SelectedBar>}
      {list.isPending ? (
        <SkeletonRows read={list} rows={3} />
      ) : list.isError && docs.length === 0 ? (
        <ErrorState compact error={list.error} retrying={list.isRefetching} onRetry={() => void list.refetch()} />
      ) : docs.length === 0 ? (
        <EmptyState
          icon={<FileText aria-hidden />}
          title={t('documents.list.empty', { kind: t(`documents.kind.${node.kind}` as 'documents.kind.topic') })}
        />
      ) : docs.length > 0 ? (
        <ScrollRegion kind="table" label={t('documents.list.title')} className="max-h-96 overflow-y-auto"><ul className="divide-y divide-border rounded-lg border border-border bg-surface">
          {docs.map((doc) => (
            <DocumentRow key={doc.id} doc={doc} here={node.id} where={under} me={me}
              select={<RowBox name={doc.title} checked={selection.isTicked(doc)} onChange={(on, range) => selection.toggle(doc, on, range)} />}
              onPick={(event) => { selection.click(doc, event); }} />
          ))}
        </ul></ScrollRegion>
      ) : null}
      <Pagination hasMore={list.hasNextPage} loading={list.isFetchingNextPage} error={list.isFetchNextPageError ? list.error : null} onMore={() => void list.fetchNextPage()} />
      <PlaceShortcuts nodeId={node.id} />
      {(deletedDocs.length > 0 || gone.hasNextPage || gone.isError) && <div className="flex flex-col gap-2">
        <button
          type="button"
          aria-expanded={deleted}
          aria-controls={deleted ? deletedId : undefined}
          className={cx('inline-flex self-start items-center gap-1 text-sm text-text-muted', quietButtonClasses)}
          onClick={() => setDeleted((on) => !on)}
        >
          <ChevronDown aria-hidden className={cx('size-4 transition-transform', !deleted && '-rotate-90')} />
          {deletedDocs.length === 0 && !gone.hasNextPage ? t('documents.list.recentlyDeleted') : t(gone.hasNextPage ? 'documents.list.recentlyDeletedMore' : 'documents.list.recentlyDeletedCount', { count: deletedDocs.length })}<DeviceChoiceWriterException writer={setDeleted} />
        </button>
        {deleted && <DeviceExpansionFacts writer={setDeleted} open={deleted} />}
        {deleted && (
          <div id={deletedId} className="flex flex-col gap-2">
            {gone.isError && deletedDocs.length === 0 ? <ErrorState compact error={gone.error} retrying={gone.isRefetching} onRetry={() => void gone.refetch()} /> : <ScrollRegion kind="table" label={t('documents.list.recentlyDeleted')} className="max-h-96 overflow-y-auto"><ul className="divide-y divide-border rounded-lg border border-border bg-surface">
            {deletedDocs.map((doc) => (
              <li key={doc.id} className="flex items-center justify-between gap-3 px-3 py-2 text-sm">
                <span data-user-content className="truncate">{doc.title}</span>
                <Button action="restore" aria-label={actionName('restore', doc.title)} title={actionName('restore', doc.title)} size="sm" pending={restore.isPending && restore.variables === doc.id} onClick={() => restore.mutate(doc.id)}>
                  {t('documents.list.restore')}
                </Button>
              </li>
            ))}
            </ul></ScrollRegion>}
            <Pagination hasMore={gone.hasNextPage} loading={gone.isFetchingNextPage} error={gone.isFetchNextPageError ? gone.error : null} onMore={() => void gone.fetchNextPage()} />
          </div>
        )}
      </div>}
    </section>
  );
}
