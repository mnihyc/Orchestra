import { useCollectionSearch } from '@/lib/collectionSearch';
import { CollectionSearch } from '@/ui/CollectionSearch';
import { ScrollRegion } from '@/ui/ScrollRegion';
/** Readable chats returned for this org need no separate admin viewing mode. */
import { useInfiniteQuery } from '@tanstack/react-query';
import { ChevronRight } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';

import { orgChatsQuery } from '@/api/chats';
import { ChatRow } from '@/features/chat/ChatRow';
import type { MessagesProps } from '@/features/chat/Messages';
import type { Chat, Org } from '@/generated/api-types';
import { ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { Skeleton } from '@/ui/Skeleton';

function Section({ label, children, onOpen }: { label: string; children: React.ReactNode; onOpen: () => void }) {
  return (
    <details className="group border-b border-border" onToggle={(event) => event.currentTarget.open && onOpen()}>
      <summary className="focus-inset flex cursor-pointer list-none items-center gap-1 px-3 py-2 text-sm font-medium text-text-muted hover:text-text">
        <ChevronRight size={16} aria-hidden className="transition-transform group-open:rotate-90" />
        {label}
      </summary>
      <div className="px-2 pb-2">{children}</div>
    </details>
  );
}

function Others({ orgId, of, kind, slug, search, me }: { orgId: string; of: string | null; kind: 'others' | 'nodes'; slug: string; search: MessagesProps['search']; me: string | null }) {
  const { t } = useTranslation();
  const names = useCollectionSearch('other-chats:' + orgId + ':' + kind);
  const chats = useInfiniteQuery(orgChatsQuery(orgId, { scope: kind, of, participating: search.participating === 'true', ...(names.q === '' ? {} : { q: names.q }) }));
  const items: Chat[] = chats.data?.pages.flatMap((page) => page.items) ?? [];
  const label = t(kind === 'others' ? 'chat.all.others' : 'chat.all.nodes');
  return <div className="flex min-w-0 flex-col gap-2">
    <CollectionSearch name={label} {...names} />
    {chats.isPending ? <Skeleton className="m-2 h-10" /> : chats.isLoadingError ? <ErrorState compact error={chats.error} title={t('chat.all.couldNotLoad')} onRetry={() => void chats.refetch()} /> : items.length === 0 ? <EmptyState compact className="px-3" title={t(kind === 'others' ? 'chat.all.noOthers' : 'chat.all.noNodes')} /> : <ScrollRegion kind="table" label={label} className="max-h-96 overflow-y-auto"><ul className="flex flex-col gap-1">
      {items.map((chat) => <ChatRow key={chat.id} chat={chat} slug={slug} selected={chat.id === search.chat} me={me} search={search} quiet={kind === 'others'} />)}
    </ul></ScrollRegion>}
    <Pagination hasMore={chats.hasNextPage} loading={chats.isFetchingNextPage} error={chats.isFetchNextPageError ? chats.error : null} onMore={() => void chats.fetchNextPage()} />
  </div>;
}

export function AllChats({ org, slug, search, me }: { org: Pick<Org, 'org_id'>; slug: string; search: MessagesProps['search']; me: string | null; onSearch?: MessagesProps['onSearch'] }) {
  const { t } = useTranslation();
  const [opened, setOpened] = useState({ others: false, nodes: false });
  return (
    <div className="shrink-0 border-b border-border">
      {
        <>
          <Section label={t('chat.all.others')} onOpen={() => setOpened((before) => ({ ...before, others: true }))}>
            {opened.others && <Others orgId={org.org_id} of={search.of ?? null} kind="others" slug={slug} search={search} me={me} />}
          </Section>
          <Section label={t('chat.all.nodes')} onOpen={() => setOpened((before) => ({ ...before, nodes: true }))}>
            {opened.nodes && <Others orgId={org.org_id} of={null} kind="nodes" slug={slug} search={search} me={me} />}
          </Section>
        </>
      }
    </div>
  );
}
