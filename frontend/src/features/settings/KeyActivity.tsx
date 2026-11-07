/** Bounded recent key calls: only the selected personal/org view reads, with native privacy intact. */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { orgTokenActivityQuery } from '@/api/directory';
import { tokenActivityQuery } from '@/api/users';
import { DETAIL_PAGE_ROWS } from '@/features/resources/detailPageSize';
import type { ApiTokenActivity } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { Table, type Column } from '@/ui/Table';

export function KeyActivity({ tokenId, orgId }: { tokenId: string; orgId?: string }) {
  // A different actual key/org owns a new cursor history, even when the surrounding dialog stays.
  return <ActivityReader key={JSON.stringify([orgId ?? null, tokenId])} tokenId={tokenId} orgId={orgId} />;
}

function ActivityOutcome({ row }: { row: ApiTokenActivity }) {
  const { t } = useTranslation();
  return <span className="inline-flex flex-wrap gap-x-2"><span>{t('operationKeys.activity.status', { status: row.status })}</span>{row.error_code !== null && <code className="wrap-anywhere">{row.error_code}</code>}{row.replayed && <span>{t('operationKeys.activity.replayed')}</span>}</span>;
}

function ActivityDetails({ row }: { row: ApiTokenActivity }) {
  const { t } = useTranslation();
  return <details className="min-w-0 text-sm"><summary className="cursor-default rounded-sm">{t('operationKeys.activity.details')}</summary>
    <div className="mt-2 flex min-w-0 flex-col gap-2">
      <ul>{row.facets.map((facet, index) => <li key={index} className="wrap-anywhere">{t(`operationKeys.action.${facet.action}`)} · {t(`operationKeys.kind.${facet.resource_kind}`)} · {facet.target_name ?? t('operationKeys.activity.undisclosedName')}{facet.target.id !== undefined && facet.target.id !== null && <> · <code>{facet.target.id}</code></>}</li>)}</ul>
      <p className="wrap-anywhere">{t('operationKeys.activity.requestId')} <code>{row.request_id}</code></p>
      <p>{t('operationKeys.activity.ip')} {row.ip ?? t('operationKeys.activity.undisclosedIp')}</p>
    </div>
  </details>;
}

function ActivityReader({ tokenId, orgId }: { tokenId: string; orgId: string | undefined }) {
  const { t } = useTranslation();
  const [cursors, setCursors] = useState<(string | null)[]>([null]);
  const [page, setPage] = useState(0);
  const query = { limit: DETAIL_PAGE_ROWS, cursor: cursors[page] ?? null };
  const list = useQuery(orgId === undefined ? tokenActivityQuery(tokenId, query) : orgTokenActivityQuery(orgId, tokenId, query));
  const busy = list.isFetching;
  const next = list.isError ? null : list.data?.next_cursor ?? null;
  const refresh = () => {
    setCursors([null]); setPage(0);
    if (page === 0) void list.refetch();
  };
  const columns: Column<ApiTokenActivity>[] = [
    { id: 'operation', header: t('operationKeys.activity.operation'), cell: (row, shown) => {
      const atShown=shown.has('at'),outcomeShown=shown.has('outcome'),durationShown=shown.has('duration');
      return <div className="flex min-w-0 flex-col gap-1">
      <code className="font-medium wrap-anywhere">{row.selection ?? row.operation}</code>
      <span className="text-sm wrap-anywhere text-text-muted">{row.method} {row.route}</span>
      {!atShown && <span className="text-sm text-text-muted"><RelativeTime at={row.at} /></span>}
      {!outcomeShown && <span className="text-sm text-text-muted"><ActivityOutcome row={row} /></span>}
      {!durationShown && <span className="text-sm text-text-muted">{t('operationKeys.activity.duration', { duration: row.duration_ms })}</span>}
      <ActivityDetails row={row} />
    </div>;
    } },
    { id: 'at', header: t('operationKeys.activity.time'), from: 'md', cell: (row) => <RelativeTime at={row.at} /> },
    { id: 'outcome', header: t('operationKeys.activity.outcome'), from: 'sm', cell: (row) => <ActivityOutcome row={row} /> },
    { id: 'duration', header: t('operationKeys.activity.durationLabel'), from: 'lg', cell: (row) => t('operationKeys.activity.duration', { duration: row.duration_ms }) },
  ];
  return <section aria-label={t('operationKeys.activity.title')} className="flex min-w-0 flex-col gap-3">
    <div className="flex flex-wrap items-center justify-between gap-2"><p className="max-w-prose text-sm text-text-muted">{t('operationKeys.activity.retention')}</p><Button size="sm" disabled={busy} onClick={refresh}>{t('operationKeys.activity.refresh')}</Button></div>
    {list.isPending || busy ? <SkeletonRows rows={3} /> : list.isError ? <ErrorState error={list.error} onRetry={() => void list.refetch()} />
      : list.data.items.length === 0 ? <EmptyState compact title={t('operationKeys.activity.none')} />
      : <Table label={t('operationKeys.activity.title')} columns={columns} rows={list.data.items} rowKey={(row) => row.id} />}
    <div className="flex flex-wrap items-center justify-between gap-2"><span className="text-sm text-text-muted">{t('operationKeys.activity.page', { page: page + 1 })}</span><div className="flex gap-2">
      <Button size="sm" disabled={busy || page === 0} onClick={() => setPage(page - 1)}>{t('resourceDetails.layout.previous')}</Button>
      <Button size="sm" disabled={busy || next === null} onClick={() => { if (next !== null) { setCursors([...cursors.slice(0, page + 1), next]); setPage(page + 1); } }}>{t('resourceDetails.layout.next')}</Button>
    </div></div>
  </section>;
}
