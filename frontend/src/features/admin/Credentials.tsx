/** A person's native sessions and tokens remain their own administrative operations. */
import { useInfiniteQuery, useMutation } from '@tanstack/react-query';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { adminSessionsQuery, revokeAdminSession } from '@/api/adminReach';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import type { AdminSession } from '@/api/adminReachTypes';
import type { AdminUser } from '@/generated/api-types';
import { AdminTokens } from './Tokens';
import { browserName } from '@/features/settings/personal/Security';
import { AsAdminMark } from '@/ui/AsAdminMark';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { StatusTag } from '@/features/admin/StatusTag';
import { Table, type Column } from '@/ui/Table';
import { UserChip } from '@/ui/UserChip';

export function PersonCredentials({ user, kind, onClose }: { user: AdminUser; kind: 'sessions' | 'tokens'; onClose: () => void }) {
  const { t } = useTranslation();
  return <Dialog open title={t(kind === 'tokens' ? 'admin.sections.tokens.title' : 'adminCredentials.sessions')} size="lg" onOpenChange={(open) => { if (!open) onClose(); }} footer={<Button onClick={onClose}>{t('common.close')}</Button>}>
    <div className="flex min-w-0 flex-col gap-4"><UserChip user={user} />{kind === 'tokens' ? <AdminTokens person={user.id} /> : <AdminSessions person={user.id} />}</div>
  </Dialog>;
}
export function AdminSessions({ person }: { person: string }) {
  const reader = useStore(client.session, (state) => JSON.stringify([state.userId, state.readerVersion]));
  return <SessionsReader key={`${person}:${reader}`} person={person} />;
}
function SessionsReader({ person }: { person: string }) {
  const { t } = useTranslation();
  const list = useInfiniteQuery(adminSessionsQuery(person));
  const rows = list.data?.pages.flatMap((page) => page.items) ?? [];
  const [ending, setEnding] = useState<AdminSession | 'all' | null>(null);
  const revoke = useMutation({
    mutationFn: ({ session }: { session?: string; reader: ReturnType<typeof libraryReader> }) => revokeAdminSession(person, session),
    onSuccess: (_answer, asked) => {
      if (!sameLibraryReader(asked.reader)) return;
      setEnding(null);
    },
  });
  const columns: Column<AdminSession>[] = [
    { id: 'browser', header: t('adminCredentials.browser'), grow: true, cell: (row, shown) => <div className="flex min-w-0 flex-col gap-1">
      <span className="flex min-w-0 flex-wrap items-center gap-1"><span className="truncate" title={row.user_agent}>{browserName(row.user_agent) || t('security.sessions.unknown')}</span>{row.current && <StatusTag tone="accent">{t('security.sessions.thisOne')}</StatusTag>}{row.admin_access && <AsAdminMark of="system" />}</span>
      {!shown.has('ip') && <span className="text-sm text-text-muted">{row.ip ?? t('security.sessions.notReported')}</span>}
      {!shown.has('created') && <span className="text-sm text-text-muted">{t('adminCredentials.created')} <RelativeTime at={row.created_at} /></span>}
      {!shown.has('used') && <span className="text-sm text-text-muted">{t('adminCredentials.used')} <RelativeTime at={row.last_used_at} /></span>}
      {!shown.has('expires') && <span className="text-sm text-text-muted">{t('adminCredentials.expires')} <RelativeTime at={row.expires_at} /></span>}
    </div> },
    { id: 'ip', header: t('operationKeys.activity.ip'), from: 'md', cell: (row) => row.ip ?? t('security.sessions.notReported') },
    { id: 'created', header: t('adminCredentials.created'), from: 'lg', cell: (row) => <RelativeTime at={row.created_at} /> },
    { id: 'used', header: t('adminCredentials.used'), from: 'sm', cell: (row) => <RelativeTime at={row.last_used_at} /> },
    { id: 'expires', header: t('adminCredentials.expires'), from: 'lg', cell: (row) => <RelativeTime at={row.expires_at} /> },
    { id: 'act', header: t('security.sessions.end'), hideHeader: true, align: 'end', cell: (row) => <Button size="sm" variant="ghost" disabled={revoke.isPending} onClick={() => { revoke.reset(); setEnding(row); }}>{t('security.sessions.end')}</Button> },
  ];
  return <div className="flex min-w-0 flex-col gap-4">
    <div className="flex flex-wrap justify-end gap-2"><Button size="sm" disabled={list.isFetching} onClick={() => void list.refetch()}>{t('operationKeys.activity.refresh')}</Button>
      <Button size="sm" disabled={list.isPending || list.isError || rows.length === 0 || revoke.isPending} onClick={() => { revoke.reset(); setEnding('all'); }}>{t('adminCredentials.endAll')}</Button></div>
    {list.isPending ? <SkeletonRows read={list} rows={3} /> : list.isError && !list.isFetchNextPageError ? <ErrorState error={list.error} onRetry={() => void list.refetch()} />
      : rows.length === 0 ? <EmptyState compact title={t('adminCredentials.noSessions')} /> : <Table label={t('adminCredentials.sessions')} rows={rows} rowKey={(row) => row.id} columns={columns} />}
    <Pagination hasMore={list.hasNextPage} loading={list.isFetchingNextPage} error={list.isFetchNextPageError ? list.error : undefined} onMore={() => void list.fetchNextPage()} />
    <ConfirmDialog open={ending !== null} onOpenChange={(open) => { if (!open && !revoke.isPending) setEnding(null); }} title={ending === 'all' ? t('adminCredentials.endAll') : t('security.sessions.endTitle', { browser: ending === null ? '' : browserName(ending.user_agent) })}
      confirmLabel={ending === 'all' ? t('adminCredentials.endAll') : t('security.sessions.end')} danger pending={revoke.isPending} error={revoke.error ?? undefined}
      onConfirm={() => { if (ending !== null) revoke.mutate({ ...(ending === 'all' ? {} : { session: ending.id }), reader: libraryReader() }); }}>
      {ending === 'all' ? t('adminCredentials.endAllBody') : t('adminCredentials.endOneBody')}
    </ConfirmDialog>
  </div>;
}
