/** A send count opens the same bounded people list as avatar stacks, only while inspected. */
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { usersQuery } from '@/api/admin';
import { meQuery } from '@/api/auth';
import { membersQuery } from '@/api/directory';
import { matchesCollectionWords, useCollectionSearch } from '@/lib/collectionSearch';
import { CollectionSearch } from '@/ui/CollectionSearch';
import { ErrorState } from '@/ui/ErrorState';
import { PeopleCard, PeopleList } from '@/ui/PeopleList';
import { Pagination } from '@/ui/Pagination';
import { SkeletonRows } from '@/ui/Skeleton';

function OrgPeople({ org, total }: { org: string; total: number }) {
  const {t}=useTranslation();
  const search=useCollectionSearch(`message-audience:${org}`);
  const list = useInfiniteQuery(membersQuery(org, { q: search.q, direct: false }));
  const me = useQuery(meQuery);
  const people = new Map((list.data?.pages.flatMap((page) => page.items) ?? []).map((member) => [member.user.id, member.user]));
  // Native org acceptance also includes its sender, including a guest member manager.
  if (me.data !== undefined && matchesCollectionWords(search.q,`${me.data.display_name} ${me.data.username}`)) people.set(me.data.id, me.data);
  return <div className="flex min-w-0 flex-col gap-2"><CollectionSearch name={t('ui.avatarStack.list')} {...search}/>
    {list.isPending ? <SkeletonRows read={list} rows={3}/> : list.isError && !list.isFetchNextPageError ? <ErrorState compact error={list.error} onRetry={()=>void list.refetch()}/> :
      <PeopleList users={[...people.values()]} total={search.q===''?total:people.size}><Pagination hasMore={list.hasNextPage} loading={list.isFetchingNextPage} error={list.isFetchNextPageError?list.error:undefined} onMore={()=>void list.fetchNextPage()}/></PeopleList>}
  </div>;
}
function SystemPeople({ total }: { total: number }) {
  const {t}=useTranslation();
  const search=useCollectionSearch('system-message-audience');
  const list = useInfiniteQuery(usersQuery({ q: search.q, status: 'active', admins: false }));
  const people = (list.data?.pages ?? []).flatMap((page) => page.items).filter((person) => person.kind === 'human');
  return <div className="flex min-w-0 flex-col gap-2"><CollectionSearch name={t('ui.avatarStack.list')} {...search}/>
    {list.isPending ? <SkeletonRows read={list} rows={3}/> : list.isError && !list.isFetchNextPageError ? <ErrorState compact error={list.error} onRetry={()=>void list.refetch()}/> :
      <PeopleList users={people} total={search.q===''?total:people.length}><Pagination hasMore={list.hasNextPage} loading={list.isFetchingNextPage} error={list.isFetchNextPageError?list.error:undefined} onMore={()=>void list.fetchNextPage()}/></PeopleList>}
  </div>;
}
export function Audience({ org, count, children }: { org: string | null; count: number | undefined; children: string }) {
  const { t } = useTranslation();
  if (count === undefined) return <span className="text-sm text-text-muted">{children}</span>;
  return <PeopleCard label={t('ui.avatarStack.list')} trigger={<button type="button" className="focus-inset rounded-sm text-sm text-text-muted underline decoration-dotted underline-offset-2">{children}</button>}>
    {org === null ? <SystemPeople total={count} /> : <OrgPeople org={org} total={count} />}
  </PeopleCard>;
}
