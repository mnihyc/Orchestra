/** One sent-list section beside the same composer in organization and system administration. */
import { useInfiniteQuery } from '@tanstack/react-query';
import { Megaphone } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { sentMessagesQuery } from '@/api/adminMessages';
import type { AdminMessageSummary } from '@/generated/api-types';
import { formatNumber } from '@/lib/format';
import { Section } from '@/shell/Page';
import { PeopleCount } from '@/ui/AvatarStack';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { linkClasses } from '@/ui/link';
import { Pagination } from '@/ui/Pagination';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { Table, type Column } from '@/ui/Table';
import { useSeen } from '@/ui/useSeen';
import { UserChip } from '@/ui/UserChip';
import { MessageComposer, type MessageScope } from './Composer';
import { messageReadRefused } from './current';

export function MessagesSection({ scope }: { scope: MessageScope }) {
  const { t, i18n } = useTranslation();
  const [open, setOpen] = useState(false);
  // The list is the last part of its page: it is read when it comes into view or its composer is opened, not with the page.
  const [anchor, seen] = useSeen();
  const list = useInfiniteQuery({ ...sentMessagesQuery(scope.kind === 'org' ? scope.id : null), enabled: seen || open });
  const refused = messageReadRefused(list.error);
  const rows = refused ? [] : list.data?.pages.flatMap((page) => page.items) ?? [];
  const columns: Column<AdminMessageSummary>[] = [
    { id: 'title', header: t('adminMessages.title'), cell: (row, shown) => {
      const senderShown = shown.has('sender'), timeShown = shown.has('time');
      return <div className="flex min-w-0 flex-col gap-1">
      <a className={`${linkClasses} min-w-0 truncate`} title={row.title} href={`/messages/${encodeURIComponent(row.id)}`}>{row.title}</a>
      {!senderShown && <UserChip user={row.sender} />}
      {!timeShown && <RelativeTime at={row.created_at} className="text-sm text-text-muted" />}
    </div>; } },
    { id: 'sender', header: t('adminMessages.sender'), from: 'sm', cell: (row) => <UserChip user={row.sender} /> },
    { id: 'time', header: t('adminMessages.sent'), from: 'md', cell: (row) => <RelativeTime at={row.created_at} /> },
    { id: 'count', header: t('adminMessages.recipients'), align: 'end', cell: (row) => row.recipient_count === 0 ? formatNumber(0, i18n.language) : <PeopleCount users={[]} total={row.recipient_count}>{formatNumber(row.recipient_count, i18n.language)}</PeopleCount> },
  ];
  return <Section id="administrator-messages" title={t('adminMessages.messages')} action={<Button icon={<Megaphone aria-hidden />} onClick={() => setOpen(true)}>{t(scope.kind === 'org' ? 'adminMessages.messageMembers' : 'adminMessages.messageEveryone')}</Button>}>
    <div ref={anchor} className="flex flex-col gap-3">
      {list.isPending ? <SkeletonRows read={list} rows={2} /> : list.isError && rows.length === 0 ? <ErrorState compact error={list.error} onRetry={() => void list.refetch()} /> : rows.length === 0 ? <EmptyState compact title={t('adminMessages.empty')} /> : <div className={list.isError ? 'opacity-60' : undefined}><Table label={t('adminMessages.sent')} columns={columns} rows={rows} rowKey={(row) => row.id} /></div>}
      {list.isError && rows.length > 0 && <ErrorState compact error={list.error} onRetry={() => void list.refetch()} />}
      <Pagination hasMore={!refused && list.hasNextPage} loading={list.isFetchingNextPage} error={!refused && list.isFetchNextPageError ? list.error : undefined} onMore={() => void list.fetchNextPage()} />
      {open && <MessageComposer scope={scope} onClose={() => setOpen(false)} />}
    </div>
  </Section>;
}
