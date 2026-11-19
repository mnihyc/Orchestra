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
  const [cursor, setCursor] = useState<string | null>(null);
  const query = useQuery(sharedPageQuery(cursor));
  const [text, setText] = useState('');
  const [kind, setKind] = useListChoice<NodeKind | 'document_container' | 'document' | 'all'>('shared', 'kind', 'all');
  const needle = text.trim().toLocaleLowerCase();
  const rows = (query.data?.items ?? []).filter((row) => {
    const name = row.kind === 'document_container' ? row.folder.name ?? '' : row.kind === 'document' ? row.document.title : row.name;
    const description = row.kind === 'document_container' || row.kind === 'document' ? '' : row.description;
    return (kind === 'all' || row.kind === kind) && (needle === '' || [name, description, ...row.path.flatMap((part) => part.redacted || part.name === null ? [] : [part.name])].some((part) => part.toLocaleLowerCase().includes(needle)));
  });
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
    <FilterBar search={<Input type="search" label={t('documentFolders.filterPage')} value={text} onChange={(event) => setText(event.target.value)} />}
      action={<Button icon={<RefreshCw aria-hidden />} pending={query.isFetching} onClick={() => void query.refetch()}>{t('sharedWithMe.refresh')}</Button>}>
      <Select<NodeKind | 'document_container' | 'document' | 'all'> label={t('sharedWithMe.kind')} value={kind} onValueChange={setKind} options={[{ value: 'all', ...choiceOption(t, 'statusFilter.any') }, { value: 'document', label: t('finder.kinds.document') }, { value: 'document_container', label: t('finder.kinds.document_container') }, ...KINDS.map((value) => ({ value, ...choiceOption(t, `placeKind.${value}`) }))]} />
    </FilterBar>
    {query.error !== null ? <ErrorState error={query.error} onRetry={() => void query.refetch()} /> : query.data === undefined ? <SkeletonRows rows={5} /> : rows.length === 0 ? <EmptyState icon={<Share2 aria-hidden />} title={t(query.data.items.length === 0 && cursor === null && query.data.next_cursor === null ? 'sharedWithMe.empty' : 'sharedWithMe.noMatches')}
      action={query.data.items.length === 0 && cursor === null && query.data.next_cursor === null ? <Button action="search" onClick={() => openFinder()}>{t('sharedWithMe.findPlaces')}</Button> : <Button onClick={() => { setText(''); setKind('all'); }}>{t('sharedWithMe.clearFilters')}</Button>} /> : <Table label={t('screens.shared.title')} columns={columns} rows={rows} rowKey={(row) => row.kind === 'document_container' ? `document_container:${row.folder.id}` : row.kind === 'document' ? `document:${row.document.id}` : `${row.kind}:${row.id}`} />}
    {query.data !== undefined && (cursor !== null || query.data.next_cursor !== null) && <div className="flex flex-wrap justify-between gap-2">
      <Button disabled={cursor === null} onClick={() => setCursor(null)}>{t('documentFolders.beginning')}</Button>
      <Button disabled={query.data.next_cursor === null} onClick={() => setCursor(query.data.next_cursor)}>{t('documentFolders.next')}</Button>
    </div>}
  </Page>;
}
