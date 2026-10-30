/** Your imports keeps the person's jobs and reports, with the same history layout as exports. */
import { useInfiniteQuery, useQuery, useQueryClient } from '@tanstack/react-query';
import { Ellipsis, FileUp } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { jobQuery } from '@/api/exports';
import { importFileNamesQuery, importKeys, myImportsQuery } from '@/api/imports';
import { PlaceLink } from '@/features/navigation/places';
import { openImport, openImportJob, openImportReport } from '@/features/people/importHostState';
import { watchImport } from '@/features/people/importJobs';
import { importActionSummary } from '@/features/people/importResults';
import { ImportUndo } from '@/features/people/ImportUndo';
import { PersonalFrame } from '@/features/settings/PersonalFrame';
import type { ImportLine as ImportRow } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { ImportPagination } from '@/features/people/ImportPagination';
import { Popover } from '@/ui/Popover';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { StatusChip } from '@/ui/StateChip';
import { Table, type Column } from '@/ui/Table';

/** A page of source names keeps long folders usable without dropping their remaining names. */
const SOURCE_NAMES_PAGE = 40;
const FROM_COLUMN = 'from';
const STATE_COLUMN = 'state';
const WHEN_COLUMN = 'when';

function SourceNames({ row }: { row: ImportRow }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const [open, setOpen] = useState(false);
  const first = row.first_file;
  if (first === null) return <span>–</span>;
  return <span className="inline-flex min-w-0 flex-wrap items-center gap-1">
    <span className="max-w-full truncate" title={first}>{first}</span>
    {row.file_count > 1 && <Popover label={t('importWizard.mine.files')} onOpenChange={setOpen} trigger={<Button size="sm" variant="ghost">{t('importWizard.mine.moreFiles', { number: formatNumber(row.file_count - 1, locale) })}</Button>}>
      {open && <SourceNamePages id={row.id} />}
    </Popover>}
  </span>;
}

/** The names, a page at a time from the server: a folder of many files is never sent whole with the list. */
function SourceNamePages({ id }: { id: string }) {
  const cache = useQueryClient();
  const options = importFileNamesQuery(id, SOURCE_NAMES_PAGE);
  const names = useInfiniteQuery({ ...options, maxPages: 1 });
  const page = names.data?.pages[0];
  if (page === undefined) return names.isError ? <ErrorState compact error={names.error} retrying={names.isRefetching} onRetry={() => void names.refetch()} /> : <SkeletonRows rows={3} />;
  return <>
    <ul className="flex min-w-0 flex-col gap-1">{page.items.map((name, index) => <li key={`${name}:${index}`} className="break-all text-sm">{name}</li>)}</ul>
    <ImportPagination first={names.data?.pageParams[0] !== null && names.data?.pageParams[0] !== undefined} more={names.hasNextPage} loading={names.isFetching}
      error={names.isFetchNextPageError ? names.error : null} onMore={() => void names.fetchNextPage()} onFirst={() => void cache.resetQueries({ queryKey: options.queryKey, exact: true })} />
  </>;
}

function ImportStateCell({ row }: { row: ImportRow }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const jobId = row.state === 'undoing' ? row.undo_job_id : row.job_id;
  const job = useQuery({ ...jobQuery(jobId ?? ''), enabled: jobId !== null && (row.state === 'pending' || row.state === 'undoing') });
  const state = t(`importWizard.report.${row.state}`);
  const percent = job.data?.progress.percent;
  return <StatusChip tone={row.state === 'failed' ? 'danger' : row.state === 'done' || row.state === 'undone' ? 'success' : 'neutral'}>
    {state}{percent === undefined ? '' : ` · ${formatNumber(percent, locale)}%`}
  </StatusChip>;
}

export function YourImports() {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const cache = useQueryClient();
  const reader = useStore(client.session, (session) => `${session.userId ?? ''}:${session.readerVersion ?? 0}`);
  const user = useStore(client.session, (session) => session.userId);
  const [undo, setUndo] = useState<{ row: ImportRow; reader: string } | null>(null);
  const listOptions = myImportsQuery();
  const list = useInfiniteQuery({ ...listOptions, maxPages: 1, enabled: user !== null });
  const rows = list.data?.pages.flatMap((page) => page.items) ?? [];
  const name = (row: ImportRow) => row.node === null ? t('importWizard.mine.gone') : row.node.redacted || row.node.name === null ? t('importWizard.mine.hidden') : row.node.name;
  const open = (row: ImportRow) => {
    const job = row.state === 'undoing' ? row.undo_job_id : row.job_id;
    if ((row.state === 'pending' || row.state === 'undoing') && job !== null) openImportJob(job, row.node?.id ?? null);
    else openImportReport(row.id);
  };
  const entries = (row: ImportRow): MenuEntry[] => [
    { id: 'report', label: t(row.state === 'pending' || row.state === 'undoing' ? 'importWizard.mine.progress' : 'importWizard.mine.report'), onSelect: () => open(row) },
    ...((row.state === 'done' || row.state === 'failed' || row.state === 'cancelled') ? [{ id: 'undo', label: t('importWizard.undo.action'), danger: true, onSelect: () => setUndo({ row, reader }) }] : []),
  ];
  const columns: Column<ImportRow>[] = [
    { id: 'name', header: t('importWizard.mine.place'), cell: (row, shown) => <div className="flex min-w-0 flex-col gap-1 py-1">
      {row.node !== null && !row.node.redacted && row.node.name !== null ? <PlaceLink place={{ id: row.node.id, kind: row.node.kind }} className="truncate font-medium text-text hover:underline">{row.node.name}</PlaceLink> : <span className="truncate text-text-muted">{name(row)}</span>}
      {!shown.has(FROM_COLUMN) && <SourceNames row={row} />}
      <span className="text-sm text-text-muted">{importActionSummary(row.actions, t, locale)}</span>
      {!shown.has(STATE_COLUMN) && <ImportStateCell row={row} />}
      {!shown.has(WHEN_COLUMN) && <span className="text-sm text-text-muted"><RelativeTime at={row.created_at} /></span>}
    </div> },
    { id: FROM_COLUMN, header: t('importWizard.mine.from'), from: 'md', cell: (row) => <SourceNames row={row} /> },
    { id: WHEN_COLUMN, header: t('importWizard.mine.when'), from: 'lg', cell: (row) => <RelativeTime at={row.created_at} /> },
    { id: STATE_COLUMN, header: t('importWizard.mine.state'), from: 'sm', cell: (row) => <ImportStateCell row={row} /> },
    { id: 'acts', header: t('importWizard.mine.report'), hideHeader: true, align: 'end', cell: (row) => <div className="flex items-center gap-1">
      <Button size="sm" aria-label={t(row.state === 'pending' || row.state === 'undoing' ? 'importWizard.mine.progressFor' : 'importWizard.mine.reportFor', { name: name(row) })} onClick={() => open(row)}>{t(row.state === 'pending' || row.state === 'undoing' ? 'importWizard.mine.progress' : 'importWizard.mine.report')}</Button>
      <Menu label={t('importWizard.mine.menuFor', { name: name(row) })} entries={entries(row)} trigger={<IconButton size="sm" label={t('importWizard.mine.menuFor', { name: name(row) })} icon={<Ellipsis aria-hidden />} />} />
    </div> },
  ];
  const make = <Button icon={<FileUp aria-hidden />} onClick={() => openImport()}>{t('importWizard.mine.import')}</Button>;
  return <PersonalFrame page="imports" lead={t('importWizard.mine.lead')}>
    <div className="flex min-w-0 flex-col gap-3">
      {rows.length > 0 && <div className="flex justify-end">{make}</div>}
      {list.isPending ? <SkeletonRows rows={4} /> : list.isError && rows.length === 0 ? <ErrorState compact error={list.error} retrying={list.isRefetching} onRetry={() => void list.refetch()} />
        : rows.length === 0 ? <EmptyState icon={<FileUp aria-hidden />} title={t('importWizard.mine.empty')} action={make} />
        : <Table label={t('importWizard.mine.title')} columns={columns} rows={rows} rowKey={(row) => row.id} boundedIdentity rowMenu={{ label: (row) => t('importWizard.mine.menuFor', { name: name(row) }), entries }} />}
      <ImportPagination first={(list.data?.pageParams[0] !== null && list.data?.pageParams[0] !== undefined)} more={list.hasNextPage} loading={list.isFetching}
        error={list.isFetchNextPageError ? list.error : null} onMore={() => void list.fetchNextPage()} onFirst={() => void cache.resetQueries({ queryKey: listOptions.queryKey, exact: true })} />
    </div>
    {undo !== null && undo.reader === reader && user !== null && <ImportUndo key={reader} id={undo.row.id} onClose={() => setUndo(null)} onStarted={(job) => {
      watchImport(job, undo.row.node?.id ?? null, false, true, undo.row.id);
      void cache.invalidateQueries({ queryKey: importKeys.one(undo.row.id), exact: true });
      void cache.invalidateQueries({ queryKey: importKeys.mine(), exact: true });
    }} />}
  </PersonalFrame>;
}
