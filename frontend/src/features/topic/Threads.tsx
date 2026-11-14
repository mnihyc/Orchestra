/** Live topic threads, searched by name in one measured table.
 *
 * Every thread of the topic, archived ones included, as a table: its state, when something last
 * happened in it and how many approvals wait. Native pages select names and the live tree overlays facts; the provider,
 * the model, the effort and the environment of each belong to the thread and show with the thread
 * screen. Starting a thread opens the dialog of the thread part, which makes one from the defaults of the topic.
 */
import { useListChoice } from '@/lib/listState';
import { MessageSquarePlus, MessagesSquare } from 'lucide-react';
import { type ReactNode, useEffect, useMemo, useRef } from 'react';
import { useInfiniteQuery } from '@tanstack/react-query';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { everythingQuery } from '@/api/everything';
import { useDebounced } from '@/lib/debounce';
import { Pagination } from '@/ui/Pagination';
import { participates } from '@/features/access/participation';
import { useListingRetired } from '@/features/navigation/useRecentUpdates';
import { useViewShown } from '@/features/workbench/viewOpening';
import { useTranslation } from 'react-i18next';

import { useSearchText } from '@/features/resources/ResourceList';
import { attentionOf, WorkChips } from '@/features/source/Attention';
import { Input } from '@/ui/Input';
import { ErrorAlert } from '@/ui/ErrorState';
import { StateLine } from '@/ui/StateLine';
import { openNodeDialog } from '@/features/navigation/NodeDialogs';
import { ParticipationFilter } from '@/features/access/ParticipationFilter';
import { FilterBar } from '@/ui/FilterBar';
import { AsAdminMark, adminAccessOf } from '@/ui/AsAdminMark';
import { PlaceLink, type PlaceRef } from '@/features/navigation/places';
import type { TreeNode, TreeThread, AdminAccess } from '@/generated/api-types';
import { denial } from '@/lib/permissions';
import { useOrgTree } from '@/realtime/stores/tree';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { PermissionGate } from '@/ui/PermissionGate';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { ThreadStateChip } from '@/ui/ThreadState';
import { type Column, Table } from '@/ui/Table';

/** The threads directly in a topic, in the order they were made, from the rows of its tree. */
export function threadsOfTopic(rows: ReadonlyMap<string, TreeNode>, topicId: string): TreeNode[] {
  return [...rows.values()]
    .filter((row) => row.kind === 'thread' && row.parent_id === topicId && !row.deleted)
    .sort((one, other) => one.tree_seq - other.tree_seq);
}

/** The control that starts a thread, for whoever may; it opens the dialog that makes one. */
export function NewThreadButton({ topic, allowed, owner }: { topic: PlaceRef; allowed: boolean; owner: Parameters<typeof denial>[3] }) {
  const { t } = useTranslation();
  return (
    <PermissionGate allowed={allowed && !topic.archived} mode="block" reason={topic.archived ? t('state.archived') : denial(t, 'topic.start_threads', topic.name, owner)}>
      <Button data-tutorial-target="new-thread" variant="primary" icon={<MessageSquarePlus aria-hidden />} onClick={() => openNodeDialog({ type: 'new-thread', parent: topic })}>
        {t('places.topic.newThread')}
      </Button>
    </PermissionGate>
  );
}

export interface TopicThreadsProps {
  orgId: string; topicId: string; topic: PlaceRef; allowed: boolean; owner: Parameters<typeof denial>[3];
  q?: string | undefined;
  onQuery?: ((query: string) => void) | undefined;
  /** An overview already has its inline starter; other entrances retain the ordinary creation act. */
  creation?: boolean;
  /** A section puts these same controls on its own heading row. */
  frame?: (controls: ReactNode, rows: ReactNode) => ReactNode;
}
export function TopicThreads(props: TopicThreadsProps) {
  const { t } = useTranslation();
  const [participating, setParticipating] = useListChoice('threads', 'participating', false);
  const [text, setText] = useSearchText(props.q ?? '', (query) => props.onQuery?.(query));
  const controls = <FilterBar search={<Input type="search" hideLabel label={t('places.topic.findThreads')} placeholder={t('ui.combobox.search')} value={text} onChange={(event) => setText(event.target.value)} autoComplete="off" />}
    participation={<ParticipationFilter checked={participating} onCheckedChange={setParticipating} />}
    action={props.creation === false ? undefined : <NewThreadButton topic={props.topic} allowed={props.allowed} owner={props.owner} />} />;
  const rows = <TopicThreadRows {...props} text={text} participating={participating} compact={props.frame !== undefined} />;
  return props.frame === undefined ? <div className="flex flex-col gap-3">{controls}{rows}</div> : props.frame(controls, rows);
}

interface TopicRow {
  id: string; kind: 'thread'; name: string; archived: boolean;
  thread: Partial<Pick<TreeThread, 'display' | 'attention' | 'pending_approvals' | 'last_activity_at'>>;
  admin_access: AdminAccess | null | undefined;
}

function TopicThreadRows({ orgId, topicId, participating, text, compact }: TopicThreadsProps & { participating: boolean; text: string; compact: boolean }) {
  const { t } = useTranslation();
  const tree = useOrgTree(orgId), shown = useViewShown();
  const reader = useStore(client.session, (state) => String(state.userId) + ':' + String(state.readerVersion));
  const match = useDebounced(text.trim());
  const options = everythingQuery(reader, topicId, { scope: 'mine', kinds: ['thread'], q: match,
    owner: null, createdFrom: '', createdTo: '', sort: 'created', direction: 'asc', showArchived: true, participating });
  const list = useInfiniteQuery({ ...options, enabled: shown });
  const retired = useListingRetired(options.queryKey, list.error);
  const membership = useMemo(() => JSON.stringify(threadsOfTopic(tree.nodes, topicId).map((row) => [row.id, row.name, row.archived, participates(row)])), [tree.nodes, topicId]);
  const scope = reader + ':' + topicId;
  const previous = useRef({ scope, membership });
  const refresh = list.refetch;
  useEffect(() => {
    if (!shown) return;
    const before = previous.current; previous.current = { scope, membership };
    if (before.scope === scope && before.membership !== membership) void refresh();
  }, [shown, scope, membership, refresh]);
  const threads = useMemo<TopicRow[]>(() => {
    if (retired) return [];
    const seen = new Set<string>();
    return (list.data?.pages.flatMap((page) => page.items) ?? []).flatMap((row): TopicRow[] => {
      if (row.kind !== 'thread' || row.my_access === 'path_only' || row.name === null || seen.has(row.id)) return [];
      seen.add(row.id);
      const live = tree.nodes.get(row.id);
      if (live?.deleted === true) return [];
      return [{ id: row.id, kind: 'thread', name: live?.name ?? row.name, archived: live?.archived ?? false,
        thread: live?.thread ?? { ...(row.thread_display === undefined || row.thread_display === null ? {} : { display: row.thread_display }), last_activity_at: row.last_activity },
        admin_access: row.admin_access }];
    });
  }, [list.data, tree.nodes, retired]);
  const error = list.error ?? tree.error;
  if (list.isPending) return <SkeletonRows read={list} rows={3} />;
  if (threads.length === 0) {
    if (error !== null) return <ErrorAlert error={error} />;
    if (match !== '') return <StateLine compact title={t('places.topic.noMatchingThreads')} />;
    if (compact) return <StateLine compact title={t('places.topic.noThreads')} />;
    return <EmptyState icon={<MessagesSquare aria-hidden />} title={t('places.topic.noThreads')} description={t('places.topic.noThreadsAbout')} />;
  }

  const columns: Column<TopicRow>[] = [
    {
      id: 'name',
      header: t('places.topics.name'),
      cell: (thread, shown) => {
        const withoutState = !shown.has('state');
        const withoutApprovals = !shown.has('approvals');
        const withoutActivity = !shown.has('activity');
        return (
          <div className="flex min-w-0 flex-col gap-1">
            <span className="flex min-w-0 items-center gap-2"><PlaceLink place={thread} className="truncate font-medium text-text hover:underline">
              {thread.name}
            </PlaceLink>{withoutApprovals && (thread.thread.pending_approvals ?? 0) > 0 && <Badge tone="warning" count={thread.thread.pending_approvals ?? 0} label={t('navigation.approvalsWaiting', { count: thread.thread.pending_approvals ?? 0 })} />}<AsAdminMark of={adminAccessOf(thread)?.place} responsive /></span>
            {withoutState && <span className="flex flex-wrap items-center gap-2"><ThreadStateChip display={thread.thread.display} /><WorkChips attention={attentionOf(thread.thread)} compact context="card" /></span>}
            {withoutActivity && thread.thread.last_activity_at !== null && thread.thread.last_activity_at !== undefined && <span className="text-sm text-text-muted">{t('places.cards.active')} <RelativeTime at={thread.thread.last_activity_at} /></span>}
          </div>
        );
      },
    },
    { id: 'state', header: t('places.topic.state'), from: 'sm', cell: (thread) => <span className="flex flex-wrap items-center gap-2"><ThreadStateChip display={thread.thread.display} /><WorkChips attention={attentionOf(thread.thread)} compact context="card" /></span> },
    {
      id: 'approvals',
      header: t('places.topic.approvals'),
      from: 'md',
      cell: (thread) =>
        (thread.thread.pending_approvals ?? 0) > 0 ? (
          <Badge tone="warning" count={thread.thread.pending_approvals ?? 0} label={t('navigation.approvalsWaiting', { count: thread.thread.pending_approvals ?? 0 })} />
        ) : null,
    },
    {
      id: 'activity',
      header: t('places.topics.activity'),
      from: 'sm',
      className: 'whitespace-nowrap text-text-muted',
      cell: (thread) => (thread.thread.last_activity_at === null || thread.thread.last_activity_at === undefined ? null : <RelativeTime at={thread.thread.last_activity_at} />),
    },
  ];

  return (
    <div className="flex flex-col gap-3">
      {error !== null && <ErrorAlert error={error} />}
      <Table key={`${participating}:${match}`} virtual boundedIdentity label={t('places.topic.threads')} columns={columns} rows={threads} rowKey={(thread) => thread.id} rowClassName={(thread) => (thread.archived || error !== null ? 'opacity-60' : undefined)} />
      <Pagination hasMore={list.hasNextPage} loading={list.isFetchingNextPage} error={list.isFetchNextPageError ? list.error : undefined} onMore={() => void list.fetchNextPage()} />
    </div>
  );
}
