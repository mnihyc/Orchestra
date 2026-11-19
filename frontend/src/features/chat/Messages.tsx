/** Direct and group chats of a root org with unread state.
 *
 * The screen at `/org/$slug/messages`: the list of the caller's direct and group chats in this org, the
 * one with the newest activity first, each row with its avatars, name, the first line of the last message,
 * its time, an unread dot and the number of mentions; the open chat beside it. "New message" opens a
 * picker of the org's people. A row's menu has Mute and Hide. For whoever may read the chats among an
 * org's people there is a switch above the list; it is off each time the screen is opened. On a small
 * screen the list and the chat take turns.
 */
import { useInfiniteQuery, useMutation, useQuery } from '@tanstack/react-query';
import { ArrowLeft, CheckCheck, MessageSquarePlus, MessagesSquare } from 'lucide-react';
import { type ReactNode, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { chatQuery, markOrgRead, orgChatsQuery } from '@/api/chats';
import { ApiError, client } from '@/api/client';
import { orgBySlugQuery } from '@/api/nodes';
import { ParticipationFilter } from '@/features/access/ParticipationFilter';
import { FilterBar } from '@/ui/FilterBar';
import { PageHeader } from '@/shell/Page';
import { failed } from '@/features/chat/acts';
import { AllChats } from '@/features/chat/AllChats';
import { ChatRow } from '@/features/chat/ChatRow';
import { ChatView } from '@/features/chat/ChatView';
import { NewChat } from '@/features/chat/NewChat';
import type { Org } from '@/generated/api-types';
import { chatWriter } from '@/realtime/stores/chat';
import { useOrgTreeFeed } from '@/realtime/live';
import { selectOrg } from '@/shell/org';
import { chatPageTitle } from '@/features/chat/ChatView';
import { openNodeDialog } from '@/features/navigation/NodeDialogs';
import { useScreenHeading } from '@/shell/screen';
import { Button } from '@/ui/Button';
import { cx } from '@/ui/cx';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Skeleton, SkeletonRows } from '@/ui/Skeleton';

/** What the address of the screen says: the chat open, the message to go to, the switch and the org it is for. */
export interface MessagesSearch {
  chat?: string;
  message?: string;
  show?: 'all';
  of?: string;
  participating?: 'true';
}

export interface MessagesProps {
  slug: string;
  search: MessagesSearch;
  onSearch: (patch: Partial<Record<keyof MessagesSearch, string | undefined>>) => void;
}

function ListSkeleton() {
  return (
    <div className="flex flex-col gap-3 p-3">
      {[0, 1, 2, 3].map((one) => (
        <div key={one} className="flex items-center gap-3">
          <Skeleton className="size-8 rounded-full" />
          <div className="flex flex-1 flex-col gap-1">
            <Skeleton className="h-4 w-1/2" />
            <Skeleton className="h-3 w-3/4" />
          </div>
        </div>
      ))}
    </div>
  );
}

/** `org` is absent for a guest of a group: they are in that chat and nowhere else of the org, and read nothing of the org itself. */
function MessagesOf({ rootId, org, slug, search, onSearch }: MessagesProps & { rootId: string; org: Org | null }) {
  const { t } = useTranslation();
  const me = useStore(client.session, (state) => state.userId);
  const [creating, setCreating] = useState(false);
  const mine = useInfiniteQuery(orgChatsQuery(rootId, { scope: 'mine', of: null }));
  useOrgTreeFeed(org === null ? null : rootId);


  const chats = useMemo(
    () =>
      (mine.data?.pages.flatMap((page) => page.items) ?? []).toSorted((one, other) =>
        (other.last_message_at ?? other.created_at).localeCompare(one.last_message_at ?? one.created_at),
      ),
    [mine.data],
  );
  const open = search.chat ?? null;
  // A permitted conversation can also be reached from the admin or guest list. Its cached authorized read supplies its name.
  const selected = useQuery({ ...chatQuery(open ?? ''), enabled: open !== null });
  const conversation = chats.find((chat) => chat.id === open) ?? selected.data;
  const canStartMessage = org !== null && org.org_kind !== 'personal';
  useScreenHeading({ title: conversation === undefined ? t('screens.messages.title') : chatPageTitle(conversation, me, t), icon: MessagesSquare, nodeId: org?.id, leaf: true, trail: conversation === undefined ? undefined : [{ label: t('screens.messages.title'), to: `/org/${slug}/messages` }] });
  const readAll = useMutation({
    mutationFn: () => markOrgRead(rootId),
    onError: (error) => failed(t('chat.failed.readAll'), error),
  });

  const listEmpty = (
    <EmptyState
      compact
      icon={<MessagesSquare aria-hidden />}
      title={org?.org_kind === 'personal' ? t('craftMessages.onlyYou') : t('chat.messages.empty')}
      action={
        org?.org_kind === 'personal' ? <Button action="create" variant="primary" onClick={() => openNodeDialog({ type: 'new-org' })}>{t('shell.rail.newOrg')}</Button> : undefined
      }
    />
  );

  const list: ReactNode = (
    <div className="flex h-full min-h-0 flex-col">
      <div className="shrink-0 border-b border-border px-3 py-3">
        <PageHeader title={t('screens.messages.title')} actions={<IconButton
          size="sm"
          label={t('chat.messages.readAll')}
          icon={<CheckCheck aria-hidden />}
          pending={readAll.isPending}
          onClick={() => readAll.mutate()}
        />} />
      </div>
      <FilterBar className="shrink-0 border-b border-border px-3 py-2"
        participation={<ParticipationFilter checked={search.participating === 'true'} onCheckedChange={(on) => onSearch({ participating: on ? 'true' : undefined })} />}
        action={canStartMessage ? <Button size="sm" variant="primary" icon={<MessageSquarePlus aria-hidden />} onClick={() => setCreating(true)}>{t('chat.new.title')}</Button> : undefined} />
      {org !== null && <AllChats org={org} slug={slug} search={search} me={me} />}
      <div className="min-h-0 flex-1 overflow-y-auto p-2">
        {mine.isPending ? (
          <ListSkeleton />
        ) : mine.isError ? (
          <ErrorState compact error={mine.error} title={t('chat.messages.couldNotLoad')} onRetry={() => void mine.refetch()} />
        ) : chats.length === 0 ? (
          listEmpty
        ) : (
          <ul aria-label={t('chat.messages.list')} className="flex flex-col gap-1">
            {chats.map((chat) => (
              <ChatRow key={chat.id} chat={chat} slug={slug} selected={chat.id === open} me={me} search={search} />
            ))}
          </ul>
        )}
        {mine.hasNextPage && (
          <div className="flex justify-center py-2">
            <Button size="sm" variant="ghost" pending={mine.isFetchingNextPage} onClick={() => void mine.fetchNextPage()}>
              {t('chat.messages.more')}
            </Button>
          </div>
        )}
      </div>
    </div>
  );

  const chatPane: ReactNode =
    open === null ? (
      <EmptyState className="h-full" icon={<MessagesSquare aria-hidden />} title={t('chat.messages.choose')} description={t('chat.messages.chooseAbout')} />
    ) : (
      <div className="flex h-full min-h-0 flex-col">
        <div className="shrink-0 border-b border-border px-2 py-1 md:hidden">
          <Button size="sm" variant="ghost" icon={<ArrowLeft aria-hidden />} onClick={() => onSearch({ chat: undefined, message: undefined })}>
            {t('screens.messages.title')}
          </Button>
        </div>
        <div className="min-h-0 flex-1">
          <ChatView key={open} chatId={open} message={search.message ?? null} />
        </div>
      </div>
    );

  return (
    <div className="flex h-full min-h-0">
      <aside aria-label={t('chat.messages.list')} className={cx('min-h-0 w-full shrink-0 border-r border-border md:w-(--drawer-width)', open !== null && 'hidden md:block')}>
        {list}
      </aside>
      <section aria-label={t('chat.messages.open')} className={cx('min-h-0 min-w-0 flex-1', open === null && 'hidden md:block')}>
        {chatPane}
      </section>
      <NewChat
        open={creating}
        onOpenChange={setCreating}
        orgId={rootId}
        me={me}
        onOpened={(chat) => {
          chatWriter.apply(chat.id, { chat }, chat.seq);
          onSearch({ chat: chat.id, message: undefined });
        }}
      />
    </div>
  );
}

/** An identifier: as long as no short name can be, so that an org's address is never taken for one. */
const IDENTIFIER = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

export function Messages(props: MessagesProps) {
  const { t } = useTranslation();
  const named = !IDENTIFIER.test(props.slug);
  const org = useQuery({ ...orgBySlugQuery(props.slug), enabled: named });
  const userId = useStore(client.session, (state) => state.userId);
  // A guest of a group may not read the org itself: the chat they were let into says which org it belongs to.
  const refused = org.error instanceof ApiError && (org.error.status === 403 || org.error.status === 404);
  const guestWay = !named || refused;
  const chat = useQuery({ ...chatQuery(props.search.chat ?? ''), enabled: guestWay && props.search.chat !== undefined });
  useEffect(() => {
    if (org.data?.parent_id === null) selectOrg(userId, org.data.id);
  }, [org.data, userId]);

  if (org.data !== undefined) return <MessagesOf {...props} rootId={org.data.org_id} org={org.data} />;
  if (named && org.isPending) return <WhileLoading />;
  if (!guestWay) {
    return (
      <>
        <ScreenName />
        <ErrorState className="h-full" error={org.error} title={t('chat.messages.couldNotLoad')} onRetry={() => void org.refetch()} />
      </>
    );
  }
  const rootId = !named ? props.slug : chat.data?.org_id;
  if (rootId !== undefined) return <MessagesOf {...props} rootId={rootId} org={null} />;
  if (chat.isPending && props.search.chat !== undefined) return <WhileLoading />;
  return (
    <>
      <ScreenName />
      <EmptyState
        className="h-full"
        icon={<MessagesSquare aria-hidden />}
        title={t('chat.messages.guestOnly')}
        description={t('chat.messages.guestOnlyAbout')}
      />
    </>
  );
}

/** The name of the screen for its tab where no org is known yet; once one is, the screen names itself with the org it is of. */
function ScreenName() {
  const { t } = useTranslation();
  useScreenHeading({ title: t('screens.messages.title'), icon: MessagesSquare });
  return null;
}

function WhileLoading() {
  return (
    <>
      <ScreenName />
      <SkeletonRows rows={6} className="p-4" />
    </>
  );
}
