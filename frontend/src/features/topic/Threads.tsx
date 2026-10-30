/** All threads of the topic, with provider, model and effort, and environment.
 *
 * Every thread of the topic, archived ones included, as a table: its state, when something last
 * happened in it and how many approvals wait. The threads come from the topic's live tree; the provider,
 * the model, the effort and the environment of each belong to the thread and show with the thread
 * screen. Starting a thread opens the dialog of the thread part, which makes one from the defaults of the topic.
 */
import { useListChoice } from '@/lib/listState';
import { MessageSquarePlus, MessagesSquare } from 'lucide-react';
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';

import { openNodeDialog } from '@/features/navigation/NodeDialogs';
import { participates } from '@/features/access/participation';
import { ParticipationFilter } from '@/features/access/ParticipationFilter';
import { FilterBar } from '@/ui/FilterBar';
import { AsAdminMark, adminAccessOf } from '@/ui/AsAdminMark';
import { PlaceLink, type PlaceRef } from '@/features/navigation/places';
import type { TreeNode } from '@/generated/api-types';
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
    <PermissionGate allowed={allowed && !topic.archived} mode="block" reason={topic.archived ? t('state.archived') : denial(t, 'topic.interact', topic.name, owner)}>
      <Button data-tutorial-target="new-thread" variant="primary" icon={<MessageSquarePlus aria-hidden />} onClick={() => openNodeDialog({ type: 'new-thread', parent: topic })}>
        {t('places.topic.newThread')}
      </Button>
    </PermissionGate>
  );
}

export interface TopicThreadsProps { orgId: string; topicId: string; topic: PlaceRef; allowed: boolean; owner: Parameters<typeof denial>[3] }
export function TopicThreads(props: TopicThreadsProps) {
  const [participating, setParticipating] = useListChoice('threads', 'participating', false);
  return <div className="flex flex-col gap-3"><FilterBar participation={<ParticipationFilter checked={participating} onCheckedChange={setParticipating} />}
    action={<NewThreadButton topic={props.topic} allowed={props.allowed} owner={props.owner} />} /><TopicThreadRows {...props} participating={participating} /></div>;
}

function TopicThreadRows({ orgId, topicId, participating }: TopicThreadsProps & { participating: boolean }) {
  const { t } = useTranslation();
  const tree = useOrgTree(orgId);
  const threads = useMemo(() => threadsOfTopic(tree.nodes, topicId).filter((thread) => !participating || participates(thread)), [tree.nodes, topicId, participating]);

  if (!tree.loaded && tree.error === null) return <SkeletonRows rows={3} />;
  if (threads.length === 0) {
    return (
      <EmptyState
        icon={<MessagesSquare aria-hidden />}
        title={t('places.topic.noThreads')}
        description={t('places.topic.noThreadsAbout')}
      />
    );
  }

  const columns: Column<TreeNode>[] = [
    {
      id: 'name',
      header: t('places.topics.name'),
      cell: (thread, shown) => {
        const withoutState = !shown.has('state');
        return (
          <div className="flex min-w-0 flex-col gap-1">
            <span className="flex min-w-0 items-center gap-2"><PlaceLink place={thread} className="truncate font-medium text-text hover:underline">
              {thread.name}
            </PlaceLink><AsAdminMark of={adminAccessOf(thread)?.place} responsive /></span>
            {withoutState && <ThreadStateChip display={thread.thread?.display} className="self-start" />}
          </div>
        );
      },
    },
    { id: 'state', header: t('places.topic.state'), from: 'sm', cell: (thread) => <ThreadStateChip display={thread.thread?.display} /> },
    {
      id: 'approvals',
      header: t('places.topic.approvals'),
      from: 'md',
      cell: (thread) =>
        (thread.thread?.pending_approvals ?? 0) > 0 ? (
          <Badge tone="warning" count={thread.thread?.pending_approvals ?? 0} label={t('navigation.approvalsWaiting', { count: thread.thread?.pending_approvals ?? 0 })} />
        ) : null,
    },
    {
      id: 'activity',
      header: t('places.topics.activity'),
      from: 'sm',
      className: 'whitespace-nowrap text-text-muted',
      cell: (thread) => (thread.thread?.last_activity_at === null || thread.thread?.last_activity_at === undefined ? null : <RelativeTime at={thread.thread.last_activity_at} />),
    },
  ];

  return (
    <div className="flex flex-col gap-3">
      <Table label={t('places.topic.threads')} columns={columns} rows={threads} rowKey={(thread) => thread.id} rowClassName={(thread) => (thread.archived ? 'opacity-60' : undefined)} />
    </div>
  );
}
