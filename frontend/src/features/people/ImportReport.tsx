/** A retained report lists every outcome and offers download and a planned undo. */
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Download } from 'lucide-react';
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { downloadImportReport, importKeys, importQuery, importReportQuery } from '@/api/imports';
import { client } from '@/api/client';
import { isPlainClick } from '@/features/chat/open';
import { canSaveImportReport, chooseImportReportFile } from '@/features/people/importReportFile';
import { ImportPagination } from '@/features/people/ImportPagination';
import { ImportReference } from '@/features/people/ImportReference';
import { ImportReasons } from '@/features/people/ImportReasons';
import { PlaceLink } from '@/features/navigation/places';
import { importReason } from '@/features/people/importReason';
import { importActionSummary } from '@/features/people/importResults';
import { watchImport } from '@/features/people/importJobs';
import { ImportUndo } from '@/features/people/ImportUndo';
import type { ImportOutcome, ImportReportFormat, ImportReportListQuery, ImportReportRow, ImportState } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { formatBytes } from '@/lib/format';
import { useViewChoice } from '@/lib/viewChoices';
import { Button } from '@/ui/Button';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Menu } from '@/ui/Menu';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { StatusChip } from '@/ui/StateChip';
import { Table, type Column } from '@/ui/Table';

type ReportFilter = 'all' | 'failed' | 'warnings' | 'skipped';
const REPORT_FILTERS: readonly ReportFilter[] = ['all', 'failed', 'warnings', 'skipped'];
const OUTCOME_WORDS: Record<ImportOutcome, 'created' | 'kept_both' | 'replaced' | 'unchanged' | 'skippedResult' | 'failedResult'> = {
  created: 'created', kept_both: 'kept_both', replaced: 'replaced', unchanged: 'unchanged', skipped: 'skippedResult', failed: 'failedResult',
};
const STATE_WORDS: Record<ImportState, 'pending' | 'done' | 'failed' | 'cancelled' | 'undoing' | 'undone'> = {
  pending: 'pending', done: 'done', failed: 'failed', cancelled: 'cancelled', undoing: 'undoing', undone: 'undone',
};

function reportFilter(filter: ReportFilter): ImportReportListQuery {
  if (filter === 'warnings') return { warned: true };
  if (filter === 'failed' || filter === 'skipped') return { outcome: filter };
  return {};
}

export function ImportReport({ id, onOpen }: { id: string; onOpen?: () => void }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const cache = useQueryClient();
  const owner = useRef({ user: client.session.getState().userId, reader: client.session.getState().readerVersion ?? 0 });
  const owns = () => {
    const session = client.session.getState();
    return session.userId === owner.current.user && (session.readerVersion ?? 0) === owner.current.reader;
  };
  const [savedFilter, rememberFilter] = useViewChoice<string>('import-report', 'filter', 'all', id);
  const [filter, setFilter] = useState<ReportFilter>(() => REPORT_FILTERS.includes(savedFilter as ReportFilter) ? savedFilter as ReportFilter : 'all');
  const [undoing, setUndoing] = useState(false);
  const [received, setReceived] = useState<number | null>(null);
  const record = useQuery(importQuery(id));
  const reportOptions = importReportQuery(id, reportFilter(filter));
  const report = useInfiniteQuery({ ...reportOptions, maxPages: 1, enabled: record.data !== undefined && record.data.state !== 'pending' });
  const download = useMutation({
    mutationFn: async (format: ImportReportFormat) => {
      const file = await chooseImportReportFile(format);
      if (file === null) return;
      try {
        if (!owns()) { await file.abort(); return; }
        await downloadImportReport(id, format, (progress) => { if (owns()) setReceived(progress.received); }, async (bytes) => {
          if (!owns()) throw new DOMException('Import reader changed', 'AbortError');
          await file.write(bytes);
        });
        if (owns()) await file.close();
        else await file.abort();
      } catch (error) { await file.abort().catch(() => undefined); throw error; }
    },
    onSettled: () => { if (owns()) setReceived(null); },
  });
  if (record.isPending) return <SkeletonRows rows={4} />;
  if (record.isError) return <ErrorState compact error={record.error} onRetry={() => void record.refetch()} />;
  const item = record.data;
  const columns: Column<ImportReportRow>[] = [
    { id: 'name', header: t('importWizard.report.item'), cell: (row) => <div className="flex min-w-0 flex-col gap-1 py-1">
      {row.made !== null ? <ImportReference id={row.made.id} kind={row.made.kind} name={row.name} onOpen={onOpen} /> : <span className="truncate" title={row.name}>{row.name}</span>}
      {row.path.length > 0 && <span className="truncate text-sm text-text-muted" title={row.path.join(' / ')}>{row.path.join(' / ')}</span>}
      {row.author !== null && item.request.options.authors !== 'accounts' && <span className="text-sm text-text-muted">{t(item.request.options.authors === 'labels' ? 'importWizard.report.originallyBy' : 'importWizard.report.sourceAuthor', { name: row.author })}</span>}
      {(row.message !== null || row.code !== null && row.code !== undefined) && <span className="text-sm text-text-muted">{row.code !== null && row.code !== undefined ? importReason({ code: row.code, message: row.message ?? '', params: row.params }, t, locale) : row.message}</span>}
      <ImportReasons reasons={row.warnings} label={t('importWizard.report.warnings')} />
    </div> },
    { id: 'result', header: t('importWizard.report.result'), cell: (row) => <StatusChip tone={row.outcome === 'failed' ? 'danger' : row.outcome === 'created' || row.outcome === 'replaced' || row.outcome === 'kept_both' ? 'success' : 'neutral'}>{t(`importWizard.report.${OUTCOME_WORDS[row.outcome]}`)}</StatusChip> },
  ];
  const rows = report.data?.pages.flatMap((page) => page.items) ?? [];
  return <div className="flex min-w-0 flex-col gap-4">
    <p className="text-sm text-text-muted tabular-nums">{t(`importWizard.report.${STATE_WORDS[item.state]}`)} · {importActionSummary(item.actions, t, locale)}</p>
    {item.failure !== null && <ErrorAlert error={item.failure} message={item.failure.code === 'internal' ? item.failure.message : importReason(item.failure, t, locale)} />}
    <ImportReasons reasons={item.losses} label={t('importWizard.review.notImported')} />
    <div className="flex flex-wrap items-end gap-2">
      <Select<ReportFilter> size="sm" label={t('importWizard.report.filter')} value={filter} onValueChange={(next) => { setFilter(next); rememberFilter(next); }} options={[
        { value: 'all', label: t('importWizard.report.all') }, { value: 'failed', label: t('importWizard.report.failed') },
        { value: 'warnings', label: t('importWizard.report.warnings') }, { value: 'skipped', label: t('importWizard.report.skipped') },
      ]} />
      <Menu label={t('importWizard.report.download')} trigger={<Button icon={<Download aria-hidden />} pending={download.isPending}>{t('importWizard.report.download')}</Button>}
        entries={[
          { id: 'json', label: t('importWizard.report.downloadJson'), disabled: canSaveImportReport() ? false : t('importWizard.report.directSaveNeeded'), onSelect: () => download.mutate('json') },
          { id: 'csv', label: t('importWizard.report.downloadCsv'), disabled: canSaveImportReport() ? false : t('importWizard.report.directSaveNeeded'), onSelect: () => download.mutate('csv') },
        ]} />
      {item.state !== 'undone' && item.node !== null && !item.node.redacted && item.node.name !== null && <Button asChild><PlaceLink place={{ id: item.node.id, kind: item.node.kind }} onClick={(event) => { if (isPlainClick(event)) onOpen?.(); }}>{t('importWizard.report.openPlace', { place: item.node.name })}</PlaceLink></Button>}
      {(item.state === 'done' || item.state === 'failed' || item.state === 'cancelled') && <Button variant="danger-ghost" onClick={() => setUndoing(true)}>{t('importWizard.undo.action')}</Button>}
    </div>
    {received !== null && <p role="status" className="text-sm text-text-muted tabular-nums">{t('importWizard.report.downloading', { bytes: formatBytes(received, locale) })}</p>}
    {download.isError && <ErrorAlert error={download.error} />}
    {item.state === 'pending' ? <StateLine compact message={t('importWizard.report.preparing')} /> : report.isPending ? <SkeletonRows rows={4} />
      : report.isError && rows.length === 0 ? <ErrorState compact error={report.error} onRetry={() => void report.refetch()} />
      : rows.length === 0 ? <StateLine compact message={t('importWizard.report.empty')} />
      : <Table key={filter} label={t('importWizard.report.title')} columns={columns} rows={rows} rowKey={(row) => String(row.position)} boundedIdentity virtual />}
    <ImportPagination first={(report.data?.pageParams[0] !== null && report.data?.pageParams[0] !== undefined)} more={report.hasNextPage} loading={report.isFetching}
      error={report.isFetchNextPageError ? report.error : null} onMore={() => void report.fetchNextPage()} onFirst={() => void cache.resetQueries({ queryKey: reportOptions.queryKey, exact: true })} />
    {undoing && <ImportUndo id={id} onClose={() => setUndoing(false)} onStarted={(job) => {
      watchImport(job, item.node?.id ?? null, false, true, id);
      void cache.invalidateQueries({ queryKey: importKeys.one(id), exact: true });
      void cache.invalidateQueries({ queryKey: importKeys.mine(), exact: true });
    }} />}
  </div>;
}
