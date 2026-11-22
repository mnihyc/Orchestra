import { useCollectionSearch } from '@/lib/collectionSearch';
/** Places, documents and folders shared from outside the trees a person can browse. */
import { useState } from 'react';
import { useListChoice } from '@/lib/listState';
import { useQuery } from '@tanstack/react-query';
import { FileText, Folder, RefreshCw, Share2 } from 'lucide-react';
import { Link } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';
import { sharedPageQuery } from '@/api/shared';
import { openFinder } from '@/features/finder/store';
import { PlaceChain } from '@/features/navigation/PlaceChain';
import { isFolderPath, PlaceLink } from '@/features/navigation/places';
import type { SharedListItem, NodeKind } from '@/generated/api-types';
import { Page, PageHeader, PlaceMark } from '@/shell/Page';
import { useScreenHeading } from '@/shell/screen';
import { Badge } from '@/ui/Badge';
import { AsAdminMark, adminAccessOf } from '@/ui/AsAdminMark';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { FilterBar } from '@/ui/FilterBar';
import { NodeIcon } from '@/ui/NodeIcon';
import { RelativeTime } from '@/ui/RelativeTime';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { Table, type Column } from '@/ui/Table';
import { UserChip } from '@/ui/UserChip';

const KINDS: readonly NodeKind[] = ['org', 'project', 'topic', 'thread'];

export function SharedScreen() {
  const { t } = useTranslation();
  useScreenHeading({ title: t('screens.shared.title'), icon: Share2 });
  const search = useCollectionSearch('shared-with-me');
  const [kind, setKind] = useListChoice<NodeKind | 'document_container' | 'document' | 'all'>('shared', 'kind', 'all');
  const filters = JSON.stringify([search.scope, search.q, kind]);
  const [position, setPosition] = useState<{ filters: string; cursor: string | null }>({ filters, cursor: null });
  const cursor = position.filters === filters ? position.cursor : null;
  const setCursor = (next: string | null) => setPosition({ filters, cursor: next });
  const query = useQuery(sharedPageQuery(cursor, search.q, kind === 'all' ? undefined : kind));
  const rows = query.data?.items ?? [];
  const columns: Column<SharedListItem>[] = [
    { id: 'name', header: t('sharedWithMe.name'), cell: (row) => row.kind === 'document'
      ? <div className="flex min-w-0 flex-col gap-1"><div className="flex min-w-0 items-center gap-2"><FileText className="size-4 shrink-0 text-text-muted" aria-hidden />
        <Link to="/doc/$documentId" params={{ documentId: row.document.id }} className="truncate font-medium text-text hover:underline">{row.document.title}</Link>
        <AsAdminMark of={adminAccessOf(row.document)?.place} responsive /></div><PlaceChain places={row.path} quiet className="text-xs" /></div>
      : row.kind === 'document_container'
      ? <div className="flex min-w-0 flex-col gap-1"><div className="flex min-w-0 items-center gap-2"><Folder className="size-4 shrink-0 text-text-muted" aria-hidden />
        <Link to="/document-folder/$folderId" params={{ folderId: row.folder.id }} className="truncate font-medium text-text hover:underline">{row.folder.name ?? t('ui.nameUnknown')}</Link>
        <AsAdminMark of={adminAccessOf(row)?.place} responsive /></div><PlaceChain places={row.path} quiet className="text-xs" /></div>
      : <div className="flex min-w-0 flex-col gap-1"><div className="flex min-w-0 items-center gap-2"><NodeIcon kind={isFolderPath(row.path) ? 'folder' : row.kind} color={row.color} own={row.custom_color} done={row.kind === 'topic' && row.status === 'done'} /><PlaceLink place={row} className="truncate font-medium text-text hover:underline">{row.name}</PlaceLink><AsAdminMark of={adminAccessOf(row)?.place} responsive />{row.archived && <Badge>{t('sharedWithMe.archived')}</Badge>}</div><PlaceChain places={row.path.filter((part) => part.id !== row.id)} quiet className="text-xs" /></div> },
    { id: 'kind', header: t('sharedWithMe.kind'), from: 'sm', cell: (row) => t(row.kind === 'document' ? 'finder.kinds.document' : row.kind === 'document_container' ? 'finder.kinds.document_container' : isFolderPath(row.path) ? 'sharedWithMe.kinds.folder' : `sharedWithMe.kinds.${row.kind}`) },
    { id: 'owner', header: t('sharedWithMe.owner'), from: 'md', cell: (row) => row.kind === 'document_container' ? '—' : <UserChip user={row.kind === 'document' ? row.document.owner : row.owner} showUsername /> },
    { id: 'changed', header: t('sharedWithMe.changed'), from: 'md', cell: (row) => row.kind === 'document_container' ? '—' : <RelativeTime at={row.kind === 'document' ? row.document.updated_at : row.updated_at} /> },
  ];
  return <Page><PageHeader title={t('screens.shared.title')} lead={t('sharedWithMe.lead')} mark={<PlaceMark><Share2 aria-hidden /></PlaceMark>} />
    <FilterBar search={<Input type="search" hideLabel label={t('ui.collection.find', { name: t('screens.shared.title') })} placeholder={t('ui.combobox.search')} value={search.text} onChange={(event) => search.onText(event.target.value)} />}
      action={<Button icon={<RefreshCw aria-hidden />} pending={query.isFetching} onClick={() => void query.refetch()}>{t('sharedWithMe.refresh')}</Button>}>
      <Select<NodeKind | 'document_container' | 'document' | 'all'> label={t('sharedWithMe.kind')} value={kind} onValueChange={setKind} options={[{ value: 'all', ...choiceOption(t, 'statusFilter.any') }, { value: 'document', label: t('finder.kinds.document') }, { value: 'document_container', label: t('finder.kinds.document_container') }, ...KINDS.map((value) => ({ value, ...choiceOption(t, `placeKind.${value}`) }))]} />
    </FilterBar>
    {query.error !== null ? <ErrorState error={query.error} onRetry={() => void query.refetch()} /> : query.data === undefined ? <SkeletonRows read={query} rows={5} /> : rows.length === 0 ? <EmptyState icon={<Share2 aria-hidden />} title={t(search.q === '' && kind === 'all' && query.data.items.length === 0 && cursor === null && query.data.next_cursor === null ? 'sharedWithMe.empty' : 'sharedWithMe.noMatches')}
      action={search.q === '' && kind === 'all' && query.data.items.length === 0 && cursor === null && query.data.next_cursor === null ? <Button action="search" onClick={() => openFinder()}>{t('sharedWithMe.findPlaces')}</Button> : <Button onClick={() => { search.onText(''); setKind('all'); setCursor(null); }}>{t('sharedWithMe.clearFilters')}</Button>} /> : <Table label={t('screens.shared.title')} columns={columns} rows={rows} virtual rowKey={(row) => row.kind === 'document_container' ? `document_container:${row.folder.id}` : row.kind === 'document' ? `document:${row.document.id}` : `${row.kind}:${row.id}`} />}
    {query.data !== undefined && (cursor !== null || query.data.next_cursor !== null) && <div className="flex flex-wrap justify-between gap-2">
      <Button disabled={cursor === null} onClick={() => setCursor(null)}>{t('documentFolders.beginning')}</Button>
      <Button disabled={query.data.next_cursor === null} onClick={() => setCursor(query.data.next_cursor)}>{t('documentFolders.next')}</Button>
    </div>}
  </Page>;
}
