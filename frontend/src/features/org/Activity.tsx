/** An org's Activity: the lines of its own history, the management of the orgs and projects in it, and members joining, moved, leaving, suspended and resumed.
 *
 * What a place's Activity is made of, for an org, a project and a topic alike: one line for each thing
 * that happened in it, newest first, with who did it and when, in pages, and chips that keep to the
 * management of the place or to the work its threads did. A line about another thing opens that
 * thing. The server supplies actor, act, object and place facts; every sentence is composed in the
 * reader's language. Audit export is where the original recorded sentence is kept.
 */
import { useListChoice } from '@/lib/listState';
import { useInfiniteQuery } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { Activity as ActivityIcon, CircleCheck, Flag, History, Megaphone, OctagonAlert, ShieldCheck, type LucideIcon } from 'lucide-react';
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { historySentence } from '@/features/inspector/historySentence';

import { activityQuery } from '@/api/nodes';
import { PlaceLink, usePlaceLookup } from '@/features/navigation/places';
import type { ActivityEntry, ActivityKind, HistoryActor } from '@/generated/api-types';
import { Avatar } from '@/ui/Avatar';
import { cx } from '@/ui/cx';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { RelativeTime } from '@/ui/RelativeTime';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { Skeleton, SkeletonGroup } from '@/ui/Skeleton';

/** The kinds of line that say what was done to places, and the kinds that say what their threads did. */
export const MANAGEMENT_KINDS: readonly ActivityKind[] = ['history'];
export const WORK_KINDS: readonly ActivityKind[] = ['turn_completed', 'turn_failed', 'milestone', 'announcement', 'approval_decided'];

type Filter = 'all' | 'management' | 'work';

const KINDS_OF: Record<Filter, readonly ActivityKind[]> = { all: [], management: MANAGEMENT_KINDS, work: WORK_KINDS };

const ICONS: Record<ActivityKind, { Icon: LucideIcon; tone: string }> = {
  history: { Icon: History, tone: 'text-text-subtle' },
  turn_completed: { Icon: CircleCheck, tone: 'text-success' },
  turn_failed: { Icon: OctagonAlert, tone: 'text-danger' },
  milestone: { Icon: Flag, tone: 'text-text-subtle' },
  announcement: { Icon: Megaphone, tone: 'text-text-subtle' },
  approval_decided: { Icon: ShieldCheck, tone: 'text-text-subtle' },
};

/** The acts that the screen can say in the reader's language from the line alone: who, what it is about, and its kind. */
const SAYABLE = new Set(['node.created', 'node.archived', 'node.unarchived', 'node.deleted', 'node.restored', 'node.moved']);
const NODE_VERBS: Readonly<Record<string, string>> = { 'node.created': 'created', 'node.archived': 'archived', 'node.unarchived': 'unarchived', 'node.deleted': 'deleted', 'node.restored': 'restored', 'node.moved': 'moved' };

const OBJECT_KINDS = {
  org: 'places.kinds.org', project: 'places.kinds.project', topic: 'places.kinds.topic', thread: 'places.kinds.thread',
  document: 'places.activity.objects.document', provider: 'places.activity.objects.provider', agent: 'places.activity.objects.agent',
  environment: 'places.activity.objects.environment', host: 'places.activity.objects.host', template: 'places.activity.objects.template',
  mcp_server: 'places.activity.objects.mcpServer', mount: 'places.activity.objects.mount', skill: 'places.activity.objects.skill',
} as const;
const WORK_VERBS = { turn_completed: 'turnCompleted', turn_failed: 'turnFailed', milestone: 'milestone', announcement: 'said', approval_decided: 'decided' } as const;

/** The name of whoever did it, where it is a person or an agent; nobody's for a line the system wrote. */
function actorName(actor: HistoryActor | null, t: TFunction): string {
  if (actor === null) return t('app.name');
  if ('agent' in actor) return actor.agent.name ?? t('places.activity.agent');
  return actor.display_name;
}

/**
 * One line of Activity in catalogue words, including older rows without structured wording.
 * `folder` says that what it is about is a project that lies in a project, which is a folder in words.
 */
export function activityLine(entry: ActivityEntry, t: TFunction, folder = false): string {
  const object = entry.object;
  const kindKey = Object.entries(OBJECT_KINDS).find(([kind]) => kind === (object?.kind ?? entry.node.kind))?.[1];
  const givenName = object === undefined || object === null ? entry.node.name : object.name;
  const name = givenName
    ?? t('places.activity.removed', { kind: kindKey === undefined ? t('places.activity.objects.thing') : t(kindKey) });
  const actor = actorName(entry.actor, t);
  if (entry.kind !== 'history' || entry.action === null || !SAYABLE.has(entry.action) || givenName === null) {
    const wording = entry.wording ?? { version: 1, verb: entry.kind === 'history' ? (NODE_VERBS[entry.action ?? ''] ?? 'changed') : WORK_VERBS[entry.kind], target: entry.node };
    return historySentence({ actor: entry.actor, text: t('places.activity.recorded', { actor, name }), wording }, t, name);
  }
  const kind = t(`places.kinds.${folder && entry.node.kind === 'project' ? 'folder' : entry.node.kind}`);
  switch (entry.action) {
    case 'node.created':
      return t('places.activity.created', { actor, kind, name });
    case 'node.archived':
      return t('places.activity.archived', { actor, name });
    case 'node.unarchived':
      return t('places.activity.unarchived', { actor, name });
    case 'node.deleted':
      return t('places.activity.deleted', { actor, kind, name });
    case 'node.restored':
      return t('places.activity.restored', { actor, name });
    default:
      return t('places.activity.moved', { actor, name });
  }
}

function Line({ entry, fromId, folder, where }: { entry: ActivityEntry; fromId: string; folder: boolean; where: string[] }) {
  const { t } = useTranslation();
  const { Icon, tone } = ICONS[entry.kind];
  const text = activityLine(entry, t, folder);
  const person = entry.actor !== null && !('agent' in entry.actor) ? entry.actor : null;
  // A line about another thing opens that thing, unless it is gone: a deleted thing has no screen to open.
  const target = entry.thread ?? (entry.node.id === fromId || entry.action === 'node.deleted' ? null : entry.node);
  return (
    <li className="flex items-start gap-3 py-3">
      {person === null ? (
        <span className="flex size-(--avatar-28) shrink-0 items-center justify-center rounded-full bg-bg-muted">
          <Icon className={tone} aria-hidden />
        </span>
      ) : (
        <Avatar text={person.avatar.text} color={person.avatar.color} kind={person.kind === 'service' ? 'service' : 'human'} size={28} />
      )}
      <div className="flex min-w-0 flex-1 flex-col">
        <p className="text-base wrap-anywhere text-text">
          {target !== null && target.name !== null ? (
            <PlaceLink place={target} className="rounded-sm hover:underline">
              {text}
            </PlaceLink>
          ) : (
            text
          )}
        </p>
        <p className="text-sm text-text-muted">
          <RelativeTime at={entry.at} />
          {entry.count > 1 && ` · ${t('places.activity.acts', { count: entry.count })}`}
          {where.length > 0 && ` · ${t('folders.inside', { where: where.join(t('places.pathJoin')) })}`}
        </p>
      </div>
    </li>
  );
}

export interface ActivityFeedProps {
  /** The place whose Activity this is. */
  nodeId: string;
  /** How many lines to show before "Show more"; the first page whole when absent. */
  preview?: number;
  /** Offers the chips that keep to the management of the place or to its threads' work. */
  chips?: boolean;
  className?: string;
}

export function ActivityFeed({ nodeId, preview, chips = false, className }: ActivityFeedProps) {
  const { t } = useTranslation();
  const [filter, setFilter] = useListChoice<Filter>('activity', 'filter', 'all');
  const [depth, setDepth] = useListChoice<'summary' | 'every'>('activity', 'depth', 'summary');
  const kinds = KINDS_OF[filter];
  const feed = useInfiniteQuery(activityQuery(nodeId, kinds, depth));
  const lookup = usePlaceLookup();
  const lines = useMemo(() => feed.data?.pages.flatMap((page) => page.items) ?? [], [feed.data]);
  const shown = preview === undefined ? lines : lines.slice(0, preview);

  return (
    <div className={cx('flex flex-col gap-3', className)}>
      <SegmentedControl
        size="sm"
        label={t('places.activity.depth')}
        value={depth}
        onValueChange={setDepth}
        segments={[
          { value: 'summary', name: t('choiceCatalog.activityDepth.summary.name') },
          { value: 'every', name: t('choiceCatalog.activityDepth.every.name') },
        ]}
        className="self-start"
      />
      {chips && (
        <SegmentedControl
          size="sm"
          label={t('places.activity.filter')}
          value={filter}
          onValueChange={setFilter}
          segments={[
            { value: 'all', name: t('choiceCatalog.activityKind.all.name') },
            { value: 'management', name: t('choiceCatalog.activityKind.management.name') },
            { value: 'work', name: t('choiceCatalog.activityKind.work.name') },
          ]}
          className="self-start"
        />
      )}
      {feed.isPending ? (
        <SkeletonGroup className="flex flex-col gap-3 py-1">
          {[0, 1, 2].map((row) => (
            <div key={row} className="flex items-center gap-3">
              <Skeleton className="size-(--avatar-28) rounded-full" />
              <div className="flex flex-1 flex-col gap-2">
                <Skeleton className="h-4 w-3/5" />
                <Skeleton className="h-3 w-1/5" />
              </div>
            </div>
          ))}
        </SkeletonGroup>
      ) : feed.isError && lines.length === 0 ? (
        <ErrorState compact error={feed.error} retrying={feed.isRefetching} onRetry={() => void feed.refetch()} />
      ) : shown.length === 0 ? (
        <EmptyState
          compact
          icon={<ActivityIcon aria-hidden />}
          title={filter === 'all' ? t('places.activity.empty') : t('places.activity.emptyKind')}
        />
      ) : (
        <>
          <ul className="flex flex-col divide-y divide-border">
            {shown.map((entry, index) => (
              <Line
                key={`${entry.at}-${entry.action ?? entry.kind}-${entry.node.id}-${index}`}
                entry={entry}
                fromId={nodeId}
                folder={entry.node.kind === 'project' && lookup.isFolder(entry.node.id)}
                // A line about something inside a folder of the place says in which folders.
                where={lookup.between(entry.node.id, nodeId)}
              />
            ))}
          </ul>
          {preview === undefined && (
            <Pagination
              hasMore={feed.hasNextPage}
              loading={feed.isFetchingNextPage}
              error={feed.isFetchNextPageError ? feed.error : undefined}
              onMore={() => void feed.fetchNextPage()}
            />
          )}
        </>
      )}
    </div>
  );
}

/** An org's Activity: the orgs and projects made in it, renamed, moved, archived and restored, and people joining and leaving. */
export function OrgActivity({ orgId }: { orgId: string }) {
  return <ActivityFeed nodeId={orgId} chips />;
}
