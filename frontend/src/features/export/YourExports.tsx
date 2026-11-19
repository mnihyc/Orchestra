/** Your exports: the archives made for the person, what each is of, when it was asked for, how large it is and until when it is kept, with Download and Remove at the end of the row.
 *
 * An archive is its person's alone. One that is being written shows how far it has come and can be cancelled;
 * one that is ready is downloaded again as often as wanted until it is removed or its time is up; one that failed
 * says why; one that was removed or ran out of time stays in the list for a month, so that it is seen to have been
 * made. An archive whose place is gone is still listed, as the archive of a place that is gone.
 */
import { useInfiniteQuery, useMutation, useQuery } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { Download, Ellipsis, PackageOpen } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { jobQuery, myExportsQuery, removeExport } from '@/api/exports';
import { PlaceLink } from '@/features/navigation/places';
import { PersonalFrame } from '@/features/settings/PersonalFrame';
import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { ByteSize } from '@/ui/ByteSize';
import { cx } from '@/ui/cx';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { Meter } from '@/ui/Meter';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { type Column, Table } from '@/ui/Table';
import { fetchArchive, useDownloading } from './download';
import { dismissFinished, failureWords } from './jobs';
import type { Export as ExportRow } from '@/generated/api-types';

/** The columns whose words move into the first one when the room is narrow. */
const ASKED = 'asked';
const SIZE = 'size';
const STATE = 'state';

/** The kinds of thing an archive was asked to hold, when not all three. */
function kindsOf(row: ExportRow): ('conversations' | 'chats' | 'documents')[] {
  const parts = row.request.parts;
  const all = ['conversations', 'chats', 'documents'] as const;
  return all.filter((kind) => parts?.[kind] !== false);
}

/** What an archive is of, in a few words: all of the place or a choice in it, the kinds when not all, and the form. */
function summary(row: ExportRow, t: TFunction, locale: string): string {
  const kinds = kindsOf(row);
  const parts: string[] = [row.request.what === 'all' ? t('export.list.everything') : t('export.list.chosen')];
  const words = { conversations: t('export.include.conversations'), chats: t('export.include.chats'), documents: t('export.include.documents') };
  if (kinds.length < 3) parts.push(kinds.map((kind) => words[kind]).join(', '));
  parts.push(row.request.package === true ? t('choiceCatalog.exportFormat.package.name') : row.request.format === 'json' ? t('export.list.json') : t('export.list.markdown'));
  if (row.counts !== null) {
    const things = row.counts.conversations + row.counts.chats + row.counts.documents;
    parts.push(t('export.list.things', { count: things, formatted: formatNumber(things, locale) }));
    if (row.request.package === true ? row.request.files !== false : row.request.files === true) parts.push(t('export.list.attachments', { count: row.counts.files, formatted: formatNumber(row.counts.files, locale) }));
  }
  return new Intl.ListFormat(locale).format(parts);
}

/**
 * What an archive does not hold, in one line: the parts that were not ticked (the attachments, and for documents the comments
 * and the earlier versions), then what the server left out of what was asked for, as the archive's own view says it. Nothing
 * is said of a part that does not apply, and the line is empty when nothing was left out.
 */
function leftOutOf(row: ExportRow, t: TFunction, locale: string): string {
  const names: string[] = [];
  if (row.request.package === true ? row.request.files === false : row.request.files !== true) names.push(t('export.list.leftAttachments'));
  if (kindsOf(row).includes('documents')) {
    if (row.request.package === true ? row.request.comments === false : row.request.comments !== true) names.push(t('export.list.leftComments'));
    if (row.request.versions !== true) names.push(t('export.list.leftVersions'));
  }
  const counted = (what: 'not_exportable' | 'too_large') => row.left_out.reduce((sum, one) => (one.what === what ? sum + one.count : sum), 0);
  const notExportable = counted('not_exportable');
  if (notExportable > 0) names.push(t('export.list.leftNotExportable', { count: notExportable, formatted: formatNumber(notExportable, locale) }));
  const tooLarge = counted('too_large');
  if (tooLarge > 0) names.push(t('export.list.leftTooLarge', { count: tooLarge, formatted: formatNumber(tooLarge, locale) }));
  if (names.length === 0) return '';
  return t('export.list.leftOut', { what: new Intl.ListFormat(locale, { style: 'narrow', type: 'conjunction' }).format(names) });
}

/** How an archive that is no longer there ended: nobody had cancelled it, it was removed before its time, or its time was up. */
function endedAs(row: ExportRow, now: number): 'cancelled' | 'removed' | 'expired' {
  if (row.ready_at === null) return 'cancelled';
  return row.expires_at !== null && Date.parse(row.expires_at) > now ? 'removed' : 'expired';
}

/** How far a job has come, looked at again and again while it runs. */
function Writing({ row }: { row: ExportRow }) {
  const { t } = useTranslation();
  const job = useQuery({ ...jobQuery(row.job_id ?? ''), enabled: row.job_id !== null });
  const percent = job.data?.progress.percent ?? 0;
  const phase = job.data?.progress.phase ?? '';
  const phaseKey = phase === 'select' || phase === 'write' || phase === 'store' ? phase : 'write';
  return (
    <div className="flex min-w-[8rem] flex-col gap-1">
      <span className="text-sm text-text">{phase === '' ? t('export.list.preparing') : t(`export.job.phase.${phaseKey}`)}</span>
      <Meter look="progress" value={percent} label={t('export.list.progress', { percent })} />
    </div>
  );
}

/** Why an archive failed, as its job says. */
function Failure({ row }: { row: ExportRow }) {
  const { t } = useTranslation();
  const job = useQuery({ ...jobQuery(row.job_id ?? ''), enabled: row.job_id !== null, retry: false });
  return (
    <div className="flex flex-col">
      <span className="text-sm font-medium text-danger-text">{t('export.list.failed')}</span>
      {job.data !== undefined && <span className="text-sm text-text-muted">{failureWords(job.data.error, t)}</span>}
    </div>
  );
}

function StateCell({ row }: { row: ExportRow }) {
  const { t } = useTranslation();
  const [now] = useState(() => Date.now());
  if (row.state === 'pending') return <Writing row={row} />;
  if (row.state === 'failed') return <Failure row={row} />;
  if (row.state === 'ready') return <span className="text-sm text-text">{t('export.list.ready')}</span>;
  return <span className="text-sm text-text-muted">{t(`export.list.${endedAs(row, now)}`)}</span>;
}

function DownloadButton({ row }: { row: ExportRow }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const active = useDownloading(row.id);
  const total = active?.total ?? null;
  const percent = active !== null && total !== null ? Math.floor((active.received / total) * 100) : null;
  return (
    <span className="inline-flex flex-wrap items-center gap-2"><Button
      action="download"
      size="sm"
      variant="secondary"
      icon={<Download aria-hidden />}
      pending={active !== null}
      aria-label={t('export.list.downloadOf', { name: placeName(row, t) })}
      onClick={() => void fetchArchive(row.id)}
    >
      {t('export.downloadButton')}
    </Button>{active !== null && <span role="status" className="text-xs text-text-muted">{percent === null ? t('export.list.downloading') : t('export.list.downloadingAt', { percent: formatNumber(percent, locale) })}</span>}</span>
  );
}

/** What the archive's place is called: its name, or that it is gone or no longer one the person may open. */
function placeName(row: ExportRow, t: TFunction): string {
  if (row.node === null) return t('export.list.placeGone');
  return row.node.redacted || row.node.name === null ? t('export.list.placeHidden') : row.node.name;
}

export function YourExports() {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  // The person has come to see what ended: the status bar need not say it any more.
  useEffect(() => dismissFinished(), []);

  const list = useInfiniteQuery({
    ...myExportsQuery,
    // While an archive is being written the list is asked again, so that its end is seen.
  });
  const rows = list.data?.pages.flatMap((page) => page.items) ?? [];

  const [removing, setRemoving] = useState<ExportRow | null>(null);
  const remove = useMutation({
    mutationFn: (id: string) => removeExport(id),
    onSuccess: () => {
      setRemoving(null);
    },
  });

  const entries = (row: ExportRow): MenuEntry[] => {
    if (row.state === 'ready') {
      return [
        { id: 'download', label: t('export.downloadButton'), icon: <Download aria-hidden />, onSelect: () => void fetchArchive(row.id) },
        { type: 'separator', id: 'before-remove' },
        { id: 'remove', label: t('export.list.remove'), danger: true, onSelect: () => setRemoving(row) },
      ];
    }
    if (row.state === 'pending') return [{ id: 'cancel', label: t('export.list.cancel'), danger: true, onSelect: () => setRemoving(row) }];
    if (row.state === 'failed') return [{ id: 'remove', label: t('export.list.remove'), danger: true, onSelect: () => setRemoving(row) }];
    return [];
  };

  const columns: Column<ExportRow>[] = [
    {
      id: 'what',
      header: t('export.list.what'),
      cell: (row, shown) => {
        const left = leftOutOf(row, t, locale);
        return (
          <div className={cx('flex flex-col', shown.has(STATE) ? 'min-w-[15rem]' : 'min-w-0')}>
            {row.node !== null && !row.node.redacted && row.node.name !== null ? (
              <PlaceLink place={{ id: row.node.id, kind: row.node.kind }} className="truncate font-medium text-text hover:underline">
                {row.node.name}
              </PlaceLink>
            ) : (
              <span className="truncate font-medium text-text-muted">{placeName(row, t)}</span>
            )}
            <span className="text-sm text-text-muted">{summary(row, t, locale)}</span>
            {left !== '' && <span className="text-sm text-text-muted">{left}</span>}
            {!shown.has(STATE) && (
              <div className="py-1">
                <StateCell row={row} />
              </div>
            )}
            {!shown.has(ASKED) && (
              <span className="text-sm text-text-muted">
                <RelativeTime at={row.created_at} />
                {row.bytes !== null && !shown.has(SIZE) && (
                  <>
                    {' · '}
                    <ByteSize bytes={row.bytes} />
                  </>
                )}
              </span>
            )}
          </div>
        );
      },
    },
    { id: ASKED, header: t('export.list.asked'), from: 'lg', className: 'whitespace-nowrap text-text-muted', cell: (row) => <RelativeTime at={row.created_at} /> },
    {
      id: SIZE,
      header: t('export.list.size'),
      from: 'sm',
      className: 'whitespace-nowrap text-text-muted',
      cell: (row) => (row.bytes === null ? <span aria-label={t('export.list.noSize')}>—</span> : <ByteSize bytes={row.bytes} />),
    },
    {
      id: 'until',
      header: t('export.list.until'),
      from: 'lg',
      className: 'whitespace-nowrap text-text-muted',
      cell: (row) => (row.state === 'ready' && row.expires_at !== null ? <RelativeTime at={row.expires_at} /> : <span aria-label={t('export.list.noEnd')}>—</span>),
    },
    { id: STATE, header: t('export.list.state'), from: 'sm', cell: (row) => <StateCell row={row} /> },
    {
      id: 'acts',
      header: t('export.list.acts'),
      hideHeader: true,
      align: 'end',
      className: 'w-px whitespace-nowrap',
      cell: (row) => {
        const own = entries(row);
        return (
          <div className="flex items-center justify-end gap-1">
            {row.state === 'ready' && <DownloadButton row={row} />}
            {own.length > 0 && (
              <Menu
                label={t('export.list.menuOf', { name: placeName(row, t) })}
                align="end"
                entries={own}
                trigger={<IconButton size="sm" label={t('export.list.menuOf', { name: placeName(row, t) })} icon={<Ellipsis aria-hidden />} />}
              />
            )}
          </div>
        );
      },
    },
  ];

  const removingName = removing === null ? '' : placeName(removing, t);
  return (
    <PersonalFrame page="exports" lead={t('export.page.lead')}>
      {list.isPending ? (
        <SkeletonRows rows={4} />
      ) : list.isError && rows.length === 0 ? (
        <ErrorState compact error={list.error} retrying={list.isRefetching} onRetry={() => void list.refetch()} />
      ) : rows.length === 0 ? (
        <EmptyState icon={<PackageOpen aria-hidden />} title={t('export.page.empty')} description={t('export.page.emptyAbout')} />
      ) : (
        <>
          <Table
            label={t('export.page.title')}
            columns={columns}
            rows={rows}
            rowKey={(row) => row.id}
            rowMenu={{ label: (row) => t('export.list.menuOf', { name: placeName(row, t) }), entries }}
          />
          {list.hasNextPage && (
            <div className="flex justify-center">
              <Button variant="secondary" pending={list.isFetchingNextPage} onClick={() => void list.fetchNextPage()}>
                {t('export.page.more')}
              </Button>
            </div>
          )}
        </>
      )}
      <ConfirmDialog
        open={removing !== null}
        onOpenChange={(open) => !open && !remove.isPending && setRemoving(null)}
        title={removing?.state === 'pending' ? t('export.list.cancelTitle', { name: removingName }) : t('export.list.removeTitle', { name: removingName })}
        confirmLabel={removing?.state === 'pending' ? t('export.list.cancelDo', { name: removingName }) : t('export.list.removeDo', { name: removingName })}
        danger
        pending={remove.isPending}
        error={remove.error}
        onConfirm={() => {
          if (removing === null) return;
          remove.mutate(removing.id);
        }}
      >
        {removing?.state === 'pending' ? t('export.list.cancelAbout') : t('export.list.removeAbout')}
      </ConfirmDialog>
    </PersonalFrame>
  );
}
