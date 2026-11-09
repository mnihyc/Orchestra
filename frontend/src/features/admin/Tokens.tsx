/** System and per-person credential views share the existing scope/activity readers. */
import { useInfiniteQuery, useMutation } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { adminTokensQuery, revokeAdminToken } from '@/api/adminReach';
import type { AdminToken } from '@/api/adminReachTypes';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import { ScopesCell, TokenPlace } from '@/features/settings/personal/Tokens';
import { KeyScopeReview } from '@/features/settings/KeyRules';
import { KeyActivity } from '@/features/settings/KeyActivity';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { RelativeTime } from '@/ui/RelativeTime';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { SkeletonRows } from '@/ui/Skeleton';
import { Table, type Column } from '@/ui/Table';
import { UserChip } from '@/ui/UserChip';
import { AsAdminMark } from '@/ui/AsAdminMark';

export function Tokens() { return <AdminTokens />; }
export function AdminTokens({ person }: { person?: string }) {
  const reader = useStore(client.session, (state) => JSON.stringify([state.userId, state.readerVersion]));
  return <TokenReader key={JSON.stringify([reader, person ?? null])} person={person} />;
}
function TokenReview({ token, onClose }: { token: AdminToken; onClose: () => void }) {
  const { t } = useTranslation();
  const [view, setView] = useState<'scope' | 'activity'>('scope');
  const activity = token.owner.kind === 'human';
  return <Dialog open title={token.info.name} size="lg" onOpenChange={(open) => { if (!open) onClose(); }} footer={<Button onClick={onClose}>{t('common.close')}</Button>}>
    <div className="flex min-w-0 flex-col gap-4"><div className="flex flex-wrap items-center gap-2"><UserChip user={token.owner} />{token.admin_access && <AsAdminMark of="system" />}</div>
      {activity && <SegmentedControl label={t('operationKeys.activity.view')} value={view} onValueChange={setView}
        segments={[{ value: 'scope', label: t('operationKeys.review') }, { value: 'activity', label: t('operationKeys.activity.title') }]} />}
      {view === 'activity' && activity ? <KeyActivity tokenId={token.info.id} adminUserId={token.owner.id} /> : <KeyScopeReview token={token.info} />}
    </div>
  </Dialog>;
}
function TokenReader({ person }: { person: string | undefined }) {
  const { t } = useTranslation();
  const list = useInfiniteQuery(adminTokensQuery(person));
  const rows = list.data?.pages.flatMap((page) => page.items) ?? [];
  const [reviewId, setReviewId] = useState<string | null>(null);
  const review = list.isError && !list.isFetchNextPageError ? undefined : rows.find((row) => row.info.id === reviewId);
  const [ending, setEnding] = useState<AdminToken | null>(null);
  const revoke = useMutation({
    mutationFn: ({ token }: { token: AdminToken; reader: ReturnType<typeof libraryReader> }) => revokeAdminToken(token.info.id, person),
    onSuccess: (_answer, asked) => {
      if (!sameLibraryReader(asked.reader)) return;
      setEnding(null);
    },
  });
  const times = (row: AdminToken, shown: ReadonlySet<string>) => <span className="flex flex-wrap gap-x-3 gap-y-1 text-sm text-text-muted">
    {!shown.has('created') && <span>{t('adminCredentials.created')} <RelativeTime at={row.info.created_at} /></span>}
    {!shown.has('used') && <span>{t('adminCredentials.used')} {row.info.last_used_at === null ? t('tokens.neverUsed') : <RelativeTime at={row.info.last_used_at} />}</span>}
    {!shown.has('expires') && <span>{t('adminCredentials.expires')} {row.info.expires_at === null ? t('tokens.neverEnds') : <RelativeTime at={row.info.expires_at} />}</span>}
  </span>;
  const columns: Column<AdminToken>[] = [
    { id: 'name', header: t('tokens.columns.name'), grow: true, cell: (row, shown) => <div className="flex min-w-0 flex-col items-start gap-1">
      <div className="flex max-w-full items-center gap-1"><Button variant="ghost" size="sm" className="max-w-full" title={row.info.name} onClick={() => setReviewId(row.info.id)}><span className="truncate">{row.info.name}</span></Button>{row.admin_access && <AsAdminMark of="system" />}</div>
      {!shown.has('owner') && <UserChip user={row.owner} />}
      {!shown.has('scope') && <ScopesCell token={row.info} compact />}
      {!shown.has('place') && <TokenPlace node={row.info.node} />}
      {(!shown.has('created') || !shown.has('used') || !shown.has('expires')) && times(row, shown)}
    </div> },
    { id: 'owner', header: t('storage.owner'), from: 'md', cell: (row) => <UserChip user={row.owner} /> },
    { id: 'scope', header: t('tokens.columns.mayDo'), from: 'sm', cell: (row) => <ScopesCell token={row.info} /> },
    { id: 'place', header: t('search.filters.place'), from: 'lg', cell: (row) => <TokenPlace node={row.info.node} /> },
    { id: 'created', header: t('adminCredentials.created'), from: 'lg', cell: (row) => <RelativeTime at={row.info.created_at} /> },
    { id: 'used', header: t('adminCredentials.used'), from: 'md', cell: (row) => row.info.last_used_at === null ? t('tokens.neverUsed') : <RelativeTime at={row.info.last_used_at} /> },
    { id: 'expires', header: t('adminCredentials.expires'), from: 'lg', cell: (row) => row.info.expires_at === null ? t('tokens.neverEnds') : <RelativeTime at={row.info.expires_at} /> },
    { id: 'act', header: t('tokens.revoke'), hideHeader: true, align: 'end', cell: (row) => <Button size="sm" variant="ghost" disabled={revoke.isPending} onClick={() => { revoke.reset(); setEnding(row); }}>{t('tokens.revoke')}</Button> },
  ];
  return <div className="flex min-w-0 flex-col gap-4">
    <div className="flex justify-end"><Button size="sm" disabled={list.isFetching} onClick={() => void list.refetch()}>{t('operationKeys.activity.refresh')}</Button></div>
    {list.isPending ? <SkeletonRows read={list} rows={4} /> : list.isError && !list.isFetchNextPageError ? <ErrorState error={list.error} onRetry={() => void list.refetch()} />
      : rows.length === 0 ? <EmptyState compact title={t('adminCredentials.noTokens')} /> : <Table label={t('admin.sections.tokens.title')} rows={rows} rowKey={(row) => row.info.id} columns={columns} />}
    <Pagination hasMore={list.hasNextPage} loading={list.isFetchingNextPage} error={list.isFetchNextPageError ? list.error : undefined} onMore={() => void list.fetchNextPage()} />
    {review !== undefined && <TokenReview token={review} onClose={() => setReviewId(null)} />}
    <ConfirmDialog open={ending !== null} onOpenChange={(open) => { if (!open && !revoke.isPending) setEnding(null); }} title={t('tokens.revokeTitle', { name: ending?.info.name ?? '' })}
      confirmLabel={t('tokens.revokeConfirm', { name: ending?.info.name ?? '' })} danger pending={revoke.isPending} error={revoke.error ?? undefined}
      onConfirm={() => { if (ending !== null) revoke.mutate({ token: ending, reader: libraryReader() }); }}>
      {t('adminCredentials.revokeToken', { name: ending?.info.name ?? '' })}
    </ConfirmDialog>
  </div>;
}
