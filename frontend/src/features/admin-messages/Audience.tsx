/** A send count opens the same bounded people list as avatar stacks, only while inspected. */
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { usersQuery } from '@/api/admin';
import { meQuery } from '@/api/auth';
import { membersQuery } from '@/api/directory';
import { ErrorState } from '@/ui/ErrorState';
import { PeopleCard, PeopleList } from '@/ui/PeopleList';
import { Pagination } from '@/ui/Pagination';
import { SkeletonRows } from '@/ui/Skeleton';

function OrgPeople({ org, total }: { org: string; total: number }) {
  const list = useInfiniteQuery(membersQuery(org, { q: '', direct: false }));
  const me = useQuery(meQuery);
  const people = new Map((list.data?.pages.flatMap((page) => page.items) ?? []).map((member) => [member.user.id, member.user]));
  // Native org acceptance also includes its sender, including a guest member manager.
  if (me.data !== undefined) people.set(me.data.id, me.data);
  if (list.isPending) return <SkeletonRows read={list} rows={3} />;
  if (list.isError && !list.isFetchNextPageError) return <ErrorState compact error={list.error} onRetry={() => void list.refetch()} />;
  return <PeopleList users={[...people.values()]} total={total}><Pagination hasMore={list.hasNextPage} loading={list.isFetchingNextPage} error={list.isFetchNextPageError ? list.error : undefined} onMore={() => void list.fetchNextPage()} /></PeopleList>;
}
function SystemPeople({ total }: { total: number }) {
  const list = useInfiniteQuery(usersQuery({ q: '', status: 'active', admins: false }));
  if (list.isPending) return <SkeletonRows read={list} rows={3} />;
  if (list.isError && !list.isFetchNextPageError) return <ErrorState compact error={list.error} onRetry={() => void list.refetch()} />;
  const people = list.data.pages.flatMap((page) => page.items).filter((person) => person.kind === 'human');
  return <PeopleList users={people} total={total}><Pagination hasMore={list.hasNextPage} loading={list.isFetchingNextPage} error={list.isFetchNextPageError ? list.error : undefined} onMore={() => void list.fetchNextPage()} /></PeopleList>;
}
export function Audience({ org, count, children }: { org: string | null; count: number | undefined; children: string }) {
  const { t } = useTranslation();
  if (count === undefined) return <span className="text-sm text-text-muted">{children}</span>;
  return <PeopleCard label={t('ui.avatarStack.list')} trigger={<button type="button" className="focus-inset rounded-sm text-sm text-text-muted underline decoration-dotted underline-offset-2">{children}</button>}>
    {org === null ? <SystemPeople total={count} /> : <OrgPeople org={org} total={count} />}
  </PeopleCard>;
}
