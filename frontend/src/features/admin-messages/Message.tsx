/** The native received-message reader, reached by the ordinary Inbox link or sent summary. */
import { useQuery } from '@tanstack/react-query';
import { Megaphone } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { messageQuery } from '@/api/adminMessages';
import { Page, PageHeader } from '@/shell/Page';
import { useScreenHeading } from '@/shell/screen';
import { ErrorState } from '@/ui/ErrorState';
import { Markdown } from '@/ui/Markdown';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { UserChip } from '@/ui/UserChip';
import { messageReadRefused } from './current';

export function AdministratorMessage({ id }: { id: string }) {
  const { t } = useTranslation();
  const message = useQuery(messageQuery(id));
  const row = messageReadRefused(message.error) ? undefined : message.data;
  useScreenHeading({ title: row?.title ?? t('adminMessages.messages'), icon: Megaphone });
  if (message.isPending) return <Page><SkeletonRows read={message} rows={4} /></Page>;
  if (row === undefined) return <Page><ErrorState error={message.error} onRetry={() => void message.refetch()} /></Page>;
  return <Page>
    <div className={message.isError ? 'flex min-w-0 flex-col gap-6 opacity-60' : 'flex min-w-0 flex-col gap-6'}>
      <PageHeader title={row.title} facts={[
        row.org === null ? t('adminMessages.fromSystem') : t('adminMessages.fromOrg', { org: row.org.name ?? '' }),
        <UserChip key="sender" user={row.sender} />,
        <RelativeTime key="time" at={row.created_at} />,
      ]} />
      <Markdown text={row.body} />
    </div>
    {message.isError && <ErrorState compact error={message.error} onRetry={() => void message.refetch()} />}
  </Page>;
}
