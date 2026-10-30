/** Turns, human and agent messages, milestones, approvals and errors, in the conversation's own words.
 *
 * The points of a conversation worth going to, as the API lists them: what people said, what was
 * announced and the approvals that were asked. Numbered turns lead to their first visible item. Compact
 * checkboxes pick the kinds shown; an entry is a link that takes the conversation to that item and lights
 * it. The first history read is joined to held live rows as the conversation moves, without polling.
 */
import { useListChoice } from '@/lib/listState';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { Checkbox } from '@/ui/Checkbox';
import { useViewInstance } from '@/features/workbench/instance';
import { useInfiniteQuery } from '@tanstack/react-query';
import { useParams } from '@tanstack/react-router';
import { DocumentOutline } from '@/features/people/DocumentOutline';
import { Link } from '@tanstack/react-router';
import { AlignLeft, Flag, ListOrdered, MessageSquare, ShieldQuestionMark, TriangleAlert } from 'lucide-react';
import { type ComponentType } from 'react';
import { useTranslation } from 'react-i18next';

import { outlineQuery } from '@/api/threads';
import type { OutlineEntry, OutlineKind } from '@/generated/api-types';
import type { ViewProps } from '@/features/workbench/views';
import type { WorkspaceAccess } from '@/features/files/door';
import { useThreadView } from '@/realtime/stores/thread';
import { outlineRows } from '@/features/inspector/outlineRows';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { Pagination } from '@/ui/Pagination';

import { ThreadPanel } from './ThreadPanel';
import { outlineKindWord } from './words';

const KINDS: { kind: OutlineKind; includes: readonly OutlineKind[]; icon: ComponentType<{ className?: string; 'aria-hidden'?: boolean }> }[] = [
  { kind: 'turn', includes: ['turn'], icon: ListOrdered },
  { kind: 'user_message', includes: ['user_message', 'agent_message'], icon: MessageSquare },
  { kind: 'milestone', includes: ['milestone'], icon: Flag },
  { kind: 'approval', includes: ['approval'], icon: ShieldQuestionMark },
  { kind: 'error', includes: ['error'], icon: TriangleAlert },
];

export function OutlinePanel({ access }: { access: Pick<WorkspaceAccess, 'thread'> }) {
  const { t } = useTranslation();
  const threadId = access.thread.id;
  const read = useThreadReadRef(threadId);
  const query = useInfiniteQuery({ ...outlineQuery(read), staleTime: Infinity, refetchOnWindowFocus: false, refetchOnReconnect: false });
  const view = useThreadView(threadId);
  const [hidden, setHidden] = useListChoice<readonly OutlineKind[]>(`outline.${threadId}`, 'hidden', []);
  const entries = outlineRows(query.data?.pages.flatMap((page) => page.entries) ?? [], view);
  if (query.error !== null && entries.length === 0) return <ErrorState compact className="h-full" error={query.error} onRetry={() => void query.refetch()} />;
  if (query.data === undefined && entries.length === 0) return <SkeletonRows rows={5} />;
  const groupOf = (entry: OutlineEntry) => KINDS.find((group) => group.includes.some((kind) => entry.kind === kind));
  const present = KINDS.filter((group) => entries.some((entry) => group.includes.some((kind) => entry.kind === kind)));
  const shown = entries.filter((entry) => !hidden.includes(groupOf(entry)?.kind ?? entry.kind));
  return (
    <div className="flex h-full min-h-0 flex-col" data-testid="outline-panel">
      {present.length > 1 && (
        <div role="group" aria-label={t('inspector.outline.filter')} className="flex flex-wrap gap-x-3 gap-y-2 border-b border-border px-2 py-2">
          {present.map(({ kind, icon: Icon }) => {
            const on = !hidden.includes(kind);
            return (
              <Checkbox key={kind} presentation="compact" checked={on}
                label={<span className="inline-flex items-center gap-1 text-xs"><Icon aria-hidden />{t(`inspector.outline.kind.${kind}`)}</span>}
                onCheckedChange={(checked) => setHidden((before) => checked ? before.filter((one) => one !== kind) : [...before, kind])} />
            );
          })}
        </div>
      )}
      {entries.length === 0 ? (
        <EmptyState compact className="flex-1" icon={<AlignLeft aria-hidden />} title={t('inspector.outline.none')} />
      ) : shown.length === 0 ? (
        <EmptyState compact className="flex-1" icon={<AlignLeft aria-hidden />} title={t('inspector.outline.allHidden')} />
      ) : (
        <ol aria-label={t('inspector.outline.list')} className="min-h-0 flex-1 overflow-auto">
          {shown.map((entry) => {
            const Icon = groupOf(entry)?.icon ?? AlignLeft;
            const condition = 'goal_condition' in entry && typeof entry.goal_condition === 'string' ? entry.goal_condition : undefined;
            const title = condition === undefined
              ? entry.kind === 'turn' && typeof entry.turn_ordinal === 'number' ? t('inspector.outline.turn', { ordinal: entry.turn_ordinal }) : entry.title
              : condition === '' ? t('goals.line.label') : t('goals.turn.headline', { condition });
            return (
              <li key={`${entry.item_id}.${entry.kind}`}>
                <Link
                  to="/thread/$threadId"
                  params={{ threadId }}
                  search={{ item: entry.item_id }}
                  className="focus-inset flex min-h-(--row-height) items-center gap-2 px-3 py-1 text-base text-text hover:bg-bg-muted"
                >
                  <Icon className="shrink-0 text-text-muted" aria-hidden />
                  <span className="sr-only">{outlineKindWord(t, entry.kind)}: </span>
                  <span className="min-w-0 flex-1 truncate" title={title}>
                    {title}
                  </span>
                  <RelativeTime at={entry.at} className="shrink-0 text-xs text-text-muted" />
                </Link>
              </li>
            );
          })}
        </ol>
      )}
      <Pagination hasMore={query.hasNextPage} loading={query.isFetchingNextPage} error={query.isFetchNextPageError ? query.error : null} onMore={() => void query.fetchNextPage()} />
    </div>
  );
}

/** The Outline view of a thread. */
export function OutlineView(_props: ViewProps) {
  const params = useParams({ strict: false });
  const instance = useViewInstance();
  const documentId = instance === null ? params.documentId : instance.target?.kind === 'document' ? instance.target.id : undefined;
  if (documentId !== undefined) return <DocumentOutline documentId={documentId} />;
  return <ThreadPanel icon={<AlignLeft aria-hidden />}>{(thread) => <OutlinePanel access={{ thread }} />}</ThreadPanel>;
}
