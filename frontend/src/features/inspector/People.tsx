/** The thread's page in short: who views, who talks to the agent and who manages, where each comes from, presence, who lacks the model, and requests.
 * Reads the same permission page as Access, keeping inherited holders and their givers visible.
 */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { Users } from 'lucide-react';
import { pageQuery } from '@/api/access';
import { GoesWith } from '@/features/access/GoesWith';
import { gaveText, HolderView } from '@/features/access/holders';
import { holdsOfRow } from '@/features/access/model';
import { FollowMark } from '@/features/access/RowCell';
import { WaitingRequests } from '@/features/access/RequestCard';
import { openAccess } from '@/features/access/AccessDialog';
import type { ViewProps } from '@/features/workbench/views';
import type { Thread } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { useThreadView } from '@/realtime/stores/thread';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { PresenceStack } from '@/ui/PresenceStack';
import { SkeletonRows } from '@/ui/Skeleton';
import { ThreadPanel } from './ThreadPanel';

const THREAD_PERMISSIONS = ['thread.view', 'thread.interact', 'thread.manage'] as const;

function People({ thread }: { thread: Thread }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const target = { kind: 'thread' as const, id: thread.id, name: thread.name };
  const query = useQuery(pageQuery(target));
  const presence = useThreadView(thread.id).presence;
  if (query.data === undefined) return query.error === null ? <SkeletonRows rows={4} /> : <ErrorState compact error={query.error} onRetry={() => void query.refetch()} />;
  const page = query.data;
  return <div className="h-full overflow-auto p-3" data-testid="thread-people">
    <div className="flex items-center justify-between gap-2 pb-3"><PresenceStack users={presence} /><Button size="sm" onClick={() => openAccess(target)}>{t('threadViews.people.access')}</Button></div>
    <section className="mb-4 flex flex-col gap-2" aria-label={t('threadViews.people.owners')}>
      <h3 className="text-sm font-medium">{t('threadViews.people.owners')}</h3>
      {page.owners.map((entry) => <HolderView key={entry.id} holder={entry.holder} target={target} note={gaveText(t, locale, entry)} />)}
    </section>
    {THREAD_PERMISSIONS.map((permission) => {
      const row = page.rows.find((entry) => entry.permission === permission);
      const holders = row === undefined ? [] : holdsOfRow(row);
      return <section key={permission} className="mb-4 flex flex-col gap-2" aria-label={t(`threadViews.people.${permission}`)}>
        <h3 className="text-sm font-medium">{t(`threadViews.people.${permission}`)}</h3>
        {holders.map(({ holder, hold }, index) => <HolderView key={index} holder={holder} target={target} inherited={hold.entry === null} note={hold.entry === null ? <FollowMark place={hold.from} /> : gaveText(t, locale, hold.entry)} />)}
        {holders.length === 0 && <EmptyState compact icon={<Users aria-hidden />} title={t('access.cell.ownersOnly')} />}
      </section>;
    })}
    <GoesWith page={page} target={target} />
    <WaitingRequests nodeId={thread.id} />
  </div>;
}

export function PeopleView(_props: ViewProps) { return <ThreadPanel>{(thread) => <People key={thread.id} thread={thread} />}</ThreadPanel>; }
